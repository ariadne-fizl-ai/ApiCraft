//! Fizl ApiCraft for the desktop: the shared egui UI running natively
//! (eframe + wgpu), sending requests in-process with the native stack.

// Release builds are GUI apps on Windows (no console window); debug builds
// keep the console so logs and panics show up in the IDE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod icon;
mod platform;

use apicraft_ui::ApiCraftApp;

const PREFS_KEY: &str = "apicraft.prefs";

struct Host(ApiCraftApp);

impl eframe::App for Host {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }

    /// Called periodically and on shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(PREFS_KEY, self.0.prefs_string());
        self.0.flush();
    }
}

fn main() -> eframe::Result {
    apicraft_core::ids::seed(apicraft_agent::now_ms() ^ ((std::process::id() as u64) << 32));
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(apicraft_ui::APP_NAME)
            .with_app_id("fizl-apicraft")
            .with_inner_size([1360.0, 880.0])
            .with_min_inner_size([760.0, 520.0])
            .with_icon(icon::app_icon()),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        apicraft_ui::APP_NAME,
        options,
        Box::new(|cc| {
            let prefs = cc.storage.and_then(|s| s.get_string(PREFS_KEY).or_else(|| s.get_string("apipost.prefs")));
            let platform = platform::DesktopPlatform::new();
            let app = ApiCraftApp::new(&cc.egui_ctx, Box::new(platform), prefs, platform::system_ui_font());
            Ok(Box::new(Host(app)))
        }),
    )
}
