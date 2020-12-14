// log_collector/mod.rs
use std::collections::hash_map::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

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

#[derive(Debug)]
pub struct LogEntry {
    pub path: PathBuf,
    pub line: String,
}

pub struct Collector {
    root_path: PathBuf,
    root_wd: WatchDescriptor,
    live_files: HashMap<WatchDescriptor, LiveFile>,
    inotify: Inotify,
}

impl Collector {
    pub fn initialize(root_path: &Path) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;

        debug!("Initialising watch on root path {:?}", root_path);
        let root_wd = inotify.add_watch(root_path, WatchMask::CREATE)?;

        let mut collector = Self {
            root_path: root_path.to_path_buf(),
            root_wd,
            live_files: HashMap::new(),
            inotify,
        };

        for entry in fs::read_dir(root_path)? {
            let entry = entry?;
            let path = entry.path();

            debug!("{}", Event::Create { path: path.clone() });
            collector.handle_event_create(path)?;
        }

        Ok(collector)
    }

    pub fn collect_entries(&mut self, buffer: &mut [u8]) -> io::Result<Vec<LogEntry>> {
        let inotify_events = self.inotify.read_events_blocking(buffer)?;
        let mut entries = Vec::new();

        for inotify_event in inotify_events {
            trace!("Received inotify event: {:?}", inotify_event);

            if let Some(event) = self.check_event(inotify_event)? {
                debug!("{}", event);

                let live_file = match event {
                    Event::Create { path } => self.handle_event_create(path)?,
                    Event::Append { live_file } => live_file,
                    Event::Truncate { live_file } => {
                        Self::handle_event_truncate(live_file)?;
                        live_file
                    }
                };

                while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
                    if live_file.entry_buf.ends_with('\n') {
                        live_file.entry_buf.pop();
                        let entry = LogEntry {
                            path: live_file.path.clone(),
                            line: live_file.entry_buf.clone(),
                        };
                        entries.push(entry);

                        live_file.entry_buf.clear();
                    }
                }
            }
        }

        Ok(entries)
    }

    fn check_event<'ev>(
        &mut self,
        inotify_event: inotify::Event<&'ev OsStr>,
    ) -> io::Result<Option<Event>> {
        if inotify_event.wd == self.root_wd {
            if !inotify_event.mask.contains(EventMask::CREATE) {
                warn!(
                    "Received unexpected event for root fd: {:?}",
                    inotify_event.mask
                );
                return Ok(None);
            }

            let name = match inotify_event.name {
                None => {
                    warn!("Received CREATE event for root fd without a name");
                    return Ok(None);
                }
                Some(name) => name,
            };

            let mut path = PathBuf::with_capacity(self.root_path.capacity() + name.len());
            path.push(&self.root_path);
            path.push(name);

            return Ok(Some(Event::Create { path }));
        }

        let live_file = match self.live_files.get_mut(&inotify_event.wd) {
            None => {
                warn!(
                    "Received event for unregistered watch descriptor: {:?} {:?}",
                    inotify_event.mask, inotify_event.wd
                );
                return Ok(None);
            }
            Some(live_file) => live_file,
        };

        let metadata = live_file.reader.get_ref().metadata()?;
        let seekpos = live_file.reader.seek(io::SeekFrom::Current(0))?;

        if seekpos <= metadata.len() {
            Ok(Some(Event::Append { live_file }))
        } else {
            Ok(Some(Event::Truncate { live_file }))
        }
    }

    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
        let realpath = fs::canonicalize(&path)?;

        let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
        let mut reader = BufReader::new(File::open(realpath)?);
        reader.seek(io::SeekFrom::End(0))?;

        self.live_files.insert(
            wd.clone(),
            LiveFile {
                path,
                reader,
                entry_buf: String::new(),
            },
        );
        Ok(self.live_files.get_mut(&wd).unwrap())
    }

    fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
        live_file.reader.seek(io::SeekFrom::Start(0))?;
        live_file.entry_buf.clear();
        Ok(())
    }
}
