//! Headless smoke tests: draw every view, tab and dialog with a fake
//! platform, so a panic in view code fails the build instead of the app.

use apicraft_core::engine::Outcome;
use apicraft_core::http::{HttpRequest, HttpResponse};
use apicraft_core::model::*;

use crate::app::{ApiCraftApp, CollectionTab, Confirm, Modal, ReqTab, RespTab, ThemeChoice, View};
use crate::platform::{Inbox, Job, Level, Platform, Status, WsConn};

struct FakePlatform;

impl Platform for FakePlatform {
    fn status(&self) -> Status {
        Status { label: "Test".into(), detail: "headless".into(), level: Level::Good }
    }
    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }
    fn spawn(&self, _job: Job, _inbox: Inbox) {}
    fn load_catalog(&self) -> Option<Catalog> {
        None
    }
    fn save_catalog(&self, _catalog: &Catalog) -> Result<(), String> {
        Ok(())
    }
    fn storage_description(&self) -> String {
        "nowhere".into()
    }
    fn save_file(&self, _name: &str, _bytes: Vec<u8>, _inbox: Inbox) {}
    fn open_files(&self, _inbox: Inbox) {}
    fn ws_connect(&self, _url: &str, _h: &[(String, String)], _p: &[String], _seq: u32, _inbox: Inbox) -> Result<Box<dyn WsConn>, String> {
        Err("no sockets in tests".into())
    }
    fn ws_sends_headers(&self) -> bool {
        true
    }
    fn default_redirect_uri(&self) -> String {
        "http://127.0.0.1:7878/callback".into()
    }
    fn authorize(&self, _url: &str, _redirect: &str, _inbox: Inbox) -> Result<(), String> {
        Ok(())
    }
    fn native_grpc(&self) -> bool {
        true
    }
}

fn frame(ctx: &egui::Context, app: &mut ApiCraftApp, w: f32, h: f32) {
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
    // Two passes: egui sizes some things on the first.
    for _ in 0..2 {
        let mut out = ctx.run_ui(raw.clone(), |ui| app.ui(ui));
        // No renderer here: drop the texture updates deliberately.
        out.textures_delta.clear();
    }
}

fn frame_with(ctx: &egui::Context, app: &mut ApiCraftApp, events: Vec<egui::Event>) {
    let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 800.0))), events, ..Default::default() };
    let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
    out.textures_delta.clear();
}

/// Drags with the mouse from one point to another over several frames, as a person would.
fn drag(ctx: &egui::Context, app: &mut ApiCraftApp, from: egui::Pos2, to: egui::Pos2) {
    let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
    frame_with(ctx, app, vec![egui::Event::PointerMoved(from)]);
    frame_with(ctx, app, vec![button(from, true)]);
    for i in 1..=8 {
        frame_with(ctx, app, vec![egui::Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))]);
    }
    frame_with(ctx, app, vec![button(to, false)]);
    frame_with(ctx, app, vec![egui::Event::PointerMoved(egui::pos2(900.0, 600.0))]);
    frame_with(ctx, app, Vec::new());
}

/// The first sidebar row labelled `name`: its centre.
fn row_center(ctx: &egui::Context, name: &str) -> egui::Pos2 {
    let mut found = None;
    ctx.viewport(|v| {
        let widgets = &v.prev_pass.widgets;
        for (_, rects) in widgets.layers() {
            for w in rects {
                if found.is_none() && widgets.info(w.id).and_then(|i| i.label.as_deref()) == Some(name) && w.rect.left() < 300.0 {
                    found = Some(w.rect.center());
                }
            }
        }
    });
    found.unwrap_or_else(|| panic!("no sidebar row labelled {name}"))
}

#[test]
fn sidebar_drag_and_drop_reorders_and_moves() {
    let ctx = egui::Context::default();
    let mut app = ApiCraftApp::new(&ctx, Box::new(FakePlatform), None, None);
    let col = app.catalog.collections[0].id.clone();
    let rest = app.catalog.collections[0].items.iter().find(|i| i.name() == "REST").map(|i| i.id().to_string()).unwrap();
    app.expanded = [col.clone(), rest.clone()].into_iter().collect();
    // egui keeps widget labels (used to find the rows) only with this on.
    ctx.all_styles_mut(|s| s.debug.show_interactive_widgets = true);
    for _ in 0..3 {
        frame_with(&ctx, &mut app, Vec::new());
    }
    let folder = |app: &ApiCraftApp, name: &str| match app.catalog.collections[0].items.iter().find(|i| i.name() == name) {
        Some(Item::Folder(f)) => f.items.iter().map(|i| i.name().to_string()).collect::<Vec<_>>(),
        _ => panic!("no folder {name}"),
    };
    let before = folder(&app, "REST");
    assert_eq!(before, ["Get with query", "Post JSON", "Basic auth", "Digest auth"]);

    // Onto the top edge of a sibling: reorders.
    let from = row_center(&ctx, "Basic auth");
    let get = row_center(&ctx, "Get with query");
    drag(&ctx, &mut app, from, get - egui::vec2(0.0, 10.0));
    assert_eq!(folder(&app, "REST"), ["Basic auth", "Get with query", "Post JSON", "Digest auth"]);

    // Onto the middle of a folder: moves into it, at the end.
    let from = row_center(&ctx, "Digest auth");
    let to = row_center(&ctx, "Token flow");
    drag(&ctx, &mut app, from, to);
    assert_eq!(folder(&app, "REST"), ["Basic auth", "Get with query", "Post JSON"]);
    assert_eq!(folder(&app, "Token flow").last().map(String::as_str), Some("Digest auth"));
}

