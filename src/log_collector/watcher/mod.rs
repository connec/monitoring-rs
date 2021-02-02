// src/log_collector/watcher/mod.rs
//! Platform-agnostic file and directory watcher.
//!
//! The [`Watcher`] trait defines a platform-agnostic interface for a file watcher, and the
//! [`watcher`] function returns an implementation of `Watcher` for the target platform.
//!
//! The [`Watcher`] interface leaves a lot of behaviour 'implementation defined'. See the caveats in
//! the [`Watcher`] documentation for more details.
//!
//! The [`imp`] module contains the `Watcher` implementation for the target platform.

#[cfg(target_os = "linux")]
mod inotify;
#[cfg(target_os = "macos")]
mod kqueue;
#[cfg(test)]
pub(crate) mod mock;

use std::fmt::Debug;
use std::hash::Hash;
use std::io;
use std::path::Path;

#[cfg(target_os = "linux")]
use self::inotify as imp;

#[cfg(target_os = "macos")]
use self::kqueue as imp;

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

/// Tests for the `target_os`' `Watcher` implementation.
///
/// Obviously this runs differently on each platform, but that's part of the point (the tests should
/// work for either).
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
