//! Strata core: the in-memory map document, undoable edits, and tooling that
//! does not depend on any UI or file format.
//!
//! Module map:
//! - [`document`]: [`MapDocument`] and the map-level metadata.
//! - [`terrain`]: heightfield, cliff levels, texture weights, pathing.
//! - [`objects`]: placed units, doodads and points.
//! - [`regions`]: regions and cameras.
//! - [`triggers`]: trigger library model; [`galaxy`] turns it into MapScript.galaxy
//!   and lints Galaxy; [`trigger_lib`] describes the natives the editor offers.
//! - [`catalog`]: generic GameData catalog entries (units, abilities, weapons...).
//! - [`base_data`]: the base game's catalogs, read from the user's install.
//! - [`catalog_schema`]: curated field types and descriptions for common classes.
//! - [`strings`]: localized text keyed by locale.
//! - [`brush`]: terrain brush strokes shared by viewport tools and MCP.
//! - [`ops`]: serializable document operations (the MCP / command API).
//! - [`history`]: undo/redo built on reversible [`history::Edit`]s.
//! - [`known`]: curated SC2 unit, doodad and texture ids for pickers.
//! - [`models`]: which `.m3` model draws a placed unit or doodad.
//! - [`validate`]: problems shown in the Problems panel.

pub mod base_data;
pub mod brush;
pub mod catalog;
pub mod catalog_schema;
pub mod document;
pub mod galaxy;
pub mod history;
pub mod known;
pub mod models;
pub mod objects;
pub mod ops;
pub mod regions;
pub mod strings;
pub mod terrain;
pub mod trigger_lib;
pub mod triggers;
pub mod validate;

pub use document::{MapDocument, MapInfo, Player, PlayerControl, Race};
pub use glam::{Vec2, Vec3};
