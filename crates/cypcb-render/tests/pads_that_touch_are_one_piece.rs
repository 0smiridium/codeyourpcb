//! Two pads of one net whose copper touches are one piece of copper.
//!
//! `cargo test -p cypcb-render --test pads_that_touch_are_one_piece`
//!
//! KiCad joins them (`CN_VISITOR`, `connectivity_algo.cpp`), and they are
//! what `check`, the router and the ratsnest all read from
//! `cypcb_drc::rules::copper_pieces`. Until 2026-09-27 a pad joined copper
//! but not another pad, so multi_ic's U5.2 and U5.3 - two GND pads drawn
//! edge to edge - were each reported unrouted and joined by a ratsnest line
//! through copper that was already one land.
//!
//! Touching is a gap of zero, edge contact included: the gap at which
//! `ClearanceRule` reports two nets shorted.

use std::path::{Path, PathBuf};

use cypcb_render::{BoardSnapshot, PcbEngine};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// R1 at 5mm and R2 at `r2_x_mm`, both 0402: pads 0.6mm wide, 1.0mm apart,
/// so R2 at 6.6mm puts its pad 1 edge to edge with R1's pad 2. C1 is far
/// from both.
fn board(r2_x_mm: f64, nets: &str) -> String {
    format!(
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
    at 20mm, 10mm
}}
component C2 capacitor "0402" {{
    value "100nF"
    at 20mm, 16mm
}}
{nets}
"#
    )
}

const ONE_NET: &str = "net VCC {\n    R1.2\n    R2.1\n    C1.1\n}";
const TWO_NETS: &str = "net VCC {\n    R1.2\n    C1.1\n}\nnet GND {\n    R2.1\n    C2.1\n}";

fn snapshot(source: &str) -> BoardSnapshot {
    let mut engine = PcbEngine::new();
    let errors = engine.load_source(source);
    assert!(errors.is_empty(), "the board loads: {errors}");
    engine.build_snapshot()
}

fn unrouted(snapshot: &BoardSnapshot) -> Vec<String> {
    let mut pins: Vec<String> = snapshot
        .violations
        .iter()
        .filter(|v| v.kind == "unrouted-pin")
        .map(|v| {
            v.message
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    pins.sort();
    pins
}

fn of_kind(snapshot: &BoardSnapshot, kind: &str) -> Vec<String> {
    snapshot
        .violations
        .iter()
        .filter(|v| v.kind == kind)
        .map(|v| v.message.clone())
        .collect()
}

#[test]
fn two_pads_of_one_net_edge_to_edge_are_one_piece() {
    let touching = snapshot(&board(6.6, ONE_NET));
    // R1.2 and R2.1 are one land; C1.1 is the one connection still missing.
    assert_eq!(unrouted(&touching), ["C1.1"], "{:#?}", touching.violations);
    assert_eq!(touching.ratsnest.len(), 1, "{:#?}", touching.ratsnest);
    assert!(of_kind(&touching, "net-split").is_empty());
    assert!(
        of_kind(&touching, "clearance").is_empty(),
        "one net does not short itself"
    );
}

#[test]
fn two_pads_of_one_net_a_hundredth_apart_are_two() {
    // The control for the test above: 0.01mm between the same two pads.
    let apart = snapshot(&board(6.61, ONE_NET));
    assert_eq!(
        unrouted(&apart),
        ["C1.1", "R1.2", "R2.1"],
        "{:#?}",
        apart.violations
    );
    assert_eq!(apart.ratsnest.len(), 2, "{:#?}", apart.ratsnest);
}

#[test]
fn two_pads_of_two_nets_edge_to_edge_are_a_short_and_stay_two() {
    let shorted = snapshot(&board(6.6, TWO_NETS));
    let clearance = of_kind(&shorted, "clearance");
    assert!(
        clearance
            .iter()
            .any(|m| m.contains("0.000mm") || m.contains("0.00mm")),
        "copper of two nets touching is a short: {clearance:#?}"
    );
    // Neither pad reaches the other's net: each net is still two bare pads.
    assert_eq!(unrouted(&shorted), ["C1.1", "C2.1", "R1.2", "R2.1"]);
    assert_eq!(shorted.ratsnest.len(), 2, "{:#?}", shorted.ratsnest);
}

#[test]
fn multi_ic_touching_ground_pads_are_no_longer_missing() {
    let source =
        std::fs::read_to_string(repo().join("tests/fixtures/benchmark/multi_ic.kicad_pcb"))
            .expect("the fixture reads");
    let mut engine = PcbEngine::new();
    let errors = engine.load_kicad(&source);
    assert!(errors.is_empty(), "the board loads: {errors}");
    let snapshot = engine.build_snapshot();
    let bare = unrouted(&snapshot);
    for pin in ["U5.2", "U5.3", "J2.4", "J2.5", "J2.7", "J2.8"] {
        assert!(
            !bare.iter().any(|p| p == pin),
            "{pin} touches a pad of its net and is reached"
        );
    }
    // The control: their neighbours on the same parts touch nothing of their
    // net and are still reported.
    for pin in ["U5.1", "J2.6"] {
        assert!(
            bare.iter().any(|p| p == pin),
            "{pin} is still bare: {bare:?}"
        );
    }
}
