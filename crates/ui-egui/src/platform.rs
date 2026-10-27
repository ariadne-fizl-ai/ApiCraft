//! What the UI needs from the host. The desktop app and the browser app each
//! implement [`Platform`]; everything else in this crate is shared.
//!
//! Background work (sending, workflows, token fetches, file dialogs, socket
//! traffic) reports back through an [`Inbox`]: a channel the UI drains every
//! frame, which also asks egui to repaint.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use apicraft_core::auth::{self, TokenGrant, TokenResult};
use apicraft_core::engine::{self, Outcome};
use apicraft_core::http::Transport;
use apicraft_core::model::{Catalog, OAuth2};
use apicraft_core::workflow::{self, RunSummary, StepReport};

/// Events delivered to the UI thread.
pub enum Msg {
    Outcome {
        request_id: String,
        outcome: Box<Outcome>,
    },
    Step(Box<StepReport>),
    WorkflowDone {
        workflow_id: String,
        result: Result<RunSummary, String>,
    },
    Token {
        owner: String,
        result: Result<TokenResult, String>,
    },
    /// The query string the OAuth provider redirected back with.
    AuthRedirect {
        query: String,
    },
    FileOpened {
        name: String,
        bytes: Vec<u8>,
    },
    Toast {
        text: String,
        error: bool,
    },
    Ws {
        seq: u32,
        event: WsEvent,
    },
    McpDiscovered {
        request_id: String,
        result: Result<apicraft_core::mcp::McpCatalog, String>,
    },
    SchemaFetched {
        request_id: String,
        result: Result<apicraft_core::engine::ApiSchema, String>,
    },
    /// Replace the in-memory catalog (e.g. loaded from the agent's file).
    ReplaceCatalog {
        catalog: Box<Catalog>,
        note: String,
    },
    /// Something about the platform changed (agent found, ...): just repaint.
    Refresh,
}

pub enum WsEvent {
    Open,
    Text(String),
    Binary(Vec<u8>),
    Error(String),
    Closed(String),
}

/// Sender side of the UI's message queue.
#[derive(Clone)]
pub struct Inbox {
    tx: Sender<Msg>,
    ctx: egui::Context,
}

impl Inbox {
    pub fn new(tx: Sender<Msg>, ctx: egui::Context) -> Self {
        Inbox { tx, ctx }
    }

    pub fn post(&self, msg: Msg) {
        let _ = self.tx.send(msg);
        self.ctx.request_repaint();
    }

    pub fn toast(&self, text: impl Into<String>, error: bool) {
        self.post(Msg::Toast { text: text.into(), error });
    }
}

/// Work that runs off the UI thread.
pub enum Job {
    Execute {
        catalog: Box<Catalog>,
        request_id: String,
    },
    Workflow {
        catalog: Box<Catalog>,
        workflow_id: String,
        stop: Arc<AtomicBool>,
    },
    /// List what an MCP server offers (tools, resources, prompts).
    McpDiscover {
        catalog: Box<Catalog>,
        request_id: String,
    },
    /// Load a GraphQL endpoint's schema or an OData service's $metadata.
    FetchSchema {
        catalog: Box<Catalog>,
        request_id: String,
    },
    /// Download a definition (OpenAPI, WSDL, OData $metadata, Postman, catalog) to import;
    /// it arrives as `Msg::FileOpened` named by its URL.
    FetchImport {
        url: String,
    },
    /// Fetch an OAuth 2 token; `code` is set when exchanging an authorization code.
    Token {
        owner: String,
        oauth: Box<OAuth2>,
        code: Option<CodeExchange>,
    },
}

pub struct CodeExchange {
    pub code: String,
    pub verifier: String,
    pub redirect_uri: String,
}

