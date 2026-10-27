//! Dialogs.

use apicraft_core::codegen::{self, Target};
use apicraft_core::format::human_size;
use apicraft_core::model::ProxyMode;
use egui::{Align, Layout, RichText, TextEdit, Ui};

use super::response;
use super::widgets::{self, accent_button, alert, danger_button, help, kv_table, AlertKind, AuthCtx};
use crate::actions::Action;
use crate::app::{ApiCraftApp, CollectionTab, Confirm, Modal, RespTab, ThemeChoice};
use crate::highlight::Lang;
use crate::icons;
use crate::theme::pal;

pub fn show(app: &mut ApiCraftApp, ctx: &egui::Context) {
    let Some(modal) = app.modal.clone() else { return };
    let wide = matches!(modal, Modal::Code { .. } | Modal::Environments { .. } | Modal::Runtime | Modal::Collection { .. } | Modal::StepResult { .. });
    let max_w = ctx.content_rect().width() - 40.0;
    let width = match modal {
        Modal::Confirm { .. } => 440.0_f32,
        Modal::Import { .. } => 640.0,
        _ if wide => 820.0,
        _ => 520.0,
    }
    .min(max_w);
    let max_h = ctx.content_rect().height() - 80.0;
    let resp = egui::Modal::new(egui::Id::unique("modal")).show(ctx, |ui| {
        ui.set_width(width);
        ui.set_max_height(max_h);
        match modal {
            Modal::Code { request_id, curl } => code(app, ui, &request_id, curl),
            Modal::Environments { selected } => environments(app, ui, selected),
            Modal::Runtime => runtime(app, ui),
            Modal::Collection { id, tab } => collection(app, ui, &id, tab),
            Modal::Rename { .. } => rename(app, ui),
            Modal::Import { .. } => import(app, ui),
            Modal::Confirm { message, .. } => confirm(app, ui, &message),
            Modal::Settings => settings(app, ui),
            Modal::StepResult { index } => step_result(app, ui, index),
        }
    });
    if resp.should_close() {
        app.modal = None;
    }
}

fn head(app: &mut ApiCraftApp, ui: &mut Ui, title: &str) {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::icon_button(ui, icons::X, "Close").clicked() {
                app.modal = None;
            }
        });
    });
    ui.add_space(6.0);
}

/// Right-aligned dialog buttons. The row is kept to one line: a bare
/// `with_layout` here would take the modal's whole available height.
fn foot(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), add));
}

fn code(app: &mut ApiCraftApp, ui: &mut Ui, request_id: &str, curl: bool) {
    head(app, ui, "Save as code");
    let mut is_curl = curl;
    widgets::segmented(ui, &mut is_curl, &[(true, "curl"), (false, "PowerShell")]);
    if is_curl != curl {
        app.modal = Some(Modal::Code { request_id: request_id.to_string(), curl: is_curl });
    }
    ui.add_space(6.0);
    let target = if is_curl { Target::Curl } else { Target::PowerShell };
    let code = codegen::generate(&app.catalog, request_id, app.platform.now_ms(), target).unwrap_or_else(|e| format!("# {e}"));
    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| widgets::code_view(ui, "code-out", &code, Lang::Plain, &app.highlighter));
    help(ui, "Variables, params and auth are applied, so this runs the same request as Send.");
    foot(ui, |ui| {
        if accent_button(ui, format!("{} Copy", icons::COPY)).clicked() {
            app.actions.push(Action::CopyCode(request_id.to_string(), target));
        }
        if ui.button(format!("{} Save…", icons::FLOPPY_DISK)).clicked() {
            app.actions.push(Action::SaveCode(request_id.to_string(), target));
        }
    });
}

