//! `cypcb check`, the language server and the browser's engine check a board
//! against the same fab table, and find the same things. `cypcb route` and
//! `cypcb score` name the same table for it too.
//!
//! `cargo test -p cypcb-cli --test three_surfaces_check_against_one_table`
//!
//! Each surface chose its table on its own. The command line picked the table
//! for the board's layer count and the two editors did not, so a four-layer
//! board was checked against `jlcpcb_standard_4layer` by `check` and against
//! `jlcpcb_standard_2layer` in both editors: a 0.11mm gap was clean on the
//! command line and an error in the editor. The choice is one function now,
//! `cypcb_drc::table_for`, and this is what holds the three to it.
//!
//! `route` and `score` went through the same function and printed no table,
//! so nothing held them to it. Both name it now. `score` grades the board it
//! is given, so its counts are held to `check`'s as well; `route` adds copper
//! first, so only its table is.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use cypcb_lsp::document::DocumentState;
use cypcb_render::PcbEngine;

/// Two traces 0.11mm apart: inside JLCPCB's two-layer clearance (0.127mm) and
/// outside its four-layer one (0.1mm), so the table used is readable off the
/// clearance count.
fn a_board(layers: u8, fab: Option<&str>) -> String {
    let fab_line = fab.map_or(String::new(), |fab| format!("    fab {fab}\n"));
    format!(
        "version 1\n\n\
         board t {{\n    size 20mm x 20mm\n    layers {layers}\n{fab_line}}}\n\n\
         component R1 resistor \"0402\" {{\n    value \"10k\"\n    at 5mm, 5mm\n}}\n\n\
         component R2 resistor \"0402\" {{\n    value \"10k\"\n    at 15mm, 5mm\n}}\n\n\
         net A {{\n    R1.1\n    R2.1\n}}\n\n\
         net B {{\n    R1.2\n    R2.2\n}}\n\n\
         trace A {{\n    layer Top\n    width 0.127mm\n    path 5mm,10mm -> 15mm,10mm\n}}\n\n\
         trace B {{\n    layer Top\n    width 0.127mm\n    path 5mm,10.237mm -> 15mm,10.237mm\n}}\n"
    )
}

/// What one surface said: the table it named and its rows by kind.
#[derive(Debug, PartialEq, Eq)]
struct Verdict {
    table: String,
    counts: BTreeMap<String, usize>,
}

fn the_command(board: &Path) -> Verdict {
    let output = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(["check", "-o", "json"])
        .arg(board)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&output.stdout).to_string();
    let report: serde_json::Value =
        serde_json::from_str(said.trim()).unwrap_or_else(|e| panic!("{e}: {said}"));
    let mut counts = BTreeMap::new();
    for (kind, count) in report["summary"].as_object().expect("a summary") {
        counts.insert(kind.clone(), count.as_u64().expect("a count") as usize);
    }
    Verdict {
        table: report["preset"]
            .as_str()
            .expect("the table it used")
            .to_string(),
        counts,
    }
}

/// The table `route` names in its DRC line, and the violations it counts.
fn the_router(board: &Path) -> (String, usize) {
    let routed = board.with_extension("routed.cypcb");
    let output = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(["route", "--in-house"])
        .arg(board)
        .arg("-o")
        .arg(&routed)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&output.stderr).to_string();
    let line = said
        .lines()
        .find_map(|line| line.strip_prefix("DRC on the routed board: "))
        .unwrap_or_else(|| panic!("route printed no DRC line:\n{said}"));
    let (count, rest) = line
        .split_once(" violations against ")
        .unwrap_or_else(|| panic!("route did not name its table: {line}"));
    let table = rest.split(',').next().expect("a table name").to_string();
    (table, count.parse().expect("a violation count"))
}

/// The table `score` names, its row count and its clearance contacts.
fn the_scorer(board: &Path) -> (String, usize, usize) {
    let output = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .arg("score")
        .arg(board)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&output.stdout).to_string();
    let score: serde_json::Value =
        serde_json::from_str(said.trim()).unwrap_or_else(|e| panic!("{e}: {said}"));
    let number = |key: &str| score[key].as_u64().expect(key) as usize;
    (
        score["preset"]
            .as_str()
            .expect("the table it scored against")
            .to_string(),
        number("drc_violations"),
        number("clearance_contacts"),
    )
}

