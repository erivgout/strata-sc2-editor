//! Curated lists of common StarCraft II ids with display names, used by
//! pickers (unit palette, doodad palette, texture layers).
//!
//! These are *suggestions*: any id the game knows can still be typed in. Ids
//! are the catalog ids written to the Objects file (`UnitType="..."` for
//! units, `Type="..."` for doodads and points) and to `t3Terrain.xml`.

/// One known id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub id: &'static str,
    pub name: &'static str,
}

const fn k(id: &'static str, name: &'static str) -> Known {
    Known { id, name }
}

/// Grouping used by the unit picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnitGroup {
    Terran,
    Protoss,
    Zerg,
    /// Resources, watchtowers, destructibles and other neutral objects.
    Neutral,
}

impl UnitGroup {
    pub const ALL: [UnitGroup; 4] = [UnitGroup::Terran, UnitGroup::Protoss, UnitGroup::Zerg, UnitGroup::Neutral];

    pub fn label(self) -> &'static str {
        match self {
            UnitGroup::Terran => "Terran",
            UnitGroup::Protoss => "Protoss",
            UnitGroup::Zerg => "Zerg",
            UnitGroup::Neutral => "Neutral",
        }
    }

    pub fn units(self) -> &'static [Known] {
        match self {
            UnitGroup::Terran => TERRAN_UNITS,
            UnitGroup::Protoss => PROTOSS_UNITS,
            UnitGroup::Zerg => ZERG_UNITS,
            UnitGroup::Neutral => NEUTRAL_UNITS,
        }
    }
}

pub const TERRAN_UNITS: &[Known] = &[
    k("SCV", "SCV"),
    k("MULE", "MULE"),
    k("Marine", "Marine"),
    k("Marauder", "Marauder"),
    k("Reaper", "Reaper"),
    k("Ghost", "Ghost"),
    k("Hellion", "Hellion"),
    k("HellionTank", "Hellbat"),
    k("WidowMine", "Widow Mine"),
    k("SiegeTank", "Siege Tank"),
    k("SiegeTankSieged", "Siege Tank (sieged)"),
    k("Cyclone", "Cyclone"),
    k("Thor", "Thor"),
    k("VikingFighter", "Viking (air)"),
    k("VikingAssault", "Viking (ground)"),
    k("Medivac", "Medivac"),
    k("Liberator", "Liberator"),
    k("Raven", "Raven"),
    k("Banshee", "Banshee"),
    k("Battlecruiser", "Battlecruiser"),
    k("CommandCenter", "Command Center"),
    k("OrbitalCommand", "Orbital Command"),
    k("PlanetaryFortress", "Planetary Fortress"),
    k("SupplyDepot", "Supply Depot"),
    k("SupplyDepotLowered", "Supply Depot (lowered)"),
    k("Refinery", "Refinery"),
    k("Barracks", "Barracks"),
    k("EngineeringBay", "Engineering Bay"),
    k("Bunker", "Bunker"),
    k("MissileTurret", "Missile Turret"),
    k("SensorTower", "Sensor Tower"),
    k("Factory", "Factory"),
    k("GhostAcademy", "Ghost Academy"),
    k("Starport", "Starport"),
    k("Armory", "Armory"),
    k("FusionCore", "Fusion Core"),
    k("BarracksTechLab", "Tech Lab (Barracks)"),
    k("BarracksReactor", "Reactor (Barracks)"),
    k("FactoryTechLab", "Tech Lab (Factory)"),
    k("FactoryReactor", "Reactor (Factory)"),
    k("StarportTechLab", "Tech Lab (Starport)"),
    k("StarportReactor", "Reactor (Starport)"),
];

pub const PROTOSS_UNITS: &[Known] = &[
    k("Probe", "Probe"),
    k("Zealot", "Zealot"),
    k("Stalker", "Stalker"),
    k("Sentry", "Sentry"),
    k("Adept", "Adept"),
    k("HighTemplar", "High Templar"),
    k("DarkTemplar", "Dark Templar"),
    k("Archon", "Archon"),
    k("Immortal", "Immortal"),
    k("Colossus", "Colossus"),
    k("Disruptor", "Disruptor"),
    k("Observer", "Observer"),
    k("WarpPrism", "Warp Prism"),
    k("Phoenix", "Phoenix"),
    k("VoidRay", "Void Ray"),
    k("Oracle", "Oracle"),
    k("Tempest", "Tempest"),
    k("Carrier", "Carrier"),
    k("Mothership", "Mothership"),
    k("Nexus", "Nexus"),
    k("Pylon", "Pylon"),
    k("Assimilator", "Assimilator"),
    k("Gateway", "Gateway"),
    k("WarpGate", "Warp Gate"),
    k("Forge", "Forge"),
    k("CyberneticsCore", "Cybernetics Core"),
    k("PhotonCannon", "Photon Cannon"),
    k("ShieldBattery", "Shield Battery"),
    k("TwilightCouncil", "Twilight Council"),
    k("RoboticsFacility", "Robotics Facility"),
    k("Stargate", "Stargate"),
    k("TemplarArchive", "Templar Archives"),
    k("DarkShrine", "Dark Shrine"),
    k("RoboticsBay", "Robotics Bay"),
    k("FleetBeacon", "Fleet Beacon"),
];

