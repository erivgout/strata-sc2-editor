//! Strata core: the in-memory map document, undoable edits, and tooling that
//! does not depend on any UI or file format.
//!
//! Module map:
//! - [`document`]: [`MapDocument`] and the map-level metadata.
//! - [`terrain`]: heightfield, cliff levels, texture weights, pathing.
//! - [`objects`]: placed units, doodads and points.
//! - [`regions`]: regions and cameras.
//! - [`triggers`]: trigger library model; [`galaxy`] turns it into MapScript.galaxy.
//! - [`catalog`]: generic GameData catalog entries (units, abilities, weapons...).
//! - [`catalog_schema`]: curated field types and descriptions for common classes.
//! - [`strings`]: localized text keyed by locale.
//! - [`brush`]: terrain brush strokes shared by viewport tools and MCP.
//! - [`ops`]: serializable document operations (the MCP / command API).
//! - [`history`]: undo/redo built on reversible [`history::Edit`]s.
//! - [`known`]: curated SC2 unit, doodad and texture ids for pickers.
//! - [`validate`]: problems shown in the Problems panel.

pub mod brush;
pub mod catalog;
pub mod catalog_schema;
pub mod document;
pub mod galaxy;
pub mod history;
pub mod known;
pub mod objects;
pub mod ops;
pub mod regions;
pub mod strings;
pub mod terrain;
pub mod triggers;
pub mod validate;

pub use document::{MapDocument, MapInfo, Player, PlayerControl, Race};
pub use glam::{Vec2, Vec3};
