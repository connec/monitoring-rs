// src/log_collector/watcher/kqueue.rs
/// [`Watcher`] implementation for `MacOS`, based on `kqueue`.
use std::fs::File;
use std::io;
use std::os::unix::io::{IntoRawFd, RawFd};
use std::path::Path;
use std::time::Duration;

use kqueue::{self, EventData, EventFilter, FilterFlag, Ident, Vnode};

use crate::log_collector::watcher;

type Descriptor = RawFd;

impl watcher::Descriptor for Descriptor {}

type Event = kqueue::Event;

impl watcher::Event<Descriptor> for Event {
    /// Get the `RawFd` for a [`kqueue::Event`].
    ///
    /// # Panics
    ///
    /// This will panic if the event's flags don't correspond with the filters supplied in
    /// [`Watcher::add_watch`], e.g. if the event is not for a file, or it is not a write event.
    fn descriptor(&self) -> &Descriptor {
        match (&self.ident, &self.data) {
            (Ident::Fd(fd), EventData::Vnode(Vnode::Write)) => fd,
            _ => panic!("kqueue returned an unexpected event: {:?}", self),
        }
    }
}

pub(super) struct Watcher {
    inner: kqueue::Watcher,
}

impl Watcher {
    /// Watch a file for writes.
    ///
    /// `kqueue` has quite limited fidelity for file watching – the best we can do for both
    /// files and directories is to register the `EVFILT_VNODE` and `NOTE_WRITE` flags, which is
    /// described as "A write occurred on the file referenced by the descriptor.".
    /// Observationally this seems to correspond with what we want: events for files created
    /// in watched directories, and writes to watched files.
    ///
    /// # Callee responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` is canonical (symlinks are not dereferenced).
    /// - The inode behind `path` has not already been watched. `kqueue` will happily register
    ///   duplicate watches for the same path, and emit duplicate events.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn add_watch(&mut self, path: &Path) -> io::Result<<Self as watcher::Watcher>::Descriptor> {
        let file = File::open(path)?;
        let fd = file.into_raw_fd();

        self.inner
            .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
        self.inner.watch()?;

        Ok(fd)
    }
}

impl watcher::Watcher for Watcher {
    type Descriptor = Descriptor;
    type Event = Event;

    fn new() -> io::Result<Self> {
        let inner = kqueue::Watcher::new()?;
        Ok(Watcher { inner })
    }

    /// Watch a directory for newly created files.
    ///
    /// # Caller responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a directory.
    /// - See the notes on [`Watcher::add_watch`] for additional caveats.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        self.add_watch(path)
    }

    /// Watch a file for writes.
    ///
    /// # Caller responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a file.
    /// - See the notes on [`Watcher::add_watch`] for additional caveats.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        self.add_watch(path)
    }

    fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
        let kq_event = self.inner.poll(Some(Duration::new(0, 0)));
        Ok(kq_event.into_iter().collect())
    }

    fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
        let kq_event = self.inner.iter().next();
        Ok(kq_event.into_iter().collect())
    }
}
