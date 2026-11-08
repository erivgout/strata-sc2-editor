//! Design tokens, fonts and egui styling (see `docs/DESIGN.md`).
//!
//! Palette: slate-steel chrome so the map stays the brightest thing on
//! screen; a single accent, mineral cyan, marks whatever is active or
//! selected. Dark is the default theme.

use std::sync::Arc;

use egui::{
    epaint::Shadow, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin,
    Stroke, TextStyle,
};

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
    /// Fills, outlines and indicators for active / selected things.
    pub accent: Color32,
    /// Accent for text and thin lines; meets WCAG AA on panel backgrounds.
    pub accent_text: Color32,
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
    accent_text: Color32::from_rgb(0x62, 0xCD, 0xEB),
    warn: Color32::from_rgb(0xE6, 0xB4, 0x50),
    error: Color32::from_rgb(0xEC, 0x70, 0x78),
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
    accent_text: Color32::from_rgb(0x08, 0x61, 0x7D),
    warn: Color32::from_rgb(0x86, 0x5C, 0x00),
    error: Color32::from_rgb(0xB8, 0x32, 0x3C),
    ok: Color32::from_rgb(0x23, 0x73, 0x3A),
};

pub fn tokens(ctx: &egui::Context) -> Tokens {
    if ctx.global_style().visuals.dark_mode {
        DARK
    } else {
        LIGHT
    }
}

/// Font family for 16px panel headings and emphasis.
pub const SEMIBOLD: &str = "semibold";
/// Font family for labels that need a little more weight than body text.
pub const MEDIUM: &str = "medium";

pub fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

pub fn medium() -> FontFamily {
    FontFamily::Name(MEDIUM.into())
}

/// 20px semibold, for dialog and window titles.
pub fn title_style() -> TextStyle {
    TextStyle::Name("title".into())
}

/// Barlow for UI text, JetBrains Mono for code; egui's bundled fonts stay
/// behind both as fallbacks for symbols and emoji.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let embedded: [(&str, &'static [u8]); 4] = [
        (
            "Barlow-Regular",
            include_bytes!("../assets/fonts/Barlow-Regular.ttf"),
        ),
        (
            "Barlow-Medium",
            include_bytes!("../assets/fonts/Barlow-Medium.ttf"),
        ),
        (
            "Barlow-SemiBold",
            include_bytes!("../assets/fonts/Barlow-SemiBold.ttf"),
        ),
        (
            "JetBrainsMono-Regular",
            include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
        ),
    ];
    for (name, bytes) in embedded {
        fonts
            .font_data
            .insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    let fallback_prop = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let fallback_mono = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let with = |first: &str, rest: &[String]| {
        let mut v = vec![first.to_owned()];
        v.extend(rest.iter().cloned());
        v
    };
    fonts.families.insert(
        FontFamily::Proportional,
        with("Barlow-Regular", &fallback_prop),
    );
    fonts.families.insert(
        FontFamily::Monospace,
        with("JetBrainsMono-Regular", &fallback_mono),
    );
    fonts
        .families
        .insert(semibold(), with("Barlow-SemiBold", &fallback_prop));
    fonts
        .families
        .insert(medium(), with("Barlow-Medium", &fallback_prop));
    ctx.set_fonts(fonts);
}

/// Apply the full Strata style for both themes and select one.
pub fn apply(ctx: &egui::Context, dark: bool) {
    ctx.set_style_of(egui::Theme::Dark, build_style(&DARK, true));
    ctx.set_style_of(egui::Theme::Light, build_style(&LIGHT, false));
    ctx.set_theme(if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    });
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Background for hovered rows and buttons.
pub fn hover_fill(t: &Tokens, dark: bool) -> Color32 {
    if dark {
        lerp(t.bg_raised, t.stroke, 0.55)
    } else {
        Color32::WHITE
    }
}

/// Translucent accent fill for selected rows and items.
pub fn selection_fill(t: &Tokens, dark: bool) -> Color32 {
    t.accent.gamma_multiply(if dark { 0.24 } else { 0.16 })
}

