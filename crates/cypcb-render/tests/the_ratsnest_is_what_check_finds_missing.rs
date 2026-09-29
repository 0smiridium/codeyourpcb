//! The ratsnest draws the connections `check` finds missing, and no others.
//!
//! `cargo test -p cypcb-render --test the_ratsnest_is_what_check_finds_missing`
//!
//! The viewer used to skip the ratsnest of every net with a single trace on
//! it. A net wired by hand from R1.2 to C1.1 showed no line to R2.1, while
//! `check` reported R2.1 unrouted and the router routed it. The three now read
//! one function, `cypcb_drc::rules::copper_pieces`, and this holds the viewer
//! to what the checker says: on each board, one ratsnest line for each
//! connection the checker's report leaves open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cypcb_render::BoardSnapshot;
use cypcb_render::PcbEngine;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Three pads on one net, a hand trace from R1.2 to C1.1, and R2.1 bare.
fn hand_copper() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hand-copper.cypcb"),
    )
    .expect("the fixture reads")
}

/// The same board without the trace.
fn no_copper() -> String {
    let board = hand_copper();
    let start = board
        .find("trace VCC {")
        .expect("the fixture has its trace");
    let end = start + board[start..].find('}').expect("the trace block closes") + 1;
    format!("{}{}", &board[..start], &board[end..])
}

fn loaded(source: &str) -> PcbEngine {
    let mut engine = PcbEngine::new();
    let errors = engine.load_source(source);
    assert!(errors.is_empty(), "the board loads: {errors}");
    engine
}

/// A ratsnest line as the two points it joins, in mm, each end sorted.
fn lines(snapshot: &BoardSnapshot) -> Vec<((f64, f64), (f64, f64))> {
    let mut lines: Vec<_> = snapshot
        .ratsnest
        .iter()
        .map(|line| {
            let a = (line.start_x / 1e6, line.start_y / 1e6);
            let b = (line.end_x / 1e6, line.end_y / 1e6);
            if a <= b {
                (a, b)
            } else {
                (b, a)
            }
        })
        .collect();
    lines.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    lines
}

/// How many connections `check` leaves open on this board, read from its
/// report alone.
///
/// Per net: every pin `unrouted-pin` names is a piece of its own, since no
/// copper touches it. If any pin of the net is reached, the reached copper is
/// one piece, plus one for each `net-split` line. The net is short one
/// connection fewer than its pieces.
fn missing_by_check(snapshot: &BoardSnapshot) -> usize {
    let mut net_of: HashMap<(String, String), String> = HashMap::new();
    for net in &snapshot.nets {
        for pin in &net.connections {
            net_of.insert((pin.component.clone(), pin.pin.clone()), net.name.clone());
        }
    }
    let mut pads: HashMap<String, usize> = HashMap::new();
    for component in &snapshot.components {
        for pad in &component.pads {
            if let Some(net) = net_of.get(&(component.refdes.clone(), pad.number.clone())) {
                *pads.entry(net.clone()).or_default() += 1;
            }
        }
    }
    let label_net: HashMap<String, String> = net_of
        .iter()
        .map(|((part, pin), net)| (format!("{part}.{pin}"), net.clone()))
        .collect();

    let mut unrouted: HashMap<String, usize> = HashMap::new();
    let mut split: HashMap<String, usize> = HashMap::new();
    for violation in &snapshot.violations {
        match violation.kind.as_str() {
            "unrouted-pin" => {
                let label = violation.message.split_whitespace().next().unwrap_or("");
                let net = label_net
                    .get(label)
                    .unwrap_or_else(|| panic!("an unrouted pin on no net: {}", violation.message));
                *unrouted.entry(net.clone()).or_default() += 1;
            }
            "net-split" => {
                let net = violation
                    .message
                    .strip_prefix("net ")
                    .and_then(|rest| rest.split(" is in pieces").next())
                    .unwrap_or_else(|| {
                        panic!("a net-split line in another shape: {}", violation.message)
                    });
                *split.entry(net.to_string()).or_default() += 1;
            }
            _ => {}
        }
    }

    pads.iter()
        .map(|(net, &count)| {
            let bare = unrouted.get(net).copied().unwrap_or(0);
            let reached = if bare < count {
                split.get(net).copied().unwrap_or(0) + 1
            } else {
                0
            };
            (bare + reached).saturating_sub(1)
        })
        .sum()
}

