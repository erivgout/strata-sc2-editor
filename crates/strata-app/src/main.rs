#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod io;
mod mcp_link;
mod panels;
mod render;
mod state;
mod theme;
mod tools;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result<()> {
    env_logger::init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Strata")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("Strata", options, Box::new(|cc| Ok(Box::new(app::StrataApp::new(cc)))))
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window().expect("window").document().expect("document");
        let canvas = document
            .get_element_by_id("strata_canvas")
            .expect("missing #strata_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#strata_canvas is not a canvas");
        let result = eframe::WebRunner::new()
            .start(canvas, eframe::WebOptions::default(), Box::new(|cc| Ok(Box::new(app::StrataApp::new(cc)))))
            .await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match result {
                Ok(()) => loading.remove(),
                Err(e) => loading.set_inner_html(&format!("<p>Strata could not start: {e:?}</p><p>Your browser needs WebGL 2.</p>")),
            }
        }
    });
}
