// src/log_database/mod.rs

//! The interface for log storage in `monitoring-rs`.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::LogEntry;

const DATA_FILE_EXTENSION: &str = "dat";
const METADATA_FILE_EXTENSION: &str = "json";
const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

/// The configuration needed to open a database.
pub struct Config {
    /// The directory in which the database should store its data.
    pub data_directory: PathBuf,
}

enum FileType {
    DataFile,
    MetadataFile,
}

/// A log database supporting key-value rerieval.
///
/// **Note:** the functionality of this database is extremely minimal just now, and is missing vital
/// features like retention management.
///
/// That said, it should be decently fast for storing and querying UTF-8 log entries with key-value
/// metadata (via [`LogEntry`](crate::LogEntry)).
///
/// - Log lines are stored in a flat file named with a hash of the entry's metadata. Log entry
///   metadata is stored in JSON files with the same base name. Handles to all log files are kept
///   open in memory. An in-memory index is maintained for all `(key, value)` pairs of metadata to
///   the set of log files that include that metadata.
/// - Writes append a new line to the relevant file, creating a new log file and metadata file if
///   necessary (and updating the index if so).
/// - Reads are performed using a `key=value` pair. The index is used to identify the files that
///   contain relevant records, and these files are then scanned in their entirety.
///
/// The structure, interface, and storage approach of the database is likely to change in future.
pub struct Database {
    data_directory: PathBuf,
    files: HashMap<String, File>,
    index: HashMap<(String, String), HashSet<String>>,
}

impl Database {
    /// # Errors
    ///
    /// Propagates any `io::Error` that ocurrs when opening the database.
    pub fn open(config: Config) -> io::Result<Self> {
        let mut files = HashMap::new();
        let mut index = HashMap::new();
        for entry in fs::read_dir(&config.data_directory)? {
            let entry = entry?;
            let path = entry.path();

            let extension = path.extension().and_then(OsStr::to_str);
            let file_type = match extension {
                Some(DATA_FILE_EXTENSION) => FileType::DataFile,
                Some(METADATA_FILE_EXTENSION) => FileType::MetadataFile,
                _ => {
                    return Err(Self::error(format!(
                        "invalid data file {}: extension must be `{}` or `{}`",
                        path.display(),
                        DATA_FILE_EXTENSION,
                        METADATA_FILE_EXTENSION
                    )))
                }
            };

            let metadata = fs::metadata(&path)?;
            if !metadata.is_file() {
                return Err(Self::error(format!(
                    "invalid data file {}: not a file",
                    path.display()
                )));
            }

            let key_hash = path.file_stem().ok_or_else(|| {
                Self::error(format!(
                    "invalid data file name {}: empty file stem",
                    path.display()
                ))
            })?;

            let key_hash = key_hash.to_str().ok_or_else(|| {
                Self::error(format!(
                    "invalid data file name {}: non-utf8 file name",
                    path.display()
                ))
            })?;

            let file = OpenOptions::new().append(true).read(true).open(&path)?;
            match file_type {
                FileType::DataFile => {
                    files.insert(key_hash.to_string(), file);
                }
                FileType::MetadataFile => {
                    let metadata = serde_json::from_reader(file)?;
                    let key = Self::hash(&metadata);

                    for meta in metadata {
                        let keys = index
                            .entry((meta.0.to_string(), meta.1.to_string()))
                            .or_insert_with(|| HashSet::with_capacity(1));

                        if !keys.contains(&key) {
                            keys.insert(key.clone());
                        }
                    }
                }
            }
        }
        Ok(Database {
            data_directory: config.data_directory,
            files,
            index,
        })
    }

    /// # Errors
    ///
    /// Propagates any `io::Error` that occurs when querying the database.
    pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
        let keys = match self.index.get(&(key.to_string(), value.to_string())) {
            None => return Ok(None),
            Some(keys) => keys,
        };

        let mut lines = Vec::new();
        for key in keys {
            if let Some(lines_) = self.read(key)? {
                lines.extend(lines_);
            }
        }

