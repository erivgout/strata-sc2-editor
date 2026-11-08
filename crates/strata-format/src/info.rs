//! `MapInfo` and `DocumentHeader` (binary).
//!
//! ## MapInfo (`IpaM`, i.e. "MapI" byte-reversed)
//!
//! Every version: map size, the tileset and the camera (playable) bounds are
//! read with the long-standing community layout (sc2reader). Version 39
//! (0x27, written by current 5.0.x editors) is decoded completely, including
//! the player list; the parse is only trusted when it consumes the file
//! exactly. Observed on real maps:
//!
//! ```text
//! "IpaM" u32 version  u32 u32(unknown)  u32 width  u32 height
//! u32 smallPreviewType [cstr if 2]  u32 largePreviewType [cstr if 2]
//! cstr cstr (v>=0x1F, v>=0x26)  u32 u32  cstr fog ("Dark")  cstr tileset
//! u32 left bottom right top (camera bounds)  u32 baseHeight (x4096)
//! -- v39 --
//! u32 loadScreenType  cstr loadScreenImage  lpstr  24 bytes
//! cstr layout  lpstr lpstr  24 bytes flags  u8  u32 authorCount {cstr, 12 bytes}
//! lpstr loadingMusic
//! u32 playerCount { u8 id, u32 control, i32 color(-1 = default), cstr race,
//!                   u32 ?, u32 startPointObjectId, u32 aiFourCC, cstr decal }
//! u32 n {u32}  u32 u32 n {bits}  u32 n {u32}  u32 n {u32}  u32 u32 n {bits}
//! u32 lighting [+8 if 1|3, +4 if 2]  20 bytes
//! ```
//!
//! Player names live in GameStrings as `MapInfo/PlayerNN/Name`, NN being the
//! index in the player list (not the player id). Control values: 1 user,
//! 2 computer, 3 neutral, 4 hostile (0 is treated as open; inferred).
//!
//! ## DocumentHeader (`H2CS`, version 8)
//!
//! 44-byte prefix (build ids, unknown words), `u32` dependency count and
//! NUL-terminated dependency strings, `u32` entry count and entries of
//! `u16 len, key, 4-byte reversed locale ("SUne" = enUS), u16 len, value`,
//! then any tail bytes (kept verbatim). Other versions are passed through.

use std::ops::Range;

use strata_core::{Player, PlayerControl, Race};

use crate::bin::{Put, Reader};
use crate::FormatError;

pub const MAPINFO_V39: u32 = 39;

/// SC2's player colour table (index -> sRGB), as used by MapInfo colour indices.
pub const PLAYER_COLORS: [[u8; 3]; 16] = [
    [255, 255, 255],
    [180, 20, 30],
    [0, 66, 255],
    [28, 167, 234],
    [84, 0, 129],
    [235, 225, 41],
    [254, 138, 14],
    [22, 128, 0],
    [204, 166, 252],
    [31, 1, 201],
    [82, 84, 148],
    [16, 98, 70],
    [78, 42, 4],
    [150, 255, 145],
    [35, 35, 35],
    [229, 91, 176],
];

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRec {
    pub id: u8,
    pub control: u32,
    pub color: i32,
    pub race: String,
    pub unknown: u32,
    pub start_point: u32,
    pub ai: u32,
    pub decal: String,
}

impl PlayerRec {
    fn read(r: &mut Reader) -> Result<Self, FormatError> {
        Ok(Self {
            id: r.u8()?,
            control: r.u32()?,
            color: r.i32()?,
            race: r.cstr()?,
            unknown: r.u32()?,
            start_point: r.u32()?,
            ai: r.u32()?,
            decal: r.cstr()?,
        })
    }

    fn write(&self, o: &mut Vec<u8>) {
        o.push(self.id);
        o.put_u32(self.control);
        o.put_i32(self.color);
        o.put_cstr(&self.race);
        o.put_u32(self.unknown);
        o.put_u32(self.start_point);
        o.put_u32(self.ai);
        o.put_cstr(&self.decal);
    }
}

#[derive(Debug, Clone)]
pub struct MapInfoFile {
    pub version: u32,
    pub width: u32,
    pub height: u32,
    pub size_off: usize,
    pub tileset: String,
    pub tileset_range: Range<usize>,
    pub bounds: [u32; 4],
    pub bounds_off: usize,
    /// Present only when the whole file decoded (version 39).
    pub players: Option<Vec<PlayerRec>>,
    pub players_range: Range<usize>,
}

