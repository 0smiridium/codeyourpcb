//! No writer reads the clock or the environment.
//!
//! `cargo test -p cypcb-export --test no_writer_reads_the_clock`
//!
//! Each stamped file took its time from a function that read
//! `SOURCE_DATE_EPOCH` itself, and one that met a bad value could only panic:
//! a writer returns a `String`, not an error. A writer called outside the
//! export job, as the renderer's tests call them, had no other way to fail.
//! The time is now a value: the CLI reads it once with `stamp::export_time`,
//! stops on a bad one before a file is written, and hands it to each writer as
//! an argument or in `ExportJob`. This test holds that shape by reading the
//! source, so a writer that goes back to the clock fails here and not in a
//! fabricator's diff.

use std::path::{Path, PathBuf};

/// What reads the time or the environment.
const CLOCK: &[&str] = &["Utc::now", "Local::now", "SystemTime::now", "env::var"];

/// The one file of the export crate that may read them.
const STAMP: &str = "crates/cypcb-export/src/stamp.rs";

/// Where `export_time` may be called: the way in, and its own definition.
const ENTRY: &[&str] = &["crates/cypcb-cli/src/", STAMP];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels below the repo root")
        .to_path_buf()
}

/// Every line of code, as (path from the root, line number, text), in each
/// `.rs` file under `dir`. Comment lines are left out: a comment that names
/// the clock does not read it.
fn code_lines(dir: &Path, out: &mut Vec<(String, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            code_lines(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).expect("a source file reads");
            let name = path
                .strip_prefix(root())
                .expect("under the root")
                .to_string_lossy()
                .into_owned();
            for (number, line) in text.lines().enumerate() {
                if !line.trim_start().starts_with("//") {
                    out.push((name.clone(), number + 1, line.to_string()));
                }
            }
        }
    }
}

fn every_crate_source() -> Vec<(String, usize, String)> {
    let mut lines = Vec::new();
    for dir in std::fs::read_dir(root().join("crates")).expect("the crates are there") {
        code_lines(&dir.expect("a crate").path().join("src"), &mut lines);
    }
    lines
}

#[test]
fn only_the_stamp_reads_the_clock_in_the_exporters() {
    let lines = every_crate_source();
    let reads: Vec<&(String, usize, String)> = lines
        .iter()
        .filter(|(path, _, line)| {
            path.starts_with("crates/cypcb-export/src/")
                && CLOCK.iter().any(|clock| line.contains(clock))
        })
        .collect();
    // The control: the scan sees the one read there is.
    assert!(
        reads.iter().any(|(path, _, _)| path == STAMP),
        "the scan found no clock read in {STAMP}, so it cannot find one elsewhere"
    );
    let elsewhere: Vec<String> = reads
        .iter()
        .filter(|(path, _, _)| path != STAMP)
        .map(|(path, number, line)| format!("{path}:{number}: {}", line.trim()))
        .collect();
    assert!(
        elsewhere.is_empty(),
        "an exporter reads the clock or the environment instead of taking the stamp:\n{}",
        elsewhere.join("\n")
    );
}

#[test]
fn only_the_way_in_reads_the_export_time() {
    let lines = every_crate_source();
    let calls: Vec<&(String, usize, String)> = lines
        .iter()
        .filter(|(_, _, line)| line.contains("export_time("))
        .collect();
    // The control: the scan sees the CLI's call.
    assert!(
        calls
            .iter()
            .any(|(path, _, _)| path.starts_with("crates/cypcb-cli/src/")),
        "the scan found no call in the CLI, so it cannot find one elsewhere"
    );
    let elsewhere: Vec<String> = calls
        .iter()
        .filter(|(path, _, _)| !ENTRY.iter().any(|entry| path.starts_with(entry)))
        .map(|(path, number, line)| format!("{path}:{number}: {}", line.trim()))
        .collect();
    assert!(
        elsewhere.is_empty(),
        "a writer reads the export time instead of taking it as a value:\n{}",
        elsewhere.join("\n")
    );
}
