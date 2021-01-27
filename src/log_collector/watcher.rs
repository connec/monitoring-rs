// src/log_collector/watcher.rs
//! Platform-agnostic file and directory watcher.
//!
//! The [`Watcher`] trait defines a platform-agnostic interface for a file watcher, and the
//! [`watcher`] function returns an implementation of `Watcher` for the target platform.
//!
//! The [`Watcher`] interface leaves a lot of behaviour 'implementation defined'. See the caveats in
//! the [`Watcher`] documentation for more details.
//!
//! The [`imp`] module contains the `Watcher` implementation for the target platform.
use std::fmt::Debug;
use std::hash::Hash;
use std::io;
use std::path::Path;

pub(super) fn watcher() -> io::Result<impl Watcher> {
    imp::Watcher::new()
}

/// A platform-agnostic description of a watched file descriptor.
///
/// The [`Watcher`] API depends on being able to use `Descriptor`s as identifiers to correlate calls
/// to `watch_*` with events emitted by the `Watcher`. This trait is thus just a collection of other
/// traits that allow use as an identifier.
pub(super) trait Descriptor: Clone + Debug + Eq + Hash + PartialEq + Send {}

/// A platform-agnostic interface to file system events.
///
/// This currently only exposes the `Descriptor` of the registered watch. Clients can use this to
/// to correlate events with the corresponding `watch_*` call.
pub(super) trait Event<D: Descriptor>: Debug {
    fn descriptor(&self) -> &D;
}

/// A platform-agnostic file and directory watching API.
///
/// This API is intended to be used to drive log collectors, specifically:
///
/// - Generate events when new files are added to a directory (see [`Self::watch_directory`]).
/// - Generate events when new content is written to a file (see [`Self::watch_file`]).
///
/// The API is necessarily very 'lowest common denominator', and leaves a lot of behaviour
/// implementation-defined. See the notes on callee responsibilities in [`Self::watch_directory`]
/// and [`Self::watch_file`] for specifics.
pub(super) trait Watcher {
    /// An opaque reference to a watched directory or file.
    ///
    /// Instances of this type are returned by [`watch_directory`](Self::watch_directory) and
    /// [`watch_file`](Self::watch_file). They are also included in [`Event`]s emitted by the
    /// watcher, and so can be used by callers to correlate events to watched files.
    type Descriptor: Descriptor;

    /// The type of events emitted by this watcher.
    ///
    /// The only requirement on this type is that it implements [`Event`], which allows the
    /// associated `Descriptor` to be retrieved.
    type Event: Event<Self::Descriptor>;

    /// Construct a new instance of the `Watcher`.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to create the watcher.
    fn new() -> io::Result<Self>
    where
        Self: Sized;

    /// Watch a directory for newly created files.
    ///
    /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever a file
    /// is created in the directory at the given `path`.
    ///
    /// # Callee responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a directory.
    /// - `path` is canonical (e.g. implementations may not resolve symlinks, and may watch the
    ///   symlink itself).
    /// - `path` has not already been watched.
    ///
    /// The behaviour if any of these points are violated is implementation defined, and so specific
    /// behaviour should not be relied upon.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor>;

    /// Watch a file for writes.
    ///
    /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever the file
    /// at the given `path` is written to.
    ///
    /// # Callee responsibilities
    ///
    /// It is the caller's responsibility to ensure that:
    ///
    /// - `path` points to a file.
    /// - `path` is canonical (e.g. implementations may not resolve symlinks, and may watch the
    ///   symlink itself).
    /// - `path` has not already been watched.
    ///
    /// The behaviour if any of these points are violated is implementation defined, and so specific
    /// behaviour should not be relied upon.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to register the watch.
    fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor>;

    /// Read some events about the registered directories and files.
    ///
    /// This must never block, and should just return an empty `Vec` if no events are ready.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to read events.
    fn read_events(&mut self) -> io::Result<Vec<Self::Event>>;

    /// Read some events about the registered directories and files.
    ///
    /// This may block forever if no events have been registered, or if no events occur.
    ///
    /// # Errors
    ///
    /// Propagates any `io::Error` caused when attempting to read events.
    fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>>;
}

/// [`Watcher`] implementation for linux, based on `inotify`.
#[cfg(target_os = "linux")]
mod imp {
    use std::io;
    use std::path::Path;

    use inotify::{Inotify, WatchDescriptor, WatchMask};

    const INOTIFY_BUFFER_SIZE: usize = 1024;

    type Descriptor = WatchDescriptor;

    impl super::Descriptor for Descriptor {}

    #[derive(Debug)]
    pub(super) struct Event(WatchDescriptor);

    impl super::Event<Descriptor> for Event {
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

    impl super::Watcher for Watcher {
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
}

/// [`Watcher`] implementation for `MacOS`, based on `kqueue`.
#[cfg(target_os = "macos")]
mod imp {
    use std::fs::File;
    use std::io;
    use std::os::unix::io::{IntoRawFd, RawFd};
    use std::path::Path;
    use std::time::Duration;

    use kqueue::{self, EventData, EventFilter, FilterFlag, Ident, Vnode};

    type Descriptor = RawFd;

    impl super::Descriptor for Descriptor {}

    type Event = kqueue::Event;

    impl super::Event<Descriptor> for Event {
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
        fn add_watch(&mut self, path: &Path) -> io::Result<<Self as super::Watcher>::Descriptor> {
            let file = File::open(path)?;
            let fd = file.into_raw_fd();

            self.inner
                .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
            self.inner.watch()?;

            Ok(fd)
        }
    }

    impl super::Watcher for Watcher {
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
        let event_descriptors: Vec<_> = events.iter().map(Event::descriptor).collect();
        assert_eq!(event_descriptors, vec![&descriptor]);
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
        let event_descriptors: Vec<_> = events.iter().map(Event::descriptor).collect();
        assert_eq!(event_descriptors, vec![&descriptor]);
    }
}
