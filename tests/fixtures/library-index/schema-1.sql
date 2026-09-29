-- The library index as `cypcb library import` wrote it in schema 1
-- (PRAGMA user_version 1): keyed by library, with each metadata field kept
-- twice, in its column and in `metadata_json`. Tests build a file in this
-- schema to check that the CLI moves it and keeps the column's value.
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
PRAGMA user_version = 1;
