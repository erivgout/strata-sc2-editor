//! `Objects` (XML `<PlacedObjects Version="27">`): units, doodads, points and cameras.
//!
//! Observed element shapes:
//!
//! ```xml
//! <ObjectUnit Id="1915310103" Position="105.5515,102.5751,0" Rotation="1.5283" Scale="1,1,1" UnitType="Marine" Player="1"/>
//! <ObjectDoodad Id="1756549599" Variation="1" Position="12.5,72.7,7.99" Rotation="5.37" Scale="1,1,1" Type="Korhal_Antenna">
//!     <Flag Index="HeightAbsolute" Value="1"/>
//! </ObjectDoodad>
//! <ObjectPoint Id="958681981" Position="23.5,22.5,0" Scale="1,1,1" Type="StartLoc" Name="Start Location 001" Color="0,0,0,0"/>
//! <ObjectCamera Id="923" Position="127,211.5,0" Rotation="3.1315" Scale="1,1,1" Name="Cam" Color="0,0,0,0" CameraTarget="127,211.5">
//!     <CameraValue Index="Distance" Value="63.042"/> ... Pitch, Yaw (degrees), FieldOfView ...
//! </ObjectCamera>
//! ```
//!
//! Attributes Strata does not model go to [`PlacedObject::extra`] in source
//! order. Child elements (flags) are not modeled; they are kept from the
//! original element when an object is rewritten. For points, `type_id`
//! holds the point type ("Normal", "StartLoc"). Unchanged elements are
//! copied verbatim; unknown element kinds are always kept.

use std::collections::HashMap;

use strata_core::catalog::XmlNode;
use strata_core::objects::{ObjectId, ObjectKind, PlacedObject};
use strata_core::regions::Camera;
use strata_core::{Vec2, Vec3};

use crate::xml::{self, fmt_list, fmt_num, parse_list, Doc, Elem, Keep};

fn kind_of(name: &str) -> Option<ObjectKind> {
    match name {
        "ObjectUnit" => Some(ObjectKind::Unit),
        "ObjectDoodad" => Some(ObjectKind::Doodad),
        "ObjectPoint" => Some(ObjectKind::Point),
        _ => None,
    }
}

fn elem_name(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Unit => "ObjectUnit",
        ObjectKind::Doodad => "ObjectDoodad",
        ObjectKind::Point => "ObjectPoint",
    }
}

fn type_attr(k: ObjectKind) -> &'static str {
    if k == ObjectKind::Unit { "UnitType" } else { "Type" }
}

fn modeled(k: ObjectKind, key: &str) -> bool {
    matches!(key, "Id" | "Position" | "Rotation" | "Scale" | "Name")
        || key == type_attr(k)
        || (k == ObjectKind::Unit && key == "Player")
}

fn vec3(s: Option<&str>, default: Vec3) -> Vec3 {
    match s {
        Some(s) => {
            let v = parse_list(s);
            Vec3::new(
                v.first().copied().unwrap_or(default.x),
                v.get(1).copied().unwrap_or(default.y),
                v.get(2).copied().unwrap_or(default.z),
            )
        }
        None => default,
    }
}

fn vec2(s: &str) -> Vec2 {
    let v = parse_list(s);
    Vec2::new(v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0))
}

pub fn object_from(el: &Elem) -> Option<PlacedObject> {
    let kind = kind_of(&el.name)?;
    let id: u32 = el.attr("Id")?.trim().parse().ok()?;
    let mut o = PlacedObject::new(ObjectId(id), kind, el.attr(type_attr(kind)).unwrap_or(""), Vec3::ZERO);
    o.position = vec3(el.attr("Position"), Vec3::ZERO);
    o.rotation = el.attr("Rotation").and_then(|r| r.trim().parse().ok()).unwrap_or(0.0);
    o.scale = vec3(el.attr("Scale"), Vec3::ONE);
    o.name = el.attr("Name").unwrap_or("").to_owned();
    o.player = if kind == ObjectKind::Unit { el.attr("Player").and_then(|p| p.trim().parse().ok()).unwrap_or(0) } else { 0 };
    o.extra = el.attrs.iter().filter(|(k, _)| !modeled(kind, k)).cloned().collect();
    Some(o)
}

fn camera_value(el: &Elem, index: &str) -> Option<f32> {
    el.children_named("CameraValue").find(|c| c.attr("Index") == Some(index))?.attr("Value")?.trim().parse().ok()
}

