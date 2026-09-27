//! `cypcb library import` reports the rows it wrote, and names what it did not.
//!
//! `cargo test -p cypcb-cli --test an_import_counts_the_rows_it_writes`
//!
//! The index keys a footprint by source and name. A footprint that a second
//! library also holds was written over the first library's row with an
//! UPDATE, and the import still counted both: `Indexed 2 footprint(s)` over
//! one row, and the first library had lost its footprint without a word.

use std::path::{Path, PathBuf};
use std::process::Command;

use cypcb_library::LibraryManager;

fn footprint() -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/svg-pcb/kicad-components/SOT-23-5.kicad_mod");
    std::fs::read_to_string(&fixture).expect("the fixture footprint reads")
}

/// A fresh directory holding the given `.pretty/file` paths, each one the same
/// footprint, `SOT-23-5`.
fn libraries(case: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cypcb-cli-import-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for file in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).expect("a library folder");
        std::fs::write(&path, footprint()).expect("the footprint is written");
    }
    dir
}

/// Run the import; the text it printed, and the count on its `Indexed` line.
fn import(dir: &Path) -> (String, usize) {
    let out = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(["library", "import", "."])
        .current_dir(dir)
        .output()
        .expect("cypcb runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "{said}");
    let indexed = said
        .lines()
        .find_map(|line| line.strip_prefix("Indexed "))
        .and_then(|rest| rest.split(' ').next())
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("no `Indexed <n>` line in:\n{said}"));
    (said, indexed)
}

fn rows(dir: &Path) -> usize {
    LibraryManager::new(&dir.join("cypcb-library.db"))
        .expect("the index opens")
        .component_count()
        .expect("the index counts")
}

#[test]
fn a_name_another_library_holds_is_refused_and_named() {
    let dir = libraries(
        "two-libraries",
        &["A.pretty/SOT-23-5.kicad_mod", "B.pretty/SOT-23-5.kicad_mod"],
    );
    let (said, indexed) = import(&dir);

    assert_eq!(indexed, rows(&dir), "{said}");
    assert_eq!(indexed, 1, "{said}");
    assert!(
        said.contains(
            "'SOT-23-5' from B.pretty is not indexed: the name is already indexed from A.pretty"
        ),
        "{said}"
    );
    assert!(said.contains("1 footprint(s) not indexed"), "{said}");
    let kept = LibraryManager::new(&dir.join("cypcb-library.db"))
        .unwrap()
        .get_component("kicad", "SOT-23-5")
        .unwrap()
        .expect("the footprint is indexed");
    assert_eq!(kept.library, "A", "the first library keeps its footprint");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_name_twice_in_one_library_is_refused_and_named() {
    let dir = libraries(
        "one-library",
        &["A.pretty/SOT-23-5.kicad_mod", "A.pretty/copy.kicad_mod"],
    );
    let (said, indexed) = import(&dir);

    assert_eq!(indexed, rows(&dir), "{said}");
    assert_eq!(indexed, 1, "{said}");
    assert!(
        said.contains("'SOT-23-5' from A.pretty is not indexed: A.pretty holds another footprint by that name"),
        "{said}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn importing_a_library_again_rewrites_its_rows() {
    let dir = libraries("again", &["A.pretty/SOT-23-5.kicad_mod"]);
    let (_, first) = import(&dir);
    let (said, again) = import(&dir);

    assert_eq!((first, again), (1, 1), "{said}");
    assert_eq!(again, rows(&dir), "{said}");
    assert!(!said.contains("not indexed"), "{said}");

    let _ = std::fs::remove_dir_all(&dir);
}
