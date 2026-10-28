//! The window: sidebar, the current view, dialogs and toasts.

mod modals;
mod request;
mod response;
mod sidebar;
pub mod widgets;
mod workflow;

use egui::{Align, Align2, CornerRadius, Frame, Layout, Margin, RichText, Stroke, Ui};

use crate::actions::Action;
use crate::app::{ApiCraftApp, Modal, ThemeChoice, View};
use crate::icons;
use crate::platform::Level;
use crate::theme::{pal, RADIUS};
use widgets::accent_button;

pub fn frame(app: &mut ApiCraftApp, ui: &mut Ui) {
    let p = pal(ui);
    if app.prefs.sidebar_open {
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(272.0)
            .size_range(220.0..=440.0)
            .frame(Frame::new().fill(p.sidebar).inner_margin(Margin { left: 10, right: 10, top: 12, bottom: 8 }))
            .show(ui, |ui| sidebar::show(app, ui));
    } else {
        egui::Panel::left("sidebar-rail")
            .resizable(false)
            .exact_size(56.0)
            .frame(Frame::new().fill(p.sidebar).inner_margin(Margin { left: 6, right: 6, top: 12, bottom: 10 }))
            .show(ui, |ui| sidebar::rail(app, ui));
    }
    egui::CentralPanel::default().frame(Frame::new().fill(p.surface)).show(ui, |ui| match app.view.clone() {
        View::Request(id) if app.catalog.find_request(&id).is_some() => request::show(app, ui, &id),
        View::Workflow(id) if app.catalog.workflow(&id).is_some() => workflow::show(app, ui, &id),
        _ => welcome(app, ui),
    });
    modals::show(app, ui.ctx());
    toasts(app, ui.ctx());
}

/// The bar across the top of a view. `title` draws the left part.
pub(crate) fn topbar(app: &mut ApiCraftApp, ui: &mut Ui, title: impl FnOnce(&mut ApiCraftApp, &mut Ui)) {
    let p = pal(ui);
    Frame::new().inner_margin(Margin { left: 14, right: 12, top: 8, bottom: 8 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            let tools_w = 540.0_f32.min(ui.available_width() * 0.62);
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width() - tools_w, 40.0), Layout::top_down(Align::Min), |ui| title(app, ui));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // theme
                let dark = ui.visuals().dark_mode;
                if widgets::icon_button(ui, if dark { icons::SUN } else { icons::MOON }, "Toggle light/dark").clicked() {
                    app.prefs.theme = if dark { ThemeChoice::Light } else { ThemeChoice::Dark };
                    app.apply_theme(ui.ctx());
                }
                // how requests are sent
                let st = app.platform.status();
                let dot = match st.level {
                    Level::Good => p.ok_text,
                    Level::Warn => p.warn_text,
                    Level::Neutral => p.faint,
                };
                if widgets::status_button(ui, dot, &st.label).on_hover_text(format!("{}\n\nClick for settings.", st.detail)).clicked() {
                    app.modal = Some(Modal::Settings);
                }
                // captured values
                let n = app.catalog.runtime.len();
                let tip = "Variables captured from responses while sending requests and running workflows. Click to view, edit or clear them.";
                if widgets::count_button(ui, icons::VARIABLE, "Variables", n).on_hover_text(tip).clicked() {
                    app.modal = Some(Modal::Runtime);
                }
                if widgets::icon_button(ui, icons::GEAR, "Manage environments").clicked() {
                    app.modal = Some(Modal::Environments { selected: None });
                }
                // environment picker
                let current = app.catalog.active_env().map(|e| e.name.clone()).unwrap_or_else(|| "No environment".into());
                let mut active = app.catalog.active_environment.clone();
                egui::ComboBox::from_id_salt("env").selected_text(format!("{} {current}", icons::GLOBE)).width(170.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut active, None, "No environment");
                    for e in &app.catalog.environments {
                        ui.selectable_value(&mut active, Some(e.id.clone()), &e.name);
                    }
                });
                app.catalog.active_environment = active;
                ui.add_space(4.0);
                let redo = ui.add_enabled(app.history.can_redo(), egui::Button::new(RichText::new(icons::ARROW_U_UP_RIGHT).size(16.0)).frame(false));
                if redo.on_hover_text("Redo (Ctrl+Y)").clicked() {
                    app.actions.push(Action::Redo);
                }
                if widgets::icon_button(ui, icons::ARROW_U_UP_LEFT, "Undo (Ctrl+Z): brings back deleted steps, requests, folders and other changes").clicked() {
                    app.actions.push(Action::Undo);
                }
            });
        });
    });
    let r = ui.min_rect();
    ui.painter().hline(r.x_range(), ui.cursor().top(), Stroke::new(1.0, p.border));
}

fn welcome(app: &mut ApiCraftApp, ui: &mut Ui) {
    topbar(app, ui, |_, ui| {
        ui.label(RichText::new("Fizl ApiCraft").strong().size(17.0));
    });
    let p = pal(ui);
    ui.add_space(ui.available_height() * 0.25);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icons::SEND).size(34.0).color(p.faint));
        ui.label(RichText::new("Fizl ApiCraft").strong().size(18.0));
        ui.label(
            RichText::new("Pick a request in the sidebar, or start a new one. REST, GraphQL, OData, SOAP, gRPC, WebSocket and MCP are all supported.")
                .color(p.muted),
        );
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space((ui.available_width() - 230.0).max(0.0) / 2.0);
            if accent_button(ui, format!("{} New request", icons::PLUS)).clicked() {
                app.actions.push(Action::NewRequest(None));
            }
            if ui.button(format!("{} Import", icons::UPLOAD)).clicked() {
                app.modal = Some(Modal::Import { text: String::new(), target: None, url: String::new() });
            }
        });
    });
}

fn toasts(app: &mut ApiCraftApp, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let dark = ctx.theme() == egui::Theme::Dark;
    let p = crate::theme::palette(dark);
    let mut close = None;
    egui::Area::new(egui::Id::unique("toasts")).anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -20.0)).order(egui::Order::Foreground).interactable(true).show(
        ctx,
        |ui| {
            for (i, t) in app.toasts.iter().enumerate() {
                Frame::new()
                    .fill(if t.error { p.toast_error } else { p.toast })
                    .corner_radius(CornerRadius::same(RADIUS))
                    .inner_margin(Margin { left: 14, right: 8, top: 8, bottom: 8 })
                    .shadow(ctx.style_of(ctx.theme()).visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_max_width(520.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(&t.text).color(egui::Color32::from_rgb(241, 243, 246))).wrap());
                            if ui.add(egui::Button::new(RichText::new(icons::X).color(egui::Color32::from_gray(200))).frame(false)).clicked() {
                                close = Some(i);
                            }
                        });
                    });
                ui.add_space(6.0);
            }
        },
    );
    if let Some(i) = close {
        app.toasts.remove(i);
    }
}
