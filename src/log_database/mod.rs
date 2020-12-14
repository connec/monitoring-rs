// src/log_database/mod.rs
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::PathBuf;

const DATA_FILE_EXTENSION: &str = "dat";
const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

pub struct Config {
    pub data_directory: PathBuf,
}

pub struct Database {
    data_directory: PathBuf,
    files: HashMap<String, File>,
}

impl Database {
    pub fn open(config: Config) -> io::Result<Self> {
        let mut files = HashMap::new();
        for entry in fs::read_dir(&config.data_directory)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(OsStr::to_str) != Some(DATA_FILE_EXTENSION) {
                return Err(Self::error(format!(
                    "invalid data file {}: extension must be `{}`",
                    path.display(),
                    DATA_FILE_EXTENSION
                )));
            }

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

            files.insert(key_hash.to_string(), file);
        }
        Ok(Database {
            data_directory: config.data_directory,
            files,
        })
    }

    pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
        let mut file = match self.files.get(&Self::hash(key)) {
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

    pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
        let key_hash = Self::hash(key);
        let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
            Some(file) => (file, true),
            None => {
                let mut path = self.data_directory.clone();
                path.push(&key_hash);
                path.set_extension(DATA_FILE_EXTENSION);

                let file = OpenOptions::new()
                    .append(true)
                    .create(true)
                    .read(true)
                    .open(&path)?;

                // Using `.or_insert` here is annoying since we know there is no entry, but
                // `hash_map::entry::insert` is unstable
                // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
                let file = self.files.entry(key_hash).or_insert(file);

                (file, false)
            }
        };

        if needs_delimeter {
            file.write_all(&[DATA_FILE_RECORD_SEPARATOR])?;
        }
        file.write_all(line.as_ref())?;

        Ok(())
    }

    fn hash(key: &str) -> String {
        let digest = md5::compute(&key);
        format!("{:x}", digest)
    }

    fn error(message: String) -> io::Error {
        io::Error::new(io::ErrorKind::Other, message)
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, Database};

    #[test]
    fn test_new_db() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let config = Config {
            data_directory: tempdir.path().to_path_buf(),
        };
        let mut database = Database::open(config).expect("unable to open database");

        assert_eq!(
            database.read("foo").expect("unable to read from database"),
            None
        );

        database
            .write("foo", "line1")
            .expect("unable to write to database");
        assert_eq!(
            database.read("foo").expect("unable to read from database"),
            Some(vec!["line1".to_string()])
        );

        database
            .write("foo", "line2")
            .expect("unable to write to database");
        assert_eq!(
            database.read("foo").expect("unable to read from database"),
            Some(vec!["line1".to_string(), "line2".to_string()])
        );
    }

    #[test]
    fn test_existing_db() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");

        {
            let config = Config {
                data_directory: tempdir.path().to_path_buf(),
            };
            let mut database = Database::open(config).expect("unable to open database");

            database
                .write("foo", "line1")
                .expect("failed to write to database");
            database
                .write("foo", "line2")
                .expect("failed to write to database");

            drop(database);
        }

        let config = Config {
            data_directory: tempdir.path().to_path_buf(),
        };
        let database = Database::open(config).expect("unable to open database");

        assert_eq!(
            database.read("foo").expect("unable to read from database"),
            Some(vec!["line1".to_string(), "line2".to_string()])
        );
    }
}
