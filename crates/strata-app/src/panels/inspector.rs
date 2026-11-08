//! Tool options and the properties of whatever is selected.
//!
//! Property edits apply to the document live (so the map updates while you
//! drag a value) and are recorded as one undoable edit when the interaction
//! ends: drag released, field unfocused, or a choice made in a combo box.

use std::collections::HashMap;

use egui::{Color32, RichText};
use strata_core::base_data::{BaseCatalog, BaseUnit};
use strata_core::brush::{BrushShape, Falloff};
use strata_core::catalog::CatalogKind;
use strata_core::history::Edit;
use strata_core::known::{self, UnitGroup};
use strata_core::objects::{ObjectKind, PlacedObject};
use strata_core::regions::{Camera, Region, Shape};
use strata_core::terrain::{pathing, TextureLayer, MAX_TEXTURE_LAYERS};

use crate::render::mesh::texture_color;
use crate::render::overlay::player_color;
use crate::state::{EditorState, Mode};
use crate::theme::{self, Tokens};
use crate::tools::select::{plural, selected_objects};
use crate::tools::{CliffOp, SculptTool};

#[derive(Default)]
pub struct InspectorState {
    pending: Option<Pending>,
    unit_search: String,
    doodad_search: String,
    /// Base-game unit list: include campaign and story units, not just melee.
    all_units: bool,
}

/// Snapshot taken when a property edit starts; committed when it ends.
struct Pending {
    label: String,
    revision: u64,
    before: Before,
}

enum Before {
    Objects(Vec<PlacedObject>),
    Regions(Vec<Region>),
    Cameras(Vec<Camera>),
    Layers(Vec<TextureLayer>),
}

/// A structural change to the texture layer list.
type LayerOp = Box<dyn FnOnce(&mut Vec<TextureLayer>)>;
/// One property change applied to every selected object.
type ObjectOp = Box<dyn Fn(&mut PlacedObject)>;

/// Tracks whether any property widget is mid-interaction this frame.
struct Edits {
    active: bool,
}

impl Edits {
    fn track(&mut self, r: &egui::Response) {
        self.active |= r.has_focus() || r.dragged() || r.is_pointer_button_down_on();
    }
}

fn begin(state: &mut EditorState, label: &str, before: impl FnOnce(&EditorState) -> Before) {
    if state.ui.inspector.pending.is_none() {
        let b = before(state);
        state.ui.inspector.pending =
            Some(Pending { label: label.into(), revision: state.history.revision(), before: b });
    }
}

fn commit_pending(state: &mut EditorState) {
    let Some(p) = state.ui.inspector.pending.take() else { return };
    if p.revision != state.history.revision() {
        // History moved under us (undo mid-edit); the snapshot is stale.
        return;
    }
    let edit = match p.before {
        Before::Objects(before) => {
            let edits: Vec<Edit> = before
                .into_iter()
                .filter_map(|b| {
                    let a = state.doc.objects.get(b.id)?.clone();
                    (a != b).then_some(Edit::Object { id: b.id, before: Some(b), after: Some(a) })
                })
                .collect();
            match edits.len() {
                0 => None,
                1 => edits.into_iter().next(),
                _ => Some(Edit::Batch(edits)),
            }
        }
        Before::Regions(before) => {
            (before != state.doc.regions).then(|| Edit::Regions { before, after: state.doc.regions.clone() })
        }
        Before::Cameras(before) => {
            (before != state.doc.cameras).then(|| Edit::Cameras { before, after: state.doc.cameras.clone() })
        }
        Before::Layers(before) => (before != state.doc.terrain.layers)
            .then(|| Edit::TextureLayers { before, after: state.doc.terrain.layers.clone() }),
    };
    if let Some(e) = edit {
        state.doc.dirty.merge(e.dirty());
        state.push_edit(p.label, e);
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let mut edits = Edits { active: false };
    ui.spacing_mut().item_spacing.y = 6.0;

    let has_selection =
        !state.selection.objects.is_empty() || state.selection.region.is_some() || state.selection.camera.is_some();

    ui.label(RichText::new(state.mode.label()).size(16.0).strong());
    tool_options(ui, state, &mut edits, &tk);

    ui.add_space(4.0);
    ui.separator();
    if has_selection {
        if !state.selection.objects.is_empty() {
            object_properties(ui, state, &mut edits, &tk);
        }
        if state.selection.region.is_some() {
            region_properties(ui, state, &mut edits);
        }
        if state.selection.camera.is_some() {
            camera_properties(ui, state, &mut edits);
        }
    } else {
        ui.label(RichText::new("Nothing selected").strong());
        ui.label(
            RichText::new("Click a unit on the map, or press 6 to place units. Press 1 for Select, then drag a box to select several.")
                .color(tk.text_dim),
        );
    }

    if !edits.active {
        commit_pending(state);
    }
}

// ---------------------------------------------------------------- tool options

fn hint(ui: &mut egui::Ui, tk: &Tokens, text: &str) {
    ui.label(RichText::new(text).color(tk.text_dim).size(12.0));
}

fn tool_options(ui: &mut egui::Ui, state: &mut EditorState, edits: &mut Edits, tk: &Tokens) {
    match state.mode {
        Mode::Select => hint(ui, tk, "Click to select, Shift+click to add, drag a box to select several. Drag to move (N toggles snapping). R / Shift+R rotate, Delete removes, Ctrl+D duplicates, F frames."),
        Mode::Sculpt => {
            sub_tool_picker(ui, state, false);
            hint(ui, tk, state.ui.viewport.tools.settings.sculpt.hint());
            brush_controls(ui, state);
        }
        Mode::Paint => {
            texture_layers(ui, state, edits, tk);
            sub_tool_picker(ui, state, false);
            hint(ui, tk, "Hold Ctrl to erase while painting.");
            brush_controls(ui, state);
        }
        Mode::Cliff => {
            sub_tool_picker(ui, state, false);
            cliff_level_grid(ui, state);
            hint(ui, tk, "Raise and lower step one level from where the stroke starts. Hold Ctrl to invert.");
            brush_controls(ui, state);
        }
        Mode::Pathing => {
            sub_tool_picker(ui, state, false);
            ui.horizontal(|ui| {
                swatch(ui, crate::render::overlay::MAP.error);
                ui.label("Unwalkable");
                swatch(ui, crate::render::overlay::MAP.warn);
                ui.label("Unbuildable");
            });
            hint(ui, tk, "Hold Ctrl to clear the chosen flags.");
            brush_controls(ui, state);
        }
        Mode::Units => {
            player_picker(ui, state, tk);
            unit_picker(ui, state, tk);
            placement_rotation(ui, state);
            hint(ui, tk, "Click to place. Hold Shift to keep placing. Shift+wheel or R rotates.");
        }
        Mode::Doodads => {
            doodad_picker(ui, state, tk);
            placement_rotation(ui, state);
            hint(ui, tk, "Click to place. Hold Shift to keep placing.");
        }
        Mode::Points => {
            ui.horizontal(|ui| {
                ui.label("Type");
                for k in known::POINT_TYPES {
                    let on = state.ui.viewport.tools.settings.point_type == k.id;
                    if ui.add(egui::Button::selectable(on, k.name)).clicked() {
                        state.ui.viewport.tools.settings.point_type = k.id.into();
                    }
                }
            });
            hint(ui, tk, "Click on the map to place a named point. Start locations mark where players begin.");
        }
        Mode::Regions => {
            hint(ui, tk, "Drag on the map to draw a rectangle. Hold Alt to draw a circle.");
            item_list(ui, state, true);
        }
        Mode::Cameras => {
            hint(ui, tk, "Click on the map to add a camera that looks from the current view angle.");
            item_list(ui, state, false);
        }
    }
}

/// A color swatch followed by a selectable label; one click target.
fn player_choice(ui: &mut egui::Ui, on: bool, color: Color32, label: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        swatch(ui, color);
        ui.add(egui::Button::selectable(on, label))
    })
    .inner
}

