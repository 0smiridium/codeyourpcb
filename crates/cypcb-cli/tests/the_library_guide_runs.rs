//! The library guide, run rather than read.
//!
//! `cargo test -p cypcb-cli --test the_library_guide_runs`
//!
//! The guide said a design names an imported footprint as `kicad::<name>`.
//! Followed step by step, `cypcb library import` indexed the footprint,
//! `cypcb library search` found it, and `cypcb check` refused the design that
//! named it with `unknown footprint`: nothing but the `library` command read
//! the index. The steps it showed were calls into a Rust API that did not
//! exist.
//!
//! What this holds: the commands and the design are read out of the guide,
//! run in an empty directory holding one `.pretty` folder from the fixtures,
//! and each command exits and prints what its comment in the guide says.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the crate sits two levels below the repo root")
        .to_path_buf()
}

fn guide() -> String {
    std::fs::read_to_string(repo_root().join("docs/user-guide/library-management.md"))
        .expect("the guide is where it was")
}

/// The body of the first fenced block of `language` after `heading`.
fn block_after(text: &str, heading: &str, language: &str) -> String {
    let from = text
        .find(heading)
        .unwrap_or_else(|| panic!("the guide has no {heading:?}"));
    let open = format!("```{language}\n");
    let start = text[from..]
        .find(&open)
        .map(|at| from + at + open.len())
        .unwrap_or_else(|| panic!("no ```{language} block under {heading:?}"));
    let end = text[start..]
        .find("```")
        .map(|at| start + at)
        .expect("the block is closed");
    text[start..end].to_string()
}

struct Step {
    args: Vec<String>,
    exit: i32,
    prints: Option<String>,
}

/// `cypcb check board.cypcb   # exit 1, prints: Unconnected pin: R1.1`
fn steps(text: &str) -> Vec<Step> {
    block_after(text, "## Use a KiCad footprint by name", "sh")
        .lines()
        .filter_map(|line| line.strip_prefix("cypcb "))
        .map(|rest| {
            let (command, claim) = rest.split_once('#').unwrap_or((rest, ""));
            let exit = claim
                .split_once("exit ")
                .and_then(|(_, tail)| tail.split(|c: char| !c.is_ascii_digit()).next())
                .and_then(|digits| digits.parse().ok())
                .unwrap_or(0);
            let prints = claim
                .split_once("prints: ")
                .map(|(_, said)| said.trim().to_string());
            Step {
                args: command.split_whitespace().map(str::to_string).collect(),
                exit,
                prints,
            }
        })
        .collect()
}

/// An empty project holding the one library the guide imports.
fn project(who: &str) -> cypcb_fixtures::ScratchDir {
    let dir = cypcb_fixtures::scratch_dir(&format!("cypcb-library-guide-{who}"));
    let from = repo_root().join("tests/fixtures/kicad-tools/tests/fixtures/Test_Library.pretty");
    let to = dir.join("libraries/Test_Library.pretty");
    std::fs::create_dir_all(&to).expect("a place to work");
    let files = cypcb_fixtures::tree::tracked_in(&from);
    assert!(!files.is_empty(), "the fixture library is there");
    for path in files {
        let name = path.file_name().expect("a file name");
        std::fs::copy(&path, to.join(name)).expect("a file to copy");
    }
    std::fs::write(
        dir.join("board.cypcb"),
        block_after(&guide(), "## Use a KiCad footprint by name", "cypcb"),
    )
    .expect("the design is written");
    dir
}

fn run(args: &[String], cwd: &Path) -> (Option<i32>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|err| panic!("`cypcb {}` did not run: {err}", args.join(" ")));
    let said = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    (output.status.code(), said)
}

#[test]
fn every_step_the_guide_lists_runs_and_ends_the_way_it_says() {
    let steps = steps(&guide());
    let verbs: Vec<String> = steps
        .iter()
        .map(|step| {
            step.args
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    for wanted in ["library import", "library search", "check board.cypcb"] {
        assert!(
            verbs.iter().any(|verb| verb == wanted),
            "the guide's steps have to import, search and check a design: {verbs:?}"
        );
    }

    let dir = project("steps");
    for step in &steps {
        let (code, said) = run(&step.args, &dir);
        assert_eq!(
            code,
            Some(step.exit),
            "`cypcb {}` is in the guide as exiting {} and exited {code:?}:\n{said}",
            step.args.join(" "),
            step.exit
        );
        assert!(
            !said.contains("unknown footprint"),
            "`cypcb {}` refused the footprint the guide imported:\n{said}",
            step.args.join(" ")
        );
        if let Some(prints) = &step.prints {
            assert!(
                said.contains(prints.as_str()),
                "the guide says `cypcb {}` prints {prints:?}:\n{said}",
                step.args.join(" ")
            );
        }
    }
}

/// The guide's name for the part is the name `search` prints, so a person
/// who copies one into the other gets a design that loads.
#[test]
fn the_design_names_the_footprint_the_way_search_prints_it() {
    let text = guide();
    let design = block_after(&text, "## Use a KiCad footprint by name", "cypcb");
    let searched = steps(&text)
        .into_iter()
        .find(|step| step.args.get(1).map(String::as_str) == Some("search"))
        .and_then(|step| step.prints)
        .expect("the guide says what search prints");
    assert!(
        design.contains(&format!("\"{searched}\"")),
        "search prints {searched:?} and the design does not name it:\n{design}"
    );
}

/// The index is what resolves the name: the same design with no index beside
/// it or above it is refused, and one in a folder below the index loads.
#[test]
fn the_design_reads_the_nearest_index_and_nothing_else() {
    let dir = project("where");
    let (code, said) = run(
        &["library".into(), "import".into(), "libraries".into()],
        &dir,
    );
    assert_eq!(code, Some(0), "{said}");

    let below = dir.join("boards/rev_a");
    std::fs::create_dir_all(&below).expect("a place to work");
    std::fs::copy(dir.join("board.cypcb"), below.join("board.cypcb")).expect("a copy");
    let (_, said) = run(&["check".into(), "boards/rev_a/board.cypcb".into()], &dir);
    assert!(
        said.contains("Unconnected pin: R1.1"),
        "a design below the index resolves the part from it:\n{said}"
    );

    let alone = cypcb_fixtures::scratch_dir("cypcb-library-guide-alone");
    std::fs::copy(dir.join("board.cypcb"), alone.join("board.cypcb")).expect("a copy");
    let (code, said) = run(&["check".into(), "board.cypcb".into()], &alone);
    assert_eq!(code, Some(1), "{said}");
    assert!(
        said.contains("unknown footprint: 'kicad::Test_Library:R_0603_1608Metric'")
            && said.contains("cypcb library import"),
        "with no index the part is refused, and the refusal says where the name \
         is looked for:\n{said}"
    );
}
