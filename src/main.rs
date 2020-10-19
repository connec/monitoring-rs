// main.rs
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

struct Event {
    event_type: EventType,
    path: PathBuf,
}

enum EventType {
    Modify,
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
            if let Some(event) = self.check_event(event) {
                let handler = match event.event_type {
                    EventType::Modify => Self::handle_event_modify,
                };
                handler(self, event.path)?;
            }
        }

        Ok(())
    }

    fn check_event<'ev>(&self, event: inotify::Event<&'ev OsStr>) -> Option<Event> {
        let event_type = if event.mask.contains(EventMask::MODIFY) {
            Some(EventType::Modify)
        } else {
            None
        }?;

        let name = event.name?;
        let mut path = PathBuf::with_capacity(self.path.capacity() + name.len());
        path.push(&self.path);
        path.push(name);

        Some(Event { event_type, path })
    }

    fn handle_event_modify(&mut self, path: PathBuf) -> io::Result<()> {
        if let Some(file) = self.live_files.get_mut(&path) {
            io::copy(file, &mut self.stdout)?;
        } else {
            let mut file = File::open(&path)?;

            use std::io::Seek;
            file.seek(io::SeekFrom::End(0))?;

            self.live_files.insert(path, file);
        }

        Ok(())
    }
}

fn main() -> io::Result<()> {
    let mut collector = Collector::new(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