fn swatch(ui: &mut egui::Ui, c: Color32) {
    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
    ui.painter().rect_filled(r, 2.0, c);
}

/// A row of mutually related toggle buttons. `compact` draws them as one
/// segmented control on a recessed track (the floating HUD); otherwise they
/// wrap like ordinary buttons (the Inspector).
fn segmented<R>(ui: &mut egui::Ui, compact: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    if compact {
        let tk = theme::tokens(ui.ctx());
        egui::Frame::new()
            .fill(tk.bg_deep)
            .corner_radius(4)
            .inner_margin(egui::Margin::same(2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    add(ui)
                })
                .inner
            })
            .inner
    } else {
        ui.horizontal_wrapped(add).inner
    }
}

/// Sub-tool row for brush modes (shared with the floating HUD).
pub fn sub_tool_picker(ui: &mut egui::Ui, state: &mut EditorState, compact: bool) {
    let s = &mut state.ui.viewport.tools.settings;
    match state.mode {
        Mode::Sculpt => segmented(ui, compact, |ui| {
            for t in SculptTool::ALL {
                if ui.add(egui::Button::selectable(s.sculpt == t, t.label())).on_hover_text(t.hint()).clicked() {
                    s.sculpt = t;
                }
            }
        }),
        Mode::Paint => segmented(ui, compact, |ui| {
            if ui.add(egui::Button::selectable(!s.paint_erase, "Paint")).clicked() {
                s.paint_erase = false;
            }
            if ui
                .add(egui::Button::selectable(s.paint_erase, "Erase"))
                .on_hover_text("Or hold Ctrl while painting")
                .clicked()
            {
                s.paint_erase = true;
            }
        }),
        Mode::Cliff => {
            segmented(ui, compact, |ui| {
                for op in CliffOp::ALL {
                    if ui.add(egui::Button::selectable(s.cliff_op == op, op.label())).clicked() {
                        s.cliff_op = op;
                    }
                }
            });
            if compact && s.cliff_op == CliffOp::Set {
                ui.add(egui::DragValue::new(&mut s.cliff_level).range(0..=15).prefix("Level "))
                    .on_hover_text("Cliff level to paint");
            }
        }
        Mode::Pathing => {
            let flags = |ui: &mut egui::Ui| {
                // Only this flag is written to .SC2Map files today.
                let on = s.pathing_flags & pathing::UNWALKABLE != 0;
                if ui
                    .add(egui::Button::selectable(on, "Unwalkable"))
                    .on_hover_text("Ground units cannot path here")
                    .clicked()
                {
                    s.pathing_flags ^= pathing::UNWALKABLE;
                }
            };
            if compact {
                segmented(ui, true, flags);
            } else {
                ui.horizontal_wrapped(flags);
            }
            segmented(ui, compact, |ui| {
                if ui.add(egui::Button::selectable(!s.pathing_clear, "Paint")).clicked() {
                    s.pathing_clear = false;
                }
                if ui
                    .add(egui::Button::selectable(s.pathing_clear, "Clear"))
                    .on_hover_text("Or hold Ctrl while painting")
                    .clicked()
                {
                    s.pathing_clear = true;
                }
            });
        }
        _ => {}
    }
}

