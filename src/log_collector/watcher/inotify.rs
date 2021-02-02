// src/log_collector/watcher/inotify.rs
//! [`Watcher`] implementation for linux, based on `inotify`.
use std::io;
use std::path::Path;

use inotify::{Inotify, WatchDescriptor, WatchMask};

use crate::log_collector::watcher;

const INOTIFY_BUFFER_SIZE: usize = 1024;

type Descriptor = WatchDescriptor;

impl watcher::Descriptor for Descriptor {}

#[derive(Debug)]
pub(super) struct Event(WatchDescriptor);

impl watcher::Event<Descriptor> for Event {
    fn descriptor(&self) -> &Descriptor {
        &self.0
    }
}

impl<S> From<inotify::Event<S>> for Event {
    fn from(inotify_event: inotify::Event<S>) -> Self {
        Self(inotify_event.wd)
    }
}

pub(super) struct Watcher {
    inner: Inotify,
    buffer: [u8; INOTIFY_BUFFER_SIZE],
}

impl watcher::Watcher for Watcher {
    type Descriptor = Descriptor;

    type Event = Event;

    fn new() -> io::Result<Self> {
        let inner = Inotify::init()?;
        Ok(Watcher {
            inner,
            buffer: [0; INOTIFY_BUFFER_SIZE],
        })
    }

    /// Watch a directory for newly created files.
    ///
    /// # Callee responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a directory.
    /// - `path` is canonical (symlinks are not dereferenced).
    /// - The inode behind `path` has not already been watched. `inotify` merges duplicate
    ///   watches for the same path, and returns the `Descriptor` of the original watch.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        let descriptor = self
            .inner
            .add_watch(path, WatchMask::CREATE | WatchMask::DONT_FOLLOW)?;
        Ok(descriptor)
    }

    /// Watch a file for writes.
    ///
    /// # Callee responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a file.
    /// - `path` is canonical (symlinks are not dereferenced).
    /// - The inode behind `path` has not already been watched. `inotify` merges duplicate
    ///   watches for the same path, and returns the `Descriptor` of the original watch.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        let descriptor = self
            .inner
            .add_watch(path, WatchMask::MODIFY | WatchMask::DONT_FOLLOW)?;
        Ok(descriptor)
    }

    fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
        let inotify_events = self.inner.read_events(&mut self.buffer)?;
        Ok(inotify_events.map(Event::from).collect())
    }

    fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
        let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
        Ok(inotify_events.map(Event::from).collect())
    }
}