fn outcome_for(app: &ApiCraftApp, id: &str, body: &str, content_type: &str) -> Outcome {
    let name = app.catalog.find_request(id).map(|(_, r)| r.name.clone()).unwrap_or_default();
    Outcome {
        request_id: id.into(),
        request_name: name,
        sent: Some(HttpRequest {
            method: "POST".into(),
            url: "https://api.test/x".into(),
            headers: vec![("Accept".into(), "*/*".into())],
            body: b"{\"a\":1}".to_vec(),
            grpc: false,
            timeout_ms: 0,
        }),
        response: Some(HttpResponse {
            status: 201,
            status_text: "Created".into(),
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.as_bytes().to_vec(),
            trailers: vec![("grpc-status".into(), "0".into())],
            elapsed_ms: 12.0,
        }),
        body_text: body.into(),
        extracted: vec![(VarScope::Runtime, "token".into(), "abc".into())],
        checks: vec![apicraft_core::extract::CheckResult { label: "status is 2xx".into(), passed: true, detail: "got 201".into() }],
        warnings: vec!["sent without a token".into()],
        ..Default::default()
    }
}

#[test]
fn renders_every_view_tab_and_dialog() {
    let ctx = egui::Context::default();
    let mut app = ApiCraftApp::new(&ctx, Box::new(FakePlatform), None, None);
    let ids: Vec<String> = app.catalog.all_requests().iter().map(|(_, r)| r.id.clone()).collect();
    assert!(ids.len() >= 10, "the sample catalog covers every protocol");
    let tabs = [
        ReqTab::Params,
        ReqTab::Headers,
        ReqTab::Body,
        ReqTab::Auth,
        ReqTab::Soap,
        ReqTab::Proto,
        ReqTab::Messages,
        ReqTab::Mcp,
        ReqTab::Capture,
        ReqTab::Checks,
        ReqTab::Docs,
    ];
    for (n, id) in ids.iter().enumerate() {
        app.open_request(id);
        for tab in tabs {
            app.req_tab = tab;
            frame(&ctx, &mut app, 1400.0, 900.0);
        }
        // With a response, in every response tab; JSON and XML bodies alternate.
        let (body, ct) =
            if n % 2 == 0 { ("{\"ok\":true,\"items\":[1,2.5,null,\"x\"]}", "application/json") } else { ("<a x=\"1\"><b>t</b><!-- c --></a>", "text/xml") };
        let o = outcome_for(&app, id, body, ct);
        app.outcomes.insert(id.clone(), o);
        for rt in [RespTab::Body, RespTab::Headers, RespTab::Checks, RespTab::Captured, RespTab::Sent] {
            app.resp_tab = rt;
            app.resp_raw = rt == RespTab::Headers;
            frame(&ctx, &mut app, 1400.0, 900.0);
        }
        // Narrow layout puts the response below.
        frame(&ctx, &mut app, 700.0, 800.0);
    }

    // Every auth editor.
    let first = ids[0].clone();
    app.open_request(&first);
    app.req_tab = ReqTab::Auth;
    for (kind, _) in Auth::KINDS {
        if let Some(r) = app.catalog.find_request_mut(&first) {
            r.auth = Auth::of_kind(kind, &r.auth);
        }
        frame(&ctx, &mut app, 1400.0, 900.0);
    }

    // Workflows, with and without a run.
    let wid = app.catalog.workflows[0].id.clone();
    app.view = View::Workflow(wid.clone());
    frame(&ctx, &mut app, 1400.0, 900.0);
    app.expanded.insert(app.catalog.workflows[0].steps[0].id.clone());
    frame(&ctx, &mut app, 700.0, 800.0);

    // Every dialog.
    let col = app.catalog.collections[0].id.clone();
    let env = app.catalog.environments[0].id.clone();
    let modals = vec![
        Modal::Code { request_id: first.clone(), curl: true },
        Modal::Code { request_id: first.clone(), curl: false },
        Modal::Environments { selected: Some(env) },
        Modal::Runtime,
        Modal::Collection { id: col.clone(), tab: CollectionTab::General },
        Modal::Collection { id: col.clone(), tab: CollectionTab::Variables },
        Modal::Collection { id: col.clone(), tab: CollectionTab::Auth },
        Modal::Rename { id: col.clone(), value: "x".into() },
        Modal::Import { text: String::new(), target: None, url: String::new() },
        Modal::Confirm { message: "Delete?".into(), action: Confirm::Delete(col) },
        Modal::Settings,
        Modal::StepResult { index: 0 },
    ];
    for m in modals {
        app.modal = Some(m);
        frame(&ctx, &mut app, 1400.0, 900.0);
    }

    // Dark theme, collapsed sidebar, welcome view.
    app.modal = None;
    app.prefs.theme = ThemeChoice::Dark;
    app.apply_theme(&ctx);
    app.prefs.sidebar_open = false;
    app.view = View::Welcome;
    frame(&ctx, &mut app, 1400.0, 900.0);
    app.toast("hello", false);
    frame(&ctx, &mut app, 1400.0, 900.0);
}
