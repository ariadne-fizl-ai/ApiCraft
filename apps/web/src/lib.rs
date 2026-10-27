//! Fizl ApiCraft in the browser: the shared egui UI compiled to WebAssembly
//! and drawn into a canvas by eframe's web runner.

#[cfg(target_arch = "wasm32")]
mod dom;
#[cfg(target_arch = "wasm32")]
mod net;
#[cfg(target_arch = "wasm32")]
mod platform;

#[cfg(target_arch = "wasm32")]
mod web {
    use apicraft_ui::ApiCraftApp;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    const PREFS_KEY: &str = "apicraft.prefs";

    struct Host(ApiCraftApp);

    impl eframe::App for Host {
        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            self.0.ui(ui);
        }

        fn save(&mut self, storage: &mut dyn eframe::Storage) {
            storage.set_string(PREFS_KEY, self.0.prefs_string());
            self.0.flush();
        }

        fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
            Some(&mut *self)
        }
    }

    #[wasm_bindgen(start)]
    pub fn start() {
        console_error_panic_hook::set_once();
        apicraft_core::ids::seed((js_sys::Math::random() * 9.007e15) as u64 ^ crate::dom::now_ms());
        wasm_bindgen_futures::spawn_local(async {
            let document = crate::dom::window().document().expect("document");
            let canvas = document.get_element_by_id("app").expect("a <canvas id=\"app\">").dyn_into::<web_sys::HtmlCanvasElement>().expect("#app is a canvas");
            let result = eframe::WebRunner::new()
                .start(
                    canvas,
                    eframe::WebOptions::default(),
                    Box::new(|cc| {
                        let prefs = cc.storage.and_then(|s| s.get_string(PREFS_KEY).or_else(|| s.get_string("apipost.prefs")));
                        let platform = crate::platform::WebPlatform::new();
                        Ok(Box::new(Host(ApiCraftApp::new(&cc.egui_ctx, Box::new(platform), prefs, None))))
                    }),
                )
                .await;
            // Hide the loading message, or show what went wrong.
            if let Some(el) = document.get_element_by_id("loading") {
                match result {
                    Ok(()) => el.remove(),
                    Err(e) => el.set_inner_html(&format!("<p>Fizl ApiCraft could not start: {}</p>", crate::dom::js_err(&e))),
                }
            }
        });
    }
}
