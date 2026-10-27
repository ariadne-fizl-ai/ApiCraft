//! The window icon.

pub fn app_icon() -> egui::IconData {
    let size = 64;
    egui::IconData { rgba: apicraft_ui::brand::icon_rgba(size), width: size, height: size }
}