fn environments(app: &mut ApiCraftApp, ui: &mut Ui, selected: Option<String>) {
    head(app, ui, "Environments");
    let p = pal(ui);
    let sel = selected.or_else(|| app.catalog.active_environment.clone()).or_else(|| app.catalog.environments.first().map(|e| e.id.clone()));
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(190.0);
            for e in &app.catalog.environments {
                let active = app.catalog.active_environment.as_deref() == Some(&e.id);
                let label = if active { format!("{}   \u{2022} active", e.name) } else { e.name.clone() };
                if ui.add_sized([190.0, 28.0], egui::Button::selectable(sel.as_deref() == Some(&e.id), label)).clicked() {
                    app.modal = Some(Modal::Environments { selected: Some(e.id.clone()) });
                }
            }
            ui.add_space(6.0);
            if ui.button(format!("{} New environment", icons::PLUS)).clicked() {
                let e = apicraft_core::model::Environment::new(format!("Environment {}", app.catalog.environments.len() + 1));
                let id = e.id.clone();
                app.catalog.environments.push(e);
                if app.catalog.active_environment.is_none() {
                    app.catalog.active_environment = Some(id.clone());
                }
                app.modal = Some(Modal::Environments { selected: Some(id) });
            }
        });
        ui.separator();
        ui.vertical(|ui| {
            let Some(id) = sel.clone() else {
                ui.label(RichText::new("Environments hold variables like {{base_url}} that change between dev, staging and production.").color(p.muted));
                return;
            };
            let active = app.catalog.active_environment.as_deref() == Some(&id);
            let Some(e) = app.catalog.environments.iter_mut().find(|e| e.id == id) else { return };
            ui.label(RichText::new("Name").strong());
            ui.add(TextEdit::singleline(&mut e.name).desired_width(f32::INFINITY));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| kv_table(ui, &format!("env-{id}"), &mut e.variables, "variable", "value"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if !active && accent_button(ui, "Make active").clicked() {
                    app.catalog.active_environment = Some(id.clone());
                }
                if ui.button(format!("{} Export as Postman", icons::DOWNLOAD)).clicked() {
                    app.actions.push(Action::ExportEnvironment(id.clone()));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if danger_button(ui, format!("{} Delete", icons::TRASH)).clicked() {
                        app.modal =
                            Some(Modal::Confirm { message: "Delete this environment and its variables?".into(), action: Confirm::DeleteEnv(id.clone()) });
                    }
                });
            });
        });
    });
}

fn runtime(app: &mut ApiCraftApp, ui: &mut Ui) {
    head(app, ui, "Runtime variables");
    help(ui, "Values captured from responses. They override environment and collection variables until cleared, and are saved with the catalog.");
    ui.add_space(6.0);
    egui::ScrollArea::vertical().max_height(380.0).show(ui, |ui| kv_table(ui, "runtime", &mut app.catalog.runtime, "variable", "value"));
    foot(ui, |ui| {
        if accent_button(ui, "Done").clicked() {
            app.modal = None;
        }
        if danger_button(ui, format!("{} Clear all", icons::TRASH)).clicked() {
            app.catalog.runtime.clear();
        }
    });
}

fn collection(app: &mut ApiCraftApp, ui: &mut Ui, id: &str, tab: CollectionTab) {
    let name = app.catalog.collection(id).map(|c| c.name.clone()).unwrap_or_default();
    head(app, ui, &name);
    let mut t = tab;
    if widgets::tabs(
        ui,
        &mut t,
        &[(CollectionTab::General, "General".into(), 0), (CollectionTab::Variables, "Variables".into(), 0), (CollectionTab::Auth, "Auth".into(), 0)],
    ) {
        app.modal = Some(Modal::Collection { id: id.to_string(), tab: t });
    }
    let native = app.platform.native_grpc();
    let default_redirect = app.platform.default_redirect_uri();
    let now = app.platform.now_ms();
    let ApiCraftApp { catalog, actions, .. } = app;
    let Some(c) = catalog.collection_mut(id) else { return };
    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| match t {
        CollectionTab::General => {
            ui.label(RichText::new("Name").strong());
            ui.add(TextEdit::singleline(&mut c.name).desired_width(f32::INFINITY));
            ui.add_space(6.0);
            ui.label(RichText::new("Description").strong());
            ui.add(TextEdit::multiline(&mut c.description).desired_rows(5).desired_width(f32::INFINITY));
        }
        CollectionTab::Variables => {
            help(ui, "Available to every request in this collection as {{name}}. Environments override them.");
            ui.add_space(4.0);
            kv_table(ui, &format!("cvars-{id}"), &mut c.variables, "variable", "value");
        }
        CollectionTab::Auth => {
            help(ui, "Requests set to \u{201c}Inherit\u{201d} use this.");
            ui.add_space(4.0);
            let cx = AuthCtx { owner: id, allow_inherit: false, inherited: None, native, default_redirect, now };
            let mut ignore = None;
            widgets::auth_editor(ui, &mut c.auth, &cx, actions, &mut ignore);
        }
    });
    foot(ui, |ui| {
        if accent_button(ui, "Done").clicked() {
            app.modal = None;
        }
    });
}