        Ok(Some(lines))
    }

    /// # Errors
    ///
    /// Propagates any `io::Error` that occurs when querying the database.
    pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
        let key = Self::hash(&entry.metadata);

        for meta in &entry.metadata {
            let keys = self
                .index
                .entry((meta.0.to_string(), meta.1.to_string()))
                .or_insert_with(|| HashSet::with_capacity(1));

            // We'd ideally use `HashSet::get_or_insert_owned`, but it's currently unstable
            // ([#60896](https://github.com/rust-lang/rust/issues/60896)).
            if !keys.contains(&key) {
                keys.insert(key.clone());
            }
        }

        let (file, needs_delimeter) = if let Some(file) = self.files.get_mut(&key) {
            (file, true)
        } else {
            let mut entry_path = self.data_directory.clone();
            entry_path.push(&key);

            let mut metadata_path = entry_path;
            metadata_path.set_extension(METADATA_FILE_EXTENSION);
            fs::write(&metadata_path, serde_json::to_vec(&entry.metadata)?)?;

            let mut data_path = metadata_path;
            data_path.set_extension(DATA_FILE_EXTENSION);

            let file = OpenOptions::new()
                .append(true)
                .create(true)
                .read(true)
                .open(&data_path)?;

            // Using `.or_insert` here is annoying since we know there is no entry, but
            // `hash_map::entry::insert` is unstable
            // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
            let file = self.files.entry(key).or_insert(file);

            (file, false)
        };

        if needs_delimeter {
            file.write_all(&[DATA_FILE_RECORD_SEPARATOR])?;
        }
        file.write_all(entry.line.as_ref())?;

        Ok(())
    }

    fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
        let mut file = match self.files.get(key) {
            Some(file) => file,
            None => return Ok(None),
        };

        file.seek(SeekFrom::Start(0))?;
        let mut reader = BufReader::new(file);
        let mut lines = Vec::new();

        loop {
            let mut line_bytes = Vec::new();
            let bytes_read = reader.read_until(DATA_FILE_RECORD_SEPARATOR, &mut line_bytes)?;
            if bytes_read == 0 {
                break;
            }
            if line_bytes.last() == Some(&DATA_FILE_RECORD_SEPARATOR) {
                line_bytes.pop();
            }
            let line = String::from_utf8(line_bytes).map_err(|error| {
                Self::error(format!(
                    "corrupt data file for key {}: invalid utf8: {}",
                    key, error
                ))
            })?;
            lines.push(line);
        }

        Ok(Some(lines))
    }

    fn hash(metadata: &HashMap<String, String>) -> String {
        let mut digest = [0_u8; 16];
        for (key, value) in metadata.iter() {
            let mut context = md5::Context::new();
            context.consume(key);
            context.consume(value);
            let entry_digest = context.compute();

            for (digest_byte, entry_byte) in digest.iter_mut().zip(entry_digest.iter()) {
                *digest_byte ^= entry_byte;
            }
        }
        format!("{:x}", md5::Digest(digest))
    }

    fn error(message: String) -> io::Error {
        io::Error::new(io::ErrorKind::Other, message)
    }
}

#[cfg(test)]
mod tests {
    use crate::test::{self, log_entry, temp_database};

    use super::{Config, Database};

    #[test]
    fn test_new_db() -> test::Result {
        let (_tempdir, mut database) = temp_database()?;

        assert_eq!(database.query("foo", "bar")?, None);

        database.write(&log_entry("line1", &[("foo", "bar")]))?;
        assert_eq!(
            database.query("foo", "bar")?,
            Some(vec!["line1".to_string()])
        );

        database.write(&log_entry("line2", &[("foo", "bar")]))?;
        assert_eq!(
            database.query("foo", "bar")?,
            Some(vec!["line1".to_string(), "line2".to_string()])
        );

        Ok(())
    }

    #[test]
    fn test_existing_db() -> test::Result {
        let (tempdir, mut database) = temp_database()?;

        database.write(&log_entry("line1", &[("foo", "bar")]))?;
        database.write(&log_entry("line2", &[("foo", "bar")]))?;
        drop(database);

        let config = Config {
            data_directory: tempdir.path().to_path_buf(),
        };
        let database = Database::open(config)?;

        assert_eq!(
            database.query("foo", "bar")?,
            Some(vec!["line1".to_string(), "line2".to_string()])
        );

        Ok(())
    }

    #[test]
    fn test_query_metadata() -> test::Result {
        let (_tempdir, mut database) = temp_database()?;

        database.write(&log_entry("line1", &[]))?;
        database.write(&log_entry("line2", &[("hello", "world")]))?;
        database.write(&log_entry("line2", &[("hello", "foo")]))?;

        assert_eq!(
            database.query("hello", "world")?,
            Some(vec!["line2".to_string()])
        );

        Ok(())
    }
}
