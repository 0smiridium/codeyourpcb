//! Two pads of one net whose copper touches are one piece to the router.
//!
//! `cargo test -p cypcb-autoroute --test pads_that_touch_need_no_route`
//!
//! The router keeps one pad per piece of copper `cypcb_drc::rules::copper_pieces`
//! finds. Since 2026-09-27 two pads of one net drawn edge to edge are one
//! piece there, as KiCad's connectivity has them, so the router lays nothing
//! between them. Until then it routed a trace from one to the other over
//! copper that was already one land.

use std::path::Path;

use cypcb_autoroute::{route_board, AutorouteConfig};
use cypcb_drc::{preset_for_world, ruleset_for_world};
use cypcb_rules::presets::RulesPreset;
use cypcb_world::footprint::FootprintLibrary;
use cypcb_world::{sync_ast_to_world, BoardWorld};

/// R1 at 5mm and R2 at `r2_x_mm`, both 0402: R2 at 6.6mm puts its pad 1 edge
/// to edge with R1's pad 2. VCC joins those two pads; GND is the control, a
/// net that has to be routed on either board.
fn routed_segments(r2_x_mm: f64) -> (usize, usize) {
    let source = format!(
        r#"version 1
board touching {{
    size 30mm x 20mm
    layers 2
}}
component R1 resistor "0402" {{
    value "10k"
    at 5mm, 10mm
}}
component R2 resistor "0402" {{
    value "10k"
    at {r2_x_mm}mm, 10mm
}}
component C1 capacitor "0402" {{
    value "100nF"
    at 12mm, 16mm
}}
net VCC {{
    R1.2
    R2.1
}}
net GND {{
    R1.1
    C1.1
}}
"#
    );
    let parsed = cypcb_parser::parse(&source);
    assert!(parsed.is_ok(), "the board parses");
    let mut world = BoardWorld::new();
    let mut library = FootprintLibrary::new();
    let synced = sync_ast_to_world(&parsed.value, &source, &mut world, &mut library);
    assert!(synced.errors.is_empty(), "the board loads");
    let net = |world: &BoardWorld, name: &str| {
        world
            .nets()
            .find(|(_, n)| *n == name)
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("the board has a {name} net"))
    };
    let (vcc, gnd) = (net(&world, "VCC"), net(&world, "GND"));
    let preset = preset_for_world(RulesPreset::JlcpcbStandard2Layer, &world);
    let rules = ruleset_for_world(preset, &world);
    let result = route_board(&mut world, &library, &rules, &AutorouteConfig::default());
    let count = |id| result.routes.iter().filter(|s| s.net_id == id).count();
    (count(vcc), count(gnd))
}

#[test]
fn the_router_lays_nothing_between_two_pads_of_one_net_that_touch() {
    let (vcc, gnd) = routed_segments(6.6);
    assert!(gnd > 0, "GND has to be routed, so the router ran");
    assert_eq!(
        vcc, 0,
        "R1.2 and R2.1 touch, and the router laid {vcc} VCC segments"
    );
}

#[test]
fn the_router_joins_the_same_two_pads_a_hundredth_apart() {
    // The control: 0.01mm between them is a connection to make.
    let (vcc, gnd) = routed_segments(6.61);
    assert!(gnd > 0, "GND has to be routed, so the router ran");
    assert!(
        vcc > 0,
        "R1.2 and R2.1 are 0.01mm apart and nothing joins them"
    );
}

/// R1.2 and R2.1 edge to edge on GND, and C1.1 on GND to the right of R2.
/// The segments the router lays for GND, as start and end in mm.
fn ground_to_a_touching_pair() -> Vec<((f64, f64), (f64, f64))> {
    let source = r#"version 1
board pair {
    size 30mm x 20mm
    layers 2
}
component R1 resistor "0402" {
    value "10k"
    at 5mm, 10mm
}
component R2 resistor "0402" {
    value "10k"
    at 6.6mm, 10mm
}
component C1 capacitor "0402" {
    value "100nF"
    at 12mm, 10mm
}
net GND {
    R1.2
    R2.1
    C1.1
}
"#;
    let parsed = cypcb_parser::parse(source);
    assert!(parsed.is_ok(), "the board parses");
    let mut world = BoardWorld::new();
    let mut library = FootprintLibrary::new();
    let synced = sync_ast_to_world(&parsed.value, source, &mut world, &mut library);
    assert!(synced.errors.is_empty(), "the board loads");
    let preset = preset_for_world(RulesPreset::JlcpcbStandard2Layer, &world);
    let rules = ruleset_for_world(preset, &world);
    let result = route_board(&mut world, &library, &rules, &AutorouteConfig::default());
    result
        .routes
        .iter()
        .map(|s| {
            (
                (s.start.x.to_mm(), s.start.y.to_mm()),
                (s.end.x.to_mm(), s.end.y.to_mm()),
            )
        })
        .collect()
}

#[test]
fn the_router_reaches_a_touching_pair_at_its_nearer_pad() {
    let segments = ground_to_a_touching_pair();
    // The control: one connection is missing, and the router made it.
    assert!(
        !segments.is_empty(),
        "C1.1 is joined to nothing and has to be routed"
    );
    // R1.2 spans 5.2mm to 5.8mm, R2.1 5.8mm to 6.4mm, C1.1 11.2mm to 11.8mm.
    // Every point the router lays is right of R1.2: it stops at R2.1, the
    // pad of the pair nearer C1.1, instead of running on to R1.2.
    let leftmost = segments
        .iter()
        .flat_map(|(a, b)| [a.0, b.0])
        .fold(f64::INFINITY, f64::min);
    assert!(
        leftmost >= 5.8,
        "the GND route reaches {leftmost}mm, into R1.2, past the pad it touches: {segments:?}"
    );
}

#[test]
fn the_router_enters_a_pad_of_a_touching_pair_as_it_enters_any_pad() {
    // A pad reached because it touches the pad the ratsnest keeps is entered
    // as a pad the ratsnest routes to is: through its zone, to its centre.
    // When any cell of its copper was the goal, a route could graze its edge
    // and stop - on multi_ic's J2.5 that was a trace entering the land at 29.9
    // degrees, where R-08 asks for 45.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/benchmark/multi_ic.kicad_pcb");
    let parsed = cypcb_kicad::parse_kicad_pcb(&path).expect("multi_ic parses");
    let (mut world, library) = (parsed.world, parsed.library);
    let preset = preset_for_world(RulesPreset::JlcpcbStandard2Layer, &world);
    let rules = ruleset_for_world(preset, &world);
    let result = route_board(&mut world, &library, &rules, &AutorouteConfig::default());
    cypcb_router::apply_routes(&mut world, &result);
    world.rebuild_spatial_index_from_library(&library);
    let (sharp, report) = cypcb_drc::rules::pad_entry::measure_entries(&mut world);

    // The control: the rule looked, and found the sharp entries the rest of
    // the board still has.
    assert!(
        report.examined > 0 && !sharp.is_empty(),
        "R-08 measured nothing"
    );
    let pairs = ["U5.2", "U5.3", "J2.4", "J2.5", "J2.7", "J2.8"];
    let on_a_pair: Vec<&str> = sharp
        .iter()
        .map(|v| v.message.as_str())
        .filter(|m| pairs.iter().any(|pin| m.starts_with(&format!("{pin}:"))))
        .collect();
    assert!(
        on_a_pair.is_empty(),
        "a pad of a touching pair entered at a wedge: {on_a_pair:?}"
    );
}