/// Size / strength / falloff / shape as a labeled grid in the Inspector.
/// (The floating HUD shows only size and strength; see `viewport::brush_hud`.)
pub fn brush_controls(ui: &mut egui::Ui, state: &mut EditorState) {
    let uses_strength = matches!(state.mode, Mode::Sculpt | Mode::Paint);
    let b = &mut state.brush;
    let size = |ui: &mut egui::Ui, b: &mut strata_core::brush::Brush| {
        ui.add(egui::Slider::new(&mut b.radius, 0.5..=64.0).logarithmic(true).max_decimals(1))
            .on_hover_text("Brush radius in cells ( [ and ] )");
    };
    let strength = |ui: &mut egui::Ui, b: &mut strata_core::brush::Brush| {
        ui.add(egui::Slider::new(&mut b.strength, 0.01..=1.0).max_decimals(2)).on_hover_text("Shift+[ and Shift+]");
    };
    let falloff = |ui: &mut egui::Ui, b: &mut strata_core::brush::Brush| {
        egui::ComboBox::from_id_salt("falloff")
            .selected_text(falloff_label(b.falloff))
            .width(90.0)
            .show_ui(ui, |ui| {
                for f in [Falloff::Smooth, Falloff::Linear, Falloff::Sharp, Falloff::Constant] {
                    ui.selectable_value(&mut b.falloff, f, falloff_label(f));
                }
            })
            .response
            .on_hover_text("How strength fades toward the edge of the brush");
    };
    let shape = |ui: &mut egui::Ui, b: &mut strata_core::brush::Brush| {
        let square = b.shape == BrushShape::Square;
        if ui.add(egui::Button::selectable(!square, "Circle")).clicked() {
            b.shape = BrushShape::Circle;
        }
        if ui.add(egui::Button::selectable(square, "Square")).clicked() {
            b.shape = BrushShape::Square;
        }
    };
    ui.spacing_mut().slider_width = (ui.available_width() - 150.0).clamp(80.0, 200.0);
    egui::Grid::new("brush-grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Size");
        size(ui, b);
        ui.end_row();
        if uses_strength {
            ui.label("Strength");
            strength(ui, b);
            ui.end_row();
            ui.label("Falloff");
            falloff(ui, b);
            ui.end_row();
        }
        ui.label("Shape");
        ui.horizontal(|ui| shape(ui, b));
        ui.end_row();
    });
}

fn falloff_label(f: Falloff) -> &'static str {
    match f {
        Falloff::Constant => "Hard edge",
        Falloff::Linear => "Linear",
        Falloff::Smooth => "Smooth",
        Falloff::Sharp => "Sharp",
    }
}

fn cliff_level_grid(ui: &mut egui::Ui, state: &mut EditorState) {
    let s = &mut state.ui.viewport.tools.settings;
    ui.add_enabled_ui(s.cliff_op == CliffOp::Set, |ui| {
        ui.label("Level");
        egui::Grid::new("cliff-levels").spacing([2.0, 2.0]).min_col_width(24.0).show(ui, |ui| {
            for level in 0u8..16 {
                if ui
                    .add_sized([26.0, 22.0], egui::Button::selectable(s.cliff_level == level, level.to_string()))
                    .clicked()
                {
                    s.cliff_level = level;
                }
                if level % 8 == 7 {
                    ui.end_row();
                }
            }
        });
    });
}

