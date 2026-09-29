//! A net the designer wired by hand is one piece of copper to the router.
//!
//! `cargo test -p cypcb-autoroute --test a_hand_wired_net_is_one_piece_to_the_router`
//!
//! The router keeps one pad per piece of copper that
//! `cypcb_drc::rules::copper_pieces` finds, the pieces `check` and the
//! viewer's ratsnest read. Keep every pad of a piece and the connection the
//! hand trace already makes is routed a second time: on
//! `examples/uat-routing-locked.cypcb` that was a second VCC trace beside the
//! locked one, nine segments where five do the job.

use std::path::Path;

use cypcb_autoroute::{route_board, AutorouteConfig};
use cypcb_drc::{preset_for_world, ruleset_for_world};
use cypcb_rules::presets::RulesPreset;
use cypcb_world::footprint::FootprintLibrary;
use cypcb_world::{sync_ast_to_world, BoardWorld};

#[test]
fn the_router_lays_no_copper_on_a_net_the_hand_trace_already_joins() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/uat-routing-locked.cypcb");
    let source = std::fs::read_to_string(&path).expect("the example reads");
    let parsed = cypcb_parser::parse(&source);
    assert!(parsed.is_ok(), "the example parses");
    let mut world = BoardWorld::new();
    let mut library = FootprintLibrary::new();
    let synced = sync_ast_to_world(&parsed.value, &source, &mut world, &mut library);
    assert!(synced.errors.is_empty(), "the example loads");

    let net = |world: &BoardWorld, name: &str| {
        world
            .nets()
            .find(|(_, n)| *n == name)
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("the example has a {name} net"))
    };
    let (vcc, gnd) = (net(&world, "VCC"), net(&world, "GND"));

    let preset = preset_for_world(RulesPreset::JlcpcbStandard2Layer, &world);
    let rules = ruleset_for_world(preset, &world);
    let result = route_board(&mut world, &library, &rules, &AutorouteConfig::default());

    // The control: the router ran and laid the net nobody wired.
    assert!(
        result.routes.iter().any(|segment| segment.net_id == gnd),
        "GND has no hand copper and has to be routed"
    );
    let again: Vec<_> = result
        .routes
        .iter()
        .filter(|segment| segment.net_id == vcc)
        .collect();
    assert!(
        again.is_empty(),
        "the locked trace already joins R1.2 and C1.1, and the router laid {} more segments of VCC",
        again.len()
    );
}
