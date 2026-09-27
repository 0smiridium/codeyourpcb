//! A footprint from the index resolves in the editor as it does in `check`.
//!
//! `cargo test -p cypcb-lsp --test the_editor_reads_the_library_index`
//!
//! The language server built its footprint table from the built-ins alone, so
//! a design naming `kicad::R_0603_1608Metric` after `cypcb library import` was
//! underlined as an unknown footprint in the editor while the command line
//! accepted it, and hovering the name called it a typo.
//!
//! The board writes the bare name, which still resolves while one library
//! alone holds it; the editor shows and offers it in full,
//! `kicad::Test_Library:R_0603_1608Metric`.

use std::path::{Path, PathBuf};

use cypcb_library::{LibraryManager, SearchFilters};
use cypcb_lsp::completion::completion_at_position;
use cypcb_lsp::document::{DocumentState, Position};
use cypcb_lsp::hover::hover_at_position;

const BOARD: &str = r#"version 1

board test {
    size 30mm x 30mm
    layers 2
}

component R1 resistor "kicad::R_0603_1608Metric" {
    value "330"
    at 15mm, 15mm
}
"#;

fn fixture_library() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/kicad-tools/tests/fixtures")
        .canonicalize()
        .expect("the fixture libraries are there")
}

/// A project directory with the fixture library imported into its index and
/// the board written into it; `with_index` false leaves the index out.
fn project(tag: &str, with_index: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cypcb-lsp-index-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a place to work");
    if with_index {
        let mut manager = LibraryManager::new(&dir.join("cypcb-library.db")).expect("an index");
        manager.add_kicad_search_path(fixture_library());
        let imported = manager
            .auto_import_folder(&fixture_library())
            .expect("the fixture imports");
        assert!(
            imported.iter().any(|name| name == "Test_Library"),
            "{imported:?}"
        );
    }
    std::fs::write(dir.join("board.cypcb"), BOARD).expect("the board is written");
    dir
}

fn opened(dir: &Path) -> DocumentState {
    let uri = format!("file://{}", dir.join("board.cypcb").display());
    let mut doc = DocumentState::new(uri, BOARD.to_string(), 1);
    doc.parse();
    doc.build_world();
    doc
}

fn card_over_the_footprint(doc: &DocumentState) -> String {
    let (line, text) = BOARD
        .lines()
        .enumerate()
        .find(|(_, line)| line.starts_with("component R1 "))
        .expect("the board places R1");
    let character = text.find('"').expect("the line names a footprint") as u32 + 2;
    hover_at_position(
        doc,
        &Position {
            line: line as u32,
            character,
        },
    )
    .expect("hovering a footprint name says something")
    .content
}