fn texture_layers(ui: &mut egui::Ui, state: &mut EditorState, edits: &mut Edits, tk: &Tokens) {
    ui.label(RichText::new("Texture layers").strong());
    let n = state.doc.terrain.layers.len();
    let mut action: Option<(&str, LayerOp)> = None;
    let mut new_paint = None;
    for i in (0..n).rev() {
        ui.horizontal(|ui| {
            let id = state.doc.terrain.layers[i].texture.clone();
            let c = texture_color(&id);
            swatch(ui, Color32::from_rgb(c[0], c[1], c[2]));
            let selected = state.ui.viewport.tools.settings.paint_layer == i;
            if ui
                .add(egui::Button::selectable(selected, format!("{}", i + 1)))
                .on_hover_text("Paint this layer")
                .clicked()
            {
                new_paint = Some(i);
            }
            let mut text = id.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(120.0).hint_text("Texture id"));
            edits.track(&r);
            if r.changed() {
                begin(state, "Rename texture layer", |s| Before::Layers(s.doc.terrain.layers.clone()));
                state.doc.terrain.layers[i].texture = text;
            }
            if i == 0 {
                ui.label(RichText::new("base").color(tk.text_dim).size(12.0));
            }
            if ui
                .add_enabled(i + 1 < n, egui::Button::new("Up").small())
                .on_hover_text("Move layer up (paints over lower layers)")
                .clicked()
            {
                action = Some(("Reorder texture layers", Box::new(move |l| l.swap(i, i + 1))));
            }
            if ui.add_enabled(i > 0, egui::Button::new("Down").small()).on_hover_text("Move layer down").clicked() {
                action = Some(("Reorder texture layers", Box::new(move |l| l.swap(i, i - 1))));
            }
            if ui.add_enabled(n > 1, egui::Button::new("Remove").small()).on_hover_text("Remove this layer").clicked() {
                action = Some((
                    "Remove texture layer",
                    Box::new(move |l| {
                        l.remove(i);
                        if i == 0 {
                            if let Some(b) = l.first_mut() {
                                b.weights.iter_mut().for_each(|w| *w = 255);
                            }
                        }
                    }),
                ));
            }
        });
    }
    if let Some(i) = new_paint {
        state.ui.viewport.tools.settings.paint_layer = i;
    }

    // Add a layer.
    let can_add = n < MAX_TEXTURE_LAYERS;
    ui.horizontal(|ui| {
        let tileset = state.doc.info.tileset.clone();
        let s = &mut state.ui.viewport.tools.settings;
        let r = ui.add_enabled(
            can_add,
            egui::TextEdit::singleline(&mut s.new_layer_texture).desired_width(120.0).hint_text("Texture id"),
        );
        egui::ComboBox::from_id_salt("known-textures").selected_text("Known").width(70.0).show_ui(ui, |ui| {
            for t in known::textures_for(&tileset) {
                if ui.selectable_label(s.new_layer_texture == t.id, format!("{} ({})", t.name, t.id)).clicked() {
                    s.new_layer_texture = t.id.into();
                }
            }
        });
        let id = s.new_layer_texture.trim().to_owned();
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if (ui.add_enabled(can_add && !id.is_empty(), egui::Button::new("Add layer")).clicked() || enter)
            && can_add
            && !id.is_empty()
        {
            let verts = state.doc.terrain.heights.len();
            let empty = state.doc.terrain.layers.is_empty();
            action = Some((
                "Add texture layer",
                Box::new(move |l| {
                    l.push(TextureLayer { texture: id, weights: vec![if empty { 255 } else { 0 }; verts] })
                }),
            ));
            state.ui.viewport.tools.settings.new_layer_texture.clear();
            state.ui.viewport.tools.settings.paint_layer = n;
        }
    });
    if !can_add {
        hint(ui, tk, &format!("SC2 allows at most {MAX_TEXTURE_LAYERS} texture layers."));
    }
    if n == 1 {
        hint(ui, tk, "Add a second layer to paint over the base texture.");
    }

    if let Some((label, f)) = action {
        commit_pending(state);
        let before = state.doc.terrain.layers.clone();
        let mut after = before.clone();
        f(&mut after);
        state.commit(label, Edit::TextureLayers { before, after });
        let n = state.doc.terrain.layers.len();
        let s = &mut state.ui.viewport.tools.settings;
        s.paint_layer = s.paint_layer.min(n.saturating_sub(1));
    }
}

fn player_picker(ui: &mut egui::Ui, state: &mut EditorState, tk: &Tokens) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Player");
        let mut ids: Vec<(u8, String)> = state.doc.info.players.iter().map(|p| (p.id, p.name.clone())).collect();
        ids.insert(0, (0, "Neutral".into()));
        for (id, name) in ids {
            let on = state.place_player == id;
            let c = player_color(&state.doc, id, tk);
            let r = player_choice(ui, on, c, &id.to_string()).on_hover_text(name);
            if r.clicked() {
                state.place_player = id;
            }
        }
    });
}

fn unit_picker(ui: &mut egui::Ui, state: &mut EditorState, tk: &Tokens) {
    ui.horizontal(|ui| {
        ui.label("Unit type");
        ui.add(egui::TextEdit::singleline(&mut state.place_type).desired_width(f32::INFINITY).hint_text("Any unit id"));
    });
    let search = &mut state.ui.inspector.unit_search;
    ui.add(egui::TextEdit::singleline(search).hint_text("Search units").desired_width(f32::INFINITY));
    let q = search.to_lowercase();
    if let Some(base) = state.base.clone() {
        base_unit_list(ui, state, &base, &q, tk);
        return;
    }
    let mut pick = None;
    egui::ScrollArea::vertical().auto_shrink([false, true]).id_salt("unit-list").max_height(260.0).show(ui, |ui| {
        for g in UnitGroup::ALL {
            let items: Vec<_> = g
                .units()
                .iter()
                .filter(|u| q.is_empty() || u.name.to_lowercase().contains(&q) || u.id.to_lowercase().contains(&q))
                .collect();
            if items.is_empty() {
                continue;
            }
            let open = !q.is_empty() || items.iter().any(|u| u.id == state.place_type);
            egui::CollapsingHeader::new(g.label())
                .id_salt(("units", g.label()))
                .open(if q.is_empty() { None } else { Some(true) })
                .default_open(open)
                .show(ui, |ui| {
                    for u in items {
                        let r = ui.selectable_label(state.place_type == u.id, u.name).on_hover_text(u.id);
                        if r.clicked() {
                            pick = Some(u.id);
                        }
                    }
                });
        }
    });
    if let Some(id) = pick {
        state.place_type = id.into();
    }
    if known::is_neutral_unit(&state.place_type) {
        hint(ui, tk, "Resources and neutral objects are placed for the neutral player.");
    }
    hint(ui, tk, "Showing common units. Load game data (Map menu) to list every unit in StarCraft II.");
}

type UnitFilter = fn(&BaseUnit) -> bool;

