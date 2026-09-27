use crate::error::LibraryError;
use crate::models::{Component, ComponentMetadata, LibraryInfo};
use rusqlite::{params, Connection};

/// SQLite schema for library management with FTS5 full-text search
pub const LIBRARY_SCHEMA: &str = r#"
-- Libraries table: tracks all library sources
CREATE TABLE IF NOT EXISTS libraries (
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT,
    version TEXT,
    enabled INTEGER NOT NULL DEFAULT 1,
    component_count INTEGER DEFAULT 0,
    PRIMARY KEY (source, name)
);

-- Components table: stores all component data
CREATE TABLE IF NOT EXISTS components (
    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    name TEXT NOT NULL,
    library TEXT NOT NULL,
    category TEXT,
    footprint_data TEXT,
    description TEXT,
    datasheet_url TEXT,
    manufacturer TEXT,
    mpn TEXT,
    value TEXT,
    package TEXT,
    step_model_path TEXT,
    metadata_json TEXT,
    UNIQUE(source, library, name),
    FOREIGN KEY (source, library) REFERENCES libraries(source, name)
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_components_category ON components(category);
CREATE INDEX IF NOT EXISTS idx_components_manufacturer ON components(manufacturer);
CREATE INDEX IF NOT EXISTS idx_components_value ON components(value);

-- FTS5 virtual table for full-text search with BM25 ranking
CREATE VIRTUAL TABLE IF NOT EXISTS components_fts USING fts5(
    source,
    name,
    category,
    description,
    manufacturer,
    mpn,
    value,
    package
);

-- Triggers to keep FTS5 in sync with components table. A search joins the
-- two on rowid, and two libraries can hold one name, so a row is found by
-- its rowid and not by its name.
CREATE TRIGGER IF NOT EXISTS components_ai AFTER INSERT ON components BEGIN
    INSERT INTO components_fts(rowid, source, name, category, description, manufacturer, mpn, value, package)
    VALUES (new.rowid, new.source, new.name, new.category, new.description, new.manufacturer, new.mpn, new.value, new.package);
END;

CREATE TRIGGER IF NOT EXISTS components_ad AFTER DELETE ON components BEGIN
    DELETE FROM components_fts WHERE rowid = old.rowid;
END;

CREATE TRIGGER IF NOT EXISTS components_au AFTER UPDATE ON components BEGIN
    DELETE FROM components_fts WHERE rowid = old.rowid;
    INSERT INTO components_fts(rowid, source, name, category, description, manufacturer, mpn, value, package)
    VALUES (new.rowid, new.source, new.name, new.category, new.description, new.manufacturer, new.mpn, new.value, new.package);
END;
"#;

/// SQLite schema for metadata (version tracking, 3D models)
pub const METADATA_SCHEMA: &str = r#"
-- Library versions table: tracks import history for rollback
CREATE TABLE IF NOT EXISTS library_versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    library_name TEXT NOT NULL,
    version_id TEXT,
    imported_at TEXT NOT NULL,
    component_count INTEGER NOT NULL,
    notes TEXT
);

CREATE INDEX IF NOT EXISTS idx_library_versions_lookup ON library_versions(source, library_name, imported_at);
"#;

/// The schema this crate writes, kept in `PRAGMA user_version`.
///
/// 0 is a file written before the version was kept: it keys a component by
/// source and name, so two libraries could not hold one name. 1 keys it by
/// source, library and name, the way KiCad's `LIB_ID` does.
pub const SCHEMA_VERSION: i64 = 1;

/// Initialize the library database schema
///
/// A file in an older schema is moved to this one first, with every row kept.
pub fn initialize_schema(conn: &Connection) -> Result<(), LibraryError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let has_components: bool = conn.query_row(
        "SELECT count(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'components'",
        [],
        |row| row.get(0),
    )?;
    if has_components && version < SCHEMA_VERSION {
        key_components_by_library(conn)?;
    }
    conn.execute_batch(LIBRARY_SCHEMA)?;
    // Written only when it changes: every write moves the change counter in
    // the file header, and the editor reads that counter to tell whether the
    // index changed since it last read it.
    if version != SCHEMA_VERSION {
        conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))?;
    }
    initialize_metadata_schema(conn)?;
    Ok(())
}

