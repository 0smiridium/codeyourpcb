//! Two pads of one net whose copper touches are one piece to the router.
//!
//! `cargo test -p cypcb-autoroute --test pads_that_touch_need_no_route`
//!
//! The router keeps one pad per piece of copper `cypcb_drc::rules::copper_pieces`
//! finds. Since 2026-09-27 two pads of one net drawn edge to edge are one
//! piece there, as KiCad's connectivity has them, so the router lays nothing
//! between them. Until then it routed a trace from one to the other over
//! copper that was already one land.

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
