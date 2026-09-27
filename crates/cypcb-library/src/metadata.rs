use crate::error::LibraryError;
use crate::models::ComponentMetadata;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

/// Library version record for tracking import history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryVersion {
    pub id: Option<i64>,
    pub source: String,
    pub library_name: String,
    pub version_id: Option<String>,
    /// ISO 8601 timestamp (YYYY-MM-DDTHH:MM:SSZ)
    pub imported_at: String,
    pub component_count: usize,
    pub notes: Option<String>,
}

/// Format SystemTime as ISO 8601 / RFC 3339 string
fn format_timestamp(time: SystemTime) -> String {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("Time went backwards");
    let secs = duration.as_secs();
    let nanos = duration.subsec_nanos();

    // Convert to date/time components
    const SECS_PER_DAY: u64 = 86400;
    const SECS_PER_HOUR: u64 = 3600;
    const SECS_PER_MINUTE: u64 = 60;

    let days_since_epoch = secs / SECS_PER_DAY;
    let secs_today = secs % SECS_PER_DAY;
    let hours = secs_today / SECS_PER_HOUR;
    let minutes = (secs_today % SECS_PER_HOUR) / SECS_PER_MINUTE;
    let seconds = secs_today % SECS_PER_MINUTE;

    // Days since Unix epoch (1970-01-01) to Gregorian calendar
    // Simplified: Assume ~365.25 days/year for rough conversion
    let years_since_1970 = days_since_epoch / 365;
    let year = 1970 + years_since_1970;
    let day_of_year = days_since_epoch % 365;

    // Rough month/day calculation (simplified, good enough for timestamps)
    let month = (day_of_year / 30).min(11) + 1;
    let day = (day_of_year % 30) + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        day,
        hours,
        minutes,
        seconds,
        nanos / 1_000_000
    )
}

/// Track a new library version import
pub fn track_version(
    conn: &Connection,
    source: &str,
    library_name: &str,
    component_count: usize,
    notes: Option<&str>,
) -> Result<LibraryVersion, LibraryError> {
    let timestamp = format_timestamp(SystemTime::now());

    conn.execute(
        "INSERT INTO library_versions (source, library_name, version_id, imported_at, component_count, notes)
         VALUES (?1, ?2, NULL, ?3, ?4, ?5)",
        params![source, library_name, &timestamp, component_count, notes],
    )?;

    let id = conn.last_insert_rowid();

    Ok(LibraryVersion {
        id: Some(id),
        source: source.to_string(),
        library_name: library_name.to_string(),
        version_id: None,
        imported_at: timestamp,
        component_count,
        notes: notes.map(|s| s.to_string()),
    })
}