/// Move a file keyed by (source, name) to the key (source, library, name).
///
/// SQLite cannot change a table's UNIQUE constraint, so the table is built
/// again under the new schema and the rows are copied with their rowids. The
/// old key is the new one without the library, so no row can conflict. The
/// search index is built again from the copied rows. It all happens in one
/// transaction: a copy that does not keep every row changes nothing.
fn key_components_by_library(conn: &Connection) -> Result<(), LibraryError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(
        "DROP TRIGGER IF EXISTS components_ai;
         DROP TRIGGER IF EXISTS components_ad;
         DROP TRIGGER IF EXISTS components_au;
         DROP INDEX IF EXISTS idx_components_category;
         DROP INDEX IF EXISTS idx_components_manufacturer;
         DROP INDEX IF EXISTS idx_components_value;
         DROP TABLE IF EXISTS components_fts;
         ALTER TABLE components RENAME TO components_keyed_by_name;",
    )?;
    tx.execute_batch(LIBRARY_SCHEMA)?;
    let before: usize =
        tx.query_row("SELECT count(*) FROM components_keyed_by_name", [], |row| {
            row.get(0)
        })?;
    let copied = tx.execute(
        "INSERT INTO components
         (rowid, source, name, library, category, footprint_data, description, datasheet_url,
          manufacturer, mpn, value, package, step_model_path, metadata_json)
         SELECT rowid, source, name, library, category, footprint_data, description, datasheet_url,
                manufacturer, mpn, value, package, step_model_path, metadata_json
         FROM components_keyed_by_name",
        [],
    )?;
    if copied != before {
        return Err(LibraryError::NotIndexed(format!(
            "moving the index to the library key copied {copied} of {before} rows; \
             the file is left as it was"
        )));
    }
    tx.execute_batch("DROP TABLE components_keyed_by_name;")?;
    tx.commit()?;
    Ok(())
}

/// Initialize the metadata schema
pub fn initialize_metadata_schema(conn: &Connection) -> Result<(), LibraryError> {
    conn.execute_batch(METADATA_SCHEMA)?;
    Ok(())
}

/// Insert a library into the database
///
/// A design names a footprint `source::library:name` and splits it at the
/// first `:`, so a library name that holds one is refused. KiCad refuses it
/// in a library nickname too.
pub fn insert_library(conn: &Connection, lib: &LibraryInfo) -> Result<(), LibraryError> {
    if lib.name.contains(':') {
        return Err(LibraryError::NotIndexed(format!(
            "{} is not indexed: a design names a footprint `{}::<library>:<name>`, \
             so a library name cannot hold ':'",
            shown(&lib.source, &lib.name),
            lib.source
        )));
    }
    conn.execute(
        "INSERT OR REPLACE INTO libraries (source, name, path, version, enabled, component_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            &lib.source,
            &lib.name,
            &lib.path,
            &lib.version,
            lib.enabled as i32,
            lib.component_count,
        ],
    )?;
    Ok(())
}

/// List all libraries in the database
pub fn list_libraries(conn: &Connection) -> Result<Vec<LibraryInfo>, LibraryError> {
    let mut stmt = conn.prepare(
        "SELECT source, name, path, version, enabled, component_count
         FROM libraries
         ORDER BY source, name",
    )?;

    let libraries = stmt
        .query_map([], |row| {
            Ok(LibraryInfo {
                source: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                version: row.get(3)?,
                enabled: row.get::<_, i32>(4)? != 0,
                component_count: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(libraries)
}

/// Why a component was not written to the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// One library holds two components by this name. The first one written
    /// stays.
    TwiceInOneLibrary {
        source: String,
        name: String,
        library: String,
    },
}

/// A KiCad library is the `.pretty` folder a person sees on disk.
fn shown(source: &str, library: &str) -> String {
    if source == "kicad" {
        format!("{library}.pretty")
    } else {
        format!("library '{library}'")
    }
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::TwiceInOneLibrary {
                source,
                name,
                library,
            } => write!(
                f,
                "'{name}' from {} is not indexed: {} holds another footprint by that name, \
                 and that one is indexed",
                shown(source, library),
                shown(source, library)
            ),
        }
    }
}

/// What writing one component did to the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// A new row.
    Inserted,
    /// The row this library already held under that name, rewritten.
    Updated,
    /// Nothing was written.
    Rejected(Rejection),
}

