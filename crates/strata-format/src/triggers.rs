//! Triggers and `MapScript.galaxy`.
//!
//! Strata keeps its own trigger model ([`TriggerLibrary`]) as pretty JSON in
//! `Strata/Triggers.json` inside the map, and compiles it to
//! `MapScript.galaxy` with [`strata_core::galaxy::generate`] (CRLF line
//! endings, like the editor).
//!
//! Behaviour by case:
//!
//! - **Strata triggers present** (`Strata/Triggers.json` parses): loaded into
//!   the document. When triggers are dirty, both the JSON and
//!   `MapScript.galaxy` are rewritten; otherwise both pass through.
//! - **SC2-editor triggers** (a `Triggers` file, or a `MapScript.galaxy`
//!   without Strata JSON): the library is loaded *empty* except for a note in
//!   `custom_script` listing the editor triggers by name (from
//!   `TriggerStrings.txt`). `Triggers`, `MapScript.galaxy` and the trigger
//!   strings are always written back byte-identical while the library is
//!   still just that note — even if the trigger flag is dirty. Once the user
//!   adds Strata triggers/variables/script, Strata takes over
//!   `MapScript.galaxy`; the editor's `Triggers` file is still kept verbatim
//!   (the SC2 editor would regenerate the script from it on its next save).
//! - **No triggers at all** (new map): JSON and script are generated.

use strata_core::triggers::TriggerLibrary;
use strata_core::MapDocument;

use crate::{lookup, put, xml, FileMap};

pub const STRATA_JSON: &str = "Strata/Triggers.json";
pub const EDITOR_TRIGGERS: &str = "Triggers";
pub const SCRIPT: &str = "MapScript.galaxy";
const TRIGGER_STRINGS: &str = "enUS.SC2Data/LocalizedData/TriggerStrings.txt";

fn strata_lib(files: &FileMap) -> Option<TriggerLibrary> {
    serde_json::from_slice(lookup(files, STRATA_JSON)?).ok()
}

fn has_editor_script(files: &FileMap) -> bool {
    strata_lib(files).is_none() && (lookup(files, EDITOR_TRIGGERS).is_some() || lookup(files, SCRIPT).is_some())
}

/// Names of SC2-editor triggers, in file order (best effort).
pub fn editor_trigger_names(files: &FileMap) -> Vec<String> {
    let Some(doc) = lookup(files, EDITOR_TRIGGERS).and_then(|b| xml::parse_bytes(b).ok()) else {
        return Vec::new();
    };
    let names = lookup(files, TRIGGER_STRINGS).map(crate::strings::parse).unwrap_or_default();
    doc.root
        .children_named("Element")
        .filter(|e| e.attr("Type") == Some("Trigger"))
        .map(|e| {
            let id = e.attr("Id").unwrap_or("");
            names.get(&format!("Trigger/Name/{id}")).cloned().unwrap_or_else(|| id.to_owned())
        })
        .collect()
}

/// The placeholder `custom_script` for maps whose triggers come from the SC2 editor.
pub fn editor_note(files: &FileMap) -> String {
    let names = editor_trigger_names(files);
    let mut s = String::from("// Strata: this map's triggers were made in the StarCraft II editor and are\n");
    s.push_str("// preserved unchanged (Triggers, MapScript.galaxy). Adding Strata triggers,\n");
    s.push_str("// variables or script here makes Strata generate MapScript.galaxy instead.\n");
    if !names.is_empty() {
        s.push_str(&format!("// Editor triggers ({}):\n", names.len()));
        for n in names.iter().take(50) {
            s.push_str(&format!("//   {n}\n"));
        }
        if names.len() > 50 {
            s.push_str(&format!("//   ... and {} more\n", names.len() - 50));
        }
    }
    s
}

pub fn load(files: &FileMap) -> TriggerLibrary {
    if let Some(lib) = strata_lib(files) {
        return lib;
    }
    if has_editor_script(files) {
        return TriggerLibrary { custom_script: editor_note(files), ..Default::default() };
    }
    TriggerLibrary::default()
}

fn is_placeholder(lib: &TriggerLibrary, files: &FileMap) -> bool {
    lib.items.is_empty() && lib.variables.is_empty() && lib.custom_script == editor_note(files)
}

fn crlf(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + s.len() / 20);
    let mut prev = '\0';
    for c in s.chars() {
        if c == '\n' && prev != '\r' {
            o.push('\r');
        }
        o.push(c);
        prev = c;
    }
    o
}

pub fn save(doc: &MapDocument, orig: &FileMap, out: &mut FileMap) -> Result<(), crate::FormatError> {
    if has_editor_script(orig) && is_placeholder(&doc.triggers, orig) {
        return Ok(());
    }
    let json = serde_json::to_vec_pretty(&doc.triggers)
        .map_err(|e| crate::FormatError::Malformed { file: STRATA_JSON.into(), reason: e.to_string() })?;
    put(out, STRATA_JSON, json);
    put(out, SCRIPT, crlf(&strata_core::galaxy::generate(doc)).into_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRIG: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<TriggerData>\r\n    <Root>\r\n        <Item Type=\"Trigger\" Id=\"1AEBEBB1\"/>\r\n    </Root>\r\n    <Element Type=\"Trigger\" Id=\"1AEBEBB1\">\r\n        <Event Type=\"FunctionCall\" Id=\"A0C4D350\"/>\r\n    </Element>\r\n</TriggerData>";

    #[test]
    fn editor_triggers_are_preserved() {
        let mut files = FileMap::new();
        files.insert("Triggers".into(), TRIG.as_bytes().to_vec());
        files.insert(SCRIPT.into(), b"void InitMap () {}\r\n".to_vec());
        files.insert(TRIGGER_STRINGS.into(), "\u{feff}Trigger/Name/1AEBEBB1=Melee Initialization\r\n".into());
        let lib = load(&files);
        assert!(lib.items.is_empty());
        assert!(lib.custom_script.contains("Melee Initialization"));
        let mut doc = MapDocument::new("T", 32, 32);
        doc.triggers = lib;
        doc.dirty.triggers = true;
        let mut out = FileMap::new();
        save(&doc, &files, &mut out).unwrap();
        assert!(out.is_empty());
        // Taking over: a real trigger makes Strata write the script.
        doc.triggers = TriggerLibrary::with_melee_init();
        save(&doc, &files, &mut out).unwrap();
        assert!(String::from_utf8_lossy(&out[SCRIPT]).contains("gt_MeleeInitialization_Func"));
        assert!(out[SCRIPT].windows(2).any(|w| w == b"\r\n"));
        let mut merged = files.clone();
        merged.extend(out);
        assert_eq!(load(&merged), doc.triggers);
    }
}