/// List all versions for a library, ordered by import time (newest first)
pub fn list_versions(
    conn: &Connection,
    source: &str,
    library_name: &str,
) -> Result<Vec<LibraryVersion>, LibraryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, library_name, version_id, imported_at, component_count, notes
         FROM library_versions
         WHERE source = ?1 AND library_name = ?2
         ORDER BY imported_at DESC",
    )?;

    let versions = stmt
        .query_map(params![source, library_name], |row| {
            Ok(LibraryVersion {
                id: Some(row.get(0)?),
                source: row.get(1)?,
                library_name: row.get(2)?,
                version_id: row.get(3)?,
                imported_at: row.get(4)?,
                component_count: row.get::<_, usize>(5)?,
                notes: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(versions)
}

/// Get the most recent version for a library
pub fn latest_version(
    conn: &Connection,
    source: &str,
    library_name: &str,
) -> Result<Option<LibraryVersion>, LibraryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, library_name, version_id, imported_at, component_count, notes
         FROM library_versions
         WHERE source = ?1 AND library_name = ?2
         ORDER BY imported_at DESC
         LIMIT 1",
    )?;

    let mut rows = stmt.query(params![source, library_name])?;

    if let Some(row) = rows.next()? {
        Ok(Some(LibraryVersion {
            id: Some(row.get(0)?),
            source: row.get(1)?,
            library_name: row.get(2)?,
            version_id: row.get(3)?,
            imported_at: row.get(4)?,
            component_count: row.get::<_, usize>(5)?,
            notes: row.get(6)?,
        }))
    } else {
        Ok(None)
    }
}

/// Associate a 3D STEP model path with the component `source::written`
///
/// `written` is `library:name`, or a bare `name` one library alone holds, as
/// [`crate::schema::get_component`] reads it. A bare name several libraries
/// hold is [`LibraryError::Ambiguous`] and changes nothing: by name alone the
/// path went to the component in every library that held the name.
pub fn associate_step_model(
    conn: &Connection,
    source: &str,
    written: &str,
    step_path: &str,
) -> Result<(), LibraryError> {
    let Some(component) = crate::schema::get_component(conn, source, written)? else {
        return Err(LibraryError::NotFound(format!(
            "Component {}::{} not found",
            source, written
        )));
    };

    conn.execute(
        "UPDATE components SET step_model_path = ?1
         WHERE source = ?2 AND library = ?3 AND name = ?4",
        params![step_path, source, component.library, component.id.name],
    )?;

    Ok(())
}

/// Get the STEP model path of the component `source::written`, named as
/// [`associate_step_model`] names it
pub fn get_step_model_path(
    conn: &Connection,
    source: &str,
    written: &str,
) -> Result<Option<String>, LibraryError> {
    Ok(crate::schema::get_component(conn, source, written)?
        .and_then(|component| component.metadata.step_model_path))
}

/// Get the metadata (description, datasheet, manufacturer, etc.) of the
/// component `source::written`, named as [`associate_step_model`] names it
pub fn get_component_metadata(
    conn: &Connection,
    source: &str,
    written: &str,
) -> Result<Option<ComponentMetadata>, LibraryError> {
    Ok(crate::schema::get_component(conn, source, written)?.map(|component| component.metadata))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Component, ComponentId, ComponentMetadata, LibraryInfo};
    use crate::schema::{initialize_schema, insert_component, insert_library};

    #[test]
    fn test_track_version() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        let version =
            track_version(&conn, "kicad", "Resistors", 42, Some("Initial import")).unwrap();

        assert_eq!(version.source, "kicad");
        assert_eq!(version.library_name, "Resistors");
        assert_eq!(version.component_count, 42);
        assert_eq!(version.notes, Some("Initial import".to_string()));
        assert!(version.id.is_some());
        assert!(version.imported_at.contains("T")); // ISO 8601 format
    }

    #[test]
    fn test_list_versions_chronological() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Track multiple versions
        track_version(&conn, "kicad", "Resistors", 40, Some("v1")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        track_version(&conn, "kicad", "Resistors", 45, Some("v2")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        track_version(&conn, "kicad", "Resistors", 50, Some("v3")).unwrap();

        let versions = list_versions(&conn, "kicad", "Resistors").unwrap();
        assert_eq!(versions.len(), 3);

        // Should be newest first
        assert_eq!(versions[0].component_count, 50);
        assert_eq!(versions[1].component_count, 45);
        assert_eq!(versions[2].component_count, 40);
    }

    #[test]
    fn test_latest_version() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // No versions yet
        let latest = latest_version(&conn, "kicad", "Resistors").unwrap();
        assert!(latest.is_none());

        // Track versions
        track_version(&conn, "kicad", "Resistors", 40, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        track_version(&conn, "kicad", "Resistors", 50, Some("Latest")).unwrap();

        let latest = latest_version(&conn, "kicad", "Resistors").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().component_count, 50);
    }

    #[test]
    fn test_associate_step_model() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert library and component
        let library = LibraryInfo {
            source: "kicad".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        let component = Component {
            id: ComponentId::new("kicad", "R_0805"),
            library: "TestLib".to_string(),
            category: None,
            footprint_data: None,
            metadata: ComponentMetadata::default(),
        };
        insert_component(&conn, &component).unwrap();

        // Associate STEP model
        associate_step_model(&conn, "kicad", "R_0805", "/models/r_0805.step").unwrap();

        // Retrieve STEP model path
        let path = get_step_model_path(&conn, "kicad", "R_0805").unwrap();
        assert_eq!(path, Some("/models/r_0805.step".to_string()));
    }

    #[test]
    fn test_associate_step_model_not_found() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Try to associate with nonexistent component
        let result = associate_step_model(&conn, "kicad", "NonExistent", "/models/test.step");

        assert!(result.is_err());
        match result {
            Err(LibraryError::NotFound(msg)) => {
                assert!(msg.contains("NonExistent"));
            }
            _ => panic!("Expected NotFound error"),
        }
    }

    #[test]
    fn test_get_component_metadata() {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();

        // Insert library and component with metadata
        let library = LibraryInfo {
            source: "kicad".to_string(),
            name: "TestLib".to_string(),
            path: None,
            version: None,
            enabled: true,
            component_count: 0,
        };
        insert_library(&conn, &library).unwrap();

        let component = Component {
            id: ComponentId::new("kicad", "R_0805"),
            library: "TestLib".to_string(),
            category: Some("Resistors".to_string()),
            footprint_data: None,
            metadata: ComponentMetadata {
                description: Some("0805 Resistor".to_string()),
                datasheet_url: Some("https://example.com/r0805.pdf".to_string()),
                manufacturer: Some("Yageo".to_string()),
                mpn: Some("RC0805FR-0710KL".to_string()),
                value: Some("10k".to_string()),
                package: Some("0805".to_string()),
                step_model_path: None,
            },
        };
        insert_component(&conn, &component).unwrap();

        // Retrieve metadata
        let metadata = get_component_metadata(&conn, "kicad", "R_0805").unwrap();
        assert!(metadata.is_some());

        let metadata = metadata.unwrap();
        assert_eq!(metadata.description, Some("0805 Resistor".to_string()));
        assert_eq!(metadata.manufacturer, Some("Yageo".to_string()));
        assert_eq!(metadata.value, Some("10k".to_string()));
    }

    /// `A` and `B` each hold `R_0805`, made by `from A` and `from B`. `A`
    /// alone holds `R_ONLY`.
    fn two_libraries_with_one_name() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize_schema(&conn).unwrap();
        for (library, name) in [("A", "R_0805"), ("A", "R_ONLY"), ("B", "R_0805")] {
            insert_library(
                &conn,
                &LibraryInfo {
                    source: "kicad".to_string(),
                    name: library.to_string(),
                    path: None,
                    version: None,
                    enabled: true,
                    component_count: 0,
                },
            )
            .unwrap();
            insert_component(
                &conn,
                &Component {
                    id: ComponentId::new("kicad", name),
                    library: library.to_string(),
                    category: None,
                    footprint_data: None,
                    metadata: ComponentMetadata {
                        manufacturer: Some(format!("from {library}")),
                        ..Default::default()
                    },
                },
            )
            .unwrap();
        }
        conn
    }

    /// (library, name, STEP model path) of each row, in key order.
    fn step_models(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        conn.prepare("SELECT library, name, step_model_path FROM components ORDER BY library, name")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn some(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    /// By source and name alone, a read got whichever library came first
    /// and an edit changed the row in every library that held the name.
    #[test]
    fn every_lookup_finds_the_component_its_library_key_names() {
        let conn = two_libraries_with_one_name();
        let made_by = |written: &str| {
            get_component_metadata(&conn, "kicad", written)
                .unwrap()
                .and_then(|metadata| metadata.manufacturer)
        };
        assert_eq!(made_by("A:R_0805"), some("from A"));
        assert_eq!(made_by("B:R_0805"), some("from B"));
        assert_eq!(made_by("R_ONLY"), some("from A"));
        assert_eq!(made_by("B:R_ONLY"), None);

        associate_step_model(&conn, "kicad", "B:R_0805", "b.step").unwrap();
        associate_step_model(&conn, "kicad", "R_ONLY", "only.step").unwrap();
        let step = |written: &str| get_step_model_path(&conn, "kicad", written).unwrap();
        assert_eq!(step("A:R_0805"), None);
        assert_eq!(step("B:R_0805"), some("b.step"));
        assert_eq!(step("A:R_ONLY"), some("only.step"));
        assert_eq!(
            step_models(&conn),
            [
                ("A".to_string(), "R_0805".to_string(), None),
                ("A".to_string(), "R_ONLY".to_string(), some("only.step")),
                ("B".to_string(), "R_0805".to_string(), some("b.step")),
            ]
        );

        match associate_step_model(&conn, "kicad", "B:R_ONLY", "none.step") {
            Err(LibraryError::NotFound(message)) => assert!(message.contains("B:R_ONLY")),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_name_two_libraries_hold_is_ambiguous_and_changes_nothing() {
        let conn = two_libraries_with_one_name();
        associate_step_model(&conn, "kicad", "B:R_0805", "b.step").unwrap();
        let before = step_models(&conn);

        fn candidates<T: std::fmt::Debug>(result: Result<T, LibraryError>) -> Vec<String> {
            match result {
                Err(LibraryError::Ambiguous { candidates, .. }) => candidates,
                other => panic!("expected Ambiguous, got {other:?}"),
            }
        }
        let both = ["kicad::A:R_0805", "kicad::B:R_0805"];
        assert_eq!(
            candidates(associate_step_model(&conn, "kicad", "R_0805", "bare.step")),
            both
        );
        assert_eq!(
            candidates(get_step_model_path(&conn, "kicad", "R_0805")),
            both
        );
        assert_eq!(
            candidates(get_component_metadata(&conn, "kicad", "R_0805")),
            both
        );
        assert_eq!(step_models(&conn), before);
    }
}