/// Every placeable unit of the base game, grouped by race then by kind.
fn base_unit_list(ui: &mut egui::Ui, state: &mut EditorState, base: &BaseCatalog, q: &str, tk: &Tokens) {
    ui.checkbox(&mut state.ui.inspector.all_units, "Include campaign units")
        .on_hover_text("Also list campaign, hero and story-mode units, not just multiplayer ones");
    let all = state.ui.inspector.all_units;
    let units: Vec<&BaseUnit> = base
        .units()
        .iter()
        // Unnamed melee units are internal variants; they show up when searched.
        .filter(|u| all || (u.is_melee() && (u.named || !q.is_empty())) || u.id == state.place_type)
        .filter(|u| q.is_empty() || u.name.to_lowercase().contains(q) || u.id.to_lowercase().contains(q))
        .collect();
    let mut names: HashMap<&str, usize> = HashMap::new();
    for u in &units {
        *names.entry(u.name.as_str()).or_default() += 1;
    }
    let sections: [(&str, UnitFilter); 3] = [
        ("Units", |u| !u.structure && u.object_type != "Hero"),
        ("Heroes", |u| u.object_type == "Hero"),
        ("Structures", |u| u.structure),
    ];
    let mut pick: Option<(String, UnitGroup)> = None;
    egui::ScrollArea::vertical().id_salt("base-unit-list").max_height(320.0).show(ui, |ui| {
        if units.is_empty() {
            hint(ui, tk, "No units match. Any unit id can still be typed above.");
        }
        for g in UnitGroup::ALL {
            let items: Vec<&BaseUnit> = units.iter().copied().filter(|u| u.race == g).collect();
            if items.is_empty() {
                continue;
            }
            let open = items.iter().any(|u| u.id == state.place_type);
            egui::CollapsingHeader::new(format!("{}  ({})", g.label(), items.len()))
                .id_salt(("base-units", g.label()))
                .open(if q.is_empty() { None } else { Some(true) })
                .default_open(open)
                .show(ui, |ui| {
                    for (label, keep) in sections {
                        let sub: Vec<&BaseUnit> = items.iter().copied().filter(|u| keep(u)).collect();
                        if sub.is_empty() {
                            continue;
                        }
                        ui.label(RichText::new(label).small().color(tk.text_dim));
                        for u in sub {
                            let text = if names.get(u.name.as_str()).copied().unwrap_or(0) > 1 && u.name != u.id {
                                format!("{} ({})", u.name, u.id)
                            } else {
                                u.name.clone()
                            };
                            let tip = format!(
                                "{} · {}{} · {}.SC2Mod",
                                u.id,
                                u.object_type,
                                if u.air { " · air" } else { "" },
                                u.source_mod
                            );
                            if ui.selectable_label(state.place_type == u.id, text).on_hover_text(tip).clicked() {
                                pick = Some((u.id.clone(), u.race));
                            }
                        }
                    }
                });
        }
    });
    if let Some((id, race)) = pick {
        state.place_type = id;
        // Resources, rocks and critters belong to the neutral player.
        if race == UnitGroup::Neutral {
            state.place_player = 0;
        } else if state.place_player == 0 {
            state.place_player = 1;
        }
    }
    if base.unit(&state.place_type).is_some_and(|u| u.race == UnitGroup::Neutral) {
        hint(ui, tk, "Neutral objects are usually owned by the neutral player.");
    } else if !state.place_type.is_empty() && !base.has_unit(&state.place_type) {
        hint(ui, tk, &format!("\"{}\" is not a base-game unit; make sure the map defines it.", state.place_type));
    }
}

fn doodad_picker(ui: &mut egui::Ui, state: &mut EditorState, tk: &Tokens) {
    ui.horizontal(|ui| {
        ui.label("Doodad");
        ui.add(
            egui::TextEdit::singleline(&mut state.place_type).desired_width(f32::INFINITY).hint_text("Any doodad id"),
        );
    });
    let search = &mut state.ui.inspector.doodad_search;
    ui.add(egui::TextEdit::singleline(search).hint_text("Search doodads").desired_width(f32::INFINITY));
    let q = search.to_lowercase();
    if let Some(base) = state.base.clone() {
        let matches: Vec<&String> =
            base.doodads().iter().filter(|d| q.is_empty() || d.to_lowercase().contains(&q)).collect();
        let shown = matches.len().min(400);
        egui::ScrollArea::vertical().auto_shrink([false, true]).id_salt("base-doodad-list").max_height(260.0).show(ui, |ui| {
            for d in &matches[..shown] {
                if ui.selectable_label(state.place_type == **d, d.as_str()).clicked() {
                    state.place_type = (*d).clone();
                }
            }
        });
        if matches.len() > shown {
            hint(ui, tk, &format!("{shown} of {} doodads; search to narrow the list.", matches.len()));
        }
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, true]).id_salt("doodad-list").max_height(220.0).show(ui, |ui| {
        for d in known::DOODADS {
            if !q.is_empty() && !d.name.to_lowercase().contains(&q) && !d.id.to_lowercase().contains(&q) {
                continue;
            }
            if ui.selectable_label(state.place_type == d.id, d.name).on_hover_text(d.id).clicked() {
                state.place_type = d.id.into();
            }
        }
    });
}

fn placement_rotation(ui: &mut egui::Ui, state: &mut EditorState) {
    let s = &mut state.ui.viewport.tools.settings;
    let mut deg = s.ghost_rotation.to_degrees();
    ui.horizontal(|ui| {
        ui.label("Facing");
        if ui.add(egui::DragValue::new(&mut deg).range(0.0..=360.0).speed(1.0).suffix("°")).changed() {
            s.ghost_rotation = deg.rem_euclid(360.0).to_radians();
        }
        ui.checkbox(&mut s.snap, "Snap to grid");
    });
}

