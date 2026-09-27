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
/// unknown footprint at the line that named it. A name the index could not
/// answer for - the file does not open, the entry holds no footprint, or the
/// footprint stored under the name does not parse - is marked unreadable, and
/// the sync reports that instead, naming the index by its path from the
/// design's directory.
pub fn footprint_library_for(ast: &SourceFile, design: &Path) -> FootprintLibrary {
    let mut library = FootprintLibrary::new();
    let wanted = names_from_an_index(ast);
    if wanted.is_empty() {
        return library;
    }
    let Some(index) = index_for(design) else {
        return library;
    };
    let unreadable = |why: &dyn std::fmt::Display| {
        format!(
            "{INDEX_FILE} at {} could not be read: {why}",
            from_design(design, &index)
        )
    };
    let manager = match LibraryManager::new(&index) {
        Ok(manager) => manager,
        Err(error) => {
            for full_name in wanted {
                library.mark_unreadable(full_name, unreadable(&error));
            }
            return library;
        }
    };
    for full_name in wanted {
        let Some((source, name)) = full_name.split_once("::") else {
            continue;
        };
        let component = match manager.get_component(source, name) {
            Ok(Some(component)) => component,
            Ok(None) => continue,
            Err(error) => {
                library.mark_unreadable(full_name, unreadable(&error));
                continue;
            }
        };
        // An entry can hold a part's details and no footprint. It was left
        // out like a name the index does not hold, and the design was told
        // `unknown footprint` about a name the index does hold.
        let Some(text) = component.footprint_data else {
            let why = format!(
                "{INDEX_FILE} at {} holds '{full_name}' with no footprint",
                from_design(design, &index)
            );
            library.mark_unreadable(full_name, why);
            continue;
        };
        match cypcb_kicad::import_footprint_from_str(&text) {
            Ok(mut footprint) => {
                footprint.name = full_name;
                library.register(footprint);
            }
            Err(error) => {
                let why = unreadable(&format!(
                    "the footprint stored as '{full_name}' does not parse: {error}"
                ));
                library.mark_unreadable(full_name, why);
            }
        }
    }
    library
}