pub fn build_style(t: &Tokens, dark: bool) -> egui::Style {
    let text_styles = [
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (TextStyle::Heading, FontId::new(16.0, semibold())),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
        (title_style(), FontId::new(20.0, semibold())),
    ]
    .into();
    let mut s = egui::Style {
        text_styles,
        ..Default::default()
    };

    // Spacing: a 4px grid.
    let sp = &mut s.spacing;
    sp.item_spacing = egui::vec2(8.0, 4.0);
    sp.button_padding = egui::vec2(8.0, 3.0);
    sp.window_margin = Margin::same(12);
    sp.menu_margin = Margin::same(4);
    sp.menu_spacing = 2.0;
    sp.menu_width = 240.0;
    sp.indent = 16.0;
    sp.interact_size = egui::vec2(40.0, 24.0);
    sp.slider_width = 140.0;
    sp.slider_rail_height = 4.0;
    sp.combo_width = 140.0;
    sp.combo_height = 320.0;
    sp.text_edit_width = 220.0;
    sp.icon_width = 16.0;
    sp.icon_width_inner = 8.0;
    sp.icon_spacing = 8.0;
    sp.tooltip_width = 320.0;
    sp.scroll = egui::style::ScrollStyle {
        bar_width: 8.0,
        floating_width: 4.0,
        bar_inner_margin: 4.0,
        bar_outer_margin: 0.0,
        dormant_handle_opacity: 0.35,
        active_handle_opacity: 0.6,
        interact_handle_opacity: 0.9,
        ..egui::style::ScrollStyle::floating()
    };

    s.interaction.tooltip_delay = 0.35;
    s.interaction.selectable_labels = false;
    s.animation_time = 0.12;
    s.url_in_tooltip = true;

    let v = &mut s.visuals;
    *v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.dark_mode = dark;
    v.override_text_color = None;
    v.weak_text_color = Some(t.text_dim);
    v.hyperlink_color = t.accent_text;
    v.faint_bg_color = lerp(t.bg_panel, t.bg_raised, 0.5);
    v.extreme_bg_color = t.bg_deep;
    v.text_edit_bg_color = Some(t.bg_deep);
    v.code_bg_color = t.bg_deep;
    v.warn_fg_color = t.warn;
    v.error_fg_color = t.error;
    v.panel_fill = t.bg_panel;
    v.window_fill = t.bg_raised;
    v.window_stroke = Stroke::new(1.0, t.stroke);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_highlight_topmost = false;
    let shadow_alpha = if dark { 110 } else { 36 };
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.popup_shadow = Shadow {
        offset: [0, 4],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(shadow_alpha),
    };
    v.selection.bg_fill = selection_fill(t, dark);
    v.selection.stroke = Stroke::new(1.0, t.accent_text);
    v.text_cursor.stroke = Stroke::new(2.0, t.accent_text);
    v.slider_trailing_fill = true;
    v.striped = false;
    v.indent_has_left_vline = true;
    v.collapsing_header_frame = false;
    v.resize_corner_size = 10.0;
    v.disabled_alpha = 0.45;
    v.handle_shape = egui::style::HandleShape::Circle;

    let r = CornerRadius::same(4);
    let hover = hover_fill(t, dark);
    let (button, button_stroke, hover_stroke, press) = if dark {
        (
            t.bg_raised,
            Stroke::NONE,
            Stroke::new(1.0, t.stroke),
            lerp(t.stroke, t.bg_raised, 0.2),
        )
    } else {
        (
            t.bg_raised,
            Stroke::new(1.0, t.stroke),
            Stroke::new(1.0, lerp(t.stroke, t.text_dim, 0.4)),
            t.bg_deep,
        )
    };
    let w = &mut v.widgets;
    w.noninteractive = egui::style::WidgetVisuals {
        bg_fill: t.bg_panel,
        weak_bg_fill: t.bg_panel,
        bg_stroke: Stroke::new(1.0, t.stroke),
        corner_radius: r,
        fg_stroke: Stroke::new(1.0, t.text),
        expansion: 0.0,
    };
    w.inactive = egui::style::WidgetVisuals {
        // `bg_fill` backs checkboxes, radio buttons and slider rails.
        bg_fill: t.bg_deep,
        weak_bg_fill: button,
        bg_stroke: button_stroke,
        corner_radius: r,
        fg_stroke: Stroke::new(1.0, t.text),
        expansion: 0.0,
    };
    w.hovered = egui::style::WidgetVisuals {
        bg_fill: hover,
        weak_bg_fill: hover,
        bg_stroke: hover_stroke,
        corner_radius: r,
        fg_stroke: Stroke::new(1.5, t.text),
        expansion: 0.0,
    };
    // Pressed or keyboard-focused: the accent outline doubles as the focus ring.
    w.active = egui::style::WidgetVisuals {
        bg_fill: press,
        weak_bg_fill: press,
        bg_stroke: Stroke::new(1.5, t.accent),
        corner_radius: r,
        fg_stroke: Stroke::new(1.5, t.text),
        expansion: 0.0,
    };
    w.open = egui::style::WidgetVisuals {
        bg_fill: hover,
        weak_bg_fill: hover,
        bg_stroke: Stroke::new(1.0, t.stroke),
        corner_radius: r,
        fg_stroke: Stroke::new(1.0, t.text),
        expansion: 0.0,
    };
    s
}

