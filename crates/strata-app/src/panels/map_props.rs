//! Map name, description, size, playable area, players and dependencies.

use egui::containers::menu::MenuButton;
use egui::{Button, Color32, DragValue, Id, RichText, Sense, Ui};
use strata_core::history::Edit;
use strata_core::strings::DEFAULT_LOCALE;
use strata_core::{MapInfo, Player, PlayerControl, Race};

use crate::panels::data::{commit_text, hint, TextOpts};
use crate::state::{EditorState, ToastKind};
use crate::theme::{self, Tokens};

#[derive(Default)]
pub struct MapPropsState {
    new_dep: String,
}

/// Common SC2 terrain sets (tileset ids as written in MapInfo).
pub const TILESETS: &[&str] = &[
    "Agria",
    "Aiur",
    "Avernus",
    "Bel'Shir",
    "Braxis",
    "Castanar",
    "Char",
    "Haven",
    "Korhal",
    "Mar Sara",
    "Meinhoff",
    "Monlyth",
    "New Folsom",
    "Redstone",
    "Shakuras",
    "Space Platform",
    "Tarsonis",
    "Typhon",
    "Ulaan",
    "Ulnar",
    "Valhalla",
    "Xil",
    "Zerus",
    "Zhakul'Das",
];

/// SC2's player color presets.
pub const PLAYER_COLORS: &[(&str, [u8; 3])] = &[
    ("Red", [180, 20, 30]),
    ("Blue", [0, 66, 255]),
    ("Teal", [28, 167, 234]),
    ("Purple", [84, 0, 129]),
    ("Yellow", [235, 225, 41]),
    ("Orange", [254, 138, 14]),
    ("Green", [22, 128, 0]),
    ("Light pink", [204, 166, 252]),
    ("Violet", [31, 1, 201]),
    ("Light gray", [82, 84, 148]),
    ("Dark green", [16, 98, 70]),
    ("Brown", [78, 42, 4]),
    ("Light green", [150, 255, 145]),
    ("Dark gray", [35, 35, 35]),
    ("Pink", [229, 91, 176]),
];

/// Dependency presets: (label, dependency string).
pub const DEPENDENCIES: &[(&str, &str)] = &[
    ("Core", "bnet:Core.SC2Mod/0.0/999,file:Mods/Core.SC2Mod"),
    (
        "Liberty",
        "bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod",
    ),
    (
        "Liberty multiplayer",
        "bnet:LibertyMulti.SC2Mod/0.0/999,file:Mods/LibertyMulti.SC2Mod",
    ),
    ("Swarm", "bnet:Swarm.SC2Mod/0.0/999,file:Mods/Swarm.SC2Mod"),
    (
        "Swarm multiplayer",
        "bnet:SwarmMulti.SC2Mod/0.0/999,file:Mods/SwarmMulti.SC2Mod",
    ),
    ("Void", "bnet:Void.SC2Mod/0.0/999,file:Mods/Void.SC2Mod"),
    (
        "Void multiplayer",
        "bnet:VoidMulti.SC2Mod/0.0/999,file:Mods/VoidMulti.SC2Mod",
    ),
];

const MAX_PLAYERS: usize = 15;

fn race_label(r: Race) -> &'static str {
    match r {
        Race::Terran => "Terran",
        Race::Protoss => "Protoss",
        Race::Zerg => "Zerg",
        Race::Random => "Random",
        Race::Neutral => "Neutral",
    }
}

const CONTROLS: [PlayerControl; 5] = [
    PlayerControl::User,
    PlayerControl::Computer,
    PlayerControl::Neutral,
    PlayerControl::Hostile,
    PlayerControl::Open,
];

fn control_label(c: PlayerControl) -> &'static str {
    match c {
        PlayerControl::User => "User",
        PlayerControl::Computer => "Computer",
        PlayerControl::Neutral => "Neutral",
        PlayerControl::Hostile => "Hostile",
        PlayerControl::Open => "Open slot",
    }
}

/// "Liberty.SC2Mod" from "bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod".
fn dependency_name(d: &str) -> String {
    if let Some((label, _)) = DEPENDENCIES.iter().find(|(_, v)| *v == d) {
        return (*label).to_owned();
    }
    let file = d
        .split(',')
        .find_map(|p| p.strip_prefix("file:"))
        .unwrap_or(d);
    file.rsplit(['/', '\\']).next().unwrap_or(file).to_owned()
}