/// Where `index` is, seen from the directory of `design`: `./cypcb-library.db`
/// beside it, `../cypcb-library.db` one level up, and so on. The index is found
/// by walking up from that directory, so it is always one of its ancestors.
fn from_design(design: &Path, index: &Path) -> String {
    let depth = |path: &Path| path.components().count();
    let directory = std::path::absolute(design)
        .ok()
        .and_then(|design| design.parent().map(Path::to_path_buf));
    let levels = match (directory, index.parent()) {
        (Some(directory), Some(holder)) => depth(&directory).saturating_sub(depth(holder)),
        _ => 0,
    };
    if levels == 0 {
        format!("./{INDEX_FILE}")
    } else {
        format!("{}{INDEX_FILE}", "../".repeat(levels))
    }
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
/// Empty when there is no index or it cannot be read;
/// [`index_unreadable_for`] says why it could not.
///
/// The editor asks on every completion, and an index built from the KiCad
/// library holds some ten thousand names, so a reading is kept per index file
/// and reused until the file changes. "Changes" is its modification time, its
/// length and the change counter SQLite writes into the header on every
/// commit: the time alone can repeat when two imports land inside one clock
/// tick of the file system, and the counter cannot.
pub fn index_names_for(design: &Path) -> Arc<[String]> {
    reading_for(design).map_or_else(|| Arc::from([]), |(_, reading)| reading.names)
}

/// Why the index nearest `design` gave no names, when it is there and does
/// not read, in the words the sync uses for a name the design takes from it:
/// `cypcb-library.db at <path from the design> could not be read: <reason>`.
///
/// The editor offered the built-ins alone then, and a list without the index
/// names looked the same as an index that holds none.
pub fn index_unreadable_for(design: &Path) -> Option<String> {
    let (index, reading) = reading_for(design)?;
    let why = reading.why?;
    Some(format!(
        "{INDEX_FILE} at {} could not be read: {why}",
        from_design(design, &index)
    ))
}

/// The reading of the index nearest `design`, kept until the file changes.
fn reading_for(design: &Path) -> Option<(PathBuf, Reading)> {
    let index = index_for(design)?;
    let stamp = Stamp::of(&index)?;
    let readings = READINGS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(reading) = readings.lock().unwrap().get(&index) {
        if reading.stamp == stamp {
            return Some((index, reading.clone()));
        }
    }
    let reading = read_names(&index, stamp);
    readings
        .lock()
        .unwrap()
        .insert(index.clone(), reading.clone());
    Some((index, reading))
}

fn read_names(index: &Path, stamp: Stamp) -> Reading {
    let names = LibraryManager::new(index).and_then(|manager| manager.footprint_ids());
    match names {
        Ok(ids) => Reading {
            stamp,
            names: ids.iter().map(|id| id.to_string()).collect(),
            why: None,
        },
        Err(error) => Reading {
            stamp,
            names: Arc::from([]),
            why: Some(error.to_string().into()),
        },
    }
}

/// Names read from each index file, with the stamp the file had when read.
static READINGS: OnceLock<Mutex<HashMap<PathBuf, Reading>>> = OnceLock::new();

#[derive(Clone)]
struct Reading {
    stamp: Stamp,
    names: Arc<[String]>,
    /// Why the file gave no names, when it did not read.
    why: Option<Arc<str>>,
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

#[cfg(test)]
mod tests {
    use super::*;

    const BOARD: &str = "version 1\n\nboard b {\n    size 10mm x 10mm\n    layers 2\n}\n\n\
                         component R1 resistor \"kicad::BROKEN\" {\n    at 5mm, 5mm\n}\n";

    /// A project directory holding the board in `sub`, and nothing else.
    fn project(tag: &str, sub: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("cypcb-design-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a place to work");
        std::fs::create_dir_all(dir.join(sub)).expect("a place for the board");
        let design = dir.join(sub).join("board.cypcb");
        std::fs::write(&design, BOARD).expect("the board is written");
        (dir, design)
    }

    fn why(design: &Path) -> Option<String> {
        let ast = cypcb_parser::parse(BOARD);
        assert!(
            ast.errors.is_empty(),
            "the board in this test does not parse: {:?}",
            ast.errors
        );
        footprint_library_for(&ast.value, design)
            .why_unreadable("kicad::BROKEN")
            .map(str::to_string)
    }

    #[test]
    fn an_index_that_does_not_open_is_named_with_its_path() {
        let (dir, design) = project("garbage", "boards");
        std::fs::write(
            dir.join(INDEX_FILE),
            "this file was never a database, whatever it is called",
        )
        .expect("the file is written");

        let why = why(&design).expect("the index was found and did not read");
        assert!(
            why.starts_with("cypcb-library.db at ../cypcb-library.db could not be read: "),
            "{why}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_that_does_not_parse_is_named_with_the_index() {
        let (dir, design) = project("entry", ".");
        let conn = rusqlite::Connection::open(dir.join(INDEX_FILE)).expect("the index opens");
        crate::schema::initialize_schema(&conn).expect("the schema is written");
        crate::schema::insert_library(
            &conn,
            &crate::models::LibraryInfo {
                source: "kicad".to_string(),
                name: "Broken".to_string(),
                path: None,
                version: None,
                enabled: true,
                component_count: 1,
            },
        )
        .expect("the library is stored");
        crate::schema::insert_component(
            &conn,
            &crate::models::Component {
                id: crate::models::ComponentId::new("kicad", "BROKEN"),
                library: "Broken".to_string(),
                category: None,
                footprint_data: Some("(footprint \"BROKEN\" (pad".to_string()),
                metadata: Default::default(),
            },
        )
        .expect("the entry is stored");
        drop(conn);

        let why = why(&design).expect("the entry was found and did not parse");
        assert!(
            why.starts_with(
                "cypcb-library.db at ./cypcb-library.db could not be read: \
                 the footprint stored as 'kicad::BROKEN' does not parse: "
            ),
            "{why}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The control: with no index at all, the name is simply unknown.
    #[test]
    fn without_an_index_nothing_is_unreadable() {
        let (dir, design) = project("none", ".");
        assert_eq!(why(&design), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_without_a_footprint_is_named_as_one() {
        let (dir, design) = project("no-footprint", ".");
        let conn = rusqlite::Connection::open(dir.join(INDEX_FILE)).expect("the index opens");
        crate::schema::initialize_schema(&conn).expect("the schema is written");
        crate::schema::insert_library(
            &conn,
            &crate::models::LibraryInfo {
                source: "kicad".to_string(),
                name: "Parts".to_string(),
                path: None,
                version: None,
                enabled: true,
                component_count: 1,
            },
        )
        .expect("the library is stored");
        crate::schema::insert_component(
            &conn,
            &crate::models::Component {
                id: crate::models::ComponentId::new("kicad", "BROKEN"),
                library: "Parts".to_string(),
                category: None,
                footprint_data: None,
                metadata: Default::default(),
            },
        )
        .expect("the entry is stored");
        drop(conn);

        assert_eq!(
            why(&design).as_deref(),
            Some("cypcb-library.db at ./cypcb-library.db holds 'kicad::BROKEN' with no footprint")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_index_that_gives_no_names_says_why() {
        let (dir, design) = project("names-garbage", "boards");
        std::fs::write(
            dir.join(INDEX_FILE),
            "this file was never a database, whatever it is called",
        )
        .expect("the file is written");

        assert!(index_names_for(&design).is_empty());
        let why = index_unreadable_for(&design).expect("the index is there and does not read");
        assert!(
            why.starts_with("cypcb-library.db at ../cypcb-library.db could not be read: "),
            "{why}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_an_index_no_names_are_missing() {
        let (dir, design) = project("names-none", ".");
        assert_eq!(index_unreadable_for(&design), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