fn rename(app: &mut ApiCraftApp, ui: &mut Ui) {
    head(app, ui, "Rename");
    let mut save = false;
    if let Some(Modal::Rename { value, .. }) = &mut app.modal {
        let r = ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
        if !r.has_focus() && !r.lost_focus() {
            r.request_focus();
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            save = true;
        }
    }
    foot(ui, |ui| {
        if accent_button(ui, "Save").clicked() {
            save = true;
        }
        if ui.button("Cancel").clicked() {
            app.modal = None;
        }
    });
    if save {
        app.actions.push(Action::RenameSave);
    }
}

/// Where the import dialog reads from.
#[derive(Clone, Copy, PartialEq)]
enum ImportSource {
    File,
    Url,
    Paste,
}

fn import(app: &mut ApiCraftApp, ui: &mut Ui) {
    head(app, ui, "Import");
    let p = pal(ui);
    let cols: Vec<(String, String)> = app.catalog.collections.iter().map(|c| (c.id.clone(), c.name.clone())).collect();
    let source_id = egui::Id::unique("import-source");
    let mut source = ui.ctx().data(|d| d.get_temp::<ImportSource>(source_id)).unwrap_or(ImportSource::File);
    let Some(Modal::Import { text, target, url }) = &mut app.modal else { return };

    ui.horizontal(|ui| {
        ui.label(RichText::new("Import into").strong());
        let label = target.as_ref().and_then(|t| cols.iter().find(|c| &c.0 == t)).map(|c| c.1.clone()).unwrap_or_else(|| "New collection".into());
        egui::ComboBox::from_id_salt("import-target").selected_text(label).width(260.0).show_ui(ui, |ui| {
            if ui.selectable_label(target.is_none(), "New collection").clicked() {
                *target = None;
            }
            for (id, name) in &cols {
                if ui.selectable_label(target.as_deref() == Some(id), name).clicked() {
                    *target = Some(id.clone());
                }
            }
        });
    });
    ui.add_space(8.0);
    widgets::segmented(ui, &mut source, &[(ImportSource::File, "File"), (ImportSource::Url, "URL"), (ImportSource::Paste, "Paste text")]);
    ui.ctx().data_mut(|d| d.insert_temp(source_id, source));
    ui.add_space(8.0);

    let mut go = false;
    egui::Frame::new().fill(p.surface2).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(10).inner_margin(16).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(150.0);
        match source {
            ImportSource::File => {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(icons::UPLOAD).size(28.0).color(p.faint));
                    ui.label(RichText::new("Choose a file, or drop one anywhere on the window").strong());
                    ui.add_space(8.0);
                    if accent_button(ui, format!("{} Choose file\u{2026}", icons::FOLDER_OPEN)).clicked() {
                        go = true;
                    }
                });
            }
            ImportSource::Url => {
                ui.label(RichText::new("Address of the document").strong());
                let r = ui.add(TextEdit::singleline(url).hint_text("https://petstore3.swagger.io/api/v3/openapi.json").desired_width(f32::INFINITY));
                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    go = true;
                }
                ui.add_space(4.0);
                help(ui, "Relative server URLs in an OpenAPI document are resolved against this address. For a WSDL, include ?wsdl; for OData, use the service\u{2019}s $metadata URL.");
            }
            ImportSource::Paste => {
                ui.add(
                    TextEdit::multiline(text)
                        .code_editor()
                        .desired_rows(7)
                        .desired_width(f32::INFINITY)
                        .hint_text("curl https://api.example.com -H 'Accept: application/json'\n\nor a Postman, OpenAPI or catalog JSON document"),
                );
            }
        }
    });
    ui.add_space(6.0);
    help(ui, "Reads OpenAPI / Swagger (JSON or YAML), WSDL, OData $metadata, GraphQL introspection results, Postman collections and environments, Fizl ApiCraft catalogs (.json, .json.gz) and curl commands.");
    foot(ui, |ui| {
        let label = match source {
            ImportSource::File => None,
            ImportSource::Url => Some("Import from URL"),
            ImportSource::Paste => Some("Import"),
        };
        if label.is_some_and(|l| accent_button(ui, l).clicked()) {
            go = true;
        }
        if ui.button("Cancel").clicked() {
            app.modal = None;
        }
    });
    if go {
        app.actions.push(match source {
            ImportSource::File => Action::OpenFiles,
            ImportSource::Url => Action::ImportUrl,
            ImportSource::Paste => Action::ImportText,
        });
    }
}