/// Why a playable area is invalid, if it is.
pub fn playable_error(p: [u32; 4], w: u32, h: u32) -> Option<String> {
    let [l, b, r, t] = p;
    if l >= r {
        Some("Left must be less than right.".into())
    } else if b >= t {
        Some("Bottom must be less than top.".into())
    } else if r > w || t > h {
        Some(format!(
            "The playable area must fit inside the {w} × {h} map."
        ))
    } else {
        None
    }
}

/// Smallest slot id in 1..=15 not used yet.
fn free_slot(players: &[Player]) -> Option<u8> {
    (1..=MAX_PLAYERS as u8).find(|id| !players.iter().any(|p| p.id == *id))
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).size(16.0).strong());
    ui.add_space(2.0);
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let info = &state.doc.info;
    let mut change: Option<(String, MapInfo)> = None;
    let mut rename: Option<String> = None;
    let mut toast: Option<String> = None;
    let mut set = |label: &str, f: &dyn Fn(&mut MapInfo)| {
        let mut n = info.clone();
        f(&mut n);
        if n != *info {
            change = Some((label.to_owned(), n));
        }
    };

    ui.add_space(4.0);
    section(ui, "General");
    egui::Grid::new("map_props_general")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            let (_, new) = commit_text(
                ui,
                Id::new("mp_name"),
                &info.name,
                TextOpts {
                    width: 280.0,
                    hint: "Shown in the lobby",
                    ..Default::default()
                },
            );
            if let Some(n) = new {
                rename = Some(n.trim().to_owned());
            }
            ui.end_row();

            ui.label("Description");
            let o = TextOpts {
                multiline: true,
                rows: 3,
                width: 280.0,
                hint: "What players will do on this map",
                ..Default::default()
            };
            if let (_, Some(n)) = commit_text(ui, Id::new("mp_desc"), &info.description, o) {
                set("Edit map description", &|i| i.description = n.clone());
            }
            ui.end_row();

            ui.label("Author");
            if let (_, Some(n)) = commit_text(
                ui,
                Id::new("mp_author"),
                &info.author,
                TextOpts {
                    width: 280.0,
                    ..Default::default()
                },
            ) {
                set("Edit map author", &|i| i.author = n.trim().to_owned());
            }
            ui.end_row();

            ui.label("Tileset");
            ui.horizontal(|ui| {
                let mut v = info.tileset.clone();
                egui::ComboBox::from_id_salt("mp_tileset")
                    .selected_text(&v)
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for t in TILESETS {
                            ui.selectable_value(&mut v, (*t).to_owned(), *t);
                        }
                    });
                let (_, typed) = commit_text(
                    ui,
                    Id::new("mp_tileset_text"),
                    &info.tileset,
                    TextOpts {
                        width: 110.0,
                        hint: "Other id",
                        ..Default::default()
                    },
                );
                if let Some(t) = typed {
                    v = t.trim().to_owned();
                }
                if v != info.tileset && !v.is_empty() {
                    set("Change tileset", &|i| i.tileset = v.clone());
                }
            });
            ui.end_row();
        });

    section(ui, "Size");
    ui.horizontal(|ui| {
        ui.label(format!("{} × {} cells", info.width, info.height));
        ui.add_enabled(false, Button::new("Resize…")).on_disabled_hover_text(
            "Resizing isn't available yet. To change the size, create a new map at the size you need.",
        );
    });

    section(ui, "Playable area");
    playable(ui, &tk, info, &mut set, &mut toast);

    section(ui, "Players");
    players(ui, &tk, info, &mut set);

    section(ui, "Dependencies");
    hint(
        ui,
        &tk,
        "Mods this map builds on, in load order. Melee maps need Liberty, Swarm and Void.",
    );
    dependencies(ui, &tk, info, &mut state.ui.map_props, &mut set);
    drop(set);

    if let Some(name) = rename {
        if name.is_empty() {
            toast = Some("The map needs a name.".into());
        } else {
            let before = Box::new(state.doc.info.clone());
            let mut after = before.clone();
            after.name = name.clone();
            let sb = Box::new(state.doc.strings.clone());
            let mut sa = sb.clone();
            sa.set(DEFAULT_LOCALE, "DocInfo/Name", &name);
            state.commit(
                format!("Rename map to {name}"),
                Edit::Batch(vec![
                    Edit::Info { before, after },
                    Edit::Strings {
                        before: sb,
                        after: sa,
                    },
                ]),
            );
        }
    } else if let Some((label, after)) = change {
        let before = Box::new(state.doc.info.clone());
        state.commit(
            label,
            Edit::Info {
                before,
                after: Box::new(after),
            },
        );
    }
    if let Some(t) = toast {
        state.toast(ToastKind::Error, t);
    }
}