fn item_list(ui: &mut egui::Ui, state: &mut EditorState, regions: bool) {
    let items: Vec<(strata_core::objects::ObjectId, String)> = if regions {
        state.doc.regions.iter().map(|r| (r.id, r.name.clone())).collect()
    } else {
        state.doc.cameras.iter().map(|c| (c.id, c.name.clone())).collect()
    };
    if items.is_empty() {
        return;
    }
    ui.label(RichText::new(if regions { "Regions" } else { "Cameras" }).strong());
    egui::ScrollArea::vertical().auto_shrink([false, true]).id_salt(("items", regions)).max_height(160.0).show(
        ui,
        |ui| {
            for (id, name) in items {
                let on = if regions { state.selection.region == Some(id) } else { state.selection.camera == Some(id) };
                if ui.selectable_label(on, name).clicked() {
                    state.selection.objects.clear();
                    if regions {
                        state.selection.region = Some(id);
                    } else {
                        state.selection.camera = Some(id);
                    }
                    crate::panels::viewport::frame_selection(state);
                }
            }
        },
    );
}

// ---------------------------------------------------------------- selection

/// A field shared by several objects: `Some(v)` if they agree.
fn common<T: PartialEq + Clone>(objs: &[PlacedObject], f: impl Fn(&PlacedObject) -> T) -> (T, bool) {
    let first = f(&objs[0]);
    let mixed = objs.iter().any(|o| f(o) != first);
    (first, mixed)
}

fn num_field(
    ui: &mut egui::Ui,
    edits: &mut Edits,
    value: f32,
    mixed: bool,
    speed: f64,
    suffix: &'static str,
    range: std::ops::RangeInclusive<f32>,
) -> Option<f32> {
    let mut v = value;
    let mut dv = egui::DragValue::new(&mut v).speed(speed).range(range).suffix(suffix).max_decimals(2);
    if mixed {
        dv = dv.custom_formatter(|_, _| "—".into());
    }
    let r = ui.add_sized([72.0, 20.0], dv);
    edits.track(&r);
    (r.changed() && v != value).then_some(v)
}

fn object_properties(ui: &mut egui::Ui, state: &mut EditorState, edits: &mut Edits, tk: &Tokens) {
    let objs = selected_objects(state);
    if objs.is_empty() {
        return;
    }
    let n = objs.len();
    let (kind, kind_mixed) = common(&objs, |o| o.kind);
    let title = if n == 1 {
        match kind {
            ObjectKind::Unit => "Unit".to_string(),
            ObjectKind::Doodad => "Doodad".to_string(),
            ObjectKind::Point => "Point".to_string(),
        }
    } else {
        plural(n, "object")
    };
    ui.label(RichText::new(title).size(16.0).strong());

    // Each closure edits one field on every selected object.
    let mut change: Option<(&'static str, ObjectOp)> = None;

    egui::Grid::new("object-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        if kind_mixed {
            ui.label("Kind");
            ui.label(RichText::new("Mixed").color(tk.text_dim));
            ui.end_row();
        }

        ui.label("Type");
        let (ty, ty_mixed) = common(&objs, |o| o.type_id.clone());
        let mut t = if ty_mixed { String::new() } else { ty.clone() };
        let r = ui.add(egui::TextEdit::singleline(&mut t).desired_width(150.0).hint_text(if ty_mixed {
            "—"
        } else {
            ""
        }));
        edits.track(&r);
        if r.changed() {
            change = Some(("Change type", Box::new(move |o| o.type_id = t.clone())));
        }
        ui.end_row();
        if !ty_mixed {
            let point_name = || known::POINT_TYPES.iter().find(|k| k.id == ty).map(|k| k.name);
            let base_name =
                state.base.as_ref().and_then(|b| b.display_name(CatalogKind::Unit, &ty)).map(str::to_owned);
            if let Some(name) = base_name
                .as_deref()
                .or(known::unit_name(&ty))
                .or(known::doodad_name(&ty))
                .or_else(point_name)
                .filter(|n| *n != ty)
            {
                ui.label("");
                ui.label(RichText::new(name).color(tk.text_dim).size(12.0));
                ui.end_row();
            }
        }

        ui.label("Name");
        let (name, name_mixed) = common(&objs, |o| o.name.clone());
        let mut nm = if name_mixed { String::new() } else { name };
        let r = ui.add(egui::TextEdit::singleline(&mut nm).desired_width(150.0).hint_text(if name_mixed {
            "—"
        } else {
            "Optional"
        }));
        edits.track(&r);
        if r.changed() {
            change = Some(("Rename", Box::new(move |o| o.name = nm.clone())));
        }
        ui.end_row();

        if kind == ObjectKind::Unit && !kind_mixed {
            ui.label("Player");
            let (pl, pl_mixed) = common(&objs, |o| o.player);
            let mut ids: Vec<(u8, String)> = state.doc.info.players.iter().map(|p| (p.id, p.name.clone())).collect();
            ids.insert(0, (0, "Neutral".into()));
            let text = if pl_mixed {
                "—".to_string()
            } else {
                ids.iter().find(|(i, _)| *i == pl).map(|(i, n)| format!("{i} · {n}")).unwrap_or(format!("{pl}"))
            };
            let mut chosen = None;
            egui::ComboBox::from_id_salt("obj-player").selected_text(text).show_ui(ui, |ui| {
                for (id, name) in &ids {
                    let c = player_color(&state.doc, *id, tk);
                    if player_choice(ui, !pl_mixed && pl == *id, c, &format!("{id} · {name}")).clicked() {
                        chosen = Some(*id);
                    }
                }
            });
            if let Some(p) = chosen {
                change = Some(("Change owner", Box::new(move |o| o.player = p)));
            }
            ui.end_row();
        }

        ui.label("Position");
        ui.horizontal(|ui| {
            let w = state.doc.world_size();
            let (x, xm) = common(&objs, |o| o.position.x);
            if let Some(v) = num_field(ui, edits, x, xm, 0.1, "", 0.0..=w.x) {
                change = Some(("Move", Box::new(move |o| o.position.x = v)));
            }
            let (y, ym) = common(&objs, |o| o.position.y);
            if let Some(v) = num_field(ui, edits, y, ym, 0.1, "", 0.0..=w.y) {
                change = Some(("Move", Box::new(move |o| o.position.y = v)));
            }
        });
        ui.end_row();

        ui.label("Height");
        let (z, zm) = common(&objs, |o| o.position.z);
        if let Some(v) = num_field(ui, edits, z, zm, 0.05, "", -16.0..=96.0) {
            change = Some(("Change height", Box::new(move |o| o.position.z = v)));
        }
        ui.end_row();

        ui.label("Rotation");
        let (rot, rm) = common(&objs, |o| (o.rotation.to_degrees().rem_euclid(360.0) * 100.0).round() / 100.0);
        if let Some(v) = num_field(ui, edits, rot, rm, 1.0, "°", -360.0..=720.0) {
            change = Some(("Rotate", Box::new(move |o| o.rotation = v.rem_euclid(360.0).to_radians())));
        }
        ui.end_row();

        ui.label("Scale");
        let (sc, sm) = common(&objs, |o| o.scale.x);
        if let Some(v) = num_field(ui, edits, sc, sm, 0.01, "", 0.05..=20.0) {
            change = Some(("Scale", Box::new(move |o| o.scale = glam::Vec3::splat(v))));
        }
        ui.end_row();
    });

    // Re-snap height when moving in XY so objects stay on the ground.
    if let Some((label, f)) = change {
        let label = if n == 1 { label.to_string() } else { format!("{label} {}", plural(n, "object")) };
        begin(state, &label, |s| Before::Objects(selected_objects(s)));
        let ids: Vec<_> = state.selection.objects.iter().copied().collect();
        for id in ids {
            let t = &state.doc.terrain;
            if let Some(o) = state.doc.objects.get_mut(id) {
                let (ox, oy) = (o.position.x, o.position.y);
                f(o);
                if o.position.x != ox || o.position.y != oy {
                    o.position.z = t.sample_height(o.position.x, o.position.y);
                }
            }
        }
        state.doc.dirty.objects = true;
    }

    ui.horizontal(|ui| {
        if ui.button("Frame").on_hover_text("Frame the selection (F)").clicked() {
            crate::panels::viewport::frame_selection(state);
        }
        if ui.button("Duplicate").on_hover_text("Ctrl+D").clicked() {
            commit_pending(state);
            crate::tools::select::duplicate_selection(state);
        }
        if ui.button("Delete").on_hover_text("Delete").clicked() {
            commit_pending(state);
            crate::tools::select::delete_selection(state);
        }
    });
}

