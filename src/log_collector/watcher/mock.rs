// src/log_collector/watcher/mock.rs
//! Mock [`Watcher`](crate::log_collector::watcher::Watcher) implementation.
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::log_collector::watcher;

/// The watch descriptor type for [`Watcher`].
///
/// This is the most trivial way that we can represent a unique call to
/// [`watch_directory`](Watcher::watch_directory) or [`watch_file`](Watcher::watch_file). We can do
/// this thanks to the callee responsibilities (and in this implementation, assertions) on those
/// methods.
type Descriptor = PathBuf;

impl watcher::Descriptor for Descriptor {}

/// The event type for [`Watcher`].
///
/// This is the most trivial way that we can represent events. Since the only thing we need from a
/// [`watcher::Event`](crate::log_collector::watcher::Event) is a
/// [`watcher::Descriptor`](crate::log_collector::watcher::Descriptor), we can just use the same
/// representation as [`Descriptor`].
type Event = PathBuf;

impl watcher::Event<Descriptor> for Event {
    /// Get the descriptor for this event.
    ///
    /// For this implementation, the `Event` and `Descriptor` have the same representation, so this
    /// is exactly `&self`.
    fn descriptor(&self) -> &Descriptor {
        &self
    }
}

/// A mock [`Watcher`](crate::log_collector::watcher::Watcher) implementation.
///
/// This watches no actual files, but rather asserts invariants and offers assertions on how the
/// watcher is used.
pub(crate) struct Watcher {
    mock: Rc<RefCell<Mock>>,
}

/// The inner-type of [`Watcher`] that maintains the list of watched paths and pushed events.
struct Mock {
    watched_paths: Vec<PathBuf>,
    pending_events: Vec<PathBuf>,
}

impl Watcher {
    /// Create a new instance.
    pub(crate) fn new() -> Self {
        Self {
            mock: Rc::new(RefCell::new(Mock {
                watched_paths: Vec::new(),
                pending_events: Vec::new(),
            })),
        }
    }

    /// Simulate a new file appearing in the given watched directory.
    ///
    /// The path to a newly created empty file is returned, and an event for the watched directory
    /// is pushed for later collection by [`read_events`] or [`read_events_blocking`].
    ///
    /// # Panics
    ///
    /// This will panic if the given `dir_path` is not in `watched_paths`.
    pub(crate) fn simulate_new_file(&mut self, dir_path: &PathBuf) -> io::Result<PathBuf> {
        assert!(
            self.mock.borrow().watched_paths.contains(dir_path),
            "Can't simulate new file in unwatched path: {:?}",
            dir_path
        );

        let path = dir_path.join("test.log");
        File::create(&path)?;
        self.mock.borrow_mut().pending_events.push(dir_path.clone());

        Ok(path)
    }

    /// Simulate a write to a watched file.
    ///
    /// The given `text` is written to the watched file at `path`, and an event for the file is
    /// pushed for later collection by [`read_events`] or [`read_events_blocking`].
    ///
    /// # Panics
    ///
    /// This will panic if the given `path` is not in `watched_paths`.
    pub(crate) fn simulate_write(&mut self, path: &PathBuf, text: &str) -> io::Result<()> {
        use std::io::Write;

        assert!(
            self.mock.borrow().watched_paths.contains(path),
            "Can't simulate write in unwatched path: {:?}",
            path
        );

        write!(OpenOptions::new().append(true).open(path)?, "{}", text)?;
        self.mock.borrow_mut().pending_events.push(path.clone());

        Ok(())
    }
}

impl Clone for Watcher {
    fn clone(&self) -> Self {
        Self {
            mock: Rc::clone(&self.mock),
        }
    }
}

impl watcher::Watcher for Watcher {
    type Descriptor = PathBuf;
    type Event = PathBuf;

    fn new() -> io::Result<Self> {
        Ok(Self::new())
    }

    /// Watch a directory for newly created files.
    ///
    /// This records that `path` has been watched, and returns it as the [`Descriptor`] (opaque to
    /// callers).
    ///
    /// Additionally, assertions are in place to validate the callee responsibilities of the trait
    /// method:
    ///
    /// - `path` points to a directory.
    /// - `path` is canonical.
    /// - `path` has not already been watched.
    fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        let watched_paths = &mut self.mock.borrow_mut().watched_paths;
        let canonical_path = path.canonicalize()?;

        assert!(
            path.is_dir(),
            "called watch_directory with file path {:?}",
            path
        );
        assert_eq!(
            path, canonical_path,
            "called watch_directory with link {:?} to {:?}",
            path, canonical_path
        );
        assert!(
            !watched_paths.contains(&canonical_path),
            "called watch_directory with duplicate path {:?}",
            path
        );

        watched_paths.push(canonical_path.clone());
        Ok(canonical_path)
    }

    /// Watch a file for writes.
    ///
    /// This records that `path` has been watched, and returns it as the [`Descriptor`] (opaque to
    /// callers).
    ///
    /// Additionally, assertions are in place to validate the callee responsibilities of the trait
    /// method:
    ///
    /// - `path` points to a file.
    /// - `path` is canonical.
    /// - `path` has not already been watched.
    fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
        let watched_paths = &mut self.mock.borrow_mut().watched_paths;
        let canonical_path = path.canonicalize()?;

        assert!(
            path.is_file(),
            "called watch_file with file path {:?}",
            path
        );
        assert_eq!(
            path, canonical_path,
            "called watch_file with link {:?} to {:?}",
            path, canonical_path
        );
        assert!(
            !watched_paths.contains(&canonical_path),
            "called watch_file with duplicate path {:?}",
            path
        );

        watched_paths.push(canonical_path.clone());
        Ok(canonical_path)
    }

    /// Read some events about the registered directories and files.
    ///
    /// This pops whatever [`Event`]s have been supplied through [`push_event`].
    fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
        let pending_events = &mut self.mock.borrow_mut().pending_events;
        let events = std::mem::replace(pending_events, Vec::new());
        Ok(events)
    }

    /// Read some events about the registered directories and files.
    ///
    /// This pops whatever [`Event`]s have been supplied through [`push_event`].
    ///
    /// # Panics
    ///
    /// This currently panics if there are no events, since this is primarily intended for use in
    /// tests, and blocking in a test is more likely to be a bug with usage of the mock.
    fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
        let events = self.read_events()?;
        if events.is_empty() {
            panic!("called read_events_blocking with no events prepared");
        }
        Ok(events)
    }
}
