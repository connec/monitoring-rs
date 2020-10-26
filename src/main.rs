// main.rs
#[macro_use]
extern crate log;

use std::collections::hash_map::{self, HashMap};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Seek, Stdout};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[derive(Debug)]
enum Event<'collector> {
    Create {
        entry: hash_map::VacantEntry<'collector, PathBuf, File>,
    },
    Append {
        entry: hash_map::OccupiedEntry<'collector, PathBuf, File>,
    },
    Truncate {
        entry: hash_map::OccupiedEntry<'collector, PathBuf, File>,
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
            Event::Create { entry } => entry.key(),
            Event::Append { entry } => entry.key(),
            Event::Truncate { entry } => entry.key(),
        }
    }
}

impl std::fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.name(), self.path().display())
    }
}

struct EventContext<'collector> {
    event: Event<'collector>,
    stdout: &'collector mut Stdout,
}

struct Collector {
    path: PathBuf,
    stdout: Stdout,
    live_files: HashMap<PathBuf, File>,
    inotify: Inotify,
}

impl Collector {
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;

        debug!("Initialising watch on path {:?}", &path);
        inotify.add_watch(path, WatchMask::MODIFY)?;

        Ok(Self {
            path: path.to_path_buf(),
            stdout: io::stdout(),
            live_files: HashMap::new(),
            inotify,
        })
    }

    pub fn handle_events(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let events = self.inotify.read_events_blocking(buffer)?;

        for event in events {
            trace!("Received event: {:?}", event);
            if let Some(mut context) = self.check_event(event)? {
                debug!("{}", context.event);
                match context.event {
                    Event::Create { entry } => Self::handle_event_create(entry)?,
                    Event::Append { entry } => {
                        Self::handle_event_append(entry, &mut context.stdout)?
                    }
                    Event::Truncate { entry } => {
                        Self::handle_event_truncate(entry, &mut context.stdout)?
                    }
                };
            }
        }

        Ok(())
    }

    fn check_event<'ev>(
        &mut self,
        event: inotify::Event<&'ev OsStr>,
    ) -> io::Result<Option<EventContext>> {
        if !event.mask.contains(EventMask::MODIFY) {
            return Ok(None);
        }

        let name = match event.name {
            None => return Ok(None),
            Some(name) => name,
        };
        let mut path = PathBuf::with_capacity(self.path.capacity() + name.len());
        path.push(&self.path);
        path.push(name);

        let event = match self.live_files.entry(path) {
            hash_map::Entry::Vacant(entry) => Event::Create { entry },
            hash_map::Entry::Occupied(mut entry) => {
                let metadata = entry.get().metadata()?;
                let seekpos = entry.get_mut().seek(io::SeekFrom::Current(0))?;

                if seekpos <= metadata.len() {
                    Event::Append { entry }
                } else {
                    Event::Truncate { entry }
                }
            }
        };

        Ok(Some(EventContext {
            event,
            stdout: &mut self.stdout,
        }))
    }

    fn handle_event_create(entry: hash_map::VacantEntry<'_, PathBuf, File>) -> io::Result<()> {
        let mut file = File::open(entry.key())?;
        file.seek(io::SeekFrom::End(0))?;

        entry.insert(file);

        Ok(())
    }

    fn handle_event_append(
        mut entry: hash_map::OccupiedEntry<'_, PathBuf, File>,
        stdout: &mut Stdout,
    ) -> io::Result<()> {
        io::copy(entry.get_mut(), stdout)?;

        Ok(())
    }

    fn handle_event_truncate(
        mut entry: hash_map::OccupiedEntry<'_, PathBuf, File>,
        stdout: &mut Stdout,
    ) -> io::Result<()> {
        entry.get_mut().seek(io::SeekFrom::Start(0))?;
        io::copy(entry.get_mut(), stdout)?;

        Ok(())
    }
}

fn main() -> io::Result<()> {
    env_logger::init();

    let mut collector = Collector::new(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
