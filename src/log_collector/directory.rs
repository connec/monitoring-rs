use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

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
            Event::Append { live_file, .. } => &live_file.path,
            Event::Truncate { live_file, .. } => &live_file.path,
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

            for event in self.check_event(watcher_event)? {
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

    fn check_event(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
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
        if self.entry_buf.len() == 0 {
            let entries = match self.collect_entries() {
                Ok(entries) => entries,
                Err(error) => return Some(Err(error)),
            };
            self.entry_buf = entries.into_iter();
        }
        Some(Ok(self.entry_buf.next()?))
    }
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;

    use crate::log_collector::watcher::watcher;
    use crate::LogEntry;

    use super::Collector;

    #[test]
    fn collect_entries_empty_file() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let mut collector =
            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
                .expect("unable to initialize collector");

        let mut file_path = tempdir.path().to_path_buf();
        file_path.push("test.log");
        File::create(file_path).expect("failed to create temp file");

        let entries = collector
            .collect_entries()
            .expect("failed to collect entries");
        assert_eq!(entries, Vec::<LogEntry>::new());
    }

    #[test]
    fn collect_entries_nonempty_file() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let mut collector =
            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
                .expect("unable to initialize collector");

        let mut file_path = tempdir.path().to_path_buf();
        file_path.push("test.log");
        let mut file = File::create(&file_path).expect("failed to create temp file");

        collector
            .collect_entries()
            .expect("failed to collect entries");

        writeln!(file, "hello?").expect("failed to write to file");
        writeln!(file, "world!").expect("failed to write to file");

        let entries = collector
            .collect_entries()
            .expect("failed to collect entries");
        let expected_path = fs::canonicalize(file_path)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let expected_entries = vec![
            LogEntry {
                line: "hello?".to_string(),
                metadata: vec![("path".to_string(), expected_path.clone())]
                    .into_iter()
                    .collect(),
            },
            LogEntry {
                line: "world!".to_string(),
                metadata: vec![("path".to_string(), expected_path)]
                    .into_iter()
                    .collect(),
            },
        ];
        assert_eq!(entries, expected_entries);
    }

    #[test]
    fn iterator_yields_entries() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let mut collector =
            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
                .expect("unable to initialize collector");

        let mut file_path = tempdir.path().to_path_buf();
        file_path.push("test.log");
        let mut file = File::create(file_path).expect("failed to create temp file");

        collector
            .collect_entries()
            .expect("failed to collect entries");

        writeln!(file, "hello?").expect("failed to write to file");
        writeln!(file, "world!").expect("failed to write to file");

        let entry = collector
            .next()
            .expect("expected at least 1 entry")
            .expect("failed to collect entries");
        assert_eq!(entry.line, "hello?".to_string());

        let entry = collector
            .next()
            .expect("expected at least 2 entries")
            .expect("failed to collect entries");
        assert_eq!(entry.line, "world!".to_string());
    }
}
