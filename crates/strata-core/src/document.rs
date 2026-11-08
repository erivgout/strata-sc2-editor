use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::catalog::Catalog;
use crate::objects::ObjectLayer;
use crate::regions::{Camera, Region};
use crate::strings::Strings;
use crate::terrain::Terrain;
use crate::triggers::TriggerLibrary;

/// Which top-level parts of a document changed since load. The format layer
/// re-emits only dirty components and copies everything else byte-for-byte,
/// so opening and saving a map never loses data Strata does not understand.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirtyFlags {
    pub info: bool,
    pub terrain: bool,
    pub objects: bool,
    pub regions: bool,
    pub cameras: bool,
    pub triggers: bool,
    pub data: bool,
    pub strings: bool,
}

impl DirtyFlags {
    pub fn all() -> Self {
        Self {
            info: true,
            terrain: true,
            objects: true,
            regions: true,
            cameras: true,
            triggers: true,
            data: true,
            strings: true,
        }
    }

    pub fn any(&self) -> bool {
        self.info
            || self.terrain
            || self.objects
            || self.regions
            || self.cameras
            || self.triggers
            || self.data
            || self.strings
    }

    pub fn merge(&mut self, other: DirtyFlags) {
        self.info |= other.info;
        self.terrain |= other.terrain;
        self.objects |= other.objects;
        self.regions |= other.regions;
        self.cameras |= other.cameras;
        self.triggers |= other.triggers;
        self.data |= other.data;
        self.strings |= other.strings;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Race {
    #[default]
    Terran,
    Protoss,
    Zerg,
    Random,
    Neutral,
}

impl Race {
    pub const ALL: [Race; 5] = [Race::Terran, Race::Protoss, Race::Zerg, Race::Random, Race::Neutral];

    /// Race id as written in SC2 data ("Terr", "Prot", "Zerg", "Rand", "Neut").
    pub fn sc2_id(self) -> &'static str {
        match self {
            Race::Terran => "Terr",
            Race::Protoss => "Prot",
            Race::Zerg => "Zerg",
            Race::Random => "Rand",
            Race::Neutral => "Neut",
        }
    }

    pub fn from_sc2_id(id: &str) -> Option<Race> {
        Race::ALL.into_iter().find(|r| r.sc2_id().eq_ignore_ascii_case(id))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlayerControl {
    #[default]
    User,
    Computer,
    Neutral,
    Hostile,
    Open,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    /// SC2 player slot id, 0 = neutral, 1..=15 playable.
    pub id: u8,
    pub name: String,
    pub race: Race,
    pub control: PlayerControl,
    /// sRGB color.
    pub color: [u8; 3],
    pub team: u8,
    /// Start location in world units, if any.
    pub start_location: Option<glam::Vec2>,
}

/// Map-level metadata (MapInfo + DocumentHeader + localized name/description).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapInfo {
    pub name: String,
    pub description: String,
    pub author: String,
    /// Terrain tileset / texture set id, e.g. "Char", "Agria", "Bel'Shir".
    pub tileset: String,
    /// Map size in cells. Matches `Terrain::width/height`.
    pub width: u32,
    pub height: u32,
    /// Playable bounds in cells: [left, bottom, right, top].
    pub playable: [u32; 4],
    pub players: Vec<Player>,
    /// Dependencies such as "bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod".
    pub dependencies: Vec<String>,
}

impl MapInfo {
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        let palette: [[u8; 3]; 4] = [[180, 20, 30], [0, 66, 255], [28, 167, 234], [84, 0, 129]];
        let players = (1u8..=2)
            .map(|id| Player {
                id,
                name: format!("Player {id}"),
                race: Race::Random,
                control: PlayerControl::User,
                color: palette[(id as usize - 1) % palette.len()],
                team: id,
                start_location: None,
            })
            .collect();
        Self {
            name: name.into(),
            description: String::new(),
            author: String::new(),
            tileset: "Char".into(),
            width,
            height,
            playable: [8.min(width / 4), 8.min(height / 4), width - 8.min(width / 4), height - 8.min(height / 4)],
            players,
            dependencies: vec![
                "bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod".into(),
                "bnet:Swarm.SC2Mod/0.0/999,file:Mods/Swarm.SC2Mod".into(),
                "bnet:Void.SC2Mod/0.0/999,file:Mods/Void.SC2Mod".into(),
            ],
        }
    }
}

/// A complete map open in the editor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapDocument {
    pub info: MapInfo,
    pub terrain: Terrain,
    pub objects: ObjectLayer,
    pub regions: Vec<Region>,
    pub cameras: Vec<Camera>,
    pub triggers: TriggerLibrary,
    pub data: Catalog,
    pub strings: Strings,
    /// Archive files Strata does not model (or has not modified), keyed by
    /// archive path with `/` separators. Written back verbatim on save.
    pub passthrough: BTreeMap<String, Vec<u8>>,
    /// Components changed since load; drives what the saver regenerates.
    pub dirty: DirtyFlags,
}

impl MapDocument {
    /// A fresh, flat map of the given size in cells.
    pub fn new(name: impl Into<String>, width: u32, height: u32) -> Self {
        let mut strings = Strings::default();
        let info = MapInfo::new(name, width, height);
        strings.set("enUS", "DocInfo/Name", &info.name);
        Self {
            terrain: Terrain::new(width, height),
            info,
            objects: ObjectLayer::default(),
            regions: Vec::new(),
            cameras: Vec::new(),
            triggers: TriggerLibrary::with_melee_init(),
            data: Catalog::default(),
            strings,
            passthrough: BTreeMap::new(),
            dirty: DirtyFlags::all(),
        }
    }

    /// World-space size (1 cell = 1 world unit, as in SC2).
    pub fn world_size(&self) -> glam::Vec2 {
        glam::Vec2::new(self.terrain.width as f32, self.terrain.height as f32)
    }
}
