//! esp32_starter routes with one pin left open, and no more.
//!
//! `cargo test -p cypcb-render --test esp32_starter_routes_all_but_one_pin`
//!
//! This is the board compared against another tool's output, so a
//! connection the router leaves open costs more here than a sharp entry. On
//! 2026-09-27 `cypcb route --fast` left one pin open, J1.B5, and a change to
//! where the router may end a route on a pad of two touching pads left the
//! BOOT net open as well: U1.27 and SW2.1, with the router reporting
//! `converged=false`.

use std::path::Path;

use cypcb_render::PcbEngine;

#[test]
fn esp32_starter_after_fast_routing_leaves_at_most_one_pin_open() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/benchmark/esp32_starter.cypcb");
    let source = std::fs::read_to_string(&path).expect("the fixture reads");
    let mut engine = PcbEngine::new();
    let errors = engine.load_source(&source);
    assert!(errors.is_empty(), "the board loads: {errors}");

    // The control: before routing most of the board is open, so the count
    // below is a count of something.
    let before = engine
        .build_snapshot()
        .violations
        .iter()
        .filter(|v| v.kind == "unrouted-pin")
        .count();
    assert!(before > 1, "the board as drawn has {before} open pins");

    // `cypcb route --fast`: the default settings, once.
    let status = engine.auto_route();
    assert!(status.contains("\"ok\":true"), "the board routes: {status}");
    let open: Vec<String> = engine
        .build_snapshot()
        .violations
        .iter()
        .filter(|v| v.kind == "unrouted-pin")
        .map(|v| v.message.clone())
        .collect();
    assert!(
        open.len() <= 1,
        "{} pins open after routing, where one (J1.B5) was: {open:#?}",
        open.len()
    );
}