pub fn parse_mapinfo(d: &[u8]) -> Result<MapInfoFile, FormatError> {
    let mut r = Reader::new(d, "MapInfo");
    if r.bytes(4)? != b"IpaM" {
        return Err(r.err("bad magic"));
    }
    let version = r.u32()?;
    if version >= 0x18 {
        r.skip(8)?;
    }
    let size_off = r.pos;
    let width = r.u32()?;
    let height = r.u32()?;
    if r.u32()? == 2 {
        r.cstr()?;
    }
    if r.u32()? == 2 {
        r.cstr()?;
    }
    if version >= 0x1F {
        r.cstr()?;
    }
    if version >= 0x26 {
        r.cstr()?;
    }
    if version >= 0x1F {
        r.u32()?;
    }
    r.u32()?;
    r.cstr()?; // fog
    let ts = r.pos;
    let tileset = r.cstr()?;
    let tileset_range = ts..r.pos;
    let bounds_off = r.pos;
    let bounds = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
    let mut info = MapInfoFile {
        version,
        width,
        height,
        size_off,
        tileset,
        tileset_range,
        bounds,
        bounds_off,
        players: None,
        players_range: 0..0,
    };
    if version == MAPINFO_V39 {
        r.u32()?; // base height
        if let Ok((players, range)) = parse_v39_tail(&mut r) {
            info.players = Some(players);
            info.players_range = range;
        }
    }
    Ok(info)
}

fn parse_v39_tail(r: &mut Reader) -> Result<(Vec<PlayerRec>, Range<usize>), FormatError> {
    r.u32()?;
    r.cstr()?;
    r.lpstr()?;
    r.skip(24)?;
    r.cstr()?;
    r.lpstr()?;
    r.lpstr()?;
    r.skip(24)?;
    r.skip(1)?;
    let authors = r.u32()?;
    if authors > 999 {
        return Err(r.err("implausible author count"));
    }
    for _ in 0..authors {
        r.cstr()?;
        r.skip(12)?;
    }
    r.lpstr()?;
    let start = r.pos;
    let count = r.u32()?;
    if count > 16 {
        return Err(r.err("implausible player count"));
    }
    let mut players = Vec::new();
    for _ in 0..count {
        let p = PlayerRec::read(r)?;
        if p.control > 5 || p.id > 15 {
            return Err(r.err("implausible player record"));
        }
        players.push(p);
    }
    let end = r.pos;
    // Validate the remainder so we only trust a layout that fits exactly.
    let arr = |r: &mut Reader| -> Result<(), FormatError> {
        let n = r.u32()? as usize;
        if n > 256 {
            return Err(r.err("bad array"));
        }
        r.skip(n * 4)
    };
    let bits = |r: &mut Reader| -> Result<(), FormatError> {
        r.u32()?;
        let n = r.u32()? as usize;
        if n > 256 {
            return Err(r.err("bad bitset"));
        }
        r.skip(n.div_ceil(8))
    };
    arr(r)?;
    bits(r)?;
    arr(r)?;
    arr(r)?;
    bits(r)?;
    match r.u32()? {
        1 | 3 => r.skip(8)?,
        2 => r.skip(4)?,
        _ => {}
    }
    r.skip(20)?;
    if r.pos != r.data.len() {
        return Err(r.err("trailing bytes"));
    }
    Ok((players, start..end))
}

fn write_players(o: &mut Vec<u8>, recs: &[PlayerRec]) {
    o.put_u32(recs.len() as u32);
    for p in recs {
        p.write(o);
    }
}

/// Rewrite an existing MapInfo with new size, tileset, bounds and (v39) players.
pub fn patch_mapinfo(orig: &[u8], f: &MapInfoFile, info: &strata_core::MapInfo, recs: Option<&[PlayerRec]>) -> Vec<u8> {
    let mut o = Vec::with_capacity(orig.len() + 64);
    o.extend_from_slice(&orig[..f.size_off]);
    o.put_u32(info.width);
    o.put_u32(info.height);
    o.extend_from_slice(&orig[f.size_off + 8..f.tileset_range.start]);
    o.put_cstr(&info.tileset);
    o.extend_from_slice(&orig[f.tileset_range.end..f.bounds_off]);
    for b in info.playable {
        o.put_u32(b);
    }
    match (recs, f.players.is_some()) {
        (Some(recs), true) => {
            o.extend_from_slice(&orig[f.bounds_off + 16..f.players_range.start]);
            write_players(&mut o, recs);
            o.extend_from_slice(&orig[f.players_range.end..]);
        }
        _ => o.extend_from_slice(&orig[f.bounds_off + 16..]),
    }
    o
}

