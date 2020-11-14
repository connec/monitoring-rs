// main.rs
#[macro_use]
extern crate log;

use std::collections::hash_map::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Seek, Stdout};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[derive(Debug)]
enum Event<'collector> {
    Create {
        path: PathBuf,
    },
    Append {
        stdout: &'collector mut Stdout,
        live_file: &'collector mut LiveFile,
    },
    Truncate {
        stdout: &'collector mut Stdout,
        live_file: &'collector mut LiveFile,
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
    file: File,
}

struct Collector {
    root_path: PathBuf,
    root_wd: WatchDescriptor,
    stdout: Stdout,
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
            stdout: io::stdout(),
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

    pub fn handle_events(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let inotify_events = self.inotify.read_events_blocking(buffer)?;

        for inotify_event in inotify_events {
            trace!("Received inotify event: {:?}", inotify_event);

            if let Some(event) = self.check_event(inotify_event)? {
                debug!("{}", event);

                match event {
                    Event::Create { path } => self.handle_event_create(path),
                    Event::Append { stdout, live_file } => {
                        Self::handle_event_append(stdout, &mut live_file.file)
                    }
                    Event::Truncate { stdout, live_file } => {
                        Self::handle_event_truncate(stdout, &mut live_file.file)
                    }
                }?;
            }
        }

        Ok(())
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

        let stdout = &mut self.stdout;
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

        let metadata = live_file.file.metadata()?;
        let seekpos = live_file.file.seek(io::SeekFrom::Current(0))?;

        if seekpos <= metadata.len() {
            Ok(Some(Event::Append { stdout, live_file }))
        } else {
            Ok(Some(Event::Truncate { stdout, live_file }))
        }
    }

    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<()> {
        let realpath = fs::canonicalize(&path)?;

        let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
        let mut file = File::open(realpath)?;
        file.seek(io::SeekFrom::End(0))?;

        self.live_files.insert(wd, LiveFile { path, file });

        Ok(())
    }

    fn handle_event_append(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
        io::copy(&mut file, stdout)?;

        Ok(())
    }

    fn handle_event_truncate(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
        file.seek(io::SeekFrom::Start(0))?;
        io::copy(&mut file, stdout)?;

        Ok(())
    }
}

fn main() -> io::Result<()> {
    env_logger::init();

    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
