//! The response pane, and the WebSocket session view.

use apicraft_core::engine::Outcome;
use apicraft_core::format::human_size;
use apicraft_core::model::{Protocol, VarScope};
use egui::{Align, Layout, RichText, TextEdit, Ui};

use super::widgets::{self, alert, chip, help, pill, AlertKind};
use crate::actions::Action;
use crate::app::{ApiCraftApp, RespTab, WsDir};
use crate::highlight::{Highlighter, Lang};
use crate::icons;
use crate::theme::{pal, status_colors, Palette};

pub fn show(app: &mut ApiCraftApp, ui: &mut Ui, id: &str) {
    let p = pal(ui);
    if app.catalog.find_request(id).map(|(_, r)| r.protocol == Protocol::WebSocket).unwrap_or(false) {
        return ws_panel(app, ui, id);
    }
    if app.sending.contains(id) {
        ui.add_space(ui.available_height() * 0.3);
        ui.vertical_centered(|ui| {
            ui.spinner();
            ui.label(RichText::new("Sending\u{2026}").strong());
        });
        return;
    }
    let Some(o) = app.outcomes.get(id) else {
        ui.add_space(ui.available_height() * 0.3);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(icons::SEND).size(30.0).color(p.faint));
            ui.label(RichText::new("No response yet").strong());
            ui.label(RichText::new("Send the request to see the response here.").color(p.muted));
            ui.label(RichText::new("Ctrl + Enter").size(12.0).color(p.faint));
        });
        return;
    };
    let pretty = app.pretty.get(id).cloned().unwrap_or_default();
    let ApiCraftApp { resp_tab, resp_raw, highlighter, actions, .. } = app;
    outcome_view(ui, o, resp_tab, resp_raw, &pretty, highlighter, Some(actions), id);
}

pub fn summary_row(ui: &mut Ui, o: &Outcome) {
    let p = pal(ui);
    ui.horizontal_wrapped(|ui| {
        match (&o.response, &o.error) {
            (Some(r), _) => {
                let (fill, color) = status_colors(p, r.status);
                pill(ui, format!("{} {}", r.status, r.status_text), fill, color);
            }
            (None, Some(_)) => {
                pill(ui, "Error", p.err_bg, p.err_text);
            }
            _ => {}
        }
        if let Some(g) = &o.grpc {
            let (fill, color) = if g.ok() { (p.ok_bg, p.ok_text) } else { (p.err_bg, p.err_text) };
            pill(ui, format!("gRPC {} {}", g.status.unwrap_or(0), g.status_name()), fill, color);
        }
        if let Some(r) = &o.response {
            ui.label(RichText::new(format!("{} {:.0} ms", icons::TIMER, r.elapsed_ms)).color(p.muted).size(12.5));
            ui.label(RichText::new(human_size(r.body.len())).color(p.muted).size(12.5));
        }
        if !o.checks.is_empty() {
            let passed = o.checks.iter().filter(|c| c.passed).count();
            let all = passed == o.checks.len();
            ui.label(
                RichText::new(format!("{} {passed}/{} checks", if all { icons::CHECK } else { icons::X }, o.checks.len()))
                    .color(if all { p.ok_text } else { p.err_text })
                    .size(12.5),
            );
        }
        if !o.extracted.is_empty() {
            ui.label(RichText::new(format!("{} {} captured", icons::VARIABLE, o.extracted.len())).color(p.ok_text).size(12.5));
        }
    });
}

fn alerts(ui: &mut Ui, o: &Outcome) {
    if let Some(e) = &o.error {
        alert(ui, AlertKind::Error, e);
    }
    if let Some(g) = &o.grpc {
        if !g.ok() && !g.message.is_empty() {
            alert(ui, AlertKind::Error, &g.message);
        }
    }
    if let Some(f) = &o.fault {
        alert(ui, AlertKind::Error, f);
    }
    for w in &o.warnings {
        alert(ui, AlertKind::Warn, w);
    }
    for e in &o.extract_errors {
        alert(ui, AlertKind::Warn, e);
    }
}