type Setter<'a> = dyn FnMut(&str, &dyn Fn(&mut MapInfo)) + 'a;

fn playable(
    ui: &mut Ui,
    tk: &Tokens,
    info: &MapInfo,
    set: &mut Setter<'_>,
    toast: &mut Option<String>,
) {
    let (w, h) = (info.width, info.height);
    let pid = Id::new("mp_playable_edit");
    let mut p: [u32; 4] = ui.data(|d| d.get_temp(pid)).unwrap_or(info.playable);
    let mut active = false;
    ui.horizontal(|ui| {
        egui::Grid::new("mp_playable")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                for (n, label) in ["Left", "Bottom", "Right", "Top"].into_iter().enumerate() {
                    ui.label(label);
                    let max = if n % 2 == 0 { w } else { h };
                    let r = ui.add(DragValue::new(&mut p[n]).range(0..=max).speed(0.25));
                    active |= r.dragged() || r.has_focus();
                    ui.end_row();
                }
            });
        ui.add_space(12.0);
        preview(ui, tk, info, p);
    });
    let err = playable_error(p, w, h);
    if let Some(e) = &err {
        ui.colored_label(tk.error, e);
    } else {
        hint(
            ui,
            tk,
            format!("{} × {} playable cells", p[2] - p[0], p[3] - p[1]),
        );
    }
    if active {
        ui.data_mut(|d| d.insert_temp(pid, p));
    } else {
        ui.data_mut(|d| d.remove::<[u32; 4]>(pid));
        if p != info.playable {
            match err {
                None => set("Edit playable area", &|i| i.playable = p),
                Some(e) => *toast = Some(format!("{e} The playable area was not changed.")),
            }
        }
    }
    ui.horizontal(|ui| {
        if ui.button("Use whole map").clicked() {
            set("Use whole map as playable area", &|i| {
                i.playable = [0, 0, w, h]
            });
        }
        if ui
            .button("Use default margins")
            .on_hover_text("Leave a border for the camera, like new maps")
            .clicked()
        {
            let d = MapInfo::new("", w, h).playable;
            set("Reset playable area", &|i| i.playable = d);
        }
    });
}

/// A small drawing of the map with its playable area and start locations.
fn preview(ui: &mut Ui, tk: &Tokens, info: &MapInfo, p: [u32; 4]) {
    let (w, h) = (info.width.max(1) as f32, info.height.max(1) as f32);
    let scale = 120.0 / w.max(h);
    let size = egui::vec2(w * scale, h * scale);
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let r = resp.rect;
    let to_screen = |x: f32, y: f32| egui::pos2(r.left() + x * scale, r.bottom() - y * scale);
    painter.rect_filled(r, 2.0, tk.bg_deep);
    painter.rect_stroke(
        r,
        2.0,
        egui::Stroke::new(1.0, tk.stroke),
        egui::StrokeKind::Inside,
    );
    let pr = egui::Rect::from_two_pos(
        to_screen(p[0] as f32, p[1] as f32),
        to_screen(p[2] as f32, p[3] as f32),
    );
    painter.rect_filled(pr, 0.0, tk.accent.gamma_multiply(0.15));
    painter.rect_stroke(
        pr,
        0.0,
        egui::Stroke::new(1.0, tk.accent),
        egui::StrokeKind::Inside,
    );
    for pl in &info.players {
        if let Some(s) = pl.start_location {
            let c = Color32::from_rgb(pl.color[0], pl.color[1], pl.color[2]);
            painter.circle_filled(to_screen(s.x, s.y), 3.5, c);
        }
    }
    resp.on_hover_text("Map (outline), playable area (highlighted) and start locations");
}

