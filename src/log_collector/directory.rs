//! A log collector that watches a directory of log files.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

use log::{debug, trace, warn};

use crate::LogEntry;

use super::watcher::{watcher, Event as _, Watcher};

/// Configuration for [`initialize`].
pub struct Config {
    /// The root path from which to collect logs.
    pub root_path: PathBuf,
}

#[derive(Debug)]
#[allow(variant_size_differences)]
enum Event<'collector> {
    Create {
        path: PathBuf,
        canonical_path: PathBuf,
    },
    Append {
        watched_file: &'collector mut WatchedFile,
    },
    Truncate {
        watched_file: &'collector mut WatchedFile,
    },
}

impl Event<'_> {
    fn name(&self) -> &str {
        match self {
            Event::Create { .. } => "Create",
            Event::Append { .. } => "Append",
            Event::Truncate { .. } => "Truncate",
        }
    }

    fn path(&self) -> &Path {
        match self {
            Event::Create { path, .. } => path,
            Event::Append { watched_file, .. } | Event::Truncate { watched_file, .. } => {
                &watched_file.paths[0].as_ref()
            }
        }
    }
}

impl std::fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.name(), self.path().display())
    }
}

#[derive(Debug)]
struct WatchedFile {
    paths: Vec<String>,
    reader: BufReader<File>,
    entry_buf: String,
}

pub(super) struct Collector<W: Watcher> {
    root_path: PathBuf,
    root_wd: W::Descriptor,
    watched_files: HashMap<W::Descriptor, WatchedFile>,
    watched_paths: HashMap<PathBuf, W::Descriptor>,
    watcher: W,
    entry_buf: std::vec::IntoIter<LogEntry>,
}

/// Initialize a `Collector` that watches a directory of log files.
///
/// This will start a watch (using `inotify` or `kqueue`) on `config.root_path` and any files
/// therein. Whenever the files change, new lines are emitted as `LogEntry` records.
///
/// # Caveats
///
/// This collector does not reliably handle symlinks in the `root_path` to other files in the
/// `root_path`. In that situation, `LogEntry` records will have just one of the paths, and the
/// chosen path might change after restarts.
///
/// # Errors
///
/// Propagates any `io::Error`s that occur during initialization.
pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
    let watcher = watcher()?;
    Collector::initialize(config, watcher)
}

impl<W: Watcher> Collector<W> {
    pub(super) fn initialize(config: Config, mut watcher: W) -> io::Result<Self> {
        let Config { root_path } = config;

        debug!("Initialising watch on root path {:?}", root_path);
        let root_wd = watcher.watch_directory(&root_path.canonicalize()?)?;

        let mut collector = Self {
            root_path,
            root_wd,
            watched_files: HashMap::new(),
            watched_paths: HashMap::new(),
            watcher,
            entry_buf: vec![].into_iter(),
        };

        for entry in fs::read_dir(&collector.root_path)? {
            let entry = entry?;
            if collector.watched_paths.contains_key(&entry.path()) {
                continue;
            }

            let path = entry.path().to_path_buf();
            let canonical_path = path.canonicalize()?;

            debug!(
                "{}",
                Event::Create {
                    path: path.clone(),
                    canonical_path: canonical_path.clone(),
                }
            );
            collector.handle_event_create(path, canonical_path)?;
        }

        Ok(collector)
    }

    fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
        let watcher_events = self.watcher.read_events_blocking()?;

        let mut entries = Vec::new();
        let mut read_file = |watched_file: &mut WatchedFile| -> io::Result<()> {
            while watched_file.reader.read_line(&mut watched_file.entry_buf)? != 0 {
                if watched_file.entry_buf.ends_with('\n') {
                    watched_file.entry_buf.pop();

                    let mut metadata = HashMap::new();
                    for path in &watched_file.paths {
                        metadata.insert("path".to_string(), path.clone());
                        entries.push(LogEntry {
                            line: watched_file.entry_buf.clone(),
                            metadata: metadata.clone(),
                        });
                    }

                    watched_file.entry_buf.clear();
                }
            }
            Ok(())
        };

        for watcher_event in watcher_events {
            trace!("Received inotify event: {:?}", watcher_event);

            let mut new_paths = Vec::new();

            for event in self.check_event(&watcher_event)? {
                debug!("{}", event);

                let watched_file = match event {
                    Event::Create {
                        path,
                        canonical_path,
                    } => {
                        new_paths.push((path, canonical_path));
                        continue;
                    }
                    Event::Append { watched_file } => watched_file,
                    Event::Truncate { watched_file } => {
                        Self::handle_event_truncate(watched_file)?;
                        watched_file
                    }
                };

                read_file(watched_file)?;
            }

            for (path, canonical_path) in new_paths {
                let watched_file = self.handle_event_create(path, canonical_path)?;
                read_file(watched_file)?;
            }
        }

