//! A footprint from the index resolves in the editor as it does in `check`.
//!
//! `cargo test -p cypcb-lsp --test the_editor_reads_the_library_index`
//!
//! The language server built its footprint table from the built-ins alone, so
//! a design naming `kicad::R_0603_1608Metric` after `cypcb library import` was
//! underlined as an unknown footprint in the editor while the command line
//! accepted it, and hovering the name called it a typo.

use std::path::{Path, PathBuf};

use cypcb_library::LibraryManager;
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
