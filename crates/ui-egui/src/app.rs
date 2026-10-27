//! Application state and the top-level frame.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use apicraft_core::engine::{apply_outcome, Outcome};
use apicraft_core::model::*;
use apicraft_core::workflow::{RunSummary, StepReport};

use crate::highlight::Highlighter;
use crate::platform::{Inbox, Job, Msg, Platform, WsConn, WsEvent};
use crate::{actions, theme, views};

#[derive(Debug, Clone, PartialEq)]
pub enum View {
    Welcome,
    Request(String),
    Workflow(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqTab {
    Params,
    Headers,
    Body,
    Auth,
    Soap,
    Proto,
    Messages,
    Mcp,
    /// GraphQL query and variables.
    GraphQl,
    /// GraphQL schema browser.
    Schema,
    /// OData resource, query options and metadata.
    OData,
    Capture,
    Checks,
    Docs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespTab {
    Body,
    Headers,
    Checks,
    Captured,
    Sent,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    Delete(String),
    DeleteEnv(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Modal {
    Code { request_id: String, curl: bool },
    Environments { selected: Option<String> },
    Runtime,
    Collection { id: String, tab: CollectionTab },
    Rename { id: String, value: String },
    Import { text: String, target: Option<String>, url: String },
    Confirm { message: String, action: Confirm },
    Settings,
    StepResult { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionTab {
    General,
    Variables,
    Auth,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WsDir {
    Out,
    In,
    Info,
    Error,
}

pub struct WsEntry {
    pub dir: WsDir,
    pub text: String,
    pub at: f64,
}

pub struct WsSession {
    pub seq: u32,
    pub request_id: String,
    pub conn: Box<dyn WsConn>,
    pub open: bool,
    pub closed: bool,
    pub log: Vec<WsEntry>,
    pub started: f64,
}

pub struct WorkflowRun {
    pub workflow_id: String,
    pub running: bool,
    pub stop: Arc<AtomicBool>,
    pub reports: Vec<StepReport>,
    pub summary: Option<RunSummary>,
    pub error: Option<String>,
}

pub struct PendingOAuth {
    pub owner: String,
    pub state: String,
    pub verifier: String,
    pub redirect_uri: String,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub until: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

/// UI preferences kept between runs (by the host's key/value storage).
pub struct Prefs {
    pub theme: ThemeChoice,
    pub sidebar_open: bool,
}

pub struct ApiCraftApp {
    pub(crate) platform: Box<dyn Platform>,
    pub(crate) inbox: Inbox,
    rx: Receiver<Msg>,
    pub catalog: Catalog,
    pub(crate) view: View,
    pub(crate) req_tab: ReqTab,
    pub(crate) resp_tab: RespTab,
    pub(crate) resp_raw: bool,
    pub(crate) outcomes: HashMap<String, Outcome>,
    /// Requests in flight. A cancelled one is removed and its result ignored.
    pub(crate) sending: HashSet<String>,
    pub(crate) expanded: HashSet<String>,
    pub(crate) filter: String,
    pub(crate) modal: Option<Modal>,
    pub(crate) toasts: Vec<Toast>,
    pub(crate) ws: Option<WsSession>,
    pub(crate) ws_compose: String,
    ws_seq: u32,
    pub(crate) run: Option<WorkflowRun>,
    pub(crate) oauth: Option<PendingOAuth>,
    pub(crate) prefs: Prefs,
    pub(crate) add_step: String,
    pub(crate) highlighter: Highlighter,
    /// Parsed services of the last .proto shown, keyed by its hash.
    pub(crate) proto_cache: ProtoCache,
    /// What each MCP request's server offers, from the last Discover.
    pub(crate) mcp_catalogs: HashMap<String, Result<apicraft_core::mcp::McpCatalog, String>>,
    /// GraphQL schemas and OData metadata, from the last load.
    pub(crate) schemas: HashMap<String, Result<apicraft_core::engine::ApiSchema, String>>,
    /// MCP discoveries and schema loads in flight.
    pub(crate) discovering: std::collections::HashSet<String>,
    /// Pretty-printed bodies, computed once when a response arrives.
    pub(crate) pretty: HashMap<String, String>,
    pub(crate) actions: Vec<actions::Action>,
    saved_hash: u64,
    last_save_check: f64,
    started: bool,
    pub(crate) now: f64,
}

/// Parsed services of the last .proto shown, keyed by its hash.
pub(crate) type ProtoCache = std::cell::RefCell<Option<(u64, Result<Vec<apicraft_core::grpc::ServiceInfo>, String>)>>;

fn pretty_of(o: &Outcome) -> String {
    let ct = if o.grpc.is_some() { "application/json" } else { o.response.as_ref().map(|r| r.content_type()).unwrap_or("") };
    apicraft_core::format::pretty_body(ct, &o.body_text)
}

fn catalog_hash(c: &Catalog) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(c).unwrap_or_default().hash(&mut h);
    h.finish()
}

impl ApiCraftApp {
    /// `prefs` is what [`Self::prefs_string`] returned last time, if anything.
    pub fn new(ctx: &egui::Context, platform: Box<dyn Platform>, prefs: Option<String>, ui_font: Option<Vec<u8>>) -> Self {
        theme::install(ctx);
        theme::install_fonts(ctx, ui_font);
        let (tx, rx) = channel();
        let inbox = Inbox::new(tx, ctx.clone());
        let (catalog, fresh) = match platform.load_catalog() {
            Some(c) => (c, false),
            None => (crate::sample::catalog(), true),
        };
        let mut prefs_v = Prefs { theme: ThemeChoice::System, sidebar_open: true };
        if let Some(v) = prefs.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) {
            prefs_v.theme = match v.get("theme").and_then(|t| t.as_str()) {
                Some("light") => ThemeChoice::Light,
                Some("dark") => ThemeChoice::Dark,
                _ => ThemeChoice::System,
            };
            prefs_v.sidebar_open = v.get("sidebar").and_then(|b| b.as_bool()).unwrap_or(true);
        }
        let mut app = ApiCraftApp {
            platform,
            inbox,
            rx,
            saved_hash: if fresh { 0 } else { catalog_hash(&catalog) },
            catalog,
            view: View::Welcome,
            req_tab: ReqTab::Params,
            resp_tab: RespTab::Body,
            resp_raw: false,
            outcomes: HashMap::new(),
            sending: HashSet::new(),
            expanded: HashSet::new(),
            filter: String::new(),
            modal: None,
            toasts: Vec::new(),
            ws: None,
            ws_compose: String::new(),
            ws_seq: 0,
            run: None,
            oauth: None,
            prefs: prefs_v,
            add_step: String::new(),
            highlighter: Highlighter::default(),
            proto_cache: Default::default(),
            pretty: HashMap::new(),
            mcp_catalogs: HashMap::new(),
            schemas: HashMap::new(),
            discovering: HashSet::new(),
            actions: Vec::new(),
            last_save_check: 0.0,
            started: false,
            now: 0.0,
        };
        app.apply_theme(ctx);
        for c in &app.catalog.collections {
            app.expanded.insert(c.id.clone());
        }
        if let Some((_, r)) = app.catalog.all_requests().first() {
            let id = r.id.clone();
            app.open_request(&id);
        }
        app
    }

    /// Preferences to persist between runs.
    pub fn prefs_string(&self) -> String {
        serde_json::json!({
            "theme": match self.prefs.theme { ThemeChoice::Light => "light", ThemeChoice::Dark => "dark", ThemeChoice::System => "system" },
            "sidebar": self.prefs.sidebar_open,
        })
        .to_string()
    }

    pub(crate) fn apply_theme(&self, ctx: &egui::Context) {
        ctx.set_theme(match self.prefs.theme {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        });
    }

    /// Saves now if anything changed (call on exit).
    pub fn flush(&mut self) {
        let h = catalog_hash(&self.catalog);
        if h != self.saved_hash {
            match self.platform.save_catalog(&self.catalog) {
                Ok(()) => self.saved_hash = h,
                Err(e) => self.toast(format!("Could not save: {e}"), true),
            }
        }
    }

    pub(crate) fn toast(&mut self, text: impl Into<String>, error: bool) {
        let until = self.now + if error { 7.0 } else { 3.5 };
        self.toasts.push(Toast { text: text.into(), error, until });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    pub(crate) fn current_request(&self) -> Option<&Request> {
        match &self.view {
            View::Request(id) => self.catalog.find_request(id).map(|(_, r)| r),
            _ => None,
        }
    }

    pub(crate) fn default_tab(&self) -> ReqTab {
        match self.current_request().map(|r| r.protocol) {
            Some(Protocol::Soap) | Some(Protocol::Grpc) => ReqTab::Body,
            Some(Protocol::WebSocket) => ReqTab::Messages,
            Some(Protocol::Mcp) => ReqTab::Mcp,
            Some(Protocol::GraphQl) => ReqTab::GraphQl,
            Some(Protocol::OData) => ReqTab::OData,
            _ => ReqTab::Params,
        }
    }

    pub(crate) fn open_request(&mut self, id: &str) {
        self.view = View::Request(id.to_string());
        self.req_tab = self.default_tab();
        self.resp_tab = RespTab::Body;
        let path = actions::ancestors_of(&self.catalog, id);
        self.expanded.extend(path);
    }

    pub(crate) fn spawn(&self, job: Job) {
        self.platform.spawn(job, self.inbox.clone());
    }

    pub(crate) fn next_ws_seq(&mut self) -> u32 {
        self.ws_seq += 1;
        self.ws_seq
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Outcome { request_id, outcome } => {
                if !self.sending.remove(&request_id) {
                    return; // cancelled
                }
                apply_outcome(&mut self.catalog, &outcome);
                self.pretty.insert(request_id.clone(), pretty_of(&outcome));
                self.outcomes.insert(request_id, *outcome);
            }
            Msg::Step(rep) => {
                if let Some(o) = &rep.outcome {
                    apply_outcome(&mut self.catalog, o);
                    self.pretty.insert(rep.request_id.clone(), pretty_of(o));
                    self.outcomes.insert(rep.request_id.clone(), o.clone());
                }
                if let Some(run) = &mut self.run {
                    run.reports.push(*rep);
                }
            }
            Msg::WorkflowDone { workflow_id, result } => {
                let mut note = None;
                if let Some(run) = self.run.as_mut().filter(|r| r.workflow_id == workflow_id) {
                    run.running = false;
                    match result {
                        Ok(s) => {
                            note = Some(if s.failed == 0 {
                                (format!("Workflow passed: {} step(s)", s.passed), false)
                            } else {
                                (format!("Workflow failed: {} passed, {} failed", s.passed, s.failed), true)
                            });
                            run.summary = Some(s);
                        }
                        Err(e) => run.error = Some(e),
                    }
                }
                if let Some((t, e)) = note {
                    self.toast(t, e);
                }
            }
            Msg::Token { owner, result } => match result {
                Ok(t) => {
                    self.catalog.store_oauth_token(&owner, &t.access_token, &t.refresh_token, t.expires_at);
                    self.toast("Got a new access token", false);
                }
                Err(e) => self.toast(e, true),
            },
            Msg::AuthRedirect { query } => actions::finish_authorization(self, &query),
            Msg::FileOpened { name, bytes } => actions::import_bytes(self, &bytes, &name),
            Msg::Toast { text, error } => self.toast(text, error),
            Msg::Ws { seq, event } => self.ws_event(seq, event),
            Msg::McpDiscovered { request_id, result } => {
                self.discovering.remove(&request_id);
                match &result {
                    Ok(c) => self.toast(
                        format!(
                            "{}: {} tool(s), {} resource(s), {} prompt(s)",
                            if c.server.is_empty() { "Server" } else { &c.server },
                            c.tools.len(),
                            c.resources.len(),
                            c.prompts.len()
                        ),
                        false,
                    ),
                    Err(e) => self.toast(format!("Discovery failed: {e}"), true),
                }
                self.mcp_catalogs.insert(request_id, result);
            }
            Msg::SchemaFetched { request_id, result } => {
                self.discovering.remove(&request_id);
                match &result {
                    Ok(apicraft_core::engine::ApiSchema::GraphQl(s)) => {
                        let count = |t: &Option<String>| t.as_deref().and_then(|n| s.type_def(n)).map(|t| t.fields.len()).unwrap_or(0);
                        self.toast(format!("Schema loaded: {} query and {} mutation field(s)", count(&s.query_type), count(&s.mutation_type)), false)
                    }
                    Ok(apicraft_core::engine::ApiSchema::OData(m)) => self.toast(
                        format!("$metadata loaded: {} entity set(s), {} operation(s)", m.entity_sets.len() + m.singletons.len(), m.operations.len()),
                        false,
                    ),
                    Err(e) => self.toast(format!("Could not load the schema: {e}"), true),
                }
                self.schemas.insert(request_id, result);
            }
            Msg::FolderPicked { path, purpose } => self.platform.load_folder(path, purpose, self.inbox.clone()),
            Msg::FolderLoaded { path, purpose, result } => actions::folder_loaded(self, &path, purpose, result.map(|c| *c)),
            Msg::ReplaceCatalog { catalog, note } => {
                self.catalog = *catalog;
                self.saved_hash = catalog_hash(&self.catalog);
                self.outcomes.clear();
                for c in &self.catalog.collections {
                    self.expanded.insert(c.id.clone());
                }
                self.view = View::Welcome;
                if let Some((_, r)) = self.catalog.all_requests().first() {
                    let id = r.id.clone();
                    self.open_request(&id);
                }
                self.toast(note, false);
            }
            Msg::Refresh => {}
        }
    }

    fn ws_event(&mut self, seq: u32, event: WsEvent) {
        let now = self.now;
        let Some(w) = self.ws.as_mut().filter(|w| w.seq == seq) else { return };
        let (dir, text) = match event {
            WsEvent::Open => {
                w.open = true;
                (WsDir::Info, "Connected".to_string())
            }
            WsEvent::Text(t) => (WsDir::In, t),
            WsEvent::Binary(b) => (
                WsDir::In,
                match String::from_utf8(b.clone()) {
                    Ok(s) => s,
                    Err(_) => format!("(binary, {} bytes) {}", b.len(), b.iter().take(64).map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")),
                },
            ),
            WsEvent::Error(e) => (WsDir::Error, e),
            WsEvent::Closed(why) => {
                w.open = false;
                w.closed = true;
                (WsDir::Info, why)
            }
        };
        w.log.push(WsEntry { dir, text, at: now });
        if w.log.len() > 2000 {
            w.log.drain(..500);
        }
    }

    fn autosave(&mut self) {
        if self.now - self.last_save_check < 0.7 {
            return;
        }
        self.last_save_check = self.now;
        let h = catalog_hash(&self.catalog);
        if h != self.saved_hash {
            match self.platform.save_catalog(&self.catalog) {
                Ok(()) => self.saved_hash = h,
                Err(e) => {
                    self.saved_hash = h; // don't repeat the error every second
                    self.toast(format!("Could not save the catalog: {e}"), true);
                }
            }
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let send = KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let new = KeyboardShortcut::new(Modifiers::COMMAND, Key::N);
        if ctx.input_mut(|i| i.consume_shortcut(&send)) && self.modal.is_none() {
            match self.view.clone() {
                View::Request(id) => {
                    let ws = self.current_request().map(|r| r.protocol == Protocol::WebSocket).unwrap_or(false);
                    if ws {
                        if self.ws.as_ref().map(|w| w.request_id == id && w.open).unwrap_or(false) && !self.ws_compose.is_empty() {
                            self.actions.push(actions::Action::WsSendCompose);
                        } else if self.ws.as_ref().map(|w| w.request_id != id || w.closed).unwrap_or(true) {
                            self.actions.push(actions::Action::WsConnect(id));
                        }
                    } else {
                        self.actions.push(actions::Action::Send(id));
                    }
                }
                View::Workflow(id) => self.actions.push(actions::Action::RunWorkflow(id)),
                View::Welcome => {}
            }
        }
        if ctx.input_mut(|i| i.consume_shortcut(&save)) {
            self.flush();
            self.toast("Saved", false);
        }
        if ctx.input_mut(|i| i.consume_shortcut(&new)) {
            self.actions.push(actions::Action::NewRequest(None));
        }
    }

    /// Draws one frame. Hosts call this from `eframe::App::ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.now = ctx.input(|i| i.time);
        if !self.started {
            self.started = true;
            self.platform.start(self.inbox.clone());
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
        if let Some(external) = self.platform.poll_external_catalog() {
            self.reload_external(external);
        }
        self.shortcuts(&ctx);
        actions::handle_dropped_files(self, &ctx);

        views::frame(self, ui);

        let pending = std::mem::take(&mut self.actions);
        for a in pending {
            actions::apply(self, a, &ctx);
        }
        self.autosave();
        self.toasts.retain(|t| t.until > self.now);
        if !self.toasts.is_empty() || self.run.as_ref().map(|r| r.running).unwrap_or(false) || !self.sending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        } else {
            // Wake now and then to notice catalog changes made elsewhere.
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }

    /// Takes a catalog changed outside the app, unless there are unsaved edits.
    fn reload_external(&mut self, external: Catalog) {
        if catalog_hash(&self.catalog) != self.saved_hash {
            self.toast("The catalog file changed on disk while you had unsaved edits; your version is kept and will be saved over it.", true);
            return;
        }
        self.catalog = external;
        self.saved_hash = catalog_hash(&self.catalog);
        let gone = match &self.view {
            View::Request(id) => self.catalog.find_request(id).is_none(),
            View::Workflow(id) => self.catalog.workflow(id).is_none(),
            View::Welcome => false,
        };
        if gone {
            self.view = View::Welcome;
        }
        self.toast("Catalog updated from disk", false);
    }

    pub(crate) fn stop_workflow(&mut self) {
        if let Some(r) = &self.run {
            r.stop.store(true, Ordering::Relaxed);
        }
    }
}