/// The full response view. `actions` is `None` in read-only places (step results).
#[allow(clippy::too_many_arguments)]
pub fn outcome_view(ui: &mut Ui, o: &Outcome, tab: &mut RespTab, raw: &mut bool, pretty: &str, hl: &Highlighter, actions: Option<&mut Vec<Action>>, id: &str) {
    let p = pal(ui);
    summary_row(ui, o);
    ui.add_space(4.0);
    alerts(ui, o);
    let header_n = o.response.as_ref().map(|r| r.headers.len() + r.trailers.len()).unwrap_or(0);
    widgets::tabs(
        ui,
        tab,
        &[
            (RespTab::Body, "Body".into(), 0),
            (RespTab::Headers, "Headers".into(), header_n),
            (RespTab::Checks, "Checks".into(), o.checks.len()),
            (RespTab::Captured, "Captured".into(), o.extracted.len()),
            (RespTab::Sent, "Request".into(), 0),
        ],
    );
    match *tab {
        RespTab::Body => {
            let ct =
                if o.grpc.is_some() { "application/json".to_string() } else { o.response.as_ref().map(|r| r.content_type().to_string()).unwrap_or_default() };
            ui.horizontal(|ui| {
                let mut r = *raw;
                widgets::segmented(ui, &mut r, &[(false, "Pretty"), (true, "Raw")]);
                *raw = r;
                ui.label(RichText::new(&ct).size(12.0).color(p.muted));
                if let Some(acts) = actions {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(format!("{} Save", icons::FLOPPY_DISK)).clicked() {
                            acts.push(Action::SaveResponse);
                        }
                        if ui.small_button(format!("{} Copy", icons::COPY)).clicked() {
                            ui.ctx().copy_text(o.body_text.clone());
                        }
                    });
                }
            });
            ui.add_space(4.0);
            if o.body_text.is_empty() {
                help(ui, "(empty body)");
            } else {
                let text = if *raw || pretty.is_empty() { o.body_text.as_str() } else { pretty };
                let lang = Lang::detect(&ct, text);
                egui::ScrollArea::both().id_salt(("resp-body", id)).auto_shrink([false, false]).show(ui, |ui| {
                    widgets::code_view(ui, ("resp", id, *raw), text, lang, hl);
                });
            }
        }
        RespTab::Headers => {
            egui::ScrollArea::vertical().id_salt(("resp-h", id)).auto_shrink([false, false]).show(ui, |ui| {
                if let Some(r) = &o.response {
                    widgets::pairs_table(ui, "resp-headers", &r.headers);
                    if !r.trailers.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Trailers").strong());
                        widgets::pairs_table(ui, "resp-trailers", &r.trailers);
                    }
                }
            });
        }
        RespTab::Checks => checks_list(ui, o, p),
        RespTab::Captured => {
            if o.extracted.is_empty() {
                help(ui, "Nothing captured. Add capture rules in the request's Capture tab.");
            } else {
                egui::Grid::new("captured").num_columns(3).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                    for (scope, k, v) in &o.extracted {
                        ui.label(RichText::new(format!("{{{{{k}}}}}")).monospace().strong());
                        let r = ui.add(egui::Label::new(RichText::new(v).monospace()).wrap().selectable(true));
                        crate::textmenu::copy_menu(&r, v);
                        ui.label(RichText::new(if *scope == VarScope::Environment { "environment" } else { "runtime" }).color(p.muted).size(12.0));
                        ui.end_row();
                    }
                });
            }
        }
        RespTab::Sent => sent_view(ui, o, hl, id),
    }
}

pub fn checks_list(ui: &mut Ui, o: &Outcome, p: &Palette) {
    if o.checks.is_empty() {
        help(ui, "No checks. Add some in the request's Checks tab.");
    }
    for c in &o.checks {
        ui.horizontal(|ui| {
            ui.label(RichText::new(if c.passed { icons::CHECK } else { icons::X }).color(if c.passed { p.ok_text } else { p.danger }));
            ui.add(egui::Label::new(&c.label).wrap());
            ui.add(egui::Label::new(RichText::new(&c.detail).color(p.muted).size(12.0)).wrap());
        });
    }
}