fn color_swatch(ui: &mut Ui, tk: &Tokens, id: u8, color: [u8; 3]) -> Option<[u8; 3]> {
    let c = Color32::from_rgb(color[0], color[1], color[2]);
    let name = PLAYER_COLORS
        .iter()
        .find(|(_, v)| *v == color)
        .map(|(n, _)| *n)
        .unwrap_or("Custom");
    let b = Button::new("")
        .fill(c)
        .stroke(egui::Stroke::new(1.0, tk.stroke))
        .min_size(egui::vec2(36.0, 18.0));
    let mut picked = None;
    let (resp, _) = MenuButton::from_button(b).ui(ui, |ui| {
        ui.label(format!("Color for player {id}"));
        egui::Grid::new(("mp_colors", id))
            .num_columns(5)
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                for (n, (label, rgb)) in PLAYER_COLORS.iter().enumerate() {
                    let fill = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                    let stroke = if *rgb == color {
                        egui::Stroke::new(2.0, tk.accent)
                    } else {
                        egui::Stroke::new(1.0, tk.stroke)
                    };
                    if ui
                        .add(
                            Button::new("")
                                .fill(fill)
                                .stroke(stroke)
                                .min_size(egui::vec2(28.0, 20.0)),
                        )
                        .on_hover_text(*label)
                        .clicked()
                    {
                        picked = Some(*rgb);
                        ui.close();
                    }
                    if n % 5 == 4 {
                        ui.end_row();
                    }
                }
            });
    });
    resp.on_hover_text(name);
    picked
}

fn players(ui: &mut Ui, tk: &Tokens, info: &MapInfo, set: &mut Setter<'_>) {
    egui::Grid::new("mp_players")
        .num_columns(8)
        .striped(true)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for h in [
                "Slot",
                "Name",
                "Race",
                "Control",
                "Color",
                "Team",
                "Start location",
                "",
            ] {
                ui.label(RichText::new(h).color(tk.text_dim));
            }
            ui.end_row();
            for (n, p) in info.players.iter().enumerate() {
                let pid = p.id;
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(pid.to_string())
                });
                if let (_, Some(name)) = commit_text(
                    ui,
                    Id::new(("mp_player_name", pid)),
                    &p.name,
                    TextOpts {
                        width: 120.0,
                        ..Default::default()
                    },
                ) {
                    set(&format!("Rename player {pid}"), &|i| {
                        i.players[n].name = name.trim().to_owned()
                    });
                }
                let mut race = p.race;
                egui::ComboBox::from_id_salt(("mp_race", pid))
                    .selected_text(race_label(race))
                    .width(90.0)
                    .show_ui(ui, |ui| {
                        for r in Race::ALL {
                            ui.selectable_value(&mut race, r, race_label(r));
                        }
                    });
                if race != p.race {
                    set(&format!("Set player {pid} race"), &|i| {
                        i.players[n].race = race
                    });
                }
                let mut control = p.control;
                egui::ComboBox::from_id_salt(("mp_control", pid))
                    .selected_text(control_label(control))
                    .width(100.0)
                    .show_ui(ui, |ui| {
                        for c in CONTROLS {
                            ui.selectable_value(&mut control, c, control_label(c));
                        }
                    });
                if control != p.control {
                    set(&format!("Set player {pid} control"), &|i| {
                        i.players[n].control = control
                    });
                }
                if let Some(c) = color_swatch(ui, tk, pid, p.color) {
                    set(&format!("Set player {pid} color"), &|i| {
                        i.players[n].color = c
                    });
                }
                let mut team = p.team;
                egui::ComboBox::from_id_salt(("mp_team", pid))
                    .selected_text(team.to_string())
                    .width(48.0)
                    .show_ui(ui, |ui| {
                        for t in 1..=MAX_PLAYERS as u8 {
                            ui.selectable_value(&mut team, t, t.to_string());
                        }
                    });
                if team != p.team {
                    set(&format!("Set player {pid} team"), &|i| {
                        i.players[n].team = team
                    });
                }
                match p.start_location {
                    Some(s) => ui.label(format!("{:.1}, {:.1}", s.x, s.y)),
                    None => ui.label(RichText::new("Not placed").color(tk.text_dim)),
                }
                .on_hover_text("Place start locations on the map");
                let can_remove = info.players.len() > 1;
                let r = ui.add_enabled(can_remove, Button::new("Remove").small());
                if r.on_disabled_hover_text("A map needs at least one player slot")
                    .clicked()
                {
                    set(&format!("Remove player slot {pid}"), &|i| {
                        i.players.remove(n);
                    });
                }
                ui.end_row();
            }
        });
    let full = info.players.len() >= MAX_PLAYERS;
    let b = ui
        .add_enabled(!full, Button::new("Add player slot"))
        .on_disabled_hover_text("SC2 maps have at most 15 player slots");
    if b.clicked() {
        if let Some(id) = free_slot(&info.players) {
            let color = PLAYER_COLORS[(id as usize - 1) % PLAYER_COLORS.len()].1;
            let p = Player {
                id,
                name: format!("Player {id}"),
                race: Race::Random,
                control: PlayerControl::User,
                color,
                team: id,
                start_location: None,
            };
            set(&format!("Add player slot {id}"), &|i| {
                i.players.push(p.clone());
                i.players.sort_by_key(|p| p.id);
            });
        }
    }
}

