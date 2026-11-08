//! Build a small playable map from scratch with the Strata core API and save it
//! as an .SC2Map, for launching in the real game as a smoke test.
//! Usage: cargo run -p strata-format --example smoke_map -- <out.SC2Map>

use strata_core::brush::{BrushTool, Falloff};
use strata_core::objects::ObjectKind;
use strata_core::ops::Op;
use strata_core::triggers::{Call, Expr, Literal, Stmt, Trigger, TriggerItem};
use strata_core::MapDocument;

fn run(doc: &mut MapDocument, op: Op) -> serde_json::Value {
    op.run(doc).expect("op failed").result
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: smoke_map <out.SC2Map>");
    let mut doc = MapDocument::new("Strata smoke test", 64, 64);

    // A hill in the middle so the terrain isn't flat.
    run(
        &mut doc,
        Op::Brush {
            tool: BrushTool::Raise,
            points: vec![[32.0, 32.0], [33.0, 33.0]],
            radius: 8.0,
            strength: 0.6,
            shape: Default::default(),
            falloff: Falloff::Smooth,
        },
    );

    // Start locations, one per player, plus units for each side.
    for (x, y) in [(16.0, 16.0), (48.0, 48.0)] {
        run(&mut doc, Op::PlaceObject { kind: ObjectKind::Point, type_id: "StartLoc".into(), x, y, player: None, rotation: 0.0, name: String::new() });
    }
    for i in 0..3 {
        let f = i as f32;
        run(&mut doc, Op::PlaceObject { kind: ObjectKind::Unit, type_id: "Marine".into(), x: 22.0 + f, y: 20.0, player: Some(1), rotation: 0.0, name: String::new() });
        run(&mut doc, Op::PlaceObject { kind: ObjectKind::Unit, type_id: "Zergling".into(), x: 42.0 + f, y: 44.0, player: Some(2), rotation: 3.14, name: String::new() });
    }

    // Map init: show a message so the run is visibly ours.
    let mut t = Trigger::new(doc.triggers.alloc_id(), "Strata hello");
    t.events.push(Call { function: "TriggerAddEventMapInit".into(), args: vec![] });
    t.actions.push(Stmt::Call(Call {
        function: "UIDisplayMessage".into(),
        args: vec![
            Expr::Call(Call { function: "PlayerGroupAll".into(), args: vec![] }),
            Expr::Preset("c_messageAreaSubtitle".into()),
            Expr::Call(Call { function: "StringToText".into(), args: vec![Expr::Literal(Literal::String("Hello from Strata".into()))] }),
        ],
    }));
    doc.triggers.items.push(TriggerItem::Trigger(t));
    doc.dirty.triggers = true;

    let problems = strata_core::validate::validate(&doc);
    for p in &problems {
        println!("{:?}: {}", p.severity, p.message);
    }
    let bytes = strata_format::save_sc2map(&doc).expect("save");
    std::fs::write(&out, &bytes).expect("write");
    let back = strata_format::load_sc2map(bytes).expect("reload");
    println!("wrote {out}: {} objects, {} triggers, script:", back.objects.objects.len(), back.triggers.triggers().len());
    println!("{}", strata_core::galaxy::generate(&back));
}
