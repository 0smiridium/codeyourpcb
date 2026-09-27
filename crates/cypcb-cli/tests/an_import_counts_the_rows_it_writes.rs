//! `cypcb library import` reports the rows it wrote, and names what it did not.
//!
//! `cargo test -p cypcb-cli --test an_import_counts_the_rows_it_writes`
//!
//! The index keyed a footprint by source and name. A footprint that a second
//! library also holds was written over the first library's row with an
//! UPDATE, and the import still counted both: `Indexed 2 footprint(s)` over
//! one row, and the first library had lost its footprint without a word.
//! The key is source, library and name now, as KiCad keys a footprint, so
//! both libraries keep theirs. A re-import left the rows of files deleted
//! since, so the index held more than the import reported; they leave now.
//! A whole `.pretty` folder deleted left every row it had; its library leaves
//! now, when the directory that held it is imported again.

use std::path::{Path, PathBuf};
use std::process::Command;

use cypcb_library::LibraryManager;

/// The fixture footprint, called `name`.
fn footprint(name: &str) -> String {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/svg-pcb/kicad-components/SOT-23-5.kicad_mod");
    std::fs::read_to_string(&fixture)
        .expect("the fixture footprint reads")
        .replacen("(module SOT-23-5 ", &format!("(module {name} "), 1)
}

/// A fresh directory holding the given `.pretty/file` paths, each one the same
/// footprint, `SOT-23-5`.
fn libraries(case: &str, files: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cypcb-cli-import-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for file in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).expect("a library folder");
        std::fs::write(&path, footprint("SOT-23-5")).expect("the footprint is written");
    }
    dir
}

