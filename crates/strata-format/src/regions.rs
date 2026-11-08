//! `Regions` (XML).
//!
//! Observed:
//!
//! ```xml
//! <region id="6">
//!     <name value="Spawns"/>
//!     <color value="255,255,0,0"/>        <!-- A,R,G,B; optional -->
//!     <invisible/>
//!     <shape type="rect"><quad value="x0,y0,x1,y1"/></shape>
//!     <shape type="circle"><center value="x,y"/><radius value="r"/></shape>
//!     <shape type="diamond"><center value="x,y"/><width value="w"/><height value="h"/></shape>
//!     <shape type="..."><negative/>...</shape>   <!-- subtractive -->
//! </region>
//! ```
//!
//! Diamonds load as 4-point polygons and are written back as diamonds.
//! Negative shapes and unknown shape types are not modeled; they are kept
//! from the original element whenever a region is rewritten. General
//! polygons are written as `<shape type="polygon">` with `<point value>`
//! children (inferred, not observed in editor output).

use std::collections::HashMap;

use strata_core::catalog::XmlNode;
use strata_core::objects::ObjectId;
use strata_core::regions::{Region, Shape};
use strata_core::Vec2;

use crate::xml::{self, fmt_list, fmt_num, parse_list, Doc, Elem, Keep};

const DEFAULT_COLOR: [u8; 3] = [255, 255, 255];

fn v2(s: Option<&str>) -> Vec2 {
    let v = s.map(parse_list).unwrap_or_default();
    Vec2::new(v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0))
}

fn num(s: Option<&str>) -> f32 {
    s.and_then(|s| s.trim().parse().ok()).unwrap_or(0.0)
}

/// A modeled (positive, known) shape, or `None`.
fn shape_from(el: &Elem) -> Option<Shape> {
    if el.name != "shape" || el.child("negative").is_some() {
        return None;
    }
    match el.attr("type")? {
        "rect" => {
            let q = parse_list(el.child_value("quad")?);
            if q.len() < 4 {
                return None;
            }
            Some(Shape::Rect {
                min: Vec2::new(q[0].min(q[2]), q[1].min(q[3])),
                max: Vec2::new(q[0].max(q[2]), q[1].max(q[3])),
            })
        }
        "circle" => Some(Shape::Circle { center: v2(el.child_value("center")), radius: num(el.child_value("radius")) }),
        "diamond" => {
            let c = v2(el.child_value("center"));
            let hw = num(el.child_value("width")) / 2.0;
            let hh = num(el.child_value("height")) / 2.0;
            Some(Shape::Polygon {
                points: vec![
                    Vec2::new(c.x + hw, c.y),
                    Vec2::new(c.x, c.y + hh),
                    Vec2::new(c.x - hw, c.y),
                    Vec2::new(c.x, c.y - hh),
                ],
            })
        }
        "polygon" => Some(Shape::Polygon { points: el.children_named("point").map(|p| v2(p.attr("value"))).collect() }),
        _ => None,
    }
}

fn color_from(s: &str) -> [u8; 3] {
    let v: Vec<u8> = s.split(',').map(|p| p.trim().parse().unwrap_or(0)).collect();
    if v.len() >= 4 { [v[1], v[2], v[3]] } else { DEFAULT_COLOR }
}

pub fn region_from(el: &Elem) -> Option<Region> {
    if el.name != "region" {
        return None;
    }
    let id: u32 = el.attr("id")?.trim().parse().ok()?;
    Some(Region {
        id: ObjectId(id),
        name: el.child_value("name").unwrap_or("").to_owned(),
        color: el.child_value("color").map(color_from).unwrap_or(DEFAULT_COLOR),
        shapes: el.children.iter().filter_map(shape_from).collect(),
    })
}

pub fn parse(doc: &Doc) -> Vec<Region> {
    doc.root.children.iter().filter_map(region_from).collect()
}

fn valued(name: &str, v: String) -> XmlNode {
    let mut n = XmlNode::new(name);
    n.set_attr("value", v);
    n
}

fn diamond_of(points: &[Vec2]) -> Option<(Vec2, f32, f32)> {
    if points.len() != 4 {
        return None;
    }
    let c = Vec2::new((points[0].x + points[2].x) / 2.0, (points[1].y + points[3].y) / 2.0);
    let ok = (points[0].y - c.y).abs() < 1e-4
        && (points[2].y - c.y).abs() < 1e-4
        && (points[1].x - c.x).abs() < 1e-4
        && (points[3].x - c.x).abs() < 1e-4;
    ok.then(|| (c, points[0].x - points[2].x, points[1].y - points[3].y))
}

fn shape_node(s: &Shape) -> XmlNode {
    let mut n = XmlNode::new("shape");
    match s {
        Shape::Rect { min, max } => {
            n.set_attr("type", "rect");
            n.children.push(valued("quad", fmt_list(&[min.x, min.y, max.x, max.y])));
        }
        Shape::Circle { center, radius } => {
            n.set_attr("type", "circle");
            n.children.push(valued("center", fmt_list(&[center.x, center.y])));
            n.children.push(valued("radius", fmt_num(*radius)));
        }
        Shape::Polygon { points } => match diamond_of(points) {
            Some((c, w, h)) => {
                n.set_attr("type", "diamond");
                n.children.push(valued("center", fmt_list(&[c.x, c.y])));
                n.children.push(valued("width", fmt_num(w)));
                n.children.push(valued("height", fmt_num(h)));
            }
            None => {
                n.set_attr("type", "polygon");
                for p in points {
                    n.children.push(valued("point", fmt_list(&[p.x, p.y])));
                }
            }
        },
    }
    n
}

