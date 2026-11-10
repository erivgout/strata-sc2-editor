//! Export map image: the map seen straight down (north up), rendered
//! offscreen by the viewport's own renderer — real terrain textures, unit
//! and doodad models, water — without editor overlays, and saved as a PNG.
//!
//! Flow: the dialog starts an [`ExportJob`] on the viewport. Each frame the
//! viewport loads the job's models ([`CaptureModels`]); once nothing is
//! loading (or [`WAIT_SECONDS`] have passed) it asks the paint callback to
//! render the image ([`CaptureRequest`]). The pixels are encoded as PNG off
//! the UI thread where possible, and the app then asks where to save them.

use glam::{Mat4, Vec3};
use strata_core::objects::ObjectId;
use strata_core::MapDocument;

use crate::render::gl::{CaptureRequest, CapturedImage, FrameParams};
use crate::render::models::CaptureModels;

/// Image widths offered, in pixels.
pub const WIDTHS: [u32; 4] = [512, 1024, 2048, 4096];
/// How long an export waits for models before rendering without them.
pub const WAIT_SECONDS: f64 = 20.0;
/// Haze start for exports: far enough that there is none.
pub const NO_FOG: f32 = 1.0e9;

/// What part of the map to export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Map,
    Playable,
    Region(ObjectId),
}

/// The export dialog's choices.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportForm {
    pub area: Area,
    pub width: u32,
    pub units: bool,
    pub doodads: bool,
}

impl ExportForm {
    /// Defaults: the selected region if there is one, else the playable area.
    pub fn new(selected_region: Option<ObjectId>) -> Self {
        Self {
            area: selected_region.map_or(Area::Playable, Area::Region),
            width: 2048,
            units: true,
            doodads: true,
        }
    }
}

/// World rect [x0, y0, x1, y1] an area covers, clamped to the map.
pub fn area_rect(doc: &MapDocument, area: Area) -> Result<[f32; 4], String> {
    let (w, h) = (doc.terrain.width as f32, doc.terrain.height as f32);
    let r = match area {
        Area::Map => [0.0, 0.0, w, h],
        Area::Playable => {
            let [l, b, r, t] = doc.info.playable;
            [l as f32, b as f32, r as f32, t as f32]
        }
        Area::Region(id) => {
            let region = doc
                .regions
                .iter()
                .find(|r| r.id == id)
                .ok_or("That region no longer exists. Pick another area.")?;
            let mut lo = glam::Vec2::splat(f32::MAX);
            let mut hi = glam::Vec2::splat(f32::MIN);
            for s in &region.shapes {
                let (a, b) = s.bounds();
                lo = lo.min(a);
                hi = hi.max(b);
            }
            if region.shapes.is_empty() {
                return Err(format!("Region \"{}\" has no shapes, so it has no area to export.", region.name));
            }
            [lo.x, lo.y, hi.x, hi.y]
        }
    };
    let r = [r[0].clamp(0.0, w), r[1].clamp(0.0, h), r[2].clamp(0.0, w), r[3].clamp(0.0, h)];
    if r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0 {
        return Err("That area is smaller than one cell inside the map. Pick a larger area.".into());
    }
    Ok(r)
}

/// Pixel size for `rect` at `width`, keeping its aspect ratio, scaled down
/// to fit `max_side` (0 = unknown, no limit). Returns (width, height, clamped).
pub fn image_size(rect: [f32; 4], width: u32, max_side: u32) -> (u32, u32, bool) {
    let aspect = (rect[3] - rect[1]) / (rect[2] - rect[0]);
    let w = width.max(1) as f32;
    let h = (w * aspect).round().max(1.0);
    let limit = if max_side == 0 { f32::MAX } else { max_side as f32 };
    let scale = (limit / w).min(limit / h).min(1.0);
    let (w, h) = ((w * scale).floor().max(1.0) as u32, (h * scale).floor().max(1.0) as u32);
    (w, h, scale < 1.0)
}

/// Camera straight above the middle of `rect`, orthographic over exactly
/// that rect, north up. `heights` is the terrain's (lowest, highest).
pub fn capture_camera(rect: [f32; 4], heights: (f32, f32)) -> (Mat4, Vec3) {
    let c = Vec3::new((rect[0] + rect[2]) * 0.5, (rect[1] + rect[3]) * 0.5, heights.1);
    // Room above the highest ground for tall models.
    let eye = c + Vec3::Z * 100.0;
    let view = Mat4::look_at_rh(eye, Vec3::new(c.x, c.y, heights.0), Vec3::Y);
    let (hw, hh) = ((rect[2] - rect[0]) * 0.5, (rect[3] - rect[1]) * 0.5);
    let depth = 100.0 + (heights.1 - heights.0) + 50.0;
    let proj = Mat4::orthographic_rh_gl(-hw, hw, -hh, hh, 0.1, depth);
    (proj * view, eye)
}

