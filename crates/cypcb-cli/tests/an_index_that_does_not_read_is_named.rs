//! A library index that does not read is named, with the reason.
//!
//! `cargo test -p cypcb-cli --test an_index_that_does_not_read_is_named`
//!
//! A design naming `kicad::R_0603_1608Metric` next to a `cypcb-library.db`
//! that did not open was told `unknown footprint`, and nothing said a file was
//! involved. The person then imports the library again into the same broken
//! file, or goes looking for a typo that is not there.

use std::process::Command;

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

#[test]
fn check_names_the_index_that_does_not_open() {
    let dir = std::env::temp_dir().join(format!("cypcb-cli-garbage-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("boards")).expect("a place to work");
    std::fs::write(dir.join("boards/board.cypcb"), BOARD).expect("the board is written");
    std::fs::write(
        dir.join("cypcb-library.db"),
        "this file was never a database, whatever it is called",
    )
    .expect("the file is written");

    let out = Command::new(env!("CARGO_BIN_EXE_cypcb"))
        .args(["check", "board.cypcb"])
        .current_dir(dir.join("boards"))
        .output()
        .expect("cypcb runs");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    assert!(!out.status.success(), "{said}");
    assert!(
        said.contains("cypcb-library.db at ../cypcb-library.db could not be read: "),
        "{said}"
    );
    assert!(!said.contains("unknown footprint"), "{said}");

    let _ = std::fs::remove_dir_all(&dir);
}
