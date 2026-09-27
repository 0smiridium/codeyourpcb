-- The library index as `cypcb library import` wrote it before the library
-- was part of a footprint's key (PRAGMA user_version 0). Tests build a file
-- in this schema to check that the CLI moves it and the editor leaves it.
CREATE TABLE libraries (
    source TEXT NOT NULL, name TEXT NOT NULL, path TEXT, version TEXT,
    enabled INTEGER NOT NULL DEFAULT 1, component_count INTEGER DEFAULT 0,
    PRIMARY KEY (source, name));
CREATE TABLE components (
    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL, name TEXT NOT NULL, library TEXT NOT NULL,
    category TEXT, footprint_data TEXT, description TEXT, datasheet_url TEXT,
    manufacturer TEXT, mpn TEXT, value TEXT, package TEXT, step_model_path TEXT,
    metadata_json TEXT,
    UNIQUE(source, name),
    FOREIGN KEY (source, library) REFERENCES libraries(source, name));
CREATE INDEX idx_components_category ON components(category);
CREATE INDEX idx_components_manufacturer ON components(manufacturer);
CREATE INDEX idx_components_value ON components(value);
CREATE VIRTUAL TABLE components_fts USING fts5(
    source, name, category, description, manufacturer, mpn, value, package);
CREATE TRIGGER components_ai AFTER INSERT ON components BEGIN
    INSERT INTO components_fts(source, name, category, description, manufacturer, mpn, value, package)
    VALUES (new.source, new.name, new.category, new.description, new.manufacturer, new.mpn, new.value, new.package);
END;
CREATE TRIGGER components_ad AFTER DELETE ON components BEGIN
    DELETE FROM components_fts WHERE source = old.source AND name = old.name;
END;