        Ok(entries)
    }

    fn check_event(&mut self, watcher_event: &W::Event) -> io::Result<Vec<Event>> {
        if watcher_event.descriptor() == &self.root_wd {
            let mut events = Vec::new();

            for entry in fs::read_dir(&self.root_path)? {
                let entry = entry?;
                if self.watched_paths.contains_key(&entry.path()) {
                    continue;
                }

                let path = entry.path().to_path_buf();
                let canonical_path = path.canonicalize()?;
                events.push(Event::Create {
                    path,
                    canonical_path,
                });
            }

            return Ok(events);
        }

        let watched_file = match self.watched_files.get_mut(watcher_event.descriptor()) {
            None => {
                warn!(
                    "Received event for unregistered watch descriptor: {:?}",
                    watcher_event
                );
                return Ok(vec![]);
            }
            Some(watched_file) => watched_file,
        };

        let metadata = watched_file.reader.get_ref().metadata()?;
        let seekpos = watched_file.reader.seek(io::SeekFrom::Current(0))?;

        if seekpos <= metadata.len() {
            Ok(vec![Event::Append { watched_file }])
        } else {
            Ok(vec![Event::Truncate { watched_file }])
        }
    }

    fn handle_event_create(
        &mut self,
        path: PathBuf,
        canonical_path: PathBuf,
    ) -> io::Result<&mut WatchedFile> {
        if let Some(wd) = self.watched_paths.get(&canonical_path) {
            let wd = wd.clone();

            // unwrap is safe because we any `wd` in `watched_paths` must be present in `watched_files`
            let watched_file = self.watched_files.get_mut(&wd).unwrap();
            watched_file.paths.push(path.to_string_lossy().to_string());

            self.watched_paths.insert(path, wd);
            Ok(watched_file)
        } else {
            let wd = self.watcher.watch_file(&canonical_path)?;

            let mut reader = BufReader::new(File::open(&canonical_path)?);
            reader.seek(io::SeekFrom::End(0))?;

            let mut paths = vec![path.to_string_lossy().to_string()];
            if canonical_path != path && canonical_path.starts_with(&self.root_path) {
                paths.push(canonical_path.to_string_lossy().to_string());
            }

            if canonical_path != path {
                self.watched_paths.insert(canonical_path, wd.clone());
            }
            self.watched_paths.insert(path, wd.clone());

            Ok(self.watched_files.entry(wd).or_insert(WatchedFile {
                paths,
                reader,
                entry_buf: String::new(),
            }))
        }
    }

    fn handle_event_truncate(watched_file: &mut WatchedFile) -> io::Result<()> {
        watched_file.reader.seek(io::SeekFrom::Start(0))?;
        watched_file.entry_buf.clear();
        Ok(())
    }
}

impl<W: Watcher> super::Collector for Collector<W> {}