pub fn camera_from(el: &Elem) -> Option<Camera> {
    if el.name != "ObjectCamera" {
        return None;
    }
    let id: u32 = el.attr("Id")?.trim().parse().ok()?;
    let target = match el.attr("CameraTarget") {
        Some(t) => vec2(t),
        None => vec3(el.attr("Position"), Vec3::ZERO).truncate(),
    };
    let mut c = Camera::new(ObjectId(id), el.attr("Name").unwrap_or(""), target);
    if let Some(v) = camera_value(el, "Distance") {
        c.distance = v;
    }
    if let Some(v) = camera_value(el, "Pitch") {
        c.pitch = v;
    }
    if let Some(v) = camera_value(el, "Yaw") {
        c.yaw = v;
    }
    if let Some(v) = camera_value(el, "FieldOfView") {
        c.fov = v;
    }
    Some(c)
}

pub fn parse(doc: &Doc) -> (Vec<PlacedObject>, Vec<Camera>) {
    let mut objects = Vec::new();
    let mut cameras = Vec::new();
    for c in &doc.root.children {
        if let Some(o) = object_from(c) {
            objects.push(o);
        } else if let Some(cam) = camera_from(c) {
            cameras.push(cam);
        }
    }
    (objects, cameras)
}

/// Keep `orig` when it still parses to `v`, so unchanged values keep their text.
fn keep_or(orig: Option<&str>, same: impl Fn(&str) -> bool, new: String) -> String {
    match orig {
        Some(o) if same(o) => o.to_owned(),
        _ => new,
    }
}