/// A fresh version-39 MapInfo laid out like a new map saved by the 5.0.x editor.
pub fn new_mapinfo(info: &strata_core::MapInfo, recs: &[PlayerRec]) -> Vec<u8> {
    let mut o = Vec::with_capacity(512);
    o.extend_from_slice(b"IpaM");
    o.put_u32(MAPINFO_V39);
    o.put_u32(0x0003_533B); // observed in editor-created maps; meaning unknown
    o.put_u32(0);
    o.put_u32(info.width);
    o.put_u32(info.height);
    o.put_u32(1);
    o.put_u32(1);
    o.put_cstr("");
    o.put_cstr("");
    o.put_u32(1);
    o.put_u32(0);
    o.put_cstr("Dark");
    o.put_cstr(&info.tileset);
    for b in info.playable {
        o.put_u32(b);
    }
    o.put_u32(0x7800); // base height 7.5 * 4096 (editor default)
    o.put_u32(0); // load screen type
    o.put_cstr("");
    o.put_lpstr("");
    for v in [0u32, 0xFFFF_FFFF, 0, 0, 800, 600] {
        o.put_u32(v);
    }
    o.put_cstr("");
    o.put_lpstr("");
    o.put_lpstr("");
    for v in [0x4000u32, 1, 2, 0x2000, 0, 0] {
        o.put_u32(v);
    }
    o.push(0x10);
    o.put_u32(0); // authors
    o.put_lpstr(""); // loading music
    write_players(&mut o, recs);
    for _ in 0..7 {
        o.put_u32(0); // start locs, alliance bitset (2), team slots, teams, enemy bitset (2)
    }
    o.put_u32(0); // lighting
    o.extend_from_slice(&[0u8; 20]);
    o
}

// ---- model mapping -------------------------------------------------------

pub fn control_from_sc2(c: u32) -> PlayerControl {
    match c {
        1 => PlayerControl::User,
        2 => PlayerControl::Computer,
        3 => PlayerControl::Neutral,
        4 => PlayerControl::Hostile,
        _ => PlayerControl::Open,
    }
}

pub fn control_to_sc2(c: PlayerControl) -> u32 {
    match c {
        PlayerControl::User => 1,
        PlayerControl::Computer => 2,
        PlayerControl::Neutral => 3,
        PlayerControl::Hostile => 4,
        PlayerControl::Open => 0,
    }
}

fn default_color(id: u8, control: PlayerControl) -> [u8; 3] {
    match control {
        PlayerControl::Neutral | PlayerControl::Hostile => PLAYER_COLORS[0],
        _ => PLAYER_COLORS[id as usize % 16],
    }
}

pub fn player_from_rec(rec: &PlayerRec, name: String, start_location: Option<strata_core::Vec2>) -> Player {
    let control = control_from_sc2(rec.control);
    let race = match Race::from_sc2_id(&rec.race) {
        Some(r) => r,
        None if matches!(control, PlayerControl::Neutral | PlayerControl::Hostile) => Race::Neutral,
        None => Race::Random,
    };
    let color = if rec.color >= 0 && (rec.color as usize) < 16 {
        PLAYER_COLORS[rec.color as usize]
    } else {
        default_color(rec.id, control)
    };
    Player { id: rec.id, name, race, control, color, team: rec.id, start_location }
}

fn nearest_color(c: [u8; 3]) -> i32 {
    let d = |p: &[u8; 3]| (0..3).map(|i| (p[i] as i32 - c[i] as i32).pow(2)).sum::<i32>();
    PLAYER_COLORS.iter().enumerate().min_by_key(|(_, p)| d(p)).map(|(i, _)| i as i32).unwrap_or(0)
}

