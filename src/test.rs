// src/test.rs
use std::io;

use tempfile::TempDir;

use crate::log_database::{self, Database};
use crate::LogEntry;

/// A convenient alias to use `?` in tests.
///
/// There is a blanket `impl From<E: Error> for Box<dyn Error>`, meaning anything that implements
/// [`std::error::Error`] can be propagated using `?`.
pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;

/// Open a database in a temporary directory.
///
/// This returns the handle to the temporary directory as well as the database, since the directory
/// will be unlinked when the `TempDir` value is dropped.
///
/// # Errors
///
/// Propagates any `io::Error`s that occur when opening the database.
pub fn temp_database() -> io::Result<(TempDir, Database)> {
    let tempdir = tempfile::tempdir()?;
    let config = log_database::Config {
        data_directory: tempdir.path().to_path_buf(),
    };
    Ok((tempdir, Database::open(config)?))
}

/// Construct a `LogEntry` with the given `line` and `metadata`.
///
/// This is a convenience function to avoid having to build a `HashMap` for metadata.
#[must_use]
pub fn log_entry(line: &str, metadata: &[(&str, &str)]) -> LogEntry {
    LogEntry {
        line: line.to_string(),
        metadata: metadata
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
    }
}