/// Run `cypcb` in `dir`; its exit code and what it printed.
fn cypcb(dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("cypcb runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), said)
}

/// Run the import; the text it printed, and the count on its `Indexed` line.
fn import(dir: &Path) -> (String, usize) {
    import_from(dir, ".")
}

/// [`import`] of `directory`, into the index in `dir`.
fn import_from(dir: &Path, directory: &str) -> (String, usize) {
    let (code, said) = cypcb(dir, &["library", "import", directory]);
    assert_eq!(code, Some(0), "{said}");
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

/// A board with one part, named `footprint`.
fn board(dir: &Path, footprint: &str) {
    std::fs::write(
        dir.join("board.cypcb"),
        format!(
            "version 1\n\nboard test {{\n    size 30mm x 30mm\n    layers 2\n}}\n\n\
             component U1 ic \"{footprint}\" {{\n    at 15mm, 15mm\n}}\n"
        ),
    )
    .expect("the board is written");
}

#[test]
fn two_libraries_each_keep_a_footprint_by_one_name() {
    let dir = libraries(
        "two-libraries",
        &["A.pretty/SOT-23-5.kicad_mod", "B.pretty/SOT-23-5.kicad_mod"],
    );
    let (said, indexed) = import(&dir);

    assert_eq!(indexed, rows(&dir), "{said}");
    assert_eq!(indexed, 2, "{said}");
    assert!(!said.contains("not indexed"), "{said}");

    let (_, found) = cypcb(&dir, &["library", "search", "SOT-23-5"]);
    assert!(
        found.contains("kicad::A:SOT-23-5") && found.contains("kicad::B:SOT-23-5"),
        "search prints each one in full:\n{found}"
    );

    board(&dir, "kicad::B:SOT-23-5");
    let (_, checked) = cypcb(&dir, &["check", "board.cypcb"]);
    assert!(
        !checked.contains("footprint"),
        "the full name resolves:\n{checked}"
    );

    board(&dir, "kicad::SOT-23-5");
    let (code, checked) = cypcb(&dir, &["check", "board.cypcb"]);
    assert_eq!(code, Some(1), "{checked}");
    // The report wraps its lines inside a frame; join them back.
    let unwrapped: String = checked
        .lines()
        .map(|line| line.trim_start_matches(|c: char| c.is_whitespace() || "│×".contains(c)))
        .map(str::trim_end)
        .collect();
    assert!(
        unwrapped.contains("footprint 'kicad::SOT-23-5' is in more than one library")
            && unwrapped.contains("kicad::A:SOT-23-5")
            && unwrapped.contains("kicad::B:SOT-23-5"),
        "a bare name two libraries hold is refused with both in full:\n{checked}"
    );

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
    assert!(said.contains("1 footprint(s) not indexed"), "{said}");

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

#[test]
fn a_file_deleted_since_the_last_import_leaves_the_index() {
    let dir = libraries("deleted", &["A.pretty/SOT-23-5.kicad_mod"]);
    std::fs::write(
        dir.join("A.pretty/SOT-23-6.kicad_mod"),
        footprint("SOT-23-6"),
    )
    .expect("a second footprint");
    let (said, first) = import(&dir);
    assert_eq!((first, rows(&dir)), (2, 2), "{said}");

    std::fs::remove_file(dir.join("A.pretty/SOT-23-6.kicad_mod")).expect("the file goes");
    let (said, again) = import(&dir);

    assert_eq!(again, rows(&dir), "{said}");
    assert_eq!(again, 1, "{said}");
    assert!(
        said.contains("A: 1 footprint(s) removed from the index: their files are gone"),
        "{said}"
    );
    let gone = LibraryManager::new(&dir.join("cypcb-library.db"))
        .unwrap()
        .get_component("kicad", "A:SOT-23-6")
        .unwrap();
    assert!(
        gone.is_none(),
        "the deleted file's footprint is not in the index"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_folder_deleted_since_the_last_import_leaves_the_index() {
    let dir = libraries(
        "folder-deleted",
        &["A.pretty/SOT-23-5.kicad_mod", "B.pretty/SOT-23-5.kicad_mod"],
    );
    std::fs::write(
        dir.join("B.pretty/SOT-23-6.kicad_mod"),
        footprint("SOT-23-6"),
    )
    .expect("a second footprint");
    let (said, first) = import(&dir);
    assert_eq!((first, rows(&dir)), (3, 3), "{said}");

    std::fs::remove_dir_all(dir.join("B.pretty")).expect("the folder goes");
    let (said, again) = import(&dir);

    assert_eq!(again, rows(&dir), "{said}");
    assert_eq!(again, 1, "{said}");
    assert!(
        said.contains("B: 2 footprint(s) removed from the index: its folder B.pretty is gone"),
        "{said}"
    );
    let (_, found) = cypcb(&dir, &["library", "search", "SOT-23"]);
    assert!(
        found.contains("kicad::A:SOT-23-5") && !found.contains("kicad::B:"),
        "{found}"
    );
    let (_, listed) = cypcb(&dir, &["library", "list"]);
    assert!(
        listed.contains("A (kicad)") && !listed.contains("B (kicad)"),
        "the library is gone too:\n{listed}"
    );

    std::fs::remove_dir_all(dir.join("A.pretty")).expect("the last folder goes");
    let (code, said) = cypcb(&dir, &["library", "import", "."]);
    assert_eq!(code, Some(0), "{said}");
    assert!(
        said.contains("A: 1 footprint(s) removed from the index: its folder A.pretty is gone"),
        "{said}"
    );
    assert_eq!(rows(&dir), 0, "{said}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// One index can hold libraries from two directories. Importing one of them
/// says nothing about the folders of the other.
#[test]
fn a_folder_gone_from_another_directory_stays_until_that_one_is_imported() {
    let dir = libraries(
        "two-directories",
        &[
            "one/A.pretty/SOT-23-5.kicad_mod",
            "two/B.pretty/SOT-23-5.kicad_mod",
        ],
    );
    import_from(&dir, "one");
    import_from(&dir, "two");
    assert_eq!(rows(&dir), 2);

    std::fs::remove_dir_all(dir.join("one/A.pretty")).expect("the folder goes");
    let (said, _) = import_from(&dir, "two");
    assert!(!said.contains("removed"), "{said}");
    assert_eq!(rows(&dir), 2, "{said}");

    let (code, said) = cypcb(&dir, &["library", "import", "one"]);
    assert_eq!(code, Some(0), "{said}");
    assert!(
        said.contains("A: 1 footprint(s) removed from the index: its folder A.pretty is gone"),
        "{said}"
    );
    assert_eq!(rows(&dir), 1, "{said}");

    let _ = std::fs::remove_dir_all(&dir);
}