/// The frame parameters of an export: the viewport's terrain look without
/// grid, contours, pathing, the playable-area outline or haze.
pub fn frame_params(base: &FrameParams, rect: [f32; 4], heights: (f32, f32)) -> FrameParams {
    let (view_proj, eye) = capture_camera(rect, heights);
    FrameParams {
        view_proj,
        eye,
        playable: [-1.0e6, -1.0e6, 1.0e6, 1.0e6],
        show_grid: false,
        show_contours: false,
        cliff_tint: false,
        show_pathing: false,
        clear: [0.0, 0.0, 0.0, 1.0],
        fog_start: NO_FOG,
        ..*base
    }
}

/// A finished export, ready to save.
pub struct ExportedImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// What the user should know about it (resized, models missing).
    pub notes: Vec<String>,
}

/// An export in progress. Lives on the viewport while it runs.
pub struct ExportJob {
    pub rect: [f32; 4],
    pub width: u32,
    pub height: u32,
    pub clamped: bool,
    /// Which objects to draw and how far loading has got.
    pub capture: CaptureModels,
    /// UI time the job started waiting, seconds.
    pub started: Option<f64>,
    /// The render was requested from the paint callback.
    pub requested: bool,
    /// [`crate::state::EditorState::doc_generation`] the job belongs to.
    pub generation: u64,
    /// Objects still loading when the image was rendered.
    loading_at_render: usize,
    #[cfg(not(target_arch = "wasm32"))]
    encoding: Option<std::sync::mpsc::Receiver<Result<Vec<u8>, String>>>,
    #[cfg(not(target_arch = "wasm32"))]
    size: (u32, u32),
    pub result: Option<Result<ExportedImage, String>>,
}

impl ExportJob {
    pub fn new(doc: &MapDocument, form: ExportForm, max_side: u32, generation: u64) -> Result<Self, String> {
        let rect = area_rect(doc, form.area)?;
        let (width, height, clamped) = image_size(rect, form.width, max_side);
        let capture = CaptureModels { area: rect, units: form.units, doodads: form.doodads, ..Default::default() };
        Ok(Self {
            rect,
            width,
            height,
            clamped,
            capture,
            started: None,
            requested: false,
            generation,
            loading_at_render: 0,
            #[cfg(not(target_arch = "wasm32"))]
            encoding: None,
            #[cfg(not(target_arch = "wasm32"))]
            size: (0, 0),
            result: None,
        })
    }

    /// Short progress text for the dialog.
    pub fn progress(&self) -> String {
        if self.requested {
            return format!("Rendering {} × {} pixels…", self.width, self.height);
        }
        match self.capture.loading {
            0 => "Preparing…".into(),
            1 => "Loading models: 1 object left…".into(),
            n => format!("Loading models: {n} objects left…"),
        }
    }

    /// One frame of the job: request the render once models are in (or the
    /// wait is over), and pick up the pixels when the paint callback has
    /// them. `params` are the export's frame parameters.
    pub fn drive(&mut self, now: f64, shared: &crate::render::gl::SharedHandle, params: FrameParams) {
        if self.result.is_some() {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(rx) = &self.encoding {
            if let Ok(r) = rx.try_recv() {
                self.encoding = None;
                let (w, h) = self.size;
                self.result = Some(r.map(|png| self.image(png, w, h)));
            }
            return;
        }
        let started = *self.started.get_or_insert(now);
        let timed_out = now - started > WAIT_SECONDS;
        let Ok(mut s) = shared.lock() else { return };
        if let Some(r) = s.captured.take() {
            drop(s);
            self.finish(r);
            return;
        }
        if !self.requested && (self.capture.loading == 0 || timed_out) {
            self.loading_at_render = self.capture.loading;
            s.capture = Some(CaptureRequest { params, width: self.width as i32, height: self.height as i32, force: timed_out });
            self.requested = true;
        } else if self.requested && timed_out {
            // Still waiting on GPU uploads: render with what is there.
            if let Some(c) = s.capture.as_mut() {
                c.force = true;
            }
        }
    }

    /// Withdraw a pending render request (the job was cancelled).
    pub fn cancel(shared: &crate::render::gl::SharedHandle) {
        if let Ok(mut s) = shared.lock() {
            s.capture = None;
            s.captured = None;
        }
    }

    fn finish(&mut self, r: Result<CapturedImage, String>) {
        let img = match r {
            Ok(img) => img,
            Err(e) => {
                self.result = Some(Err(format!("Strata couldn't render the map image: {e}.")));
                return;
            }
        };
        let (w, h) = (img.width, img.height);
        let rgb = rgba_to_rgb(&img.rgba);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(strata_core::png::encode(w, h, 3, &rgb));
            });
            self.encoding = Some(rx);
            self.size = (w, h);
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.result = Some(strata_core::png::encode(w, h, 3, &rgb).map(|png| self.image(png, w, h)));
        }
    }

    fn image(&self, png: Vec<u8>, width: u32, height: u32) -> ExportedImage {
        let mut notes = Vec::new();
        if self.clamped {
            notes.push(format!(
                "made smaller than asked ({width} px wide) because your graphics card can't render larger images"
            ));
        }
        if self.loading_at_render > 0 {
            notes.push(format!(
                "{} object{} still loading and missing from it",
                self.loading_at_render,
                if self.loading_at_render == 1 { " was" } else { "s were" }
            ));
        }
        ExportedImage { png, width, height, notes }
    }
}

fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::regions::{Region, Shape};

    #[test]
    fn camera_frames_the_area_exactly_north_up() {
        let rect = [8.0, 4.0, 72.0, 52.0];
        let (vp, _) = capture_camera(rect, (2.0, 14.0));
        for z in [2.0, 8.0, 14.0, 20.0] {
            let sw = vp.project_point3(Vec3::new(rect[0], rect[1], z));
            let ne = vp.project_point3(Vec3::new(rect[2], rect[3], z));
            assert!((sw.x + 1.0).abs() < 1e-4 && (sw.y + 1.0).abs() < 1e-4, "{sw}");
            assert!((ne.x - 1.0).abs() < 1e-4 && (ne.y - 1.0).abs() < 1e-4, "{ne}");
            assert!(sw.z > -1.0 && sw.z < 1.0, "inside the depth range: {sw}");
        }
        // East is right, north is up.
        let east = vp.project_point3(Vec3::new(60.0, 28.0, 8.0));
        let north = vp.project_point3(Vec3::new(40.0, 50.0, 8.0));
        assert!(east.x > 0.0 && north.y > 0.0);
    }

    #[test]
    fn sizes_keep_the_aspect_and_fit_the_gpu() {
        assert_eq!(image_size([0.0, 0.0, 64.0, 48.0], 2048, 0), (2048, 1536, false));
        assert_eq!(image_size([0.0, 0.0, 64.0, 48.0], 4096, 8192), (4096, 3072, false));
        let (w, h, clamped) = image_size([0.0, 0.0, 32.0, 64.0], 4096, 4096);
        assert!(clamped && h == 4096 && w == 2048, "{w}x{h}");
    }

    #[test]
    fn areas() {
        let mut doc = MapDocument::new("t", 64, 48);
        doc.info.playable = [4, 6, 60, 44];
        assert_eq!(area_rect(&doc, Area::Map).unwrap(), [0.0, 0.0, 64.0, 48.0]);
        assert_eq!(area_rect(&doc, Area::Playable).unwrap(), [4.0, 6.0, 60.0, 44.0]);
        doc.regions.push(Region {
            id: ObjectId(7),
            name: "Base".into(),
            color: [255, 0, 0],
            shapes: vec![
                Shape::Rect { min: glam::Vec2::new(10.0, 10.0), max: glam::Vec2::new(20.0, 14.0) },
                Shape::Circle { center: glam::Vec2::new(60.0, 40.0), radius: 10.0 },
            ],
        });
        assert_eq!(area_rect(&doc, Area::Region(ObjectId(7))).unwrap(), [10.0, 10.0, 64.0, 48.0]);
        assert!(area_rect(&doc, Area::Region(ObjectId(99))).is_err());
    }

    #[test]
    fn capture_filter() {
        use strata_core::objects::ObjectKind;
        let c = CaptureModels { area: [0.0, 0.0, 10.0, 10.0], units: true, doodads: false, ..Default::default() };
        assert!(c.wants(ObjectKind::Unit, 5.0, 5.0, 0.0));
        assert!(!c.wants(ObjectKind::Doodad, 5.0, 5.0, 0.0));
        assert!(!c.wants(ObjectKind::Point, 5.0, 5.0, 0.0));
        assert!(!c.wants(ObjectKind::Unit, 12.0, 5.0, 0.0));
        assert!(c.wants(ObjectKind::Unit, 12.0, 5.0, 4.0));
    }

    #[test]
    fn alpha_is_dropped() {
        assert_eq!(rgba_to_rgb(&[1, 2, 3, 255, 4, 5, 6, 0]), vec![1, 2, 3, 4, 5, 6]);
    }
}