/// Build MapInfo player records for the model's players, reusing the original
/// record (unknown fields included) for every player id that already existed.
pub fn recs_from_players(players: &[Player], orig: &[PlayerRec]) -> Vec<PlayerRec> {
    let mut out = Vec::new();
    for p in players {
        let existing = orig.iter().find(|r| r.id == p.id).cloned();
        let is_new = existing.is_none();
        let mut rec = existing.unwrap_or(PlayerRec {
            id: p.id,
            control: control_to_sc2(p.control),
            color: -1,
            race: String::new(),
            unknown: 0,
            start_point: 0,
            ai: 0,
            decal: String::new(),
        });
        let prev = player_from_rec(&rec, p.name.clone(), p.start_location);
        if prev.control != p.control {
            rec.control = control_to_sc2(p.control);
        }
        if prev.race != p.race {
            rec.race = match p.race {
                Race::Random | Race::Neutral => String::new(),
                r => r.sc2_id().to_owned(),
            };
        }
        if is_new {
            let neutralish = matches!(p.control, PlayerControl::Neutral | PlayerControl::Hostile);
            rec.color = match (p.color == default_color(p.id, p.control), neutralish) {
                (true, true) => 0,
                (true, false) => -1,
                _ => nearest_color(p.color),
            };
        } else if prev.color != p.color {
            rec.color = nearest_color(p.color);
        }
        out.push(rec);
    }
    out
}

/// Records for a brand-new map: neutral (0), the model's players, hostile (15),
/// mirroring what the editor creates.
pub fn new_recs(players: &[Player]) -> Vec<Player> {
    let mut all: Vec<Player> = Vec::new();
    if !players.iter().any(|p| p.id == 0) {
        all.push(Player {
            id: 0,
            name: "Neutral".into(),
            race: Race::Neutral,
            control: PlayerControl::Neutral,
            color: PLAYER_COLORS[0],
            team: 0,
            start_location: None,
        });
    }
    all.extend(players.iter().cloned());
    if !players.iter().any(|p| p.id == 15) {
        all.push(Player {
            id: 15,
            name: "Hostile".into(),
            race: Race::Neutral,
            control: PlayerControl::Hostile,
            color: PLAYER_COLORS[0],
            team: 15,
            start_location: None,
        });
    }
    all.sort_by_key(|p| p.id);
    all
}

pub fn player_name_key(index: usize) -> String {
    format!("MapInfo/Player{index:02}/Name")
}

// ---- DocumentHeader --------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    pub prefix: Vec<u8>,
    pub deps: Vec<String>,
    /// (key, locale e.g. "enUS", value)
    pub entries: Vec<(String, String, String)>,
    pub tail: Vec<u8>,
}

fn locale_from_4cc(b: &[u8]) -> String {
    b.iter().rev().map(|&c| c as char).collect()
}

fn locale_to_4cc(l: &str) -> [u8; 4] {
    let mut b = [0u8; 4];
    for (i, c) in l.bytes().take(4).enumerate() {
        b[3 - i] = c;
    }
    b
}

pub fn parse_header(d: &[u8]) -> Result<Header, FormatError> {
    let mut r = Reader::new(d, "DocumentHeader");
    if r.bytes(4)? != b"H2CS" {
        return Err(r.err("bad magic"));
    }
    let version = r.u32()?;
    if version != 8 {
        return Err(FormatError::Unsupported(format!("DocumentHeader version {version}")));
    }
    r.pos = 0x2C;
    if d.len() < 0x30 {
        return Err(r.err("too short"));
    }
    let prefix = d[..0x2C].to_vec();
    let n = r.u32()?;
    if n > 1024 {
        return Err(r.err("implausible dependency count"));
    }
    let mut deps = Vec::new();
    for _ in 0..n {
        deps.push(r.cstr()?);
    }
    let n = r.u32()?;
    let mut entries = Vec::new();
    for _ in 0..n {
        let kl = r.u16()? as usize;
        let key = String::from_utf8_lossy(r.bytes(kl)?).into_owned();
        let loc = locale_from_4cc(r.bytes(4)?);
        let vl = r.u16()? as usize;
        let value = String::from_utf8_lossy(r.bytes(vl)?).into_owned();
        entries.push((key, loc, value));
    }
    let tail = d[r.pos..].to_vec();
    Ok(Header { prefix, deps, entries, tail })
}

