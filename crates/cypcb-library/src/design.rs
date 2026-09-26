//! The footprints a design names from an index, and where that index is.
//!
//! A design writes `"kicad::R_0603_1608Metric"` after `cypcb library import`
//! put that footprint in `cypcb-library.db`. Nothing outside the `library`
//! command read that file, so the name came back as `unknown footprint`
//! although the search had just found it. This is the one place a design's
//! `source::name` is looked up, for every command and for the language server.
//!
//! A name is resolved from these sources, first match wins:
//!
//! 1. a `footprint` the design itself defines under that name;
//! 2. a built-in footprint;
//! 3. the index nearest the design: `cypcb-library.db` in the design's own
//!    directory, else in the closest directory above it.
//!
//! Only a name holding `::` is asked of the index. A built-in name never holds
//! one, so the index cannot shadow a built-in, and the design's own definition
//! is registered over this one by the sync.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use cypcb_parser::ast::{Definition, SourceFile};
use cypcb_world::footprint::FootprintLibrary;

use crate::LibraryManager;

/// The file `cypcb library import` writes and a design reads.
pub const INDEX_FILE: &str = "cypcb-library.db";

/// The index a design at `design` reads: the nearest [`INDEX_FILE`] from the
/// design's directory upward, or `None` when there is none.
pub fn index_for(design: &Path) -> Option<PathBuf> {
    let absolute = std::path::absolute(design).ok()?;
    absolute
        .ancestors()
        .skip(1)
        .map(|directory| directory.join(INDEX_FILE))
        .find(|candidate| candidate.is_file())
}

/// The footprint library a design at `design` is synced against: the
/// built-ins, plus every `source::name` it uses that its index holds.
///
/// A name the index does not hold is left out, and the sync reports it as an
/// unknown footprint at the line that named it.
pub fn footprint_library_for(ast: &SourceFile, design: &Path) -> FootprintLibrary {
    let mut library = FootprintLibrary::new();
    let wanted = names_from_an_index(ast);
    if wanted.is_empty() {
        return library;
    }
    let Some(index) = index_for(design) else {
        return library;
    };
    let Ok(manager) = LibraryManager::new(&index) else {
        return library;
    };
    for full_name in wanted {
        let Some((source, name)) = full_name.split_once("::") else {
            continue;
        };
        let Ok(Some(component)) = manager.get_component(source, name) else {
            continue;
        };
        let Some(text) = component.footprint_data else {
            continue;
        };
        let Ok(mut footprint) = cypcb_kicad::import_footprint_from_str(&text) else {
            continue;
        };
        footprint.name = full_name;
        library.register(footprint);
    }
    library
}

/// Every footprint name the design uses that holds `::`, parts inside a
/// module included.
fn names_from_an_index(ast: &SourceFile) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    collect(&ast.definitions, &mut names);
    names
}

fn collect(definitions: &[Definition], names: &mut BTreeSet<String>) {
    for definition in definitions {
        match definition {
            Definition::Component(component) if component.footprint.value.contains("::") => {
                names.insert(component.footprint.value.clone());
            }
            Definition::Module(module) => collect(&module.definitions, names),
            _ => {}
        }
    }
}

/// Every name the index nearest `design` can resolve, written the way a
/// design writes it and `cypcb library search` prints it: `source::name`.
///
/// Empty when there is no index or it cannot be read, as
/// [`footprint_library_for`] is silent about both.
///
/// The editor asks on every completion, and an index built from the KiCad
/// library holds some ten thousand names, so a reading is kept per index file
/// and reused until the file changes. "Changes" is its modification time, its
/// length and the change counter SQLite writes into the header on every
/// commit: the time alone can repeat when two imports land inside one clock
/// tick of the file system, and the counter cannot.
pub fn index_names_for(design: &Path) -> Arc<[String]> {
    let Some(index) = index_for(design) else {
        return Arc::from([]);
    };
    let Some(stamp) = Stamp::of(&index) else {
        return Arc::from([]);
    };
    let readings = READINGS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(reading) = readings.lock().unwrap().get(&index) {
        if reading.stamp == stamp {
            return Arc::clone(&reading.names);
        }
    }
    let names = read_names(&index);
    readings.lock().unwrap().insert(
        index,
        Reading {
            stamp,
            names: Arc::clone(&names),
        },
    );
    names
}

fn read_names(index: &Path) -> Arc<[String]> {
    let Ok(manager) = LibraryManager::new(index) else {
        return Arc::from([]);
    };
    manager
        .footprint_ids()
        .map(|ids| ids.iter().map(|id| id.to_string()).collect())
        .unwrap_or_else(|_| Arc::from([]))
}

/// Names read from each index file, with the stamp the file had when read.
static READINGS: OnceLock<Mutex<HashMap<PathBuf, Reading>>> = OnceLock::new();

struct Reading {
    stamp: Stamp,
    names: Arc<[String]>,
}

/// What an index file looks like from outside, without opening the database.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: SystemTime,
    len: u64,
    /// Bytes 24..28 of the file: SQLite's file change counter.
    change_counter: [u8; 4],
}

impl Stamp {
    fn of(index: &Path) -> Option<Stamp> {
        let metadata = std::fs::metadata(index).ok()?;
        let mut header = [0u8; 28];
        let mut file = std::fs::File::open(index).ok()?;
        std::io::Read::read_exact(&mut file, &mut header).ok()?;
        Some(Stamp {
            modified: metadata.modified().ok()?,
            len: metadata.len(),
            change_counter: [header[24], header[25], header[26], header[27]],
        })
    }
}
