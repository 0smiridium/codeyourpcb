//! Are two exports of one board the same bytes when `SOURCE_DATE_EPOCH` is set?
//!
//! `cargo test -p cypcb-cli --test two_exports_with_one_source_date_are_the_same_bytes`
//!
//! Every file an export writes carries the moment it was written, so two
//! exports of one board a second apart differed in every gerber, the drill
//! file, the job file and the assembly JSON, by that line alone. The
//! reproducible-builds.org specification names the variable a build sets to
//! fix that moment, and says a malformed value should stop the build. This
//! exports each benchmark board twice with every format asked for and one
//! value set, and compares the files byte for byte. It also holds the other
//! two cases: unset, the stamp is the clock's; malformed, nothing is written.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 2023-11-14T22:13:20 UTC, as `date -u -d @1700000000` prints it.
const EPOCH: &str = "1700000000";
const STAMPED: &str = "TF.CreationDate,2023-11-14T22:13:20+0000";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels below the repo root")
        .to_path_buf()
}

/// Every board in the benchmark directory.
fn benchmark_boards() -> Vec<PathBuf> {
    let dir = repo_root().join("tests/fixtures/benchmark");
    let mut boards: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("the benchmark directory is there")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "cypcb" || ext == "kicad_pcb")
        })
        .collect();
    boards.sort();
    assert!(!boards.is_empty(), "no board under {}", dir.display());
    boards
}

/// Export `board` to `out` with every format, `SOURCE_DATE_EPOCH` set to
/// `epoch` or removed.
fn export(board: &Path, out: &Path, epoch: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cypcb"));
    command.arg("export").arg(board).arg("-o").arg(out).args([
        "--ipc2581",
        "--ipc356",
        "--svg",
        "--dxf",
        "--pdf",
        "--force",
    ]);
    match epoch {
        Some(value) => command.env("SOURCE_DATE_EPOCH", value),
        None => command.env_remove("SOURCE_DATE_EPOCH"),
    };
    command.output().expect("the binary runs")
}

fn exported(board: &Path, out: &Path, epoch: Option<&str>) {
    let output = export(board, out, epoch);
    assert!(
        output.status.success(),
        "exporting {} failed:\n{}",
        board.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every file under `dir`, by its path relative to `dir`.
fn files_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path).expect("a readable export");
                found.insert(path.strip_prefix(dir).unwrap().to_path_buf(), bytes);
            }
        }
    }
    found
}

/// The first gerber's `TF.CreationDate` value under `dir`.
fn gerber_stamp(files: &BTreeMap<PathBuf, Vec<u8>>) -> String {
    let (_, bytes) = files
        .iter()
        .find(|(path, _)| path.extension().is_some_and(|ext| ext == "gbr"))
        .expect("an export writes a gerber");
    let text = String::from_utf8_lossy(bytes);
    text.lines()
        .find_map(|line| line.strip_prefix("G04 #@! TF.CreationDate,"))
        .expect("a gerber carries TF.CreationDate")
        .trim_end_matches('*')
        .to_string()
}

#[test]
fn two_exports_with_one_value_are_the_same_bytes() {
    for board in benchmark_boards() {
        let dir = cypcb_fixtures::scratch_dir("cypcb-source-date-same");
        let (first, second) = (dir.join("first"), dir.join("second"));
        exported(&board, &first, Some(EPOCH));
        // A second apart, so a stamp taken from the clock would differ.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        exported(&board, &second, Some(EPOCH));

        let (first, second) = (files_under(&first), files_under(&second));
        assert!(!first.is_empty(), "{} wrote no file", board.display());
        assert_eq!(
            first.keys().collect::<Vec<_>>(),
            second.keys().collect::<Vec<_>>(),
            "{}: the two exports wrote different files",
            board.display()
        );
        for (path, bytes) in &first {
            assert!(
                &second[path] == bytes,
                "{}: {} differs between two exports with one SOURCE_DATE_EPOCH",
                board.display(),
                path.display()
            );
        }
        let text: String = first
            .values()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .collect();
        assert!(
            text.contains(STAMPED),
            "{}: the gerbers carry the variable's moment",
            board.display()
        );
        assert!(
            text.contains("\"export_date\": \"2023-11-14T22:13:20+00:00\""),
            "{}: so does the assembly JSON",
            board.display()
        );
        assert!(
            text.contains("\"CreationDate\": \"2023-11-14T22:13:20+0000\""),
            "{}: so does the job file",
            board.display()
        );
        assert!(
            text.contains("; #@! TF.CreationDate,2023-11-14T22:13:20+0000"),
            "{}: so does a drill file",
            board.display()
        );
        assert!(
            text.contains("origination=\"2023-11-14T22:13:20+0000\""),
            "{}: so does the IPC-2581 document",
            board.display()
        );
    }
}

#[test]
fn unset_the_stamp_is_the_clock() {
    let board = &benchmark_boards()[0];
    let dir = cypcb_fixtures::scratch_dir("cypcb-source-date-unset");
    let before = chrono::Utc::now().timestamp();
    exported(board, &dir, None);
    let after = chrono::Utc::now().timestamp();

    let stamp = gerber_stamp(&files_under(&dir));
    let at = chrono::DateTime::parse_from_str(&stamp, "%Y-%m-%dT%H:%M:%S%z")
        .unwrap_or_else(|e| panic!("{stamp:?} is not a time: {e}"))
        .timestamp();
    assert!(
        (before..=after).contains(&at),
        "the stamp {stamp} is not the clock's time between {before} and {after}"
    );
}

#[test]
fn a_malformed_value_stops_the_export_before_it_writes_a_file() {
    let board = &benchmark_boards()[0];
    for value in ["", "1.5", "+1", "soon", "99999999999999999999"] {
        let dir = cypcb_fixtures::scratch_dir("cypcb-source-date-malformed");
        let output = export(board, &dir, Some(value));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{value:?} was accepted");
        assert!(
            stderr.contains(&format!("SOURCE_DATE_EPOCH={value:?} is not a time")),
            "{value:?}: the error names the variable and its value:\n{stderr}"
        );
        assert!(
            !stderr.contains("panicked"),
            "{value:?}: the export checks before a writer does:\n{stderr}"
        );
        let written = files_under(&dir);
        assert!(
            written.is_empty(),
            "{value:?}: files were written: {:?}",
            written.keys()
        );
    }
}
