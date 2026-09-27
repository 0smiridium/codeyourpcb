//! Every footprint this repository can define keeps its pads inside its
//! courtyard.
//!
//! `cargo test -p cypcb-kicad --test every_footprint_holds_its_pads_in_its_courtyard`
//!
//! `land-outside-courtyard` asks this of the parts a board places. This asks
//! it of every footprint the four sources can produce, placed or not: the
//! built-in library, a footprint written inline in a `.cypcb` file, a KiCad
//! `.kicad_mod` and the footprints a `.kicad_pcb` carries. Each source is
//! counted, so a walk that found nothing fails instead of passing.

use std::path::{Path, PathBuf};

use cypcb_parser::ast::Definition;
use cypcb_world::footprint::{Footprint, FootprintLibrary};
use cypcb_world::{sync_ast_to_world, BoardWorld};

fn repo_root() -> PathBuf {
    cypcb_fixtures::tree::repo_root()
}

/// A corpus path as the repository names it.
fn shown(path: &Path) -> String {
    path.strip_prefix(repo_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Every file git tracks that ends in `extension`.
///
/// The disk holds more than the repository does. The main checkout carries
/// ignored copies of other projects under `viewer/`, and one of them has
/// `.kicad_mod` files with pads outside their courtyards: a walk of the disk
/// read them as this repository's footprints and failed there, while a clean
/// clone passed.
fn corpus(extension: &str) -> Vec<PathBuf> {
    let root = repo_root();
    let mut found: Vec<PathBuf> = cypcb_fixtures::tree::files_git_tracks(&root)
        .into_iter()
        .filter(|file| file.ends_with(extension))
        .filter(|file| {
            !file
                .split('/')
                .any(|part| part == "target" || part == "node_modules" || part.starts_with('.'))
        })
        .map(|file| root.join(file))
        .collect();
    found.sort();
    found
}

/// `source: footprint, N of M pads outside` for a footprint that fails.
fn verdict(source: &str, footprint: &Footprint) -> Option<String> {
    let outside = footprint.pads_outside_courtyard();
    (!outside.is_empty()).then(|| {
        format!(
            "{source}: {}, {} of {} pads outside",
            footprint.name,
            outside.len(),
            footprint.pads.len()
        )
    })
}

/// Inline footprints a `.cypcb` source defines, as the library holds them
/// after sync.
fn inline_footprints(source: &str) -> Option<Vec<Footprint>> {
    let parsed = cypcb_parser::parse(source);
    if parsed.has_errors() {
        return None;
    }
    let names: Vec<String> = parsed
        .value
        .definitions
        .iter()
        .filter_map(|d| match d {
            Definition::Footprint(f) => Some(f.name.value.clone()),
            _ => None,
        })
        .collect();
    let mut world = BoardWorld::new();
    let mut library = FootprintLibrary::new();
    // Footprints register before anything else is read, so a file whose
    // imports or parts do not resolve still yields the footprints it declares.
    let _ = sync_ast_to_world(&parsed.value, source, &mut world, &mut library);
    Some(
        names
            .iter()
            .filter_map(|name| library.get(name).cloned())
            .collect(),
    )
}

#[test]
fn every_footprint_in_the_repository_holds_its_pads() {
    let mut failures = Vec::new();

    let builtin = FootprintLibrary::new();
    let mut counted = [
        ("built-in", builtin.len()),
        ("inline", 0),
        ("kicad_mod", 0),
        ("kicad_pcb", 0),
    ];
    for (_, footprint) in builtin.iter() {
        failures.extend(verdict("built-in", footprint));
    }

    for path in corpus(".cypcb") {
        let source = std::fs::read_to_string(&path).expect("a corpus file reads");
        for footprint in inline_footprints(&source).unwrap_or_default() {
            counted[1].1 += 1;
            failures.extend(verdict(&shown(&path), &footprint));
        }
    }

    for path in corpus(".kicad_mod") {
        if let Ok(footprint) = cypcb_kicad::import_footprint(&path) {
            counted[2].1 += 1;
            failures.extend(verdict(&shown(&path), &footprint));
        }
    }

    for path in corpus(".kicad_pcb") {
        if let Ok(parsed) = cypcb_kicad::parse_kicad_pcb(&path) {
            for (_, footprint) in parsed.library.iter() {
                // The library a board is read into starts from the built-ins,
                // which are counted above already.
                if builtin
                    .get(&footprint.name)
                    .is_some_and(|b| format!("{:?}", b.pads) == format!("{:?}", footprint.pads))
                {
                    continue;
                }
                counted[3].1 += 1;
                failures.extend(verdict(&shown(&path), footprint));
            }
        }
    }

    eprintln!("footprints checked: {counted:?}");
    for (source, count) in counted {
        assert!(count > 0, "the walk found no {source} footprint");
    }
    assert!(
        failures.is_empty(),
        "{} footprints with pads outside the courtyard:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The control: the benchmark connector as it was written before its
/// courtyard learned where its centre is.
#[test]
fn a_courtyard_centred_on_pin_one_is_caught() {
    let source = r#"version 1
footprint HDR {
    courtyard 3.54mm x 31.48mm
    pad 1 rect at 0mm, 0mm size 1.7mm x 1.7mm drill 1mm
    pad 12 circle at 0mm, -27.94mm size 1.7mm x 1.7mm drill 1mm
}
"#;
    let footprints = inline_footprints(source).expect("the control parses");
    assert_eq!(footprints.len(), 1);
    let outside = footprints[0].pads_outside_courtyard();
    assert_eq!(outside.len(), 1);
    assert_eq!(outside[0].0.number, "12");
}