#[test]
fn a_footprint_the_index_holds_is_no_error_and_hovers_with_its_pads() {
    let dir = project("held", true);
    let doc = opened(&dir);

    assert!(
        doc.sync_errors.is_empty(),
        "the index holds the footprint: {:?}",
        doc.sync_errors
    );
    let card = card_over_the_footprint(&doc);
    assert!(card.contains("Pads: 2"), "{card}");
    assert!(
        card.contains("**Footprint: kicad::Test_Library:R_0603_1608Metric**"),
        "the card names the footprint in full: {card}"
    );
    let (line, _) = BOARD
        .lines()
        .enumerate()
        .find(|(_, line)| line.starts_with("component R1 "))
        .expect("the board places R1");
    let part = hover_at_position(
        &doc,
        &Position {
            line: line as u32,
            character: 0,
        },
    )
    .expect("hovering the part says something")
    .content;
    assert!(
        part.contains("Footprint: kicad::Test_Library:R_0603_1608Metric ("),
        "the part's card names its footprint in full: {part}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The control: without the index the same design is refused, so the test
/// above passes because the index was read.
#[test]
fn without_the_index_the_same_design_is_refused() {
    let dir = project("absent", false);
    let doc = opened(&dir);

    assert!(
        doc.sync_errors
            .iter()
            .any(|error| error.to_string().contains("kicad::R_0603_1608Metric")),
        "{:?}",
        doc.sync_errors
    );
    assert!(card_over_the_footprint(&doc).contains("(unknown)"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// An index that does not open is reported as that, not as a footprint nobody
/// imported: the name may well be in the file.
#[test]
fn an_index_that_does_not_open_is_reported_as_one() {
    let dir = project("garbage", false);
    std::fs::write(
        dir.join("cypcb-library.db"),
        "this file was never a database, whatever it is called",
    )
    .expect("the file is written");
    let doc = opened(&dir);

    let messages: Vec<String> = cypcb_lsp::diagnostics::run_diagnostics(&doc)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect();
    assert!(
        messages.iter().any(|message| message
            .starts_with("cypcb-library.db at ./cypcb-library.db could not be read: ")),
        "{messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("unknown footprint")),
        "{messages:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A design with a footprint of its own and a part whose footprint is being
/// typed.
const OFFERS: &str = r#"version 1

board test {
    size 30mm x 30mm
    layers 2
}

footprint OWN_PAD {
    pad 1 rect at 0mm, 0mm size 1mm x 0.5mm
}

component R1 resistor "kicad::R_0603_1608Metric" {
    value "330"
    at 15mm, 15mm
}
"#;

/// The labels offered inside R1's footprint string of [`OFFERS`], written
/// into `dir`.
fn offered_in(dir: &Path) -> Vec<String> {
    let (doc, position) = inside_the_footprint(dir);
    completion_at_position(&doc, &position)
        .into_iter()
        .map(|item| item.label)
        .collect()
}

/// [`OFFERS`] written into `dir` and opened, and the position inside R1's
/// footprint string.
fn inside_the_footprint(dir: &Path) -> (DocumentState, Position) {
    let path = dir.join("offers.cypcb");
    std::fs::write(&path, OFFERS).expect("the board is written");
    let mut doc = DocumentState::new(format!("file://{}", path.display()), OFFERS.to_string(), 1);
    doc.parse();
    doc.build_world();
    let (line, text) = OFFERS
        .lines()
        .enumerate()
        .find(|(_, line)| line.starts_with("component R1 "))
        .expect("the board places R1");
    let character = text.find('"').expect("the line names a footprint") as u32 + 1;
    (
        doc,
        Position {
            line: line as u32,
            character,
        },
    )
}

/// An index that does not read gives the completion no names. The editor is
/// told why, where it logs, and not left with a list that looks like an
/// index holding nothing.
#[test]
fn completion_says_why_an_index_gives_no_names() {
    let dir = project("offers-garbage", false);
    std::fs::write(
        dir.join("cypcb-library.db"),
        "this file was never a database, whatever it is called",
    )
    .expect("the file is written");
    let (doc, position) = inside_the_footprint(&dir);

    let why = cypcb_lsp::completion::unread_index_at(&doc, &position)
        .expect("the index is there and does not read");
    assert!(
        why.starts_with("cypcb-library.db at ./cypcb-library.db could not be read: "),
        "{why}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The control: an index that reads gives no reason.
#[test]
fn completion_is_silent_about_an_index_that_reads() {
    let dir = project("offers-reads", true);
    let (doc, position) = inside_the_footprint(&dir);

    assert_eq!(
        cypcb_lsp::completion::unread_index_at(&doc, &position),
        None
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Inside a footprint string the editor offers the built-ins, the design's
/// own `footprint` blocks, and every name the index holds - each written the
/// way `cypcb library search` prints it, which is how a design has to write
/// it.
#[test]
fn completion_offers_the_design_and_the_index_as_search_prints_them() {
    let dir = project("offers", true);
    let labels = offered_in(&dir);

    assert!(labels.iter().any(|label| label == "0402"), "{labels:?}");
    assert!(labels.iter().any(|label| label == "OWN_PAD"), "{labels:?}");

    let manager = LibraryManager::new(&dir.join("cypcb-library.db")).expect("the index opens");
    let mut printed = Vec::new();
    for query in ["0603", "0402", "SOT"] {
        for found in manager
            .search(query, &SearchFilters::default())
            .expect("the search runs")
        {
            printed.push(found.component.full_name());
        }
    }
    assert_eq!(
        printed.len(),
        3,
        "the fixture holds three footprints: {printed:?}"
    );
    for name in &printed {
        assert!(labels.contains(name), "{name} is not offered: {labels:?}");
    }
    assert!(
        !labels
            .iter()
            .any(|label| label == "kicad::R_0603_1608Metric"),
        "the board's short name is offered in full only: {labels:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The control: without the index nothing from it is offered.
#[test]
fn without_the_index_completion_offers_no_index_name() {
    let dir = project("offers-absent", false);
    let labels = offered_in(&dir);

    assert!(labels.iter().any(|label| label == "OWN_PAD"), "{labels:?}");
    assert!(
        labels.iter().all(|label| !label.contains("::")),
        "{labels:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A library folder holding one footprint named `name`, written here.
fn one_footprint_library(dir: &Path, name: &str) -> PathBuf {
    let libraries = dir.join(format!("late-{name}"));
    let pretty = libraries.join("Late.pretty");
    std::fs::create_dir_all(&pretty).expect("a place to write the library");
    std::fs::write(
        pretty.join(format!("{name}.kicad_mod")),
        format!(
            "(footprint \"{name}\"\n\t(layer \"F.Cu\")\n\t(attr smd)\n\
             \t(pad \"1\" smd rect (at -0.5 0) (size 0.5 0.5) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n\
             \t(pad \"2\" smd rect (at 0.5 0) (size 0.5 0.5) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n)\n"
        ),
    )
    .expect("the footprint is written");
    libraries
}

/// `cypcb library import` while the board is open in the editor: the next
/// completion offers what was just imported. The names are kept between
/// requests, and this is what says they are thrown away when the index moves.
#[test]
fn a_name_imported_while_the_board_is_open_is_offered_next_time() {
    let dir = project("offers-after-import", true);
    let before = offered_in(&dir);
    assert!(
        before
            .iter()
            .any(|label| label == "kicad::Test_Library:R_0603_1608Metric"),
        "the index was read before the import: {before:?}"
    );
    assert!(
        !before
            .iter()
            .any(|label| label == "kicad::Late:LATE_ARRIVAL"),
        "{before:?}"
    );

    let libraries = one_footprint_library(&dir, "LATE_ARRIVAL");
    let mut manager = LibraryManager::new(&dir.join("cypcb-library.db")).expect("the index opens");
    manager.add_kicad_search_path(libraries.clone());
    manager
        .auto_import_folder(&libraries)
        .expect("the late library imports");
    drop(manager);

    let after = offered_in(&dir);
    assert!(
        after
            .iter()
            .any(|label| label == "kicad::Late:LATE_ARRIVAL"),
        "imported after the first request and not offered: {after:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An index nobody touched is read once: the second request gets the same
/// names back without opening the database again.
#[test]
fn an_unchanged_index_is_read_once() {
    let dir = project("read-once", true);
    let board = dir.join("board.cypcb");
    let first = cypcb_library::design::index_names_for(&board);
    let second = cypcb_library::design::index_names_for(&board);
    assert!(!first.is_empty(), "the fixture index holds names");
    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "the unchanged index was read a second time"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Two writes inside one clock tick of the file system leave the index with
/// the modification time it had before. Set that time back by hand after an
/// import and the imported name still has to be offered.
#[test]
fn an_import_that_keeps_the_old_modification_time_is_still_seen() {
    let dir = project("same-tick", true);
    let index = dir.join("cypcb-library.db");
    let before = offered_in(&dir);
    assert!(
        !before.iter().any(|label| label == "kicad::Late:SAME_TICK"),
        "{before:?}"
    );
    let modified = std::fs::metadata(&index)
        .and_then(|metadata| metadata.modified())
        .expect("the index has a modification time");

    let libraries = one_footprint_library(&dir, "SAME_TICK");
    let mut manager = LibraryManager::new(&index).expect("the index opens");
    manager.add_kicad_search_path(libraries.clone());
    manager
        .auto_import_folder(&libraries)
        .expect("the late library imports");
    drop(manager);
    std::fs::File::options()
        .write(true)
        .open(&index)
        .and_then(|file| file.set_modified(modified))
        .expect("the modification time is set back");

    let after = offered_in(&dir);
    assert!(
        after.iter().any(|label| label == "kicad::Late:SAME_TICK"),
        "the import kept the old modification time and was missed: {after:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A second library holding the board's bare name: the editor reports the
/// name with both footprints in full and picks neither.
#[test]
fn a_bare_name_two_libraries_hold_is_reported_with_both() {
    let dir = project("two-libraries", true);
    let libraries = one_footprint_library(&dir, "R_0603_1608Metric");
    let mut manager = LibraryManager::new(&dir.join("cypcb-library.db")).expect("the index opens");
    manager.add_kicad_search_path(libraries.clone());
    manager
        .auto_import_folder(&libraries)
        .expect("the second library imports");
    drop(manager);

    let doc = opened(&dir);
    let errors: Vec<String> = doc.sync_errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        errors,
        [
            "footprint 'kicad::R_0603_1608Metric' is in more than one library: \
          kicad::Late:R_0603_1608Metric, kicad::Test_Library:R_0603_1608Metric"
        ],
    );

    let _ = std::fs::remove_dir_all(&dir);
}
