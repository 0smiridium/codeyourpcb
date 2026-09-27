use thiserror::Error;

/// Library management error types
#[derive(Debug, Error)]
pub enum LibraryError {
    /// S-expression parsing errors
    #[error("Parse error: {0}")]
    Parse(String),

    /// SQLite database errors
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// File I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Unsupported operation
    #[error("Operation not supported: {0}")]
    NotSupported(String),

    /// KiCad version too old
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(String),

    /// JLCPCB or other API errors
    #[error("API error: {0}")]
    ApiError(String),

    /// Library or component not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// A component the index did not write, with the reason
    #[error("{0}")]
    NotIndexed(String),

    /// A bare name that more than one library holds, with each one written
    /// in full
    #[error(
        "'{written}' is in more than one library: {}; write the one you mean",
        .candidates.join(", ")
    )]
    Ambiguous {
        /// The name as the design wrote it.
        written: String,
        /// Every component it could mean, as `source::library:name`.
        candidates: Vec<String>,
    },
}
