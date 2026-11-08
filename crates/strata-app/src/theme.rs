//! Design tokens and egui styling. Dark is the default theme.
//!
//! Palette (dark): slate-steel chrome so the map stays the brightest thing on
//! screen; a single accent, mineral cyan, marks whatever is active or selected.

use egui::Color32;

use crate::state::{EditorState, ToastKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ThemePref {
    Dark,
    Light,
    System,
}

impl ThemePref {
    pub const ALL: [ThemePref; 3] = [ThemePref::Dark, ThemePref::Light, ThemePref::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemePref::Dark => "Dark",
            ThemePref::Light => "Light",
            ThemePref::System => "Match system",
        }
    }

    pub fn resolve(self, system_dark: bool) -> bool {
        match self {
            ThemePref::Dark => true,
            ThemePref::Light => false,
            ThemePref::System => system_dark,
        }
    }
}

/// Resolved color tokens for one theme.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub bg_deep: Color32,
    pub bg_panel: Color32,
    pub bg_raised: Color32,
    pub stroke: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub ok: Color32,
}

pub const DARK: Tokens = Tokens {
    bg_deep: Color32::from_rgb(0x13, 0x19, 0x20),
    bg_panel: Color32::from_rgb(0x1B, 0x23, 0x2D),
    bg_raised: Color32::from_rgb(0x25, 0x2F, 0x3B),
    stroke: Color32::from_rgb(0x33, 0x40, 0x4F),
    text: Color32::from_rgb(0xDD, 0xE4, 0xEC),
    text_dim: Color32::from_rgb(0x8A, 0x98, 0xA8),
    accent: Color32::from_rgb(0x62, 0xCD, 0xEB),
    warn: Color32::from_rgb(0xE6, 0xB4, 0x50),
    error: Color32::from_rgb(0xE0, 0x62, 0x6A),
    ok: Color32::from_rgb(0x7B, 0xD8, 0x8F),
};

pub const LIGHT: Tokens = Tokens {
    bg_deep: Color32::from_rgb(0xD9, 0xDF, 0xE6),
    bg_panel: Color32::from_rgb(0xEA, 0xEE, 0xF2),
    bg_raised: Color32::from_rgb(0xF7, 0xF9, 0xFB),
    stroke: Color32::from_rgb(0xBC, 0xC6, 0xD1),
    text: Color32::from_rgb(0x1C, 0x27, 0x33),
    text_dim: Color32::from_rgb(0x5A, 0x68, 0x78),
    accent: Color32::from_rgb(0x0E, 0x83, 0xA8),
    warn: Color32::from_rgb(0x9A, 0x6A, 0x00),
    error: Color32::from_rgb(0xB8, 0x32, 0x3C),
    ok: Color32::from_rgb(0x2E, 0x8A, 0x45),
};

pub fn tokens(ctx: &egui::Context) -> Tokens {
    if ctx.global_style().visuals.dark_mode { DARK } else { LIGHT }
}

pub fn install_fonts(_ctx: &egui::Context) {}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let t = if dark { DARK } else { LIGHT };
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = t.bg_panel;
    v.window_fill = t.bg_raised;
    v.extreme_bg_color = t.bg_deep;
    v.faint_bg_color = t.bg_raised;
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent.gamma_multiply(0.35);
    v.selection.stroke = egui::Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.error;
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, t.stroke);
    ctx.set_visuals(v);
}

pub fn show_toasts(ctx: &egui::Context, state: &mut EditorState) {
    let dt = ctx.input(|i| i.stable_dt).min(0.1);
    state.toasts.retain_mut(|t| {
        t.ttl -= dt;
        t.ttl > 0.0
    });
    if state.toasts.is_empty() {
        return;
    }
    ctx.request_repaint();
    let tk = tokens(ctx);
    egui::Area::new(egui::Id::new("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -40.0])
        .show(ctx, |ui| {
            for t in state.toasts.iter().rev().take(4) {
                let color = match t.kind {
                    ToastKind::Info => tk.text_dim,
                    ToastKind::Success => tk.ok,
                    ToastKind::Error => tk.error,
                };
                egui::Frame::popup(ui.style()).stroke(egui::Stroke::new(1.0, color)).show(ui, |ui| {
                    ui.label(&t.text);
                });
            }
        });
}