fn region_properties(ui: &mut egui::Ui, state: &mut EditorState, edits: &mut Edits) {
    let Some(rid) = state.selection.region else { return };
    let Some(idx) = state.doc.regions.iter().position(|r| r.id == rid) else { return };
    ui.label(RichText::new("Region").size(16.0).strong());
    let mut r = state.doc.regions[idx].clone();
    let mut changed = false;
    let mut label = "Edit region";
    egui::Grid::new("region-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        let resp = ui.add(egui::TextEdit::singleline(&mut r.name).desired_width(150.0));
        edits.track(&resp);
        if resp.changed() {
            changed = true;
            label = "Rename region";
        }
        ui.end_row();
        ui.label("Color");
        let resp = ui.color_edit_button_srgb(&mut r.color);
        edits.track(&resp);
        if resp.changed() {
            changed = true;
            label = "Change region color";
        }
        ui.end_row();
    });
    let mut remove = None;
    let shape_count = r.shapes.len();
    for (i, s) in r.shapes.iter_mut().enumerate() {
        let mut f = |ui: &mut egui::Ui, v: &mut f32| {
            let resp = ui.add_sized([64.0, 20.0], egui::DragValue::new(v).speed(0.1).max_decimals(2));
            edits.track(&resp);
            if resp.changed() {
                changed = true;
                label = "Resize region";
            }
        };
        ui.horizontal(|ui| {
            let kind = match s {
                Shape::Rect { .. } => "Rectangle".to_string(),
                Shape::Circle { .. } => "Circle".to_string(),
                Shape::Polygon { points } => format!("Polygon, {} points", points.len()),
            };
            ui.label(RichText::new(kind).strong());
            if ui
                .add_enabled(shape_count > 1, egui::Button::new("Remove").small())
                .on_hover_text("Remove this shape")
                .clicked()
            {
                remove = Some(i);
            }
        });
        egui::Grid::new(("shape", i)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| match s {
            Shape::Rect { min, max } => {
                ui.label("Min");
                ui.horizontal(|ui| {
                    f(ui, &mut min.x);
                    f(ui, &mut min.y);
                });
                ui.end_row();
                ui.label("Max");
                ui.horizontal(|ui| {
                    f(ui, &mut max.x);
                    f(ui, &mut max.y);
                });
                ui.end_row();
            }
            Shape::Circle { center, radius } => {
                ui.label("Center");
                ui.horizontal(|ui| {
                    f(ui, &mut center.x);
                    f(ui, &mut center.y);
                });
                ui.end_row();
                ui.label("Radius");
                f(ui, radius);
                ui.end_row();
            }
            Shape::Polygon { .. } => {}
        });
    }
    if let Some(i) = remove {
        commit_pending(state);
        let before = state.doc.regions.clone();
        let mut after = before.clone();
        after[idx].shapes.remove(i);
        state.commit("Remove region shape", Edit::Regions { before, after });
        return;
    }
    if changed {
        begin(state, label, |s| Before::Regions(s.doc.regions.clone()));
        state.doc.regions[idx] = r;
        state.doc.dirty.regions = true;
    }
}