fn object_attrs(o: &PlacedObject, orig: Option<&Elem>) -> Vec<(String, String)> {
    let k = o.kind;
    let ta = type_attr(k);
    let get = |key: &str| orig.and_then(|e| e.attr(key));
    let value = |key: &str| -> Option<String> {
        Some(match key {
            "Id" => o.id.0.to_string(),
            "Position" => keep_or(
                get(key),
                |s| vec3(Some(s), Vec3::ZERO) == o.position,
                fmt_list(&[o.position.x, o.position.y, o.position.z]),
            ),
            "Rotation" => keep_or(get(key), |s| s.trim().parse::<f32>().ok() == Some(o.rotation), fmt_num(o.rotation)),
            "Scale" => keep_or(get(key), |s| vec3(Some(s), Vec3::ONE) == o.scale, fmt_list(&[o.scale.x, o.scale.y, o.scale.z])),
            "Player" => keep_or(get(key), |s| s.trim().parse::<u8>().ok() == Some(o.player), o.player.to_string()),
            "Name" => o.name.clone(),
            _ if key == ta => o.type_id.clone(),
            _ => return o.extra.iter().find(|(ek, _)| ek == key).map(|(_, v)| v.clone()),
        })
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut emitted: Vec<String> = Vec::new();
    if let Some(e) = orig {
        for (key, _) in &e.attrs {
            if key == "Name" && o.name.is_empty() {
                continue;
            }
            if let Some(v) = value(key) {
                out.push((key.clone(), v));
            }
            emitted.push(key.clone());
        }
    }
    let point_type = if k == ObjectKind::Point && o.type_id.is_empty() { "Normal".to_owned() } else { o.type_id.clone() };
    let mut want: Vec<(&str, String)> = vec![("Id", o.id.0.to_string())];
    want.push(("Position", fmt_list(&[o.position.x, o.position.y, o.position.z])));
    if o.rotation != 0.0 {
        want.push(("Rotation", fmt_num(o.rotation)));
    }
    want.push(("Scale", fmt_list(&[o.scale.x, o.scale.y, o.scale.z])));
    want.push((ta, point_type));
    if k == ObjectKind::Unit {
        want.push(("Player", o.player.to_string()));
    }
    if !o.name.is_empty() {
        want.push(("Name", o.name.clone()));
    }
    for (key, v) in want {
        if !emitted.iter().any(|e| e == key) {
            out.push((key.to_owned(), v));
            emitted.push(key.to_owned());
        }
    }
    for (key, v) in &o.extra {
        if !emitted.iter().any(|e| e == key) {
            out.push((key.clone(), v.clone()));
            emitted.push(key.clone());
        }
    }
    out
}

fn object_xml(doc: Option<&Doc>, o: &PlacedObject, orig: Option<&Elem>) -> String {
    let attrs = object_attrs(o, orig);
    match (doc, orig) {
        (Some(d), Some(e)) if !e.self_closing => {
            format!("{}{}</{}>", xml::start_tag(&e.name, &attrs, false), d.raw(&e.inner), e.name)
        }
        _ => xml::start_tag(elem_name(o.kind), &attrs, true),
    }
}

fn set_camera_value(n: &mut XmlNode, index: &str, v: f32) {
    let found = n.children.iter_mut().find(|c| c.name == "CameraValue" && c.attr("Index") == Some(index));
    match found {
        Some(c) => {
            if c.attr("Value").and_then(|s| s.trim().parse::<f32>().ok()) != Some(v) {
                c.set_attr("Value", fmt_num(v));
            }
        }
        None => {
            let mut c = XmlNode::new("CameraValue");
            c.set_attr("Index", index);
            c.set_attr("Value", fmt_num(v));
            n.children.push(c);
        }
    }
}

fn camera_xml(c: &Camera, orig: Option<&Elem>, nl: &str) -> String {
    let mut n = match orig {
        Some(e) => e.to_node(),
        None => {
            let mut n = XmlNode::new("ObjectCamera");
            n.set_attr("Id", c.id.0.to_string());
            n.set_attr("Position", "0,0,0");
            n.set_attr("Rotation", "0");
            n.set_attr("Scale", "1,1,1");
            n.set_attr("Name", "");
            n.set_attr("CameraTarget", "0,0");
            n
        }
    };
    let prev = orig.and_then(camera_from);
    n.set_attr("Id", c.id.0.to_string());
    if prev.as_ref().map(|p| p.target) != Some(c.target) {
        let z = vec3(n.attr("Position"), Vec3::ZERO).z;
        n.set_attr("Position", fmt_list(&[c.target.x, c.target.y, z]));
        n.set_attr("CameraTarget", fmt_list(&[c.target.x, c.target.y]));
    }
    n.set_attr("Name", c.name.clone());
    if prev.as_ref().map(|p| p.yaw) != Some(c.yaw) {
        n.set_attr("Rotation", fmt_num(c.yaw.to_radians()));
    }
    for (index, v) in [("FieldOfView", c.fov), ("Distance", c.distance), ("Pitch", c.pitch), ("Yaw", c.yaw)] {
        set_camera_value(&mut n, index, v);
    }
    xml::node_string(&n, 1, nl)
}

pub fn write(orig: Option<&Doc>, objects: &[PlacedObject], cameras: &[Camera]) -> Vec<u8> {
    let nl = orig.map(Doc::newline).unwrap_or("\r\n");
    let obj_by_id: HashMap<u32, &PlacedObject> = objects.iter().map(|o| (o.id.0, o)).collect();
    let cam_by_id: HashMap<u32, &Camera> = cameras.iter().map(|c| (c.id.0, c)).collect();
    let mut used_obj: HashMap<u32, ()> = HashMap::new();
    let mut used_cam: HashMap<u32, ()> = HashMap::new();
    let Some(doc) = orig else {
        let mut s = String::from(xml::DECL);
        s.push_str(nl);
        s.push_str("<PlacedObjects Version=\"27\">");
        for o in objects {
            s.push_str(nl);
            s.push_str(xml::INDENT);
            s.push_str(&object_xml(None, o, None));
        }
        for c in cameras {
            s.push_str(nl);
            s.push_str(xml::INDENT);
            s.push_str(&camera_xml(c, None, nl));
        }
        s.push_str(nl);
        s.push_str("</PlacedObjects>");
        s.push_str(nl);
        return s.into_bytes();
    };
    let out = xml::rebuild(
        doc,
        |el| {
            if let Some(prev) = object_from(el) {
                match obj_by_id.get(&prev.id.0) {
                    Some(cur) if cur.kind == prev.kind && used_obj.insert(prev.id.0, ()).is_none() => {
                        if **cur == prev { Keep::Raw } else { Keep::Replace(object_xml(Some(doc), cur, Some(el))) }
                    }
                    _ => Keep::Drop,
                }
            } else if let Some(prev) = camera_from(el) {
                match cam_by_id.get(&prev.id.0) {
                    Some(cur) if used_cam.insert(prev.id.0, ()).is_none() => {
                        if **cur == prev { Keep::Raw } else { Keep::Replace(camera_xml(cur, Some(el), nl)) }
                    }
                    _ => Keep::Drop,
                }
            } else {
                Keep::Raw
            }
        },
        &[],
    );
    // Second pass for objects/cameras that had no element yet.
    let fresh: Vec<String> = objects
        .iter()
        .filter(|o| !doc.root.children.iter().any(|c| object_from(c).is_some_and(|p| p.id == o.id && p.kind == o.kind)))
        .map(|o| object_xml(None, o, None))
        .chain(
            cameras
                .iter()
                .filter(|c| !doc.root.children.iter().any(|e| camera_from(e).is_some_and(|p| p.id == c.id)))
                .map(|c| camera_xml(c, None, nl)),
        )
        .collect();
    if fresh.is_empty() {
        return out;
    }
    let reparsed = xml::parse_bytes(&out).expect("rebuilt Objects is well-formed");
    xml::rebuild(&reparsed, |_| Keep::Raw, &fresh)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<PlacedObjects Version=\"27\">\r\n    <ObjectUnit Id=\"1915310103\" Position=\"105.5515,102.5751,0\" Rotation=\"1.5283\" Scale=\"1,1,1\" UnitType=\"Marine\" Player=\"1\"/>\r\n    <ObjectDoodad Id=\"5\" Variation=\"1\" Position=\"12.5197,72.73,7.9978\" Rotation=\"5.3764\" Scale=\"1,1,1\" Type=\"Korhal_Antenna\">\r\n        <Flag Index=\"HeightAbsolute\" Value=\"1\"/>\r\n    </ObjectDoodad>\r\n    <ObjectPoint Id=\"9\" Position=\"23.5,22.5,0\" Scale=\"1,1,1\" Type=\"StartLoc\" Name=\"Start Location 001\" Color=\"0,0,0,0\"/>\r\n    <ObjectCamera Id=\"923\" Position=\"127,211.5,0\" Rotation=\"3.1315\" Scale=\"1,1,1\" Name=\"Cam\" Color=\"0,0,0,0\" CameraTarget=\"127,211.5\">\r\n        <CameraValue Index=\"FieldOfView\" Value=\"27.7995\"/>\r\n        <CameraValue Index=\"Distance\" Value=\"63.042\"/>\r\n        <CameraValue Index=\"Pitch\" Value=\"23.23\"/>\r\n        <CameraValue Index=\"Yaw\" Value=\"179.427\"/>\r\n    </ObjectCamera>\r\n    <ObjectFuture Id=\"77\"/>\r\n</PlacedObjects>\r\n";

    #[test]
    fn parse_and_identity() {
        let doc = xml::parse(SRC).unwrap();
        let (objs, cams) = parse(&doc);
        assert_eq!(objs.len(), 3);
        assert_eq!(objs[0].type_id, "Marine");
        assert_eq!(objs[0].player, 1);
        assert_eq!(objs[1].extra, vec![("Variation".to_owned(), "1".to_owned())]);
        assert_eq!(objs[2].type_id, "StartLoc");
        assert_eq!(objs[2].name, "Start Location 001");
        assert_eq!(cams[0].distance, 63.042);
        assert_eq!(cams[0].yaw, 179.427);
        assert_eq!(write(Some(&doc), &objs, &cams), SRC.as_bytes());
    }

    #[test]
    fn edit_move_delete_add() {
        let doc = xml::parse(SRC).unwrap();
        let (mut objs, mut cams) = parse(&doc);
        objs[1].position.x = 20.0; // doodad keeps its Flag child
        objs.remove(0);
        let mut m = PlacedObject::new(ObjectId(42), ObjectKind::Unit, "Zergling", Vec3::new(1.0, 2.0, 0.0));
        m.player = 2;
        objs.push(m);
        cams[0].distance = 40.0;
        let out = String::from_utf8(write(Some(&doc), &objs, &cams)).unwrap();
        assert!(!out.contains("1915310103"));
        assert!(out.contains("<ObjectDoodad Id=\"5\" Variation=\"1\" Position=\"20,72.73,7.9978\" Rotation=\"5.3764\" Scale=\"1,1,1\" Type=\"Korhal_Antenna\">\r\n        <Flag Index=\"HeightAbsolute\" Value=\"1\"/>\r\n    </ObjectDoodad>"));
        assert!(out.contains("<ObjectUnit Id=\"42\" Position=\"1,2,0\" Scale=\"1,1,1\" UnitType=\"Zergling\" Player=\"2\"/>"));
        assert!(out.contains("<CameraValue Index=\"Distance\" Value=\"40\"/>"));
        assert!(out.contains("<ObjectFuture Id=\"77\"/>"));
        let re = xml::parse(&out).unwrap();
        let (o2, c2) = parse(&re);
        assert_eq!(o2, objs);
        assert_eq!(c2, cams);
    }

    #[test]
    fn fresh_file() {
        let mut p = PlacedObject::new(ObjectId(3), ObjectKind::Point, "", Vec3::new(5.0, 6.0, 0.0));
        p.name = "Spawn".into();
        let cam = Camera::new(ObjectId(4), "Intro", Vec2::new(10.0, 12.0));
        let out = write(None, std::slice::from_ref(&p), std::slice::from_ref(&cam));
        let doc = xml::parse_bytes(&out).unwrap();
        let (o, c) = parse(&doc);
        p.type_id = "Normal".into();
        assert_eq!(o, vec![p]);
        assert_eq!(c[0].target, cam.target);
        assert_eq!(c[0].distance, cam.distance);
        assert!((c[0].yaw - cam.yaw).abs() < 1e-3);
    }
}