pub const ZERG_UNITS: &[Known] = &[
    k("Larva", "Larva"),
    k("Drone", "Drone"),
    k("Overlord", "Overlord"),
    k("Overseer", "Overseer"),
    k("Queen", "Queen"),
    k("Zergling", "Zergling"),
    k("Baneling", "Baneling"),
    k("Roach", "Roach"),
    k("Ravager", "Ravager"),
    k("Hydralisk", "Hydralisk"),
    k("LurkerMP", "Lurker"),
    k("Infestor", "Infestor"),
    k("SwarmHostMP", "Swarm Host"),
    k("Ultralisk", "Ultralisk"),
    k("Mutalisk", "Mutalisk"),
    k("Corruptor", "Corruptor"),
    k("BroodLord", "Brood Lord"),
    k("Viper", "Viper"),
    k("Hatchery", "Hatchery"),
    k("Lair", "Lair"),
    k("Hive", "Hive"),
    k("Extractor", "Extractor"),
    k("SpawningPool", "Spawning Pool"),
    k("EvolutionChamber", "Evolution Chamber"),
    k("RoachWarren", "Roach Warren"),
    k("BanelingNest", "Baneling Nest"),
    k("SpineCrawler", "Spine Crawler"),
    k("SporeCrawler", "Spore Crawler"),
    k("HydraliskDen", "Hydralisk Den"),
    k("LurkerDenMP", "Lurker Den"),
    k("InfestationPit", "Infestation Pit"),
    k("Spire", "Spire"),
    k("GreaterSpire", "Greater Spire"),
    k("NydusNetwork", "Nydus Network"),
    k("UltraliskCavern", "Ultralisk Cavern"),
    k("CreepTumorBurrowed", "Creep Tumor"),
];

pub const NEUTRAL_UNITS: &[Known] = &[
    k("MineralField", "Mineral Field"),
    k("MineralField750", "Mineral Field (750)"),
    k("RichMineralField", "Rich Mineral Field"),
    k("RichMineralField750", "Rich Mineral Field (750)"),
    k("LabMineralField", "Lab Mineral Field"),
    k("LabMineralField750", "Lab Mineral Field (750)"),
    k("VespeneGeyser", "Vespene Geyser"),
    k("RichVespeneGeyser", "Rich Vespene Geyser"),
    k("SpacePlatformGeyser", "Space Platform Geyser"),
    k("ProtossVespeneGeyser", "Protoss Vespene Geyser"),
    k("XelNagaTower", "Xel'Naga Tower"),
    k("DestructibleRock6x6", "Destructible Rocks (6x6)"),
    k("DestructibleDebris6x6", "Destructible Debris (6x6)"),
    k("DestructibleRockEx16x6", "Destructible Rocks (6x6, alt)"),
    k("DestructibleRockEx14x4", "Destructible Rocks (4x4)"),
    k("DestructibleRampDiagonalHugeULBR", "Destructible Ramp Rocks (UL-BR)"),
    k("DestructibleRampDiagonalHugeBLUR", "Destructible Ramp Rocks (BL-UR)"),
    k("DestructibleDebrisRampDiagonalHugeULBR", "Destructible Ramp Debris (UL-BR)"),
    k("DestructibleDebrisRampDiagonalHugeBLUR", "Destructible Ramp Debris (BL-UR)"),
    k("UnbuildableRocksDestructible", "Unbuildable Rocks"),
    k("UnbuildableBricksDestructible", "Unbuildable Bricks"),
    k("UnbuildablePlatesDestructible", "Unbuildable Plates"),
];

/// Every known unit, tagged with its group.
pub fn all_units() -> impl Iterator<Item = (UnitGroup, &'static Known)> {
    UnitGroup::ALL.into_iter().flat_map(|g| g.units().iter().map(move |u| (g, u)))
}

/// Display name for a unit id, if it is in the curated lists.
pub fn unit_name(id: &str) -> Option<&'static str> {
    all_units().find(|(_, u)| u.id == id).map(|(_, u)| u.name)
}