fn confirm(app: &mut ApiCraftApp, ui: &mut Ui, message: &str) {
    head(app, ui, "Delete");
    let p = pal(ui);
    ui.horizontal_top(|ui| {
        ui.label(RichText::new(icons::TRASH).size(22.0).color(p.danger));
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(message).color(p.text));
            ui.label(RichText::new("This can't be undone.").size(12.0).color(p.muted));
        });
    });
    foot(ui, |ui| {
        if danger_button(ui, "Delete").clicked() {
            app.actions.push(Action::Confirm);
        }
        if ui.button("Cancel").clicked() {
            app.modal = None;
        }
    });
}

fn settings(app: &mut ApiCraftApp, ui: &mut Ui) {
    head(app, ui, "Settings");
    let tab_id = egui::Id::unique("settings-tab");
    let mut network = ui.ctx().data(|d| d.get_temp::<bool>(tab_id)).unwrap_or(false);
    widgets::segmented(ui, &mut network, &[(false, "General"), (true, "Network")]);
    ui.ctx().data_mut(|d| d.insert_temp(tab_id, network));
    ui.add_space(8.0);
    if network {
        network_settings(app, ui);
        foot(ui, |ui| {
            if accent_button(ui, "Done").clicked() {
                app.modal = None;
            }
        });
        return;
    }
    let p = pal(ui);
    let st = app.platform.status();
    alert(ui, if st.level == crate::platform::Level::Warn { AlertKind::Warn } else { AlertKind::Ok }, &st.detail);
    ui.add_space(6.0);
    let inbox = app.inbox.clone();
    app.platform.settings_ui(ui, &inbox);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Theme").strong());
        let mut t = app.prefs.theme;
        widgets::segmented(ui, &mut t, &[(ThemeChoice::System, "System"), (ThemeChoice::Light, "Light"), (ThemeChoice::Dark, "Dark")]);
        if t != app.prefs.theme {
            app.prefs.theme = t;
            app.apply_theme(ui.ctx());
        }
    });
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Examples").strong());
        if ui.button("Add the example collection").on_hover_text("REST, SOAP, gRPC, WebSocket and MCP examples, including the MCP test mode").clicked() {
            app.actions.push(Action::AddExamples);
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Storage").strong());
    let gz = apicraft_core::store::to_gzip(&app.catalog).len();
    let json = apicraft_core::store::to_json(&app.catalog).len();
    ui.label(
        RichText::new(format!(
            "{}\nCompressed JSON: {} (from {} of JSON). Export from the sidebar to keep a copy elsewhere.",
            app.platform.storage_description(),
            human_size(gz),
            human_size(json)
        ))
        .color(p.muted),
    );
    ui.add_space(10.0);
    ui.label(RichText::new(format!("Fizl ApiCraft {}  \u{b7}  \u{a9} 2026 Fizl Ltd & Simon Soanes", env!("CARGO_PKG_VERSION"))).size(12.0).color(p.faint));
    foot(ui, |ui| {
        if accent_button(ui, "Done").clicked() {
            app.modal = None;
        }
    });
}