fn camera_properties(ui: &mut egui::Ui, state: &mut EditorState, edits: &mut Edits) {
    let Some(cid) = state.selection.camera else { return };
    let Some(idx) = state.doc.cameras.iter().position(|c| c.id == cid) else { return };
    ui.label(RichText::new("Camera").size(16.0).strong());
    let mut c = state.doc.cameras[idx].clone();
    let mut changed = false;
    egui::Grid::new("camera-props").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        let r = ui.add(egui::TextEdit::singleline(&mut c.name).desired_width(150.0));
        edits.track(&r);
        changed |= r.changed();
        ui.end_row();
        let mut row = |ui: &mut egui::Ui,
                       label: &str,
                       v: &mut f32,
                       speed: f64,
                       range: std::ops::RangeInclusive<f32>,
                       suffix: &str| {
            ui.label(label);
            let r = ui.add_sized(
                [72.0, 20.0],
                egui::DragValue::new(v).speed(speed).range(range).suffix(suffix).max_decimals(2),
            );
            edits.track(&r);
            changed |= r.changed();
            ui.end_row();
        };
        let w = state.doc.world_size();
        row(ui, "Target x", &mut c.target.x, 0.1, 0.0..=w.x, "");
        row(ui, "Target y", &mut c.target.y, 0.1, 0.0..=w.y, "");
        row(ui, "Distance", &mut c.distance, 0.1, 1.0..=200.0, "");
        row(ui, "Pitch", &mut c.pitch, 0.5, 0.0..=90.0, "°");
        row(ui, "Yaw", &mut c.yaw, 0.5, 0.0..=360.0, "°");
        row(ui, "Field of view", &mut c.fov, 0.5, 5.0..=120.0, "°");
    });
    if changed {
        begin(state, "Edit camera", |s| Before::Cameras(s.doc.cameras.clone()));
        state.doc.cameras[idx] = c;
        state.doc.dirty.cameras = true;
    }
    if ui.button("View from this camera").clicked() {
        let c = &state.doc.cameras[idx];
        let z = state.doc.terrain.sample_height(c.target.x, c.target.y);
        let cam = &mut state.ui.viewport.camera;
        cam.set_ortho(false);
        cam.goal.target = glam::Vec3::new(c.target.x, c.target.y, z);
        cam.goal.distance = c.distance;
        cam.goal.pitch = c.pitch.to_radians();
        cam.goal.yaw = c.yaw.to_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::MapDocument;

    #[test]
    fn pending_object_edit_commits_once() {
        let mut s = EditorState::new(MapDocument::new("t", 32, 32));
        let id = s.doc.objects.alloc_id();
        let o = PlacedObject::new(id, ObjectKind::Unit, "Marine", glam::Vec3::new(4.0, 4.0, 10.0));
        s.commit("Place", Edit::Object { id, before: None, after: Some(o) });
        s.selection.objects.insert(id);
        let rev = s.history.revision();
        // Simulate three drag frames.
        for x in [5.0, 6.0, 7.0] {
            begin(&mut s, "Move", |s| Before::Objects(selected_objects(s)));
            s.doc.objects.get_mut(id).unwrap().position.x = x;
        }
        commit_pending(&mut s);
        assert_eq!(s.history.revision(), rev + 1);
        assert_eq!(s.doc.objects.get(id).unwrap().position.x, 7.0);
        s.undo();
        assert_eq!(s.doc.objects.get(id).unwrap().position.x, 4.0);
    }

    #[test]
    fn pickers_render_with_and_without_base_data() {
        let mut s = EditorState::new(MapDocument::new("t", 32, 32));
        let ctx = egui::Context::default();
        let frame = |s: &mut EditorState| {
            let mut out = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| show(ui, s));
            });
            out.textures_delta.clear();
        };
        for mode in [Mode::Units, Mode::Doodads] {
            s.mode = mode;
            frame(&mut s);
        }
        s.base = Some(crate::panels::data::tests::sample_base());
        s.ui.inspector.unit_search = "mineral".into();
        for mode in [Mode::Units, Mode::Doodads] {
            s.mode = mode;
            frame(&mut s);
        }
        assert_eq!(s.base.as_ref().unwrap().units().len(), 2);
    }

    #[test]
    fn layer_edit_is_undoable() {
        let mut s = EditorState::new(MapDocument::new("t", 8, 8));
        let before = s.doc.terrain.layers.clone();
        let mut after = before.clone();
        after.push(TextureLayer { texture: "AgriaGrassGreen".into(), weights: vec![0; 81] });
        s.commit("Add texture layer", Edit::TextureLayers { before: before.clone(), after });
        assert_eq!(s.doc.terrain.layers.len(), 2);
        s.undo();
        assert_eq!(s.doc.terrain.layers, before);
    }
}
