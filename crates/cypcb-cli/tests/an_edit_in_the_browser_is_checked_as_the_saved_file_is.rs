//! After an edit the browser's checker says what `cypcb check` says about the
//! design saved and read back.
//!
//! `cargo test -p cypcb-cli --test an_edit_in_the_browser_is_checked_as_the_saved_file_is`
//!
//! The engine rebuilt its index after every edit with a builder of its own,
//! which indexed parts on no copper layer. A loaded board was checked right,
//! and the first trace drawn made every pad invisible to the clearance rule:
//! the live check went quiet exactly when the design started to change. The
//! two paths agreed on a board nobody had touched, so `the_two_paths_count_the_same`
//! could not see it. This holds them together after each edit the engine
//! offers, on every benchmark board.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use cypcb_render::PcbEngine;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels below the repo root")
        .to_path_buf()
}

fn benchmark_boards() -> Vec<PathBuf> {
    let mut boards: Vec<PathBuf> = std::fs::read_dir(repo_root().join("tests/fixtures/benchmark"))
        .expect("the benchmark boards are there")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "cypcb" || ext == "kicad_pcb")
        })
        .collect();
    boards.sort();
    boards
}

fn load(board: &Path) -> PcbEngine {
    let source = std::fs::read_to_string(board).expect("a readable board");
    let mut engine = PcbEngine::new();
    let errors = if board.extension().is_some_and(|ext| ext == "cypcb") {
        engine.load_source(&source)
    } else {
        engine.load_kicad(&source)
    };
    assert!(errors.is_empty(), "{}: {errors}", board.display());
    engine
}

/// Rows of the engine's report, by kind.
fn what_the_engine_found(engine: &PcbEngine) -> BTreeMap<String, usize> {
    let rows: serde_json::Value =
        serde_json::from_str(&engine.get_violations_json()).expect("the report is JSON");
    let mut counts = BTreeMap::new();
    for row in rows.as_array().expect("the report is a list") {
        let kind = row["kind"].as_str().expect("a row names its kind");
        *counts.entry(kind.to_string()).or_insert(0) += 1;
    }
    counts
}

/// What `cypcb check` finds in the design as the engine would save it.
fn what_the_command_found_saved(
    engine: &mut PcbEngine,
    dir: &Path,
    name: &str,
) -> BTreeMap<String, usize> {
    // A routed trace saved as one drawn by hand is the same copper to the
    // checker; anything else left out would make the two boards differ.
    let not_written = engine.design_not_written();
    let lost: Vec<&str> = not_written
        .lines()
        .filter(|line| !line.ends_with("written as drawn by hand"))
        .collect();
    assert!(
        lost.is_empty(),
        "{name}: the saved design is not the whole board, so the two cannot be compared: {lost:?}"
    );
    let saved = dir.join(format!("{name}.cypcb"));
    std::fs::write(&saved, engine.design_as_dsl()).expect("the design is written");
    let output = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .arg("check")
        .arg("-o")
        .arg("json")
        .arg(&saved)
        .output()
        .expect("the binary runs");
    let said = String::from_utf8_lossy(&output.stdout).to_string();
    let report: serde_json::Value = serde_json::from_str(said.trim()).unwrap_or_else(|_| {
        panic!(
            "{name}: check did not report:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    let mut counts = BTreeMap::new();
    for (kind, count) in report["summary"].as_object().expect("a summary") {
        counts.insert(kind.clone(), count.as_u64().expect("a count") as usize);
    }
    counts
}

/// Where the edits draw: from the first part to the second, across whatever
/// copper lies between them.
struct Stroke {
    net: String,
    part: String,
    flat: [i64; 4],
}

fn stroke(engine: &mut PcbEngine) -> Stroke {
    let snapshot: serde_json::Value =
        serde_json::from_str(&engine.get_snapshot()).expect("the snapshot is JSON");
    let parts = &snapshot["components"];
    let at = |i: usize, axis: &str| parts[i][axis].as_i64().expect("a part has a place");
    Stroke {
        net: snapshot["nets"][0]["name"]
            .as_str()
            .expect("the board has a net")
            .to_string(),
        part: parts[0]["refdes"]
            .as_str()
            .expect("a part has a name")
            .to_string(),
        flat: [at(0, "x_nm"), at(0, "y_nm"), at(1, "x_nm"), at(1, "y_nm")],
    }
}

#[test]
fn every_edit_is_checked_as_the_saved_design_is() {
    let dir = cypcb_fixtures::scratch_dir("cypcb-an-edit-is-checked-as-saved");
    let boards = benchmark_boards();
    assert!(
        boards.len() >= 7,
        "only {} benchmark boards, so this proves little",
        boards.len()
    );

    let mut disagreements: Vec<String> = Vec::new();
    let mut clearance_rows_after_edits = 0;
    let mut compare = |engine: &mut PcbEngine, name: String, edited: bool| {
        let engine_found = what_the_engine_found(engine);
        let command_found = what_the_command_found_saved(engine, &dir, &name);
        if edited {
            clearance_rows_after_edits += command_found.get("clearance").copied().unwrap_or(0);
        }
        if engine_found != command_found {
            disagreements.push(format!(
                "{name}: the command says {command_found:?} and the engine says {engine_found:?}"
            ));
        }
    };

    for board in &boards {
        let name = board
            .file_stem()
            .expect("a name")
            .to_string_lossy()
            .to_string();

        let mut engine = load(board);
        let at = stroke(&mut engine);
        compare(&mut engine, format!("{name}-loaded"), false);

        let id = engine.add_trace(&at.net, "Top", 200_000, &at.flat);
        assert_ne!(id, u32::MAX, "{name}: the trace is drawn");
        engine.run_drc_incremental();
        compare(&mut engine, format!("{name}-trace-drawn"), true);

        assert!(engine.remove_trace(id), "{name}: the trace is removed");
        engine.run_drc_incremental();
        compare(&mut engine, format!("{name}-trace-removed"), true);

        assert!(
            engine.rotate_component(&at.part, 90_000),
            "{name}: the part turns"
        );
        compare(&mut engine, format!("{name}-part-turned"), true);

        let mut engine = load(board);
        let [x1, y1, x2, y2] = at.flat;
        let errors = engine.load_routes(&format!("segment 1 Top 200000 {x1} {y1} {x2} {y2}\n"));
        assert!(errors.is_empty(), "{name}: the routes load: {errors}");
        engine.run_drc_incremental();
        compare(&mut engine, format!("{name}-routes-loaded"), true);
    }

    // Without copper close enough to measure, agreement proves nothing about
    // the clearance rule, which is the one the second builder silenced.
    assert!(
        clearance_rows_after_edits > 0,
        "no edit put copper near copper"
    );
    assert!(
        disagreements.is_empty(),
        "after an edit the browser and the saved design are checked differently:\n{}",
        disagreements.join("\n")
    );
}
