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
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Strata")
        .with_app_id("strata")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([900.0, 600.0])
        .with_drag_and_drop(true);
    if let Ok(icon) =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/icons/icon-192.png"))
    {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Strata",
        options,
        Box::new(|cc| Ok(Box::new(app::StrataApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {
    use wasm_bindgen::JsCast;
    eframe::WebLogger::init(log::LevelFilter::Info).ok();
    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("window")
            .document()
            .expect("document");
        let canvas = document
            .get_element_by_id("strata_canvas")
            .expect("missing #strata_canvas")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#strata_canvas is not a canvas");
        let result = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(|cc| Ok(Box::new(app::StrataApp::new(cc)))),
            )
            .await;
        if let Some(loading) = document.get_element_by_id("loading") {
            match result {
                Ok(()) => {
                    // Fade out (CSS), then remove. Background tabs skip
                    // transitions, so removal must not wait for one.
                    let _ = loading.class_list().add_1("done");
                    let remove =
                        wasm_bindgen::closure::Closure::once_into_js(move || loading.remove());
                    if let Some(w) = web_sys::window() {
                        let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(
                            remove.unchecked_ref(),
                            400,
                        );
                    }
                }
                Err(e) => {
                    let _ = loading.class_list().add_1("failed");
                    if let Some(msg) = document.get_element_by_id("loading-status") {
                        msg.set_text_content(Some(
                            "Strata could not start. It needs WebGL 2: update your browser or turn on hardware acceleration, then reload.",
                        ));
                    }
                    if let Some(detail) = document.get_element_by_id("loading-detail") {
                        detail.set_text_content(Some(&format!("{e:?}")));
                    }
                }
            }
        }
    });
}
