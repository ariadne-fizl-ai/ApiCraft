//! The Nullify.WebMail palette, applied to egui. Same tokens as its CSS
//! (`--bg`, `--surface`, `--accent`, ...) in light and dark.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, Visuals};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub surface2: Color32,
    pub surface3: Color32,
    pub sidebar: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_soft: Color32,
    pub accent_text: Color32,
    pub selected: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub ok_bg: Color32,
    pub ok_text: Color32,
    pub err_bg: Color32,
    pub err_text: Color32,
    pub warn_bg: Color32,
    pub warn_text: Color32,
    pub toast: Color32,
    pub toast_error: Color32,
    // methods
    pub m_get: Color32,
    pub m_post: Color32,
    pub m_put: Color32,
    pub m_patch: Color32,
    pub m_delete: Color32,
    pub m_other: Color32,
    pub m_soap: Color32,
    pub m_grpc: Color32,
    pub m_ws: Color32,
    pub m_mcp: Color32,
    pub m_gql: Color32,
    // syntax
    pub j_key: Color32,
    pub j_str: Color32,
    pub j_num: Color32,
    pub j_lit: Color32,
    pub x_tag: Color32,
    pub x_attr: Color32,
    pub x_val: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub const LIGHT: Palette = Palette {
    bg: hex(0xf4f5f7),
    surface: hex(0xffffff),
    surface2: hex(0xf6f7f9),
    surface3: hex(0xeceef2),
    sidebar: hex(0xfafbfc),
    text: hex(0x151a23),
    text2: hex(0x3d4556),
    muted: hex(0x6b7385),
    faint: hex(0x9aa1b0),
    border: hex(0xe4e7ec),
    border_strong: hex(0xd2d6de),
    accent: hex(0x3b5bdb),
    accent_hover: hex(0x3049c0),
    accent_soft: hex(0xedf1fd),
    accent_text: hex(0xffffff),
    selected: hex(0xe8edfc),
    danger: hex(0xd23f3f),
    danger_soft: hex(0xfdeeee),
    ok_bg: hex(0xe9f7ef),
    ok_text: hex(0x1b6b3e),
    err_bg: hex(0xfdecec),
    err_text: hex(0x9b1c1c),
    warn_bg: hex(0xfff7e6),
    warn_text: hex(0x8a5a00),
    toast: hex(0x1d222b),
    toast_error: hex(0x8f1f1f),
    m_get: hex(0x1b8a4a),
    m_post: hex(0xb86e00),
    m_put: hex(0x2f6fd6),
    m_patch: hex(0x7a4fd0),
    m_delete: hex(0xc93434),
    m_other: hex(0x6b7385),
    m_soap: hex(0x0e8a8a),
    m_grpc: hex(0xc2367a),
    m_ws: hex(0x4c5bd4),
    m_mcp: hex(0xc2410c),
    m_gql: hex(0xb5179e),
    j_key: hex(0x3049c0),
    j_str: hex(0x1b6b3e),
    j_num: hex(0xb86e00),
    j_lit: hex(0xc2367a),
    x_tag: hex(0x3049c0),
    x_attr: hex(0x7a4fd0),
    x_val: hex(0x1b6b3e),
};

pub const DARK: Palette = Palette {
    bg: hex(0x0f1115),
    surface: hex(0x16191f),
    surface2: hex(0x1b1f26),
    surface3: hex(0x232831),
    sidebar: hex(0x13161b),
    text: hex(0xe7e9ee),
    text2: hex(0xc3c8d2),
    muted: hex(0x8f97a8),
    faint: hex(0x687083),
    border: hex(0x262b34),
    border_strong: hex(0x343a46),
    accent: hex(0x6d8cff),
    accent_hover: hex(0x8aa3ff),
    accent_soft: hex(0x1d2540),
    accent_text: hex(0x0b0f1d),
    selected: hex(0x1e2640),
    danger: hex(0xff7272),
    danger_soft: hex(0x3a1c1f),
    ok_bg: hex(0x12301f),
    ok_text: hex(0x86e3ae),
    err_bg: hex(0x3a1717),
    err_text: hex(0xffb0b0),
    warn_bg: hex(0x33270f),
    warn_text: hex(0xf3c873),
    toast: hex(0x2a303b),
    toast_error: hex(0x8f1f1f),
    m_get: hex(0x5fd08e),
    m_post: hex(0xf3b44f),
    m_put: hex(0x79a6ff),
    m_patch: hex(0xb596ff),
    m_delete: hex(0xff8080),
    m_other: hex(0x8f97a8),
    m_soap: hex(0x4fd1d1),
    m_grpc: hex(0xff7ab8),
    m_ws: hex(0x8f9bff),
    m_mcp: hex(0xfb923c),
    m_gql: hex(0xf472d0),
    j_key: hex(0x8aa3ff),
    j_str: hex(0x86e3ae),
    j_num: hex(0xf3c873),
    j_lit: hex(0xff9ccc),
    x_tag: hex(0x8aa3ff),
    x_attr: hex(0xc3a6ff),
    x_val: hex(0x86e3ae),
};

