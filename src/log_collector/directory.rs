//! A log collector that watches a directory of log files.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

use log::{debug, trace, warn};

use crate::LogEntry;

use super::watcher::{self, watcher, Watcher};

#[derive(Debug)]
enum Event<'collector> {
    Create { path: PathBuf },
    Append { live_file: &'collector mut LiveFile },
    Truncate { live_file: &'collector mut LiveFile },
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
            Event::Create { path } => path,
            Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => &live_file.path,
        }
    }
}

impl std::fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.name(), self.path().display())
    }
}

#[derive(Debug)]
struct LiveFile {
    path: PathBuf,
    reader: BufReader<File>,
    entry_buf: String,
}

struct Collector<W: Watcher> {
    root_path: PathBuf,
    root_wd: watcher::Descriptor,
    live_files: HashMap<watcher::Descriptor, LiveFile>,
    watched_files: HashMap<PathBuf, watcher::Descriptor>,
    watcher: W,
    entry_buf: std::vec::IntoIter<LogEntry>,
}

/// # Errors
///
/// Propagates any `io::Error`s that occur during initialization.
pub fn initialize(root_path: &Path) -> io::Result<impl super::Collector> {
    let watcher = watcher()?;
    Collector::initialize(root_path, watcher)
}

impl<W: Watcher> Collector<W> {
    fn initialize(root_path: &Path, mut watcher: W) -> io::Result<Self> {
        debug!("Initialising watch on root path {:?}", root_path);
        let root_wd = watcher.watch_directory(root_path)?;

        let mut collector = Self {
            root_path: root_path.to_path_buf(),
            root_wd,
            live_files: HashMap::new(),
            watched_files: HashMap::new(),
            watcher,
            entry_buf: vec![].into_iter(),
        };

        for entry in fs::read_dir(root_path)? {
            let entry = entry?;
            let path = fs::canonicalize(entry.path())?;

            debug!("{}", Event::Create { path: path.clone() });
            collector.handle_event_create(path)?;
        }

        Ok(collector)
    }

    fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
        let watcher_events = self.watcher.read_events_blocking()?;

        let mut entries = Vec::new();
        let mut read_file = |live_file: &mut LiveFile| -> io::Result<()> {
            while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
                if live_file.entry_buf.ends_with('\n') {
                    live_file.entry_buf.pop();
                    let mut metadata = HashMap::new();
                    metadata.insert(
                        "path".to_string(),
                        live_file.path.to_string_lossy().into_owned(),
                    );
                    let entry = LogEntry {
                        line: live_file.entry_buf.clone(),
                        metadata,
                    };
                    entries.push(entry);

                    live_file.entry_buf.clear();
                }
            }
            Ok(())
        };

        for watcher_event in watcher_events {
            trace!("Received inotify event: {:?}", watcher_event);

            let mut new_paths = Vec::new();

            for event in self.check_event(&watcher_event)? {
                debug!("{}", event);

                let live_file = match event {
                    Event::Create { path } => {
                        new_paths.push(path);
                        continue;
                    }
                    Event::Append { live_file } => live_file,
                    Event::Truncate { live_file } => {
                        Self::handle_event_truncate(live_file)?;
                        live_file
                    }
                };

                read_file(live_file)?;
            }

            for path in new_paths {
                let live_file = self.handle_event_create(path)?;
                read_file(live_file)?;
            }
        }

        Ok(entries)
    }

    fn check_event(&mut self, watcher_event: &watcher::Event) -> io::Result<Vec<Event>> {
        if watcher_event.descriptor == self.root_wd {
            let mut events = Vec::new();

            for entry in fs::read_dir(&self.root_path)? {
                let entry = entry?;
                let path = fs::canonicalize(entry.path())?;

                if !self.watched_files.contains_key(&path) {
                    events.push(Event::Create { path });
                }
            }

            return Ok(events);
        }

        let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
            None => {
                warn!(
                    "Received event for unregistered watch descriptor: {:?}",
                    watcher_event
                );
                return Ok(vec![]);
            }
            Some(live_file) => live_file,
        };

        let metadata = live_file.reader.get_ref().metadata()?;
        let seekpos = live_file.reader.seek(io::SeekFrom::Current(0))?;

        if seekpos <= metadata.len() {
            Ok(vec![Event::Append { live_file }])
        } else {
            Ok(vec![Event::Truncate { live_file }])
        }
    }

    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
        let wd = self.watcher.watch_file(&path)?;
        let mut reader = BufReader::new(File::open(&path)?);
        reader.seek(io::SeekFrom::End(0))?;

        self.live_files.insert(
            wd.clone(),
            LiveFile {
                path: path.clone(),
                reader,
                entry_buf: String::new(),
            },
        );
        self.watched_files.insert(path, wd.clone());
        Ok(self.live_files.get_mut(&wd).unwrap())
    }

    fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
        live_file.reader.seek(io::SeekFrom::Start(0))?;
        live_file.entry_buf.clear();
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
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::path::PathBuf;

    use tempfile::TempDir;

    use crate::log_collector::watcher::watcher;
    use crate::test::{self, log_entry};

    use super::Collector;

    #[test]
    fn collect_entries_empty_file() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;

        create_log_file(&tempdir)?;

        // A new file will trigger an event but return no entries.
        let entries = collector.collect_entries()?;
        assert_eq!(entries, vec![]);

        Ok(())
    }

    #[test]
    fn collect_entries_nonempty_file() -> test::Result {
        let tempdir = tempfile::tempdir()?;
        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;

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
        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;

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
        let mut path = fs::canonicalize(tempdir.path())?;
        path.push("test.log");

        let file = File::create(&path)?;

        Ok((path, file))
    }
}