impl<W: Watcher> Iterator for Collector<W> {
    type Item = Result<LogEntry, io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.entry_buf.len() == 0 {
            let entries = match self.collect_entries() {
                Ok(entries) => entries,
                Err(error) => return Some(Err(error)),
            };
            self.entry_buf = entries.into_iter();
        }
        // `unwrap` because we've refilled `entry_buf`
        Some(Ok(self.entry_buf.next().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::unix;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use crate::log_collector::watcher::{mock, watcher};
    use crate::test::{self, log_entry};

    use super::{Collector, Config};

    #[test]
    fn initialize_with_symlink() -> test::Result {
        let root_dir_parent = tempfile::tempdir()?;
        let logs_dir = tempfile::tempdir()?;

        let root_path = root_dir_parent.path().join("logs");
        unix::fs::symlink(logs_dir.path(), &root_path)?;

        let config = Config {
            root_path: root_path.clone(),
        };
        let mut watcher = mock::Watcher::new();
        let mut collector = Collector::initialize(config, watcher.clone())?;

        let file_path = watcher.simulate_new_file(&logs_dir.path().canonicalize()?)?;
        collector.collect_entries()?; // refresh known files

        watcher.simulate_write(&file_path, "hello?\n")?;

        let entries = collector.collect_entries()?;
        let expected_path = root_path.join(file_path.file_name().unwrap());
        assert_eq!(
            entries,
            vec![log_entry(
                "hello?",
                &[("path", expected_path.to_str().unwrap())]
            )]
        );

        Ok(())
    }

    #[test]
    fn file_with_external_symlink() -> test::Result {
        let root_dir = tempfile::tempdir()?;
        let logs_dir = tempfile::tempdir()?;

        let (src_path, _) = create_log_file(&logs_dir)?;
        let src_path_canonical = src_path.canonicalize()?;
        let dst_path = root_dir.path().join(src_path.file_name().unwrap());
        unix::fs::symlink(&src_path, &dst_path)?;

        let config = Config {
            root_path: root_dir.path().to_path_buf(),
        };
        let mut watcher = mock::Watcher::new();
        let mut collector = Collector::initialize(config, watcher.clone())?;

        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

        let entries = collector.collect_entries()?;
        assert_eq!(
            entries,
            vec![log_entry("hello?", &[("path", dst_path.to_str().unwrap())])]
        );

        Ok(())
    }

    #[test]
    fn file_with_internal_symlink() -> test::Result {
        let root_dir = tempfile::tempdir()?;
        let root_path = root_dir.path().canonicalize()?;

        let (src_path, _) = create_log_file(&root_dir)?;
        let src_path_canonical = src_path.canonicalize()?;
        let dst_path = root_path.join("linked.log");
        unix::fs::symlink(&src_path, &dst_path)?;

        let config = Config { root_path };
        let mut watcher = mock::Watcher::new();
        let mut collector = Collector::initialize(config, watcher.clone())?;

        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

        let entries = collector.collect_entries()?;
        assert_eq!(entries.len(), 2);

        let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
        assert!(
            entries.contains(&entry),
            "expected entry {:?}, but found: {:#?}",
            entry,
            entries
        );

        let entry = log_entry("hello?", &[("path", src_path_canonical.to_str().unwrap())]);
        assert!(
            entries.contains(&entry),
            "expected entry {:?}, but found: {:#?}",
            entry,
            entries
        );

        Ok(())
    }

    #[test]
    fn initialize_with_symlink_and_file_with_internal_symlink() -> test::Result {
        let root_dir_parent = tempfile::tempdir()?;
        let logs_dir = tempfile::tempdir()?;

        let root_path = root_dir_parent.path().join("logs");
        unix::fs::symlink(logs_dir.path(), &root_path)?;

        let (src_path, _) = create_log_file(&logs_dir)?;
        let src_path_canonical = src_path.canonicalize()?;
        let dst_path = root_path.join("linked.log");
        unix::fs::symlink(&src_path, &dst_path)?;

        let config = Config {
            root_path: root_path.clone(),
        };
        let mut watcher = mock::Watcher::new();
        let mut collector = Collector::initialize(config, watcher.clone())?;

        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

        let entries = collector.collect_entries()?;
        assert_eq!(entries.len(), 2);

        let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
        assert!(
            entries.contains(&entry),
            "expected entry {:?}, but found: {:#?}",
            entry,
            entries
        );

        let path = root_path.join(src_path.file_name().unwrap());
        let entry = log_entry("hello?", &[("path", path.to_str().unwrap())]);
        assert!(
            entries.contains(&entry),
            "expected entry {:?}, but found: {:#?}",
            entry,
            entries
        );

        Ok(())
    }

    #[test]
    fn collect_entries_empty_file() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let config = Config {
            root_path: tempdir.path().to_path_buf(),
        };
        let mut collector = Collector::initialize(config, watcher()?)?;

        create_log_file(&tempdir)?;

        // A new file will trigger an event but return no entries.
        let entries = collector.collect_entries()?;
        assert_eq!(entries, vec![]);

        Ok(())
    }

    #[test]
    fn collect_entries_nonempty_file() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let config = Config {
            root_path: tempdir.path().to_path_buf(),
        };
        let mut collector = Collector::initialize(config, watcher()?)?;

        let (file_path, mut file) = create_log_file(&tempdir)?;

        collector.collect_entries()?;

        writeln!(file, "hello?")?;
        writeln!(file, "world!")?;

        let entries = collector.collect_entries()?;
        assert_eq!(
            entries,
            vec![
                log_entry("hello?", &[("path", file_path.to_str().unwrap())]),
                log_entry("world!", &[("path", file_path.to_str().unwrap())]),
            ]
        );

        Ok(())
    }

    #[test]
    fn iterator_yields_entries() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let config = Config {
            root_path: tempdir.path().to_path_buf(),
        };
        let mut collector = Collector::initialize(config, watcher()?)?;

        let (file_path, mut file) = create_log_file(&tempdir)?;

        collector.collect_entries()?;

        writeln!(file, "hello?")?;
        writeln!(file, "world!")?;

        assert_eq!(
            collector.next().expect("expected at least 1 entry")?,
            log_entry("hello?", &[("path", file_path.to_str().unwrap())])
        );

        assert_eq!(
            collector.next().expect("expected at least 2 entries")?,
            log_entry("world!", &[("path", file_path.to_str().unwrap())])
        );

        Ok(())
    }

    fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
        let path = tempdir.path().join("test.log");
        let file = File::create(&path)?;

        Ok((path, file))
    }
}