/// Suggested `type_id` prefixes that should be owned by the neutral player
/// (resources, towers, destructibles) when placed.
pub fn is_neutral_unit(id: &str) -> bool {
    NEUTRAL_UNITS.iter().any(|u| u.id == id)
}

/// Point types (`<ObjectPoint Type="...">`). Start locations are points, not units.
pub const POINT_NORMAL: &str = "Normal";
pub const POINT_START_LOCATION: &str = "StartLoc";
pub const POINT_TYPES: &[Known] = &[k(POINT_NORMAL, "Point"), k(POINT_START_LOCATION, "Start location")];

/// Doodad types seen in shipped and community maps.
pub const DOODADS: &[Known] = &[
    k("Rock", "Rock"),
    k("RockLarge", "Rock (large)"),
    k("RockGiant", "Rock (giant)"),
    k("RockArch", "Rock arch"),
    k("RockWaterfall", "Rock waterfall"),
    k("CliffSpires", "Cliff spires"),
    k("CrystalLarge", "Crystal (large)"),
    k("CrystalHuge", "Crystal (huge)"),
    k("Aiur_Tree_Large", "Aiur tree (large)"),
    k("Aiur_Trees_Giant", "Aiur trees (giant)"),
    k("Aiur_Trees_Log", "Aiur log"),
    k("Aiur_Trees_Log_Small", "Aiur log (small)"),
    k("AgriaDebris", "Agria debris"),
    k("CharCrater", "Char crater"),
    k("MarSaraCrack", "Mar Sara crack"),
    k("Waterfall", "Waterfall"),
    k("TarBubble", "Tar bubble"),
    k("TarSludge", "Tar sludge"),
    k("CaveEntrance", "Cave entrance"),
    k("PipesLong", "Pipes (long)"),
    k("AlarmLight", "Alarm light"),
    k("CableLight", "Cable light"),
    k("XelNaga_EnergyStreaks", "Xel'Naga energy streaks"),
    k("XelNaga_Shrine_Long_Cliff", "Xel'Naga shrine (cliff)"),
    k("Void_Mist", "Void mist"),
];

pub fn doodad_name(id: &str) -> Option<&'static str> {
    DOODADS.iter().find(|d| d.id == id).map(|d| d.name)
}

/// Terrain textures grouped by tileset family (the `tileSet` / texture set).
pub struct TextureFamily {
    pub tileset: &'static str,
    pub textures: &'static [Known],
}

pub const TEXTURE_FAMILIES: &[TextureFamily] = &[
    TextureFamily {
        tileset: "Agria",
        textures: &[
            k("AgriaDirt", "Dirt"),
            k("AgriaDirtRough", "Rough dirt"),
            k("AgriaGrassGreen", "Green grass"),
            k("AgriaGrassYellow", "Yellow grass"),
            k("AgriaFoliage", "Foliage"),
            k("AgriaCrops", "Crops"),
            k("AgriaRock", "Rock"),
            k("AgriaConcrete", "Concrete"),
        ],
    },
    TextureFamily { tileset: "Char", textures: &[k("CharDirt", "Dirt")] },
    TextureFamily { tileset: "Shakuras", textures: &[k("ShakurasBricks", "Bricks")] },
];

/// Textures for a tileset (case-insensitive), or every known texture if the
/// tileset has no curated list.
pub fn textures_for(tileset: &str) -> Vec<&'static Known> {
    let fam: Vec<&'static Known> = TEXTURE_FAMILIES
        .iter()
        .filter(|f| f.tileset.eq_ignore_ascii_case(tileset))
        .flat_map(|f| f.textures.iter())
        .collect();
    if fam.is_empty() {
        TEXTURE_FAMILIES.iter().flat_map(|f| f.textures.iter()).collect()
    } else {
        fam
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_non_empty() {
        let mut seen = HashSet::new();
        for (_, u) in all_units() {
            assert!(!u.id.is_empty() && !u.name.is_empty());
            assert!(!u.id.contains(' '), "{} has a space", u.id);
            assert!(seen.insert(u.id), "duplicate unit id {}", u.id);
        }
        let mut seen = HashSet::new();
        for d in DOODADS {
            assert!(seen.insert(d.id), "duplicate doodad id {}", d.id);
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(unit_name("HellionTank"), Some("Hellbat"));
        assert!(is_neutral_unit("MineralField750"));
        assert!(!is_neutral_unit("Marine"));
        assert!(textures_for("agria").iter().any(|t| t.id == "AgriaGrassGreen"));
        assert!(!textures_for("Unknown").is_empty());
    }
}