/// What a batch did: the rows it wrote, the components it refused, and the
/// rows it took out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchOutcome {
    /// Components inserted or rewritten. Each one is a row in the index.
    pub written: usize,
    /// Components not written, with the reason.
    pub rejected: Vec<Rejection>,
    /// Rows of the library that the import no longer holds, removed.
    pub removed: usize,
}

/// Write one component. `written_before` says that the same batch already
/// wrote a component by this name, so a second one is a duplicate and not a
/// re-import.
///
/// Only a UNIQUE conflict on (source, library, name) leads to an UPDATE. A missing
/// library row is a FOREIGN KEY failure, and an UPDATE was run on it once:
/// it changed no row, and the component was lost with `Ok`.
fn write_component(
    conn: &Connection,
    component: &Component,
    written_before: bool,
) -> Result<Written, LibraryError> {
    let metadata_json = serde_json::to_string(&component.metadata)
        .map_err(|e| LibraryError::Parse(format!("Failed to serialize metadata: {}", e)))?;

    let inserted = conn.execute(
        "INSERT INTO components
         (source, name, library, category, footprint_data, description, datasheet_url,
          manufacturer, mpn, value, package, step_model_path, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            &component.id.source,
            &component.id.name,
            &component.library,
            &component.category,
            &component.footprint_data,
            &component.metadata.description,
            &component.metadata.datasheet_url,
            &component.metadata.manufacturer,
            &component.metadata.mpn,
            &component.metadata.value,
            &component.metadata.package,
            &component.metadata.step_model_path,
            &metadata_json,
        ],
    );

    match inserted {
        Ok(_) => return Ok(Written::Inserted),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE => {}
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY =>
        {
            return Err(LibraryError::NotIndexed(format!(
                "'{}' from {} is not indexed: the index holds no {} in source '{}'",
                component.id.name,
                shown(&component.id.source, &component.library),
                shown(&component.id.source, &component.library),
                component.id.source
            )));
        }
        Err(e) => return Err(e.into()),
    }

    if written_before {
        return Ok(Written::Rejected(Rejection::TwiceInOneLibrary {
            source: component.id.source.clone(),
            name: component.id.name.clone(),
            library: component.library.clone(),
        }));
    }

    // The same library imported again: its row is rewritten.
    let changed = conn.execute(
        "UPDATE components SET
            category = ?1,
            footprint_data = ?2,
            description = ?3,
            datasheet_url = ?4,
            manufacturer = ?5,
            mpn = ?6,
            value = ?7,
            package = ?8,
            step_model_path = ?9,
            metadata_json = ?10
         WHERE source = ?11 AND name = ?12 AND library = ?13",
        params![
            &component.category,
            &component.footprint_data,
            &component.metadata.description,
            &component.metadata.datasheet_url,
            &component.metadata.manufacturer,
            &component.metadata.mpn,
            &component.metadata.value,
            &component.metadata.package,
            &component.metadata.step_model_path,
            &metadata_json,
            &component.id.source,
            &component.id.name,
            &component.library,
        ],
    )?;
    if changed != 1 {
        return Err(LibraryError::NotIndexed(format!(
            "'{}' from {} is not indexed: rewriting its row changed {changed} rows, not 1",
            component.id.name,
            shown(&component.id.source, &component.library)
        )));
    }
    Ok(Written::Updated)
}

/// Insert a single component into the database
///
/// A component its library already holds under this name is rewritten.
pub fn insert_component(conn: &Connection, component: &Component) -> Result<(), LibraryError> {
    match write_component(conn, component, false)? {
        Written::Inserted | Written::Updated => Ok(()),
        Written::Rejected(why) => Err(LibraryError::NotIndexed(why.to_string())),
    }
}