fn region_xml(r: &Region, orig: Option<&Elem>, nl: &str) -> String {
    let mut n = XmlNode::new("region");
    n.set_attr("id", r.id.0.to_string());
    let prev = orig.and_then(region_from);
    let color_changed = prev.as_ref().map(|p| p.color) != Some(r.color);
    let shapes_changed = prev.as_ref().map(|p| &p.shapes) != Some(&r.shapes);
    let mut wrote_name = false;
    let mut wrote_color = false;
    let mut wrote_shapes = false;
    let push_shapes = |n: &mut XmlNode| n.children.extend(r.shapes.iter().map(shape_node));
    if let Some(e) = orig {
        n.attrs = e.attrs.clone();
        n.set_attr("id", r.id.0.to_string());
        for c in &e.children {
            match c.name.as_str() {
                "name" => {
                    n.children.push(valued("name", r.name.clone()));
                    wrote_name = true;
                }
                "color" => {
                    if color_changed {
                        n.children.push(valued("color", format!("255,{},{},{}", r.color[0], r.color[1], r.color[2])));
                    } else {
                        n.children.push(c.to_node());
                    }
                    wrote_color = true;
                }
                "shape" if shape_from(c).is_some() => {
                    if !shapes_changed {
                        n.children.push(c.to_node());
                    } else if !wrote_shapes {
                        push_shapes(&mut n);
                    }
                    wrote_shapes = true;
                }
                _ => n.children.push(c.to_node()),
            }
        }
    }
    if !wrote_name {
        n.children.insert(0, valued("name", r.name.clone()));
    }
    if !wrote_color && r.color != DEFAULT_COLOR {
        n.children.insert(1, valued("color", format!("255,{},{},{}", r.color[0], r.color[1], r.color[2])));
    }
    if !wrote_shapes {
        push_shapes(&mut n);
    }
    xml::node_string(&n, 1, nl)
}

pub fn write(orig: Option<&Doc>, regions: &[Region]) -> Vec<u8> {
    let nl = orig.map(Doc::newline).unwrap_or("\r\n");
    let Some(doc) = orig else {
        let mut s = String::from(xml::DECL);
        s.push_str(nl);
        s.push_str("<Regions>");
        for r in regions {
            s.push_str(nl);
            s.push_str(xml::INDENT);
            s.push_str(&region_xml(r, None, nl));
        }
        s.push_str(nl);
        s.push_str("</Regions>");
        s.push_str(nl);
        return s.into_bytes();
    };
    let by_id: HashMap<u32, &Region> = regions.iter().map(|r| (r.id.0, r)).collect();
    let mut used: HashMap<u32, ()> = HashMap::new();
    let existing: Vec<u32> = doc.root.children.iter().filter_map(region_from).map(|r| r.id.0).collect();
    let fresh: Vec<String> =
        regions.iter().filter(|r| !existing.contains(&r.id.0)).map(|r| region_xml(r, None, nl)).collect();
    xml::rebuild(
        doc,
        |el| match region_from(el) {
            Some(prev) => match by_id.get(&prev.id.0) {
                Some(cur) if used.insert(prev.id.0, ()).is_none() => {
                    if **cur == prev { Keep::Raw } else { Keep::Replace(region_xml(cur, Some(el), nl)) }
                }
                _ => Keep::Drop,
            },
            None => Keep::Raw,
        },
        &fresh,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Regions>\r\n    <region id=\"1\">\r\n        <name value=\"Camera Bound\"/>\r\n        <color value=\"255,255,0,0\"/>\r\n        <invisible/>\r\n        <shape type=\"rect\">\r\n            <quad value=\"110.674,11.6372,238.5163,68.1596\"/>\r\n        </shape>\r\n        <shape type=\"circle\">\r\n            <negative/>\r\n            <center value=\"1,2\"/>\r\n            <radius value=\"3\"/>\r\n        </shape>\r\n    </region>\r\n    <region id=\"2\">\r\n        <name value=\"D\"/>\r\n        <shape type=\"diamond\">\r\n            <center value=\"10,10\"/>\r\n            <width value=\"4\"/>\r\n            <height value=\"2\"/>\r\n        </shape>\r\n    </region>\r\n</Regions>";

    #[test]
    fn parse_identity_edit() {
        let doc = xml::parse(SRC).unwrap();
        let mut rs = parse(&doc);
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].color, [255, 0, 0]);
        assert_eq!(rs[0].shapes.len(), 1); // negative circle not modeled
        assert!(rs[1].contains(Vec2::new(11.0, 10.2)));
        assert_eq!(write(Some(&doc), &rs), SRC.as_bytes());

        rs[0].name = "Renamed".into();
        rs[0].shapes[0].translate(Vec2::new(1.0, 0.0));
        rs[1].color = [0, 255, 0];
        rs.push(Region { id: ObjectId(3), name: "New".into(), color: DEFAULT_COLOR, shapes: vec![Shape::Circle { center: Vec2::ONE, radius: 2.0 }] });
        let out = String::from_utf8(write(Some(&doc), &rs)).unwrap();
        assert!(out.contains("<negative/>"));
        assert!(out.contains("<invisible/>"));
        assert!(out.contains("<shape type=\"diamond\">"));
        assert!(out.ends_with("</Regions>"));
        let back = parse(&xml::parse(&out).unwrap());
        assert_eq!(back, rs);
    }
}