fn dependencies(
    ui: &mut Ui,
    tk: &Tokens,
    info: &MapInfo,
    ms: &mut MapPropsState,
    set: &mut Setter<'_>,
) {
    let deps = &info.dependencies;
    if deps.is_empty() {
        hint(
            ui,
            tk,
            "No dependencies. Add Liberty, Swarm and Void to use the standard game data.",
        );
    }
    let n = deps.len();
    egui::Grid::new("mp_deps")
        .num_columns(2)
        .striped(true)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (k, d) in deps.iter().enumerate() {
                ui.label(dependency_name(d)).on_hover_text(d);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(k > 0, Button::new("⏶").small())
                        .on_hover_text("Load earlier")
                        .clicked()
                    {
                        set("Reorder dependencies", &|i| i.dependencies.swap(k, k - 1));
                    }
                    if ui
                        .add_enabled(k + 1 < n, Button::new("⏷").small())
                        .on_hover_text("Load later")
                        .clicked()
                    {
                        set("Reorder dependencies", &|i| i.dependencies.swap(k, k + 1));
                    }
                    if ui.small_button("Remove").clicked() {
                        let name = dependency_name(d);
                        set(&format!("Remove dependency {name}"), &|i| {
                            i.dependencies.remove(k);
                        });
                    }
                });
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        ui.menu_button("Add preset", |ui| {
            let missing: Vec<&(&str, &str)> = DEPENDENCIES
                .iter()
                .filter(|(_, v)| !deps.iter().any(|d| d == v))
                .collect();
            if missing.is_empty() {
                hint(ui, tk, "Every preset is already added.");
            }
            for (label, v) in missing {
                if ui.button(*label).on_hover_text(*v).clicked() {
                    set(&format!("Add dependency {label}"), &|i| {
                        i.dependencies.push((*v).to_owned())
                    });
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Standard melee (Liberty, Swarm, Void)").clicked() {
                set("Use standard melee dependencies", &|i| {
                    for v in [DEPENDENCIES[1].1, DEPENDENCIES[3].1, DEPENDENCIES[5].1] {
                        if !i.dependencies.iter().any(|d| d == v) {
                            i.dependencies.push(v.to_owned());
                        }
                    }
                });
                ui.close();
            }
        });
        let r = ui.add(
            egui::TextEdit::singleline(&mut ms.new_dep)
                .hint_text("bnet:Name.SC2Mod/0.0/999,file:Mods/Name.SC2Mod")
                .desired_width(300.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let v = ms.new_dep.trim().to_owned();
        let ok = !v.is_empty() && !deps.contains(&v);
        if (ui.add_enabled(ok, Button::new("Add")).clicked() || (enter && ok)) && ok {
            set(&format!("Add dependency {}", dependency_name(&v)), &|i| {
                i.dependencies.push(v.clone())
            });
            ms.new_dep.clear();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(
            dependency_name("bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod"),
            "Liberty"
        );
        assert_eq!(dependency_name("file:Mods/Custom.SC2Mod"), "Custom.SC2Mod");
        assert!(playable_error([0, 0, 64, 64], 64, 64).is_none());
        assert!(playable_error([10, 0, 5, 64], 64, 64).is_some());
        assert!(playable_error([0, 0, 65, 64], 64, 64).is_some());
        let info = MapInfo::new("x", 64, 64);
        assert_eq!(free_slot(&info.players), Some(3));
    }
}