#[test]
fn a_pad_the_hand_trace_does_not_reach_has_the_one_line() {
    let mut engine = loaded(&hand_copper());
    let snapshot = engine.build_snapshot();
    assert_eq!(snapshot.traces.len(), 1, "the hand trace is on the board");
    // R1.2 is the pad of the hand-wired piece nearest to R2.1.
    assert_eq!(lines(&snapshot), vec![((5.5, 10.0), (11.5, 16.0))]);
}

#[test]
fn control_the_same_net_with_no_copper_has_a_line_to_every_pad() {
    let mut engine = loaded(&no_copper());
    let snapshot = engine.build_snapshot();
    assert_eq!(snapshot.traces.len(), 0, "the control has no copper");
    assert_eq!(
        lines(&snapshot),
        vec![((5.5, 10.0), (11.5, 16.0)), ((11.5, 16.0), (19.5, 10.0))]
    );
}

/// A session file routing R1.2 to C1.1 on the top layer, in the tenth-mils
/// of `(resolution mil 10)`.
fn session() -> String {
    let t = |mm: f64| format!("{:.4}", mm * 1_000_000.0 / 2540.0);
    format!(
        "(session \"hand_copper\" (routes (resolution mil 10) (network_out \
         (net \"VCC\" (wire (path F.Cu {} {} {} {} {}))))))",
        t(0.3),
        t(5.5),
        t(10.0),
        t(19.5),
        t(10.0)
    )
}

#[test]
fn a_session_file_is_copper_the_engine_holds() {
    let mut engine = loaded(&no_copper());
    let errors = engine.load_ses(&session());
    assert!(errors.is_empty(), "the session file loads: {errors}");
    let snapshot = engine.build_snapshot();
    assert_eq!(
        snapshot.traces.len(),
        1,
        "the routed trace is in the engine"
    );
    assert_eq!(lines(&snapshot), vec![((5.5, 10.0), (11.5, 16.0))]);
    let unrouted: Vec<&str> = snapshot
        .violations
        .iter()
        .filter(|v| v.kind == "unrouted-pin")
        .map(|v| v.message.as_str())
        .collect();
    assert_eq!(unrouted, vec!["R2.1 is on a net that no copper reaches"]);
}

#[test]
fn the_ratsnest_counts_what_check_finds_missing_on_every_board() {
    let mut boards: Vec<(String, BoardSnapshot)> = vec![
        (
            "hand-copper".into(),
            loaded(&hand_copper()).build_snapshot(),
        ),
        ("no-copper".into(), loaded(&no_copper()).build_snapshot()),
    ];
    for fixture in [
        "led_blink",
        "stm32_breakout",
        "multi_ic",
        "shift_driver",
        "qfp_fanout",
        "plane_board",
    ] {
        let path = repo().join(format!("tests/fixtures/benchmark/{fixture}.kicad_pcb"));
        let source = std::fs::read_to_string(&path).expect("the fixture reads");
        let mut engine = PcbEngine::new();
        let errors = engine.load_kicad(&source);
        assert!(errors.is_empty(), "{fixture} loads: {errors}");
        // Before routing, where most of the board is open.
        boards.push((format!("{fixture} as drawn"), engine.build_snapshot()));
        // `cypcb route --fast`: the default settings, once.
        let status = engine.auto_route();
        assert!(status.contains("\"ok\":true"), "{fixture} routes: {status}");
        boards.push((format!("{fixture} after --fast"), engine.build_snapshot()));
    }
    // The one benchmark board written in this language.
    let source =
        std::fs::read_to_string(repo().join("tests/fixtures/benchmark/esp32_starter.cypcb"))
            .expect("the fixture reads");
    let mut engine = loaded(&source);
    boards.push(("esp32_starter as drawn".into(), engine.build_snapshot()));
    let status = engine.auto_route();
    assert!(
        status.contains("\"ok\":true"),
        "esp32_starter routes: {status}"
    );
    boards.push(("esp32_starter after --fast".into(), engine.build_snapshot()));

    let mut open = 0;
    for (name, snapshot) in &boards {
        let drawn = snapshot.ratsnest.len();
        let missing = missing_by_check(snapshot);
        println!("{name:<32} ratsnest {drawn:>4}   check {missing:>4}");
        assert_eq!(
            drawn, missing,
            "{name}: ratsnest lines against connections check finds missing"
        );
        open += missing;
    }
    // The control: equal at zero everywhere would measure nothing.
    assert!(
        open > 0,
        "no board left a connection open, so the count compared nothing"
    );
}
