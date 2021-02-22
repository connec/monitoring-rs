// src/database/mod.rs
//! A time-series-esque database for storing and querying append-only streams of events.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// A time-series-esque database for storing and querying append-only stream of events.
#[derive(serde::Deserialize, serde::Serialize)]
pub struct Database {
    path: PathBuf,
    events: RefCell<Vec<(Labels, Event)>>,
}

/// A structure describing database queries.
pub enum Query {
    /// A query that will find events from streams with a particular label.
    Label {
        /// The label name to match.
        name: String,

        /// The label value to match.
        value: String,
    },
}

/// Labels used to identify a stream.
///
/// For now this is just a type alias, but our requirements may diverge from `BTreeMap` in future.
pub type Labels = BTreeMap<String, String>;

/// The type used for timestamps.
///
/// `u64` gives us ~585 million years at millisecond resolution. This is obviously more than we
/// need, but `u32` only gives us 50 days which is obviously too few!
///
/// This is not public. The alias just exists to make changing the timestamp type easier.
type Timestamp = u64;

/// An event that can be stored by [`Database`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Event {
    timestamp: Timestamp,
    data: Vec<u8>,
}

/// Possible error situations when opening a database.
#[derive(Debug)]
pub enum OpenError {
    /// An error occurred when trying to restore from an existing database.
    Restore(RestoreError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "error opening database")
    }
}

impl std::error::Error for OpenError {}

/// Possible error situations when restoring a database.
#[derive(Debug)]
pub enum RestoreError {
    /// An I/O error occurred when restoring (e.g. permission denied).
    ///
    /// This may be fixable by ensuring correct permissions etc.
    Io(std::io::Error),

    /// An error occurred when deserializing the database file.
    ///
    /// If this happens the database is corrupt and would need to be manually repaired or deleted.
    Deserialize(serde_json::Error),
}

/// Possible error situations when querying a database.
pub type QueryError = std::io::Error;

impl Database {
    /// Open a database at the given `path`.
    ///
    /// If `path` doesn't exist, it is created and an empty `Database` is constructed that will
    /// write its data to `path`. If `path` exists, a `Database` is restored from its contents and
    /// returned.
    ///
    /// # Errors
    ///
    /// - Any [`io::Error`]s that occur when reading or writing directories or files are propagated.
    /// - If `path` is not a directory, a [`NotDirectory`] error is returned.
    /// - If restoring from `path` fails, a [`RestoreError`] is returned.
    ///
    /// [`io::Error`]: std::io::Error
    /// [`NotDirectory`]: OpenError::NotDirectory
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        let path = path.as_ref();
        if path.exists() {
            let contents = fs::read(&path)
                .map_err(RestoreError::Io)
                .map_err(OpenError::Restore)?;
            serde_json::from_slice(&contents)
                .map_err(RestoreError::Deserialize)
                .map_err(OpenError::Restore)
        } else {
            Ok(Database {
                path: path.to_path_buf(),
                events: RefCell::new(Vec::new()),
            })
        }
    }

    /// Push a new `event` into the stream identified by `labels`.
    pub fn push(&self, labels: &Labels, event: Event) {
        self.events.borrow_mut().push((labels.clone(), event));
    }

    /// Find events in the database matching the given `query`.
    ///
    /// # Errors
    ///
    /// Any [`io::Error`]s encountered when running the query are returned.
    pub fn query(&self, query: &Query) -> Result<Vec<Event>, QueryError> {
        let results = match query {
            Query::Label { name, value } => self
                .events
                .borrow()
                .iter()
                .filter_map(|(labels, event)| {
                    if labels.get(name) == Some(value) {
                        Some(event.clone())
                    } else {
                        None
                    }
                })
                .collect(),
        };

        Ok(results)
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let file = File::create(&self.path).expect("create file");
        serde_json::to_writer(file, &self).expect("serialize database");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::os::unix::fs::PermissionsExt;

    use crate::test;

    use super::{Database, Event, OpenError, Query, RestoreError};

    #[test]
    fn fresh_database() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let db = Database::open(tempdir.path().join("data"))?;

        db.push(&make_labels(&[("l1", "v1")]), make_event(0, "e1"));
        db.push(&make_labels(&[("l1", "v2")]), make_event(1, "e2"));
        db.push(&make_labels(&[("l2", "v1")]), make_event(2, "e3"));

        let query = Query::Label {
            name: "l1".to_string(),
            value: "v2".to_string(),
        };
        assert_eq!(db.query(&query)?, vec![make_event(1, "e2")]);

        Ok(())
    }

    #[test]
    fn restored_database() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let db = Database::open(tempdir.path().join("data"))?;

        db.push(&make_labels(&[("l1", "v1")]), make_event(0, "e1"));
        db.push(&make_labels(&[("l1", "v2")]), make_event(1, "e2"));
        db.push(&make_labels(&[("l2", "v1")]), make_event(2, "e3"));
        drop(db);

        let db = Database::open(tempdir.path().join("data"))?;

        let query = Query::Label {
            name: "l1".to_string(),
            value: "v2".to_string(),
        };
        assert_eq!(db.query(&query)?, vec![make_event(1, "e2")]);

        Ok(())
    }

    #[test]
    fn restore_io_error() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let path = tempdir.path().join("data");

        // Make `Database::open` return an `io::Error` by making `data.json` unreadable.
        File::create(&path)?.set_permissions(fs::Permissions::from_mode(0o200))?;

        let error = Database::open(&path).err().unwrap();
        assert!(matches!(error, OpenError::Restore(RestoreError::Io(_))));
        assert_eq!(&format!("{}", error), "error opening database");

        Ok(())
    }

    #[test]
    fn restore_deserialize_error() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let path = tempdir.path().join("data");

        // Cause a deserialize error by writing invalid JSON.
        fs::write(&path, "oh dear")?;

        let error = Database::open(&path).err().unwrap();
        assert!(matches!(
            error,
            OpenError::Restore(RestoreError::Deserialize(_))
        ));
        assert_eq!(&format!("{}", error), "error opening database");

        Ok(())
    }

    fn make_labels(labels: &[(&str, &str)]) -> BTreeMap<String, String> {
        labels
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn make_event(timestamp: u64, data: impl AsRef<[u8]>) -> Event {
        Event {
            timestamp,
            data: data.as_ref().into(),
        }
    }
}