/// Dock tabs that match the tokens: tabs sit on a deep strip, the active tab
/// joins its panel, the focused panel's tab label takes the accent.
pub fn dock_style(style: &egui::Style) -> egui_dock::Style {
    let dark = style.visuals.dark_mode;
    let t = if dark { DARK } else { LIGHT };
    let mut d = egui_dock::Style::from_egui(style);
    let top = CornerRadius {
        nw: 4,
        ne: 4,
        sw: 0,
        se: 0,
    };

    d.dock_area_padding = None;
    d.main_surface_border_stroke = Stroke::NONE;
    d.main_surface_border_rounding = CornerRadius::ZERO;

    d.tab_bar.bg_fill = t.bg_deep;
    d.tab_bar.height = 30.0;
    d.tab_bar.hline_color = t.stroke;
    d.tab_bar.corner_radius = CornerRadius::ZERO;
    d.tab_bar.inner_margin = Margin {
        left: 4,
        right: 4,
        top: 2,
        bottom: 0,
    };

    let tab = |bg: Color32, text: Color32, outline: Color32| egui_dock::TabInteractionStyle {
        outline_color: outline,
        corner_radius: top,
        bg_fill: bg,
        text_color: text,
    };
    d.tab.active = tab(t.bg_panel, t.text, t.stroke);
    d.tab.focused = tab(t.bg_panel, t.accent_text, t.stroke);
    d.tab.inactive = tab(t.bg_deep, t.text_dim, Color32::TRANSPARENT);
    d.tab.hovered = tab(
        lerp(t.bg_deep, t.bg_panel, 0.6),
        t.text,
        Color32::TRANSPARENT,
    );
    d.tab.active_with_kb_focus = tab(t.bg_panel, t.text, t.accent);
    d.tab.focused_with_kb_focus = tab(t.bg_panel, t.accent_text, t.accent);
    d.tab.inactive_with_kb_focus = tab(t.bg_deep, t.text, t.accent);
    d.tab.tab_body.bg_fill = t.bg_panel;
    d.tab.tab_body.stroke = Stroke::NONE;
    d.tab.tab_body.corner_radius = CornerRadius::ZERO;
    d.tab.tab_body.inner_margin = Margin::same(8);
    d.tab.hline_below_active_tab_name = false;
    d.tab.spacing = 2.0;
    d.tab.minimum_width = Some(64.0);

    d.separator.width = 2.0;
    d.separator.extra_interact_width = 4.0;
    d.separator.color_idle = t.bg_deep;
    d.separator.color_hovered = t.stroke;
    d.separator.color_dragged = t.accent;

    d.buttons.close_tab_color = t.text_dim;
    d.buttons.close_tab_active_color = t.text;
    d.buttons.close_tab_bg_fill = hover_fill(&t, dark);
    d.buttons.add_tab_color = t.text_dim;
    d.buttons.add_tab_active_color = t.text;
    d.buttons.add_tab_bg_fill = hover_fill(&t, dark);

    d.overlay.selection_color = t.accent.gamma_multiply(0.3);
    d.overlay.button_color = t.bg_raised;
    d.overlay.button_border_stroke = Stroke::new(1.0, t.stroke);
    d
}

/// Frame for modal dialogs: raised surface, 8px radius, roomy margins.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    let t = tokens(ctx);
    egui::Frame::popup(&ctx.global_style())
        .fill(t.bg_raised)
        .corner_radius(8)
        .inner_margin(Margin::same(20))
}

/// Accent color for a toast kind.
pub fn toast_color(t: &Tokens, kind: ToastKind) -> Color32 {
    match kind {
        ToastKind::Info => t.accent,
        ToastKind::Success => t.ok,
        ToastKind::Error => t.error,
    }
}

/// A card with a colored leading bar, used by toasts and notices.
pub fn card<R>(ui: &mut egui::Ui, bar: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = tokens(ui.ctx());
    let frame = egui::Frame::popup(ui.style())
        .fill(t.bg_raised)
        .stroke(Stroke::new(1.0, t.stroke))
        .corner_radius(6)
        .inner_margin(Margin {
            left: 16,
            right: 12,
            top: 8,
            bottom: 8,
        });
    let resp = frame.show(ui, |ui| {
        ui.set_max_width(380.0);
        add(ui)
    });
    let r = resp.response.rect;
    let bar_rect = egui::Rect::from_min_max(
        r.min + egui::vec2(5.0, 8.0),
        egui::pos2(r.min.x + 8.0, r.max.y - 8.0),
    );
    ui.painter().rect_filled(bar_rect, 2.0, bar);
    resp.inner
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
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let n = state.toasts.len();
            for t in state.toasts.iter().skip(n.saturating_sub(4)) {
                let fade = (t.ttl / 0.3).clamp(0.0, 1.0);
                ui.set_opacity(fade);
                card(ui, toast_color(&tk, t.kind), |ui| {
                    ui.label(egui::RichText::new(&t.text).color(tk.text));
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |x: u8| {
            let x = x as f32 / 255.0;
            if x <= 0.03928 {
                x / 12.92
            } else {
                ((x + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn text_meets_wcag_aa() {
        for t in [DARK, LIGHT] {
            for bg in [t.bg_panel, t.bg_raised] {
                for fg in [t.text, t.text_dim, t.accent_text, t.warn, t.error, t.ok] {
                    let c = contrast(fg, bg);
                    assert!(c >= 4.5, "{fg:?} on {bg:?} is {c:.2}:1");
                }
            }
            assert!(contrast(t.text, t.bg_deep) >= 4.5);
            // Non-text UI (focus ring, selection marker) needs 3:1.
            assert!(contrast(t.accent, t.bg_panel) >= 3.0);
        }
    }
}