fn the_language_server(source: &str) -> Verdict {
    let mut doc = DocumentState::new("test://three-surfaces".into(), source.to_string(), 1);
    doc.parse();
    assert!(doc.parse_errors.is_empty(), "{:?}", doc.parse_errors);
    assert!(doc.build_world(), "{:?}", doc.sync_errors);
    let mut counts = BTreeMap::new();
    for violation in &doc.drc_violations {
        *counts.entry(violation.kind.to_string()).or_insert(0) += 1;
    }
    Verdict {
        table: doc
            .checked_against
            .expect("the server says what it checked against")
            .name()
            .to_string(),
        counts,
    }
}

fn the_engine(source: &str) -> Verdict {
    let mut engine = PcbEngine::new();
    let errors = engine.load_source(source);
    assert!(errors.is_empty(), "{errors:?}");
    let rows: serde_json::Value =
        serde_json::from_str(&engine.get_violations_json()).expect("the rows are JSON");
    let mut counts = BTreeMap::new();
    for row in rows.as_array().expect("a list of rows") {
        let kind = row["kind"].as_str().expect("a kind").to_string();
        *counts.entry(kind).or_insert(0) += 1;
    }
    Verdict {
        table: engine.drc_table(),
        counts,
    }
}

#[test]
fn the_three_surfaces_check_one_board_against_one_table() {
    let dir = cypcb_fixtures::scratch_dir("cypcb-three-surfaces");
    let mut clearance_by_table: BTreeMap<String, usize> = BTreeMap::new();
    let mut route_by_table: BTreeMap<String, usize> = BTreeMap::new();

    for layers in [2u8, 4] {
        for fab in [None, Some("jlcpcb"), Some("oshpark")] {
            let source = a_board(layers, fab);
            let board = dir.join(format!("{layers}-{}.cypcb", fab.unwrap_or("none")));
            std::fs::write(&board, &source).expect("the board is written");

            let command = the_command(&board);
            let server = the_language_server(&source);
            let engine = the_engine(&source);
            let case = format!("layers {layers}, fab {fab:?}");
            assert_eq!(server, command, "{case}: the server and `check` disagree");
            assert_eq!(engine, command, "{case}: the engine and `check` disagree");

            let (scored_against, rows, contacts) = the_scorer(&board);
            assert_eq!(
                (scored_against.as_str(), rows, contacts),
                (
                    command.table.as_str(),
                    command.counts.values().sum(),
                    command.counts.get("clearance").copied().unwrap_or(0)
                ),
                "{case}: `score` and `check` disagree"
            );

            let (routed_against, routed_rows) = the_router(&board);
            assert_eq!(
                routed_against, command.table,
                "{case}: `route` and `check` disagree"
            );
            route_by_table.insert(command.table.clone(), routed_rows);

            clearance_by_table.insert(
                command.table.clone(),
                command.counts.get("clearance").copied().unwrap_or(0),
            );
        }
    }

    // The control: the board tells the two JLCPCB tables apart. Without it a
    // surface stuck on either table could agree with the others by accident.
    assert_eq!(
        clearance_by_table.get("jlcpcb_standard_2layer"),
        Some(&1),
        "{clearance_by_table:?}"
    );
    assert_eq!(
        clearance_by_table.get("jlcpcb_standard_4layer"),
        Some(&0),
        "{clearance_by_table:?}"
    );
    assert_eq!(clearance_by_table.len(), 4, "{clearance_by_table:?}");
    // The same control for `route`, whose table is only named: the routed
    // board keeps the 0.11mm gap, so the count moves with the table.
    assert!(
        route_by_table.get("jlcpcb_standard_2layer") > route_by_table.get("jlcpcb_standard_4layer"),
        "{route_by_table:?}"
    );
}