pub fn sent_view(ui: &mut Ui, o: &Outcome, hl: &Highlighter, id: &str) {
    let Some(s) = &o.sent else {
        help(ui, "The request was not sent.");
        return;
    };
    egui::ScrollArea::vertical().id_salt(("sent", id)).auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&s.method).strong());
            let r = ui.add(egui::Label::new(RichText::new(&s.url).monospace()).wrap().selectable(true));
            crate::textmenu::copy_menu(&r, &s.url);
            if s.grpc {
                chip(ui, "native gRPC");
            }
        });
        ui.add_space(6.0);
        widgets::pairs_table(ui, "sent-headers", &s.headers);
        if !s.body.is_empty() {
            ui.add_space(8.0);
            let text = if o.grpc.is_some() {
                o.prepared.as_ref().and_then(|p| match &p.body {
                    apicraft_core::prepare::PreparedBody::Text(t) => Some(t.clone()),
                    _ => None,
                })
            } else {
                None
            }
            .unwrap_or_else(|| match std::str::from_utf8(&s.body) {
                Ok(t) => t.to_string(),
                Err(_) => format!("({} of binary data)", human_size(s.body.len())),
            });
            let lang = Lang::detect(s.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.as_str()).unwrap_or(""), &text);
            widgets::code_view(ui, ("sent-body", id), &text, lang, hl);
        }
    });
}

fn ws_panel(app: &mut ApiCraftApp, ui: &mut Ui, id: &str) {
    let p = pal(ui);
    let (connected, connecting) = match &app.ws {
        Some(w) if w.request_id == id => (w.open, !w.open && !w.closed),
        _ => (false, false),
    };
    ui.horizontal(|ui| {
        if connected {
            pill(ui, "Connected", p.ok_bg, p.ok_text);
        } else if connecting {
            pill(ui, "Connecting\u{2026}", p.accent_soft, p.accent);
        } else {
            chip(ui, "Disconnected");
        }
        if let Some(w) = app.ws.as_ref().filter(|w| w.request_id == id) {
            let n = w.log.iter().filter(|e| matches!(e.dir, WsDir::In | WsDir::Out)).count();
            ui.label(RichText::new(format!("{n} messages")).color(p.muted).size(12.5));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("Clear log").clicked() {
                if let Some(w) = &mut app.ws {
                    w.log.clear();
                }
            }
        });
    });
    ui.add_space(4.0);
    let compose_h = 110.0;
    let log_h = (ui.available_height() - compose_h).max(120.0);
    egui::ScrollArea::vertical().id_salt("ws-log").max_height(log_h).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        match app.ws.as_ref().filter(|w| w.request_id == id) {
            None => help(ui, "Connect to start a session. Sent and received messages appear here."),
            Some(w) => {
                for e in &w.log {
                    let (fill, arrow, color) = match e.dir {
                        WsDir::Out => (p.accent_soft, "\u{2191}", p.accent),
                        WsDir::In => (p.surface2, "\u{2193}", p.m_get),
                        WsDir::Info => (egui::Color32::TRANSPARENT, "\u{2022}", p.muted),
                        WsDir::Error => (p.err_bg, "!", p.err_text),
                    };
                    egui::Frame::new().fill(fill).corner_radius(6).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(arrow).strong().color(color));
                            ui.label(RichText::new(format!("{:>6.2}s", e.at - w.started)).monospace().size(11.5).color(p.faint));
                            let text = if matches!(e.dir, WsDir::In | WsDir::Out) {
                                apicraft_core::format::pretty_json(&e.text).unwrap_or_else(|| e.text.clone())
                            } else {
                                e.text.clone()
                            };
                            let r = ui.add(
                                egui::Label::new(RichText::new(&text).monospace().color(if matches!(e.dir, WsDir::Info) { p.muted } else { p.text }))
                                    .wrap()
                                    .selectable(true),
                            );
                            crate::textmenu::copy_menu(&r, &text);
                        });
                    });
                }
            }
        }
    });
    ui.add_space(6.0);
    let resp =
        ui.add(TextEdit::multiline(&mut app.ws_compose).hint_text("Message to send (Ctrl+Enter)").code_editor().desired_rows(3).desired_width(f32::INFINITY));
    let _ = resp;
    ui.horizontal(|ui| {
        if ui.add_enabled(connected, egui::Button::new(RichText::new(format!("{} Send", icons::SEND)).color(p.accent_text)).fill(p.accent)).clicked() {
            app.actions.push(Action::WsSendCompose);
        }
    });
}