pub fn palette(dark: bool) -> &'static Palette {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// The palette for whatever theme `ui` is drawing with.
pub fn pal(ui: &egui::Ui) -> &'static Palette {
    palette(ui.visuals().dark_mode)
}

pub const RADIUS_SM: u8 = 6;
pub const RADIUS: u8 = 10;
pub const RADIUS_LG: u8 = 14;

fn visuals(p: &Palette, dark: bool) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.surface;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.faint_bg_color = p.surface2;
    v.code_bg_color = p.surface3;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn_text;
    v.error_fg_color = p.danger;
    v.window_corner_radius = CornerRadius::same(RADIUS_LG);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_shadow = Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(if dark { 140 } else { 36 }) };
    v.popup_shadow = Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(if dark { 120 } else { 30 }) };
    v.selection.bg_fill = p.accent.gamma_multiply(0.28);
    v.selection.stroke = Stroke::new(1.0, p.accent);

    let r = CornerRadius::same(RADIUS_SM);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.surface;
    w.noninteractive.weak_bg_fill = p.surface;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text2);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.surface3;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = p.surface3;
    w.hovered.weak_bg_fill = p.surface2;
    w.hovered.bg_stroke = Stroke::new(1.0, p.faint);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.surface3;
    w.active.weak_bg_fill = p.surface3;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.surface3;
    w.open.weak_bg_fill = p.surface3;
    w.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;
    v
}

/// Installs the palette and spacing for both themes.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Light, visuals(&LIGHT, false));
    ctx.set_visuals_of(Theme::Dark, visuals(&DARK, true));
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |s| {
            s.spacing.item_spacing = egui::vec2(8.0, 6.0);
            s.spacing.button_padding = egui::vec2(10.0, 5.0);
            s.spacing.interact_size = egui::vec2(36.0, 28.0);
            s.spacing.menu_margin = Margin::same(6);
            s.spacing.window_margin = Margin::same(16);
            s.spacing.combo_width = 120.0;
            s.text_styles = [
                (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
                (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

/// Fonts: a system UI font (e.g. Segoe UI) ahead of egui's bundled one when
/// the platform hands one over, and the Phosphor icons as a fallback so icon
/// codepoints work inside any text.
pub fn install_fonts(ctx: &egui::Context, ui_font: Option<Vec<u8>>) {
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("phosphor".into(), Arc::new(egui::FontData::from_static(crate::icons::FONT_BYTES)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.push("phosphor".into());
        }
    }
    if let Some(bytes) = ui_font {
        fonts.font_data.insert("system-ui".into(), Arc::new(egui::FontData::from_owned(bytes)));
        if let Some(list) = fonts.families.get_mut(&FontFamily::Proportional) {
            list.insert(0, "system-ui".into());
        }
    }
    ctx.set_fonts(fonts);
}

pub fn method_color(p: &Palette, protocol: apicraft_core::Protocol, method: &str) -> Color32 {
    use apicraft_core::Protocol::*;
    match protocol {
        Soap => p.m_soap,
        Grpc => p.m_grpc,
        WebSocket => p.m_ws,
        Mcp => p.m_mcp,
        GraphQl => p.m_gql,
        Rest | OData => match method.to_ascii_uppercase().as_str() {
            "GET" => p.m_get,
            "POST" => p.m_post,
            "PUT" => p.m_put,
            "PATCH" => p.m_patch,
            "DELETE" => p.m_delete,
            _ => p.m_other,
        },
    }
}

pub fn method_label(protocol: apicraft_core::Protocol, method: &str) -> String {
    use apicraft_core::Protocol::*;
    match protocol {
        Soap => "SOAP".into(),
        Grpc => "gRPC".into(),
        WebSocket => "WS".into(),
        Mcp => "MCP".into(),
        GraphQl => "GQL".into(),
        Rest | OData => match method.to_ascii_uppercase().as_str() {
            "DELETE" => "DEL".into(),
            "OPTIONS" => "OPT".into(),
            m => m.chars().take(5).collect(),
        },
    }
}

/// (fill, text) for an HTTP status pill.
pub fn status_colors(p: &Palette, status: u16) -> (Color32, Color32) {
    match status {
        200..=299 => (p.ok_bg, p.ok_text),
        300..=399 => (p.accent_soft, p.accent),
        400..=499 => (p.warn_bg, p.warn_text),
        _ => (p.err_bg, p.err_text),
    }
}
