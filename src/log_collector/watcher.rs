// src/log_collector/watcher.rs
use std::io;
use std::path::Path;

pub fn watcher() -> io::Result<impl Watcher> {
    imp::Watcher::new()
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Descriptor(imp::Descriptor);

#[derive(Debug, Eq, PartialEq)]
pub struct Event {
    pub descriptor: Descriptor,
}

pub trait Watcher {
    fn new() -> io::Result<Self>
    where
        Self: Sized;

    fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor>;

    fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor>;

    fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
}

#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::path::Path;

    use inotify::{Inotify, WatchDescriptor, WatchMask};

    use super::Event;

    const INOTIFY_BUFFER_SIZE: usize = 1024;

    pub type Descriptor = WatchDescriptor;

    pub struct Watcher {
        inner: Inotify,
        buffer: [u8; INOTIFY_BUFFER_SIZE],
    }

    impl super::Watcher for Watcher {
        fn new() -> io::Result<Self> {
            let inner = Inotify::init()?;
            Ok(Watcher {
                inner,
                buffer: [0; INOTIFY_BUFFER_SIZE],
            })
        }

        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
            let descriptor = self.inner.add_watch(path, WatchMask::CREATE)?;
            Ok(super::Descriptor(descriptor))
        }

        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
            let descriptor = self.inner.add_watch(path, WatchMask::MODIFY)?;
            Ok(super::Descriptor(descriptor))
        }

        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
            let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
            let events = inotify_events.into_iter().map(|event| Event {
                descriptor: super::Descriptor(event.wd),
            });

            Ok(events.collect())
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::{IntoRawFd, RawFd};
    use std::path::Path;

    use kqueue::{EventData, EventFilter, FilterFlag, Ident, Vnode};

    use super::Event;

    pub type Descriptor = RawFd;

    pub struct Watcher {
        inner: kqueue::Watcher,
    }

    impl Watcher {
        fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
            let file = File::open(path)?;
            let fd = file.into_raw_fd();
            self.inner
                .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
            self.inner.watch()?;
            Ok(super::Descriptor(fd))
        }
    }

    impl super::Watcher for Watcher {
        fn new() -> io::Result<Self> {
            let inner = kqueue::Watcher::new()?;
            Ok(Watcher { inner })
        }

        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
            self.add_watch(path)
        }

        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
            self.add_watch(path)
        }

        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
            let kq_event = self.inner.iter().next();

            let event = kq_event.map(|kq_event| {
                let fd = match (&kq_event.ident, &kq_event.data) {
                    (&Ident::Fd(fd), &EventData::Vnode(Vnode::Write)) => fd,
                    _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
                };
                Event {
                    descriptor: super::Descriptor(fd),
                }
            });

            Ok(event.into_iter().collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::io::Write;

    use super::{imp, Event, Watcher as _};

    #[test]
    fn watch_directory_events() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");

        let mut watcher = imp::Watcher::new().expect("unable to create watcher");
        let descriptor = watcher
            .watch_directory(tempdir.path())
            .expect("unable to watch directory");

        let mut file_path = tempdir.path().to_path_buf();
        file_path.push("test.log");
        File::create(file_path).expect("failed to create temp file");

        let events = watcher
            .read_events_blocking()
            .expect("failed to read events");
        assert_eq!(events, vec![Event { descriptor }]);
    }

    #[test]
    fn watch_file_events() {
        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
        let mut file_path = tempdir.path().to_path_buf();
        file_path.push("test.log");
        let mut file = File::create(&file_path).expect("failed to create temp file");

        let mut watcher = imp::Watcher::new().expect("unable to create watcher");
        let descriptor = watcher
            .watch_file(&file_path)
            .expect("unable to watch directory");

        file.write_all(b"hello?").expect("unable to write to file");

        let events = watcher
            .read_events_blocking()
            .expect("failed to read events");
        assert_eq!(events, vec![Event { descriptor }]);
    }
}