/// Runs a job against a transport. Platforms call this from their own
/// executor (a worker thread natively, `spawn_local` in the browser).
pub async fn run_job(job: Job, transport: &dyn Transport, inbox: &Inbox) {
    match job {
        Job::Execute { catalog, request_id } => {
            let outcome = engine::execute(&catalog, &request_id, &[], &[], &[], transport).await;
            inbox.post(Msg::Outcome { request_id, outcome: Box::new(outcome) });
        }
        Job::Workflow { catalog, workflow_id, stop } => {
            let result = workflow::run(&catalog, &workflow_id, transport, &mut |rep| {
                inbox.post(Msg::Step(Box::new(rep.clone())));
                !stop.load(Ordering::Relaxed)
            })
            .await;
            inbox.post(Msg::WorkflowDone { workflow_id, result });
        }
        Job::McpDiscover { catalog, request_id } => {
            let result = engine::mcp_discover(&catalog, &request_id, transport).await;
            inbox.post(Msg::McpDiscovered { request_id, result });
        }
        Job::FetchSchema { catalog, request_id } => {
            let result = engine::fetch_schema(&catalog, &request_id, transport).await;
            inbox.post(Msg::SchemaFetched { request_id, result });
        }
        Job::FetchImport { url } => {
            let req = apicraft_core::http::HttpRequest { method: "GET".into(), url: url.clone(), timeout_ms: 60_000, ..Default::default() };
            match transport.send(req).await {
                Ok(r) if (200..300).contains(&r.status) => inbox.post(Msg::FileOpened { name: url, bytes: r.body }),
                Ok(r) => inbox.toast(format!("{url} returned {} {}", r.status, r.status_text), true),
                Err(e) => inbox.toast(format!("Could not download {url}: {e}"), true),
            }
        }
        Job::Token { owner, oauth, code } => {
            let mut o = *oauth;
            let result = match code {
                Some(c) => match auth::token_request(&o, TokenGrant::Code { code: &c.code, verifier: &c.verifier, redirect_uri: &c.redirect_uri }) {
                    Ok(req) => match transport.send(req).await {
                        Ok(resp) => auth::parse_token_response(&resp, transport.now_ms()),
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e),
                },
                None => {
                    // "Get new token": force a fresh one.
                    o.access_token.clear();
                    o.expires_at = 0;
                    if o.grant != apicraft_core::model::GrantType::Manual {
                        o.refresh_token.clear();
                    }
                    match engine::ensure_oauth_token(&mut o, transport).await {
                        Ok(Some(t)) => Ok(t),
                        Ok(None) => Err("the current token is still valid".into()),
                        Err(e) => Err(e),
                    }
                }
            };
            inbox.post(Msg::Token { owner, result });
        }
    }
}

/// A file dropped on the window, as egui hands it over.
pub type DroppedFileRef = Arc<dyn egui::DroppedFile + Send + Sync>;

/// An open WebSocket.
pub trait WsConn {
    fn send_text(&self, text: &str) -> Result<(), String>;
    fn close(&self);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    Neutral,
    Warn,
}

/// The chip in the top bar: how requests are being sent.
pub struct Status {
    pub label: String,
    pub detail: String,
    pub level: Level,
}

pub trait Platform {
    /// Called once when the UI starts (e.g. to look for the local agent).
    fn start(&self, _inbox: Inbox) {}

    fn status(&self) -> Status;
    fn now_ms(&self) -> u64;

    /// Runs a job in the background; results arrive through the inbox.
    fn spawn(&self, job: Job, inbox: Inbox);

    fn load_catalog(&self) -> Option<Catalog>;
    fn save_catalog(&self, catalog: &Catalog) -> Result<(), String>;
    /// Where the catalog is kept, for the settings dialog.
    fn storage_description(&self) -> String;

    /// A newer catalog written by someone else (the CLI, an agent through
    /// the MCP server), if there is one. Called every frame; keep it cheap.
    fn poll_external_catalog(&self) -> Option<Catalog> {
        None
    }

    /// Offers bytes to the user as a file (save dialog or download).
    fn save_file(&self, suggested_name: &str, bytes: Vec<u8>, inbox: Inbox);
    /// Lets the user pick files to import; each arrives as `Msg::FileOpened`.
    fn open_files(&self, inbox: Inbox);

    /// Reads a file dropped on the window; it arrives as `Msg::FileOpened`.
    /// Natively the bytes are read straight away; the web overrides this,
    /// since browsers only hand them over asynchronously.
    fn read_dropped_file(&self, file: DroppedFileRef, inbox: Inbox) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name = file.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "dropped file".into());
            match file.bytes() {
                Ok(bytes) => inbox.post(Msg::FileOpened { name, bytes }),
                Err(e) => inbox.toast(format!("Could not read {name}: {e}"), true),
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (file, inbox);
    }

    fn ws_connect(&self, url: &str, headers: &[(String, String)], protocols: &[String], seq: u32, inbox: Inbox) -> Result<Box<dyn WsConn>, String>;
    /// Whether custom headers reach the server on a WebSocket handshake.
    fn ws_sends_headers(&self) -> bool;

    /// Where an OAuth provider should send the browser back to.
    fn default_redirect_uri(&self) -> String;
    /// Opens the provider's sign-in page; the redirect arrives as `Msg::AuthRedirect`.
    fn authorize(&self, url: &str, redirect_uri: &str, inbox: Inbox) -> Result<(), String>;

    /// Platform-specific settings (e.g. the browser's agent options).
    fn settings_ui(&self, _ui: &mut egui::Ui, _inbox: &Inbox) {}

    /// Whether the build can send with native gRPC (otherwise gRPC-Web).
    fn native_grpc(&self) -> bool;
}