/// Insert multiple components in a single transaction
///
/// `written` counts the rows this batch put in the index; a refused
/// component is in `rejected`, not in the count.
pub fn insert_components_batch(
    conn: &mut Connection,
    components: &[Component],
) -> Result<BatchOutcome, LibraryError> {
    let tx = conn.transaction()?;
    let outcome = write_all(&tx, components)?;
    tx.commit()?;
    Ok(outcome)
}

fn write_all(conn: &Connection, components: &[Component]) -> Result<BatchOutcome, LibraryError> {
    let mut outcome = BatchOutcome::default();
    let mut names = std::collections::HashSet::new();

    for component in components {
        let written_before =
            !names.insert((&component.id.source, &component.library, &component.id.name));
        match write_component(conn, component, written_before)? {
            Written::Inserted | Written::Updated => outcome.written += 1,
            Written::Rejected(why) => outcome.rejected.push(why),
        }
    }
    Ok(outcome)
}

/// Make `library` of `source` hold `components` and nothing else, in one
/// transaction.
///
/// A re-import wrote the files it found and left the rows of files deleted
/// since, so the index held more than the folder and more than the import
/// reported. Those rows are removed now and counted in `removed`.
pub fn replace_library_components(
    conn: &mut Connection,
    source: &str,
    library: &str,
    components: &[Component],
) -> Result<BatchOutcome, LibraryError> {
    let tx = conn.transaction()?;
    let mut outcome = write_all(&tx, components)?;
    let kept: std::collections::HashSet<&str> = components
        .iter()
        .filter(|component| component.id.source == source && component.library == library)
        .map(|component| component.id.name.as_str())
        .collect();
    let held: Vec<(i64, String)> = tx
        .prepare("SELECT rowid, name FROM components WHERE source = ?1 AND library = ?2")?
        .query_map(params![source, library], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    for (rowid, name) in held {
        if !kept.contains(name.as_str()) {
            outcome.removed += tx.execute("DELETE FROM components WHERE rowid = ?1", [rowid])?;
        }
    }
    tx.commit()?;
    Ok(outcome)
}

/// Every component that carries a footprint, written in full the way a
/// design writes it: `source::library:name`, ordered that way.
pub fn footprint_names(conn: &Connection) -> Result<Vec<String>, LibraryError> {
    let mut stmt = conn.prepare(
        "SELECT source, library, name FROM components WHERE footprint_data IS NOT NULL
         ORDER BY source, library, name",
    )?;
    let names = stmt
        .query_map([], |row| {
            Ok(format!(
                "{}::{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

/// The component a design means by `source::written`.
///
/// `written` is `library:name`, split at the first `:`, or a bare `name`.
/// A bare name finds the component when one library holds it. When several
/// do, it is [`LibraryError::Ambiguous`] with each one written in full: the
/// index does not pick a library for the design.
pub fn get_component(
    conn: &Connection,
    source: &str,
    written: &str,
) -> Result<Option<Component>, LibraryError> {
    const COLUMNS: &str =
        "SELECT source, name, library, category, footprint_data, description, datasheet_url,
                manufacturer, mpn, value, package, step_model_path, metadata_json
         FROM components";
    let mut found = match written.split_once(':') {
        Some((library, name)) => components_where(
            conn,
            &format!("{COLUMNS} WHERE source = ?1 AND library = ?2 AND name = ?3"),
            params![source, library, name],
        )?,
        None => components_where(
            conn,
            &format!("{COLUMNS} WHERE source = ?1 AND name = ?2 ORDER BY library"),
            params![source, written],
        )?,
    };
    if found.len() > 1 {
        return Err(LibraryError::Ambiguous {
            written: format!("{source}::{written}"),
            candidates: found.iter().map(Component::full_name).collect(),
        });
    }
    Ok(found.pop())
}

fn components_where(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<Component>, LibraryError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut found = Vec::new();
    while let Some(row) = rows.next()? {
        let metadata_json: String = row.get(12)?;
        let metadata: ComponentMetadata = serde_json::from_str(&metadata_json)
            .map_err(|e| LibraryError::Parse(format!("Failed to parse metadata: {}", e)))?;

        found.push(Component {
            id: crate::models::ComponentId {
                source: row.get(0)?,
                name: row.get(1)?,
            },
            library: row.get(2)?,
            category: row.get(3)?,
            footprint_data: row.get(4)?,
            metadata,
        });
    }
    Ok(found)
}

/// Remove each library of `source` whose recorded folder `gone` holds for,
/// with its components, in one transaction. Returns each one's name and the
/// components removed with it, in name order.
pub fn remove_libraries_where(
    conn: &mut Connection,
    source: &str,
    gone: impl Fn(&str) -> bool,
) -> Result<Vec<(String, usize)>, LibraryError> {
    let tx = conn.transaction()?;
    let recorded: Vec<(String, String)> = tx
        .prepare(
            "SELECT name, path FROM libraries WHERE source = ?1 AND path IS NOT NULL ORDER BY name",
        )?
        .query_map(params![source], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let mut removed = Vec::new();
    for (name, folder) in recorded {
        if !gone(&folder) {
            continue;
        }
        let rows = delete_library_components(&tx, source, &name)?;
        tx.execute(
            "DELETE FROM libraries WHERE source = ?1 AND name = ?2",
            params![source, name],
        )?;
        removed.push((name, rows));
    }
    tx.commit()?;
    Ok(removed)
}

/// Delete all components for a library
pub fn delete_library_components(
    conn: &Connection,
    source: &str,
    library: &str,
) -> Result<usize, LibraryError> {
    let count = conn.execute(
        "DELETE FROM components WHERE source = ?1 AND library = ?2",
        params![source, library],
    )?;

    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ComponentId, ComponentMetadata};

    #[test]
    fn test_schema_initialization() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(tables.contains(&"libraries".to_string()));
        assert!(tables.contains(&"components".to_string()));
        assert!(tables.contains(&"components_fts".to_string()));
    }

    #[test]
    fn test_component_crud() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert a library
        let library = LibraryInfo {
            source: "test".to_string(),
            name: "TestLib".to_string(),
            path: Some("/path/to/lib".to_string()),
            version: Some("1.0".to_string()),
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        // Insert a component
        let component = Component {
            id: ComponentId::new("test", "R_0805"),
            library: "TestLib".to_string(),
            category: Some("Resistors".to_string()),
            footprint_data: Some("(footprint ...)".to_string()),
            metadata: ComponentMetadata {
                description: Some("0805 Resistor".to_string()),
                datasheet_url: None,
                manufacturer: Some("TestCorp".to_string()),
                mpn: Some("TC-R0805-10K".to_string()),
                value: Some("10k".to_string()),
                package: Some("0805".to_string()),
                step_model_path: None,
            },
        };

        insert_component(&conn, &component).unwrap();

        // Retrieve the component
        let retrieved = get_component(&conn, "test", "R_0805").unwrap();
        assert!(retrieved.is_some());

        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id.source, "test");
        assert_eq!(retrieved.id.name, "R_0805");
        assert_eq!(retrieved.library, "TestLib");
        assert_eq!(retrieved.category, Some("Resistors".to_string()));
        assert_eq!(retrieved.metadata.value, Some("10k".to_string()));
    }

    #[test]
    fn test_batch_insert() {
        let mut conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert library
        let library = LibraryInfo {
            source: "test".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        // Create multiple components
        let components = vec![
            Component {
                id: ComponentId::new("test", "R_0805"),
                library: "TestLib".to_string(),
                category: Some("Resistors".to_string()),
                footprint_data: None,
                metadata: ComponentMetadata {
                    value: Some("10k".to_string()),
                    ..Default::default()
                },
            },
            Component {
                id: ComponentId::new("test", "C_0805"),
                library: "TestLib".to_string(),
                category: Some("Capacitors".to_string()),
                footprint_data: None,
                metadata: ComponentMetadata {
                    value: Some("100nF".to_string()),
                    ..Default::default()
                },
            },
        ];

        // Batch insert
        let count = insert_components_batch(&mut conn, &components)
            .unwrap()
            .written;
        assert_eq!(count, 2);

        // Verify both components exist
        assert!(get_component(&conn, "test", "R_0805").unwrap().is_some());
        assert!(get_component(&conn, "test", "C_0805").unwrap().is_some());
    }

    #[test]
    fn test_delete_library_components() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert library and components
        let library = LibraryInfo {
            source: "test".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        let component = Component {
            id: ComponentId::new("test", "R_0805"),
            library: "TestLib".to_string(),
            category: None,
            footprint_data: None,
            metadata: ComponentMetadata::default(),
        };
        insert_component(&conn, &component).unwrap();

        // Delete library components
        let count = delete_library_components(&conn, "test", "TestLib").unwrap();
        assert_eq!(count, 1);

        // Verify component is gone
        assert!(get_component(&conn, "test", "R_0805").unwrap().is_none());
    }

    #[test]
    fn test_fts5_trigger_sync() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert library
        let library = LibraryInfo {
            source: "test".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        // Insert a component
        let component = Component {
            id: ComponentId::new("test", "R_0805"),
            library: "TestLib".to_string(),
            category: Some("Resistors".to_string()),
            footprint_data: None,
            metadata: ComponentMetadata {
                description: Some("Surface mount resistor".to_string()),
                value: Some("10k".to_string()),
                ..Default::default()
            },
        };
        insert_component(&conn, &component).unwrap();

        // Query FTS5 to verify trigger synced the data
        let mut stmt = conn
            .prepare(
                "SELECT source, name FROM components_fts WHERE components_fts MATCH 'resistor'",
            )
            .unwrap();

        let results: Vec<String> = stmt
            .query_map([], |row| {
                let source: String = row.get(0)?;
                let name: String = row.get(1)?;
                Ok(format!("{}::{}", source, name))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "test::R_0805");
    }

    #[test]
    fn test_direct_update() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        let library = LibraryInfo {
            source: "test".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        let component = Component {
            id: crate::models::ComponentId::new("test", "R_0805"),
            library: "TestLib".to_string(),
            category: Some("Resistors".to_string()),
            footprint_data: None,
            metadata: ComponentMetadata::default(),
        };

        insert_component(&conn, &component).unwrap();

        // Direct UPDATE of category
        let result = conn.execute(
            "UPDATE components SET category = ?1 WHERE source = ?2 AND name = ?3",
            params!["Passive/Resistors", "test", "R_0805"],
        );

        eprintln!("UPDATE result: {:?}", result);
        assert!(result.is_ok());

        // Try to retrieve
        let retrieved = get_component(&conn, "test", "R_0805").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(
            retrieved.unwrap().category,
            Some("Passive/Resistors".to_string())
        );
    }

    fn library(name: &str) -> LibraryInfo {
        LibraryInfo {
            source: "kicad".to_string(),
            name: name.to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        }
    }

    fn footprint(name: &str, library: &str) -> Component {
        Component {
            id: ComponentId::new("kicad", name),
            library: library.to_string(),
            category: None,
            footprint_data: Some("(footprint x)".to_string()),
            metadata: ComponentMetadata::default(),
        }
    }

    #[test]
    fn a_component_of_a_library_the_index_lacks_is_an_error() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        // The component is written once, so a second write meets the
        // UNIQUE conflict and not only the missing library.
        insert_library(&conn, &library("A")).unwrap();
        insert_component(&conn, &footprint("SOT-23-5", "A")).unwrap();

        let err = insert_component(&conn, &footprint("R_0603", "Missing"))
            .expect_err("no library row, so no component row");
        assert_eq!(
            err.to_string(),
            "'R_0603' from Missing.pretty is not indexed: the index holds no Missing.pretty in source 'kicad'"
        );
        assert!(get_component(&conn, "kicad", "R_0603").unwrap().is_none());
    }

    #[test]
    fn two_libraries_hold_one_name() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        insert_library(&conn, &library("A")).unwrap();
        insert_library(&conn, &library("B")).unwrap();
        insert_component(&conn, &footprint("SOT-23-5", "A")).unwrap();
        insert_component(&conn, &footprint("SOT-23-5", "B")).unwrap();

        let a = get_component(&conn, "kicad", "A:SOT-23-5")
            .unwrap()
            .unwrap();
        let b = get_component(&conn, "kicad", "B:SOT-23-5")
            .unwrap()
            .unwrap();
        assert_eq!((a.library.as_str(), b.library.as_str()), ("A", "B"));
        assert_eq!(a.full_name(), "kicad::A:SOT-23-5");
        assert!(get_component(&conn, "kicad", "C:SOT-23-5")
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_bare_name_two_libraries_hold_names_both_and_picks_neither() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        insert_library(&conn, &library("B")).unwrap();
        insert_library(&conn, &library("A")).unwrap();
        insert_component(&conn, &footprint("SOT-23-5", "B")).unwrap();
        insert_component(&conn, &footprint("R_0603", "B")).unwrap();

        let alone = get_component(&conn, "kicad", "SOT-23-5").unwrap().unwrap();
        assert_eq!(alone.library, "B");

        insert_component(&conn, &footprint("SOT-23-5", "A")).unwrap();
        let err = get_component(&conn, "kicad", "SOT-23-5").expect_err("two libraries hold it");
        assert_eq!(
            err.to_string(),
            "'kicad::SOT-23-5' is in more than one library: \
             kicad::A:SOT-23-5, kicad::B:SOT-23-5; write the one you mean"
        );
        let still = get_component(&conn, "kicad", "R_0603").unwrap().unwrap();
        assert_eq!(still.library, "B");
    }

    #[test]
    fn a_library_name_with_a_colon_is_refused() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        let err = insert_library(&conn, &library("A:B")).expect_err("the name holds ':'");
        assert_eq!(
            err.to_string(),
            "A:B.pretty is not indexed: a design names a footprint \
             `kicad::<library>:<name>`, so a library name cannot hold ':'"
        );
        let libraries: usize = conn
            .query_row("SELECT count(*) FROM libraries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(libraries, 0);
    }

    #[test]
    fn a_reimport_removes_the_rows_of_files_gone() {
        let mut conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        insert_library(&conn, &library("A")).unwrap();
        insert_library(&conn, &library("B")).unwrap();
        replace_library_components(
            &mut conn,
            "kicad",
            "A",
            &[footprint("SOT-23-5", "A"), footprint("R_0603", "A")],
        )
        .unwrap();
        replace_library_components(&mut conn, "kicad", "B", &[footprint("R_0603", "B")]).unwrap();

        let again =
            replace_library_components(&mut conn, "kicad", "A", &[footprint("SOT-23-5", "A")])
                .unwrap();

        let rows_of_a: usize = conn
            .query_row(
                "SELECT count(*) FROM components WHERE library = 'A'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((again.written, again.removed), (1, 1));
        assert_eq!(rows_of_a, again.written);
        assert!(get_component(&conn, "kicad", "A:R_0603").unwrap().is_none());
        assert!(get_component(&conn, "kicad", "B:R_0603").unwrap().is_some());
        let found = crate::search::search_components(
            &conn,
            "R_0603",
            &crate::models::SearchFilters::default(),
        )
        .unwrap();
        let names: Vec<String> = found.iter().map(|hit| hit.component.full_name()).collect();
        assert_eq!(names, ["kicad::B:R_0603"]);
    }

    /// The schema before the library was part of the key, as a file written
    /// then holds it, with three footprints in two libraries.
    const KEYED_BY_NAME: &str = concat!(
        include_str!("../../../tests/fixtures/library-index/schema-0.sql"),
        r#"
        INSERT INTO libraries (source, name) VALUES ('kicad', 'A'), ('kicad', 'B');
        INSERT INTO components (source, name, library, footprint_data, metadata_json) VALUES
            ('kicad', 'SOT-23-5', 'A', '(footprint "SOT-23-5")', '{}'),
            ('kicad', 'R_0603', 'A', '(footprint "R_0603")', '{}'),
            ('kicad', 'C_0603', 'B', '(footprint "C_0603")', '{}');
    "#
    );

    /// `cypcb library` opens the index with [`crate::LibraryManager::new`] and
    /// moves an old file; a design reads it with `open_read_only` and leaves
    /// it as it was.
    #[test]
    fn only_the_library_command_moves_an_old_file() {
        let dir = std::env::temp_dir().join(format!("cypcb-old-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cypcb-library.db");
        let _ = std::fs::remove_file(&file);
        Connection::open(&file)
            .unwrap()
            .execute_batch(KEYED_BY_NAME)
            .unwrap();
        let before = std::fs::read(&file).unwrap();
        let version = |file: &std::path::Path| -> i64 {
            Connection::open(file)
                .unwrap()
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .unwrap()
        };

        let reader = crate::LibraryManager::open_read_only(&file).unwrap();
        assert_eq!(
            reader.footprint_names().unwrap(),
            ["kicad::A:R_0603", "kicad::A:SOT-23-5", "kicad::B:C_0603"]
        );
        let found = reader.get_component("kicad", "B:C_0603").unwrap().unwrap();
        assert_eq!(found.full_name(), "kicad::B:C_0603");
        drop(reader);
        assert!(
            std::fs::read(&file).unwrap() == before,
            "reading wrote the file"
        );
        assert_eq!(version(&file), 0);

        crate::LibraryManager::new(&file).unwrap();
        assert_eq!(version(&file), SCHEMA_VERSION);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_in_the_old_schema_keeps_every_row_and_takes_the_library_key() {
        let dir = std::env::temp_dir().join(format!("cypcb-old-schema-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cypcb-library.db");
        let _ = std::fs::remove_file(&file);
        Connection::open(&file)
            .unwrap()
            .execute_batch(KEYED_BY_NAME)
            .unwrap();

        let conn = Connection::open(&file).unwrap();
        initialize_schema(&conn).unwrap();

        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT rowid, library || ':' || name FROM components ORDER BY rowid")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            rows,
            [
                (1, "A:SOT-23-5".to_string()),
                (2, "A:R_0603".to_string()),
                (3, "B:C_0603".to_string())
            ]
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // The new key: B takes a name A holds.
        insert_component(&conn, &footprint("SOT-23-5", "B")).unwrap();
        assert!(get_component(&conn, "kicad", "A:SOT-23-5")
            .unwrap()
            .is_some());
        assert!(get_component(&conn, "kicad", "B:SOT-23-5")
            .unwrap()
            .is_some());

        // The search index was built again from the copied rows.
        let found = crate::search::search_components(
            &conn,
            "C_0603",
            &crate::models::SearchFilters::default(),
        )
        .unwrap();
        let names: Vec<String> = found.iter().map(|hit| hit.component.full_name()).collect();
        assert_eq!(names, ["kicad::B:C_0603"]);

        // A second open finds the file in the current schema and leaves it.
        drop(conn);
        let conn = Connection::open(&file).unwrap();
        initialize_schema(&conn).unwrap();
        let rows: usize = conn
            .query_row("SELECT count(*) FROM components", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_batch_counts_the_rows_it_wrote() {
        let mut conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        insert_library(&conn, &library("A")).unwrap();
        insert_library(&conn, &library("B")).unwrap();

        let first = insert_components_batch(
            &mut conn,
            &[
                footprint("SOT-23-5", "A"),
                footprint("SOT-23-5", "A"),
                footprint("R_0603", "A"),
            ],
        )
        .unwrap();
        let second = insert_components_batch(
            &mut conn,
            &[footprint("SOT-23-5", "B"), footprint("C_0603", "B")],
        )
        .unwrap();
        let again = insert_components_batch(&mut conn, &[footprint("R_0603", "A")]).unwrap();

        let rows: usize = conn
            .query_row("SELECT count(*) FROM components", [], |row| row.get(0))
            .unwrap();
        assert_eq!((first.written, second.written, again.written), (2, 2, 1));
        assert_eq!(rows, first.written + second.written);
        assert_eq!(
            first.rejected.len() + second.rejected.len() + again.rejected.len(),
            1
        );
        assert!(matches!(
            first.rejected[0],
            Rejection::TwiceInOneLibrary { .. }
        ));
    }
}
