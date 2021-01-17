// src/log_database/mod.rs
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::LogEntry;

const DATA_FILE_EXTENSION: &str = "dat";
const METADATA_FILE_EXTENSION: &str = "json";
const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

pub struct Config {
    pub data_directory: PathBuf,
}

enum FileType {
    DataFile,
    MetadataFile,
}

pub struct Database {
    data_directory: PathBuf,
    files: HashMap<String, File>,
    index: HashMap<(String, String), HashSet<String>>,
}

impl Database {
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

                    for meta in metadata.into_iter() {
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

    pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
        let key = Self::hash(&entry.metadata);

        for meta in entry.metadata.iter() {
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

        let (file, needs_delimeter) = match self.files.get_mut(&key) {
            Some(file) => (file, true),
            None => {
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
            }
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
        let mut digest = [0u8; 16];
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
pub mod test {
    use tempfile::TempDir;

    use super::Config;
    use super::Database;

    pub fn open_temp_database() -> (Database, TempDir) {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let config = Config {
            data_directory: tempdir.path().to_path_buf(),
        };
        (
            Database::open(config).expect("unable to open database"),
            tempdir,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::LogEntry;

    use super::test::open_temp_database;
    use super::{Config, Database};

    #[test]
    fn test_new_db() {
        let (mut database, _tempdir) = open_temp_database();
        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
            .into_iter()
            .collect();

        assert_eq!(
            database
                .query("foo", "bar")
                .expect("unable to read from database"),
            None
        );

        database
            .write(&LogEntry {
                line: "line1".into(),
                metadata: metadata.clone(),
            })
            .expect("unable to write to database");
        assert_eq!(
            database
                .query("foo", "bar")
                .expect("unable to read from database"),
            Some(vec!["line1".to_string()])
        );

        database
            .write(&LogEntry {
                line: "line2".into(),
                metadata,
            })
            .expect("unable to write to database");
        assert_eq!(
            database
                .query("foo", "bar")
                .expect("unable to read from database"),
            Some(vec!["line1".to_string(), "line2".to_string()])
        );
    }

    #[test]
    fn test_existing_db() {
        let (mut database, _tempdir) = open_temp_database();
        let data_directory = database.data_directory.clone();
        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
            .into_iter()
            .collect();

        database
            .write(&LogEntry {
                line: "line1".into(),
                metadata: metadata.clone(),
            })
            .expect("failed to write to database");
        database
            .write(&LogEntry {
                line: "line2".into(),
                metadata,
            })
            .expect("failed to write to database");
        drop(database);

        let config = Config { data_directory };
        let database = Database::open(config).expect("unable to open database");

        assert_eq!(
            database
                .query("foo", "bar")
                .expect("unable to read from database"),
            Some(vec!["line1".to_string(), "line2".to_string()])
        );
    }

    #[test]
    fn test_metadata() {
        let (mut database, _tempdir) = open_temp_database();

        database
            .write(&LogEntry {
                line: "line1".into(),
                metadata: HashMap::new(),
            })
            .expect("failed to write to database");

        database
            .write(&LogEntry {
                line: "line2".into(),
                metadata: vec![("hello".to_string(), "world".to_string())]
                    .into_iter()
                    .collect(),
            })
            .expect("failed to write to database");

        database
            .write(&LogEntry {
                line: "line3".into(),
                metadata: vec![("hello".to_string(), "foo".to_string())]
                    .into_iter()
                    .collect(),
            })
            .expect("failed to write to database");

        assert_eq!(
            database
                .query("hello", "world")
                .expect("failed to query database"),
            Some(vec!["line2".to_string()])
        );
    }
}