/// Proxy and TLS settings (kept in the catalog, so the CLI uses them too).
fn network_settings(app: &mut ApiCraftApp, ui: &mut Ui) {
    let p = pal(ui);
    let honoured = app.platform.native_grpc();
    let n = &mut app.catalog.network;
    if !honoured {
        alert(
            ui,
            AlertKind::Warn,
            "The browser uses its own proxy and certificate settings. These apply when requests go through the local agent, and in the desktop app and CLI.",
        );
        ui.add_space(6.0);
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Proxy").strong());
        widgets::segmented(ui, &mut n.proxy, &[(ProxyMode::System, "System"), (ProxyMode::None, "None"), (ProxyMode::Manual, "Manual")]);
    });
    match n.proxy {
        ProxyMode::System => help(ui, "Uses the HTTPS_PROXY, HTTP_PROXY, ALL_PROXY and NO_PROXY environment variables, when set."),
        ProxyMode::None => help(ui, "Connects directly, ignoring any proxy environment variables."),
        ProxyMode::Manual => {
            ui.add_space(4.0);
            egui::Grid::new("proxy-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Proxy URL");
                ui.add(TextEdit::singleline(&mut n.proxy_url).hint_text("http://proxy.corp:8080   or   socks5://127.0.0.1:1080").desired_width(360.0));
                ui.end_row();
                ui.label("Username");
                ui.add(TextEdit::singleline(&mut n.proxy_username).hint_text("optional").desired_width(360.0));
                ui.end_row();
                ui.label("Password");
                ui.add(TextEdit::singleline(&mut n.proxy_password).password(true).desired_width(360.0));
                ui.end_row();
            });
        }
    }
    if n.proxy != ProxyMode::None {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Bypass for");
            ui.add(TextEdit::singleline(&mut n.no_proxy).hint_text("localhost, 127.0.0.1, .internal.example.com").desired_width(360.0));
        });
    }
    ui.add_space(10.0);
    ui.separator();
    ui.label(RichText::new("Certificates").strong());
    ui.checkbox(&mut n.verify_tls, "Verify server certificates");
    if !n.verify_tls {
        alert(ui, AlertKind::Warn, "Any certificate is accepted, so connections can be intercepted. Use this only for local and test servers.");
    }
    ui.add_space(6.0);
    ui.label("Trusted CA certificates");
    ui.add(
        TextEdit::multiline(&mut n.ca_certificates)
            .hint_text("PEM text (-----BEGIN CERTIFICATE-----\u{2026}), or the path of a .pem / .crt file")
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    help(ui, "When set, servers are checked against these certificates instead of the built-in roots: for a company CA, or a test server's own certificate.");
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{}  Client certificates (mutual TLS) are set in a request's or collection's Auth tab. {{{{variables}}}} work in these fields.",
            icons::SHIELD_CHECK
        ))
        .size(12.0)
        .color(p.muted),
    );
}

fn step_result(app: &mut ApiCraftApp, ui: &mut Ui, index: usize) {
    let Some(rep) = app.run.as_ref().and_then(|r| r.reports.get(index)) else {
        app.modal = None;
        return;
    };
    let title = format!("Step {}: {}", rep.index + 1, rep.name);
    let request_id = rep.request_id.clone();
    let Some(o) = rep.outcome.clone() else { return };
    head(app, ui, &title);
    let pretty = apicraft_core::format::pretty_body(o.response.as_ref().map(|r| r.content_type()).unwrap_or(""), &o.body_text);
    let mut tab = ui.ctx().data(|d| d.get_temp::<RespTab>(egui::Id::unique("step-tab"))).unwrap_or(RespTab::Body);
    let mut raw = false;
    ui.allocate_ui(egui::vec2(ui.available_width(), 460.0), |ui| {
        response::outcome_view(ui, &o, &mut tab, &mut raw, &pretty, &app.highlighter, None, "step");
    });
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::unique("step-tab"), tab));
    foot(ui, |ui| {
        if accent_button(ui, "Close").clicked() {
            app.modal = None;
        }
        if ui.button("Open request").clicked() {
            app.actions.push(Action::Open(request_id.clone()));
        }
    });
}