pub fn write_header(h: &Header) -> Vec<u8> {
    let mut o = h.prefix.clone();
    o.put_u32(h.deps.len() as u32);
    for d in &h.deps {
        o.put_cstr(d);
    }
    o.put_u32(h.entries.len() as u32);
    for (k, l, v) in &h.entries {
        o.put_lpstr(k);
        o.extend_from_slice(&locale_to_4cc(l));
        o.put_lpstr(v);
    }
    o.extend_from_slice(&h.tail);
    o
}

/// Prefix of a header written by editor 5.0.14 (build 93333) for a new map.
pub fn new_header_prefix() -> Vec<u8> {
    let mut o = Vec::with_capacity(0x2C);
    o.extend_from_slice(b"H2CS");
    o.put_u32(8);
    o.extend_from_slice(b"2S\0\0");
    o.extend_from_slice(&[1, 5, 0, 14]);
    o.put_u32(93333);
    o.put_u32(93333);
    o.put_u32(0);
    o.put_u32(0);
    o.put_u32(0);
    o.put_u32(1);
    o.put_u32(0);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_mapinfo_parses_back() {
        let mut info = strata_core::MapInfo::new("T", 128, 96);
        info.tileset = "Agria".into();
        let players = new_recs(&info.players);
        let recs = recs_from_players(&players, &[]);
        let bytes = new_mapinfo(&info, &recs);
        let f = parse_mapinfo(&bytes).unwrap();
        assert_eq!((f.width, f.height), (128, 96));
        assert_eq!(f.tileset, "Agria");
        assert_eq!(f.bounds, info.playable);
        let back = f.players.unwrap();
        assert_eq!(back.iter().map(|p| p.id).collect::<Vec<_>>(), vec![0, 1, 2, 15]);
        assert_eq!(back[0].control, 3);
        assert_eq!(back[3].control, 4);
        assert_eq!(back[1].color, -1);
    }

    #[test]
    fn new_mapinfo_matches_editor_size() {
        // A new 256x256 editor map with players 0, 1 and 15 is 273 bytes.
        let mut info = strata_core::MapInfo::new("T", 256, 256);
        info.tileset = "BlackopsLab_A".into();
        info.players.truncate(1);
        let recs = recs_from_players(&new_recs(&info.players), &[]);
        assert_eq!(new_mapinfo(&info, &recs).len(), 273);
    }

    #[test]
    fn patch_keeps_unknown_bytes() {
        let mut info = strata_core::MapInfo::new("T", 64, 64);
        let recs = recs_from_players(&new_recs(&info.players), &[]);
        let orig = new_mapinfo(&info, &recs);
        let f = parse_mapinfo(&orig).unwrap();
        info.tileset = "Bel'ShirJungle".into();
        info.playable = [1, 2, 60, 61];
        info.players[0].race = Race::Zerg;
        let all = new_recs(&info.players);
        let recs2 = recs_from_players(&all, f.players.as_ref().unwrap());
        let out = patch_mapinfo(&orig, &f, &info, Some(&recs2));
        let g = parse_mapinfo(&out).unwrap();
        assert_eq!(g.tileset, "Bel'ShirJungle");
        assert_eq!(g.bounds, [1, 2, 60, 61]);
        assert_eq!(g.players.unwrap()[1].race, "Zerg");
        // Unchanged everything -> identical bytes.
        let info0 = strata_core::MapInfo::new("T", 64, 64);
        let same = patch_mapinfo(&orig, &f, &info0, Some(f.players.as_ref().unwrap()));
        assert_eq!(same, orig);
    }

    #[test]
    fn header_roundtrip() {
        let h = Header {
            prefix: new_header_prefix(),
            deps: vec!["bnet:Void (Campaign)/0.0/999,file:Campaigns/Void.SC2Campaign".into()],
            entries: vec![("DocInfo/Name".into(), "enUS".into(), "Just Another Map".into())],
            tail: vec![],
        };
        let b = write_header(&h);
        // Same layout as an editor header: dependency at 0x30, entry count right after.
        assert_eq!(&b[0x30..0x35], b"bnet:");
        assert_eq!(b[0x6D], 1);
        assert!(b.windows(4).any(|w| w == b"SUne"));
        assert_eq!(parse_header(&b).unwrap(), h);
    }
}
