# Log collection part 12 (Kubernetes collector – again)

Let's take yet another stab at introducing a Kubernetes collector, but at the risk of immediately dooming ourselves to yak shaving let's first tidy up our `log_collector` module.

## A module of their very own

We have three `Watcher` implementations in our codebase:

- An `inotify` implementation, conditionally present in `watcher::imp` (when `target_os=linux`).
- A `kqueue` implementation, conditionally present in `watcher::imp` (when `target_os=macos`).
- A mock implementation in `directory::tests`.

At the moment, the first two of these implementations live in `watcher.rs` and the third lives in `directory.rs`.
Let's move them all into their own modules under `watcher`, starting with the first two:

```rust
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
```

```rust
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
```

```rust
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
```

With these in place we can remove our old `src/log_collector/watcher.rs`.
RIP.

```
$ rm src/log_collector/watcher.rs
```

And we can check everything still blends:

```
$ cargo test
...
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...

$ make dockertest
...
test_1        | test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

All good.
Now let's move the mock watcher:

```diff
--- a/src/log_collector/watcher/mod.rs
+++ b/src/log_collector/watcher/mod.rs
@@ -11,9 +11,10 @@

 #[cfg(target_os = "linux")]
 mod inotify;
-
 #[cfg(target_os = "macos")]
 mod kqueue;
+#[cfg(test)]
+pub(crate) mod mock;

 use std::fmt::Debug;
 use std::hash::Hash;
```

```rust
// src/log_collector/watcher/mock.rs
//! Mock [`Watcher`](crate::log_collector::watcher::Watcher) implementation.
use std::cell::RefCell;
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

    /// Push an event, for later collection by [`read_events`] or [`read_events_blocking`].
    pub(crate) fn add_event(&mut self, path: PathBuf) {
        self.mock.borrow_mut().pending_events.push(path);
    }

    /// Assert that certain paths were watched.
    ///
    /// Ordering is significant.
    ///
    /// # Panics
    ///
    /// This panics if the watched paths do not match `expected_paths`.
    pub(crate) fn assert_watched_paths<'a, I, P>(&self, expected_paths: I)
    where
        I: IntoIterator<Item = &'a P>,
        P: AsRef<Path> + 'a,
    {
        let expected_paths = expected_paths
            .into_iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>();
        assert_eq!(self.mock.borrow().watched_paths, expected_paths);
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
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -300,11 +300,10 @@ mod tests {
     use std::io::{self, Write};
     use std::os::unix;
     use std::path::PathBuf;
-    use std::rc::Rc;

     use tempfile::TempDir;

-    use crate::log_collector::watcher::watcher;
+    use crate::log_collector::watcher::{mock, watcher};
     use crate::test::{self, log_entry};

     use super::{Collector, Config};
@@ -320,24 +319,22 @@ mod tests {
         let config = Config {
             root_path: root_path.clone(),
         };
-        let watcher = mock::MockWatcher::new();
-        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+        let mut watcher = mock::Watcher::new();
+        let mut collector = Collector::initialize(config, watcher.clone())?;

         let (file_path, mut file) = create_log_file(&logs_dir)?;
         let file_path_canonical = file_path.canonicalize()?;
-        watcher.borrow_mut().add_event(root_path.canonicalize()?);
+        watcher.add_event(root_path.canonicalize()?);

         collector.collect_entries()?; // refresh known files

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(file_path_canonical.clone());
+        watcher.add_event(file_path_canonical.clone());
+
+        watcher.assert_watched_paths(&[root_path.canonicalize()?, file_path_canonical]);

         let entries = collector.collect_entries()?;
         let expected_path = root_path.join(file_path.file_name().unwrap());
-        assert_eq!(
-            watcher.borrow().watched_paths(),
-            &vec![root_path.canonicalize()?, file_path_canonical]
-        );
         assert_eq!(
             entries,
             vec![log_entry(
@@ -362,17 +359,15 @@ mod tests {
         let config = Config {
             root_path: root_dir.path().to_path_buf(),
         };
-        let watcher = mock::MockWatcher::new();
-        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+        let mut watcher = mock::Watcher::new();
+        let mut collector = Collector::initialize(config, watcher.clone())?;

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(src_path_canonical.clone());
+        watcher.add_event(src_path_canonical.clone());
+
+        watcher.assert_watched_paths(&[root_dir.path().canonicalize()?, src_path_canonical]);

         let entries = collector.collect_entries()?;
-        assert_eq!(
-            watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path_canonical]
-        );
         assert_eq!(
             entries,
             vec![log_entry("hello?", &[("path", dst_path.to_str().unwrap())])]
@@ -392,18 +387,16 @@ mod tests {
         unix::fs::symlink(&src_path, &dst_path)?;

         let config = Config { root_path };
-        let watcher = mock::MockWatcher::new();
-        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+        let mut watcher = mock::Watcher::new();
+        let mut collector = Collector::initialize(config, watcher.clone())?;

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(src_path_canonical.clone());
+        watcher.add_event(src_path_canonical.clone());

-        let entries = collector.collect_entries()?;
-        assert_eq!(
-            watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path_canonical.clone()]
-        );
+        watcher
+            .assert_watched_paths(&[root_dir.path().canonicalize()?, src_path_canonical.clone()]);

+        let entries = collector.collect_entries()?;
         assert_eq!(entries.len(), 2);

         let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
@@ -441,18 +434,15 @@ mod tests {
         let config = Config {
             root_path: root_path.clone(),
         };
-        let watcher = mock::MockWatcher::new();
-        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+        let mut watcher = mock::Watcher::new();
+        let mut collector = Collector::initialize(config, watcher.clone())?;

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(src_path_canonical.clone());
+        watcher.add_event(src_path_canonical.clone());

-        let entries = collector.collect_entries()?;
-        assert_eq!(
-            watcher.borrow().watched_paths(),
-            &vec![logs_dir.path().canonicalize()?, src_path_canonical]
-        );
+        watcher.assert_watched_paths(&[logs_dir.path().canonicalize()?, src_path_canonical]);

+        let entries = collector.collect_entries()?;
         assert_eq!(entries.len(), 2);

         let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
@@ -553,139 +543,4 @@ mod tests {

         Ok((path, file))
     }
-
-    mod mock {
-        use std::cell::RefCell;
-        use std::io;
-        use std::path::{Path, PathBuf};
-        use std::rc::Rc;
-
-        use crate::log_collector::watcher::{self, Watcher};
-
-        type Descriptor = PathBuf;
-        type Event = PathBuf;
-
-        impl watcher::Descriptor for Descriptor {}
-
-        impl watcher::Event<Descriptor> for Event {
-            fn descriptor(&self) -> &Descriptor {
-                &self
-            }
-        }
-
-        pub(super) struct MockWatcher {
-            watched_paths: Vec<PathBuf>,
-            pending_events: Vec<PathBuf>,
-        }
-
-        impl MockWatcher {
-            pub(super) fn new() -> Rc<RefCell<Self>> {
-                Rc::new(RefCell::new(<Self as Watcher>::new().unwrap()))
-            }
-
-            pub(super) fn watched_paths(&self) -> &Vec<PathBuf> {
-                &self.watched_paths
-            }
-
-            pub(super) fn add_event(&mut self, path: PathBuf) {
-                self.pending_events.push(path);
-            }
-        }
-
-        impl Watcher for MockWatcher {
-            type Descriptor = PathBuf;
-            type Event = PathBuf;
-
-            fn new() -> io::Result<Self> {
-                Ok(Self {
-                    watched_paths: Vec::new(),
-                    pending_events: Vec::new(),
-                })
-            }
-
-            fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
-                let canonical_path = path.canonicalize()?;
-
-                assert_eq!(
-                    path, canonical_path,
-                    "called watch_directory with link {:?} to {:?}",
-                    path, canonical_path
-                );
-                assert!(
-                    canonical_path.is_dir(),
-                    "called watch_directory with file path {:?}",
-                    path
-                );
-                assert!(
-                    !self.watched_paths.contains(&canonical_path),
-                    "called watch_directory with duplicate path {:?}",
-                    path
-                );
-                self.watched_paths.push(canonical_path.clone());
-                Ok(canonical_path)
-            }
-
-            fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
-                let canonical_path = path.canonicalize()?;
-
-                assert_eq!(
-                    path, canonical_path,
-                    "called watch_file with link {:?} to {:?}",
-                    path, canonical_path
-                );
-                assert!(
-                    canonical_path.is_file(),
-                    "called watch_file with file path {:?}",
-                    path
-                );
-                assert!(
-                    !self.watched_paths.contains(&canonical_path),
-                    "called watch_file with duplicate path {:?}",
-                    path
-                );
-                self.watched_paths.push(canonical_path.clone());
-                Ok(canonical_path)
-            }
-
-            fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
-                let events = std::mem::replace(&mut self.pending_events, Vec::new());
-                Ok(events)
-            }
-
-            fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
-                let events = self.read_events()?;
-                if events.is_empty() {
-                    panic!("called read_events_blocking with no events prepared, this will block forever");
-                }
-                Ok(events)
-            }
-        }
-
-        impl Watcher for Rc<RefCell<MockWatcher>> {
-            type Descriptor = <MockWatcher as Watcher>::Descriptor;
-            type Event = <MockWatcher as Watcher>::Event;
-
-            fn new() -> io::Result<Self> {
-                <MockWatcher as Watcher>::new()
-                    .map(RefCell::new)
-                    .map(Rc::new)
-            }
-
-            fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
-                self.borrow_mut().watch_directory(path)
-            }
-
-            fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
-                self.borrow_mut().watch_file(path)
-            }
-
-            fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
-                self.borrow_mut().read_events()
-            }
-
-            fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
-                self.borrow_mut().read_events_blocking()
-            }
-        }
-    }
 }
```

```
$ cargo test
...
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...

$ make dockertest
...
test_1        | test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

We've liberally commented our `mock::Watcher` implementation to apprise our future selves.
The implementation has changed slightly to encapsulate the `Rc<RefCell<_>>` shenanigans.
We've also added a method to assert against `watched_paths` with a convenient signature that allows any `ItoIterator<Item: AsRef<Path>>` to be passed (although we only ever pass `&[PathBuf]`, so this could be over-engineered).

With that, our `log_collector` directory is a bit tidier, and it's a lot easier to find our different `watcher` implementations.

```
$ tree src/log_collector
src/log_collector
├── directory.rs
├── mod.rs
└── watcher
    ├── inotify.rs
    ├── kqueue.rs
    ├── mock.rs
    └── mod.rs
```

Let's try a slightly different API for `mock::Watcher` as well, intended to prevent 'forgetting' to register events when creating/writing to files:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -322,16 +322,10 @@ mod tests {
         let mut watcher = mock::Watcher::new();
         let mut collector = Collector::initialize(config, watcher.clone())?;

-        let (file_path, mut file) = create_log_file(&logs_dir)?;
-        let file_path_canonical = file_path.canonicalize()?;
-        watcher.add_event(root_path.canonicalize()?);
-
+        let file_path = watcher.simulate_new_file(&logs_dir.path().canonicalize()?)?;
         collector.collect_entries()?; // refresh known files

-        writeln!(file, "hello?")?;
-        watcher.add_event(file_path_canonical.clone());
-
-        watcher.assert_watched_paths(&[root_path.canonicalize()?, file_path_canonical]);
+        watcher.simulate_write(&file_path, "hello?\n")?;

         let entries = collector.collect_entries()?;
         let expected_path = root_path.join(file_path.file_name().unwrap());
@@ -351,7 +345,7 @@ mod tests {
         let root_dir = tempfile::tempdir()?;
         let logs_dir = tempfile::tempdir()?;

-        let (src_path, mut file) = create_log_file(&logs_dir)?;
+        let (src_path, _) = create_log_file(&logs_dir)?;
         let src_path_canonical = src_path.canonicalize()?;
         let dst_path = root_dir.path().join(src_path.file_name().unwrap());
         unix::fs::symlink(&src_path, &dst_path)?;
@@ -362,10 +356,7 @@ mod tests {
         let mut watcher = mock::Watcher::new();
         let mut collector = Collector::initialize(config, watcher.clone())?;

-        writeln!(file, "hello?")?;
-        watcher.add_event(src_path_canonical.clone());
-
-        watcher.assert_watched_paths(&[root_dir.path().canonicalize()?, src_path_canonical]);
+        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

         let entries = collector.collect_entries()?;
         assert_eq!(
@@ -381,7 +372,7 @@ mod tests {
         let root_dir = tempfile::tempdir()?;
         let root_path = root_dir.path().canonicalize()?;

-        let (src_path, mut file) = create_log_file(&root_dir)?;
+        let (src_path, _) = create_log_file(&root_dir)?;
         let src_path_canonical = src_path.canonicalize()?;
         let dst_path = root_path.join("linked.log");
         unix::fs::symlink(&src_path, &dst_path)?;
@@ -390,11 +381,7 @@ mod tests {
         let mut watcher = mock::Watcher::new();
         let mut collector = Collector::initialize(config, watcher.clone())?;

-        writeln!(file, "hello?")?;
-        watcher.add_event(src_path_canonical.clone());
-
-        watcher
-            .assert_watched_paths(&[root_dir.path().canonicalize()?, src_path_canonical.clone()]);
+        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

         let entries = collector.collect_entries()?;
         assert_eq!(entries.len(), 2);
@@ -426,7 +413,7 @@ mod tests {
         let root_path = root_dir_parent.path().join("logs");
         unix::fs::symlink(logs_dir.path(), &root_path)?;

-        let (src_path, mut file) = create_log_file(&logs_dir)?;
+        let (src_path, _) = create_log_file(&logs_dir)?;
         let src_path_canonical = src_path.canonicalize()?;
         let dst_path = root_path.join("linked.log");
         unix::fs::symlink(&src_path, &dst_path)?;
@@ -437,10 +424,7 @@ mod tests {
         let mut watcher = mock::Watcher::new();
         let mut collector = Collector::initialize(config, watcher.clone())?;

-        writeln!(file, "hello?")?;
-        watcher.add_event(src_path_canonical.clone());
-
-        watcher.assert_watched_paths(&[logs_dir.path().canonicalize()?, src_path_canonical]);
+        watcher.simulate_write(&src_path_canonical, "hello?\n")?;

         let entries = collector.collect_entries()?;
         assert_eq!(entries.len(), 2);
diff --git a/src/log_collector/watcher/mock.rs b/src/log_collector/watcher/mock.rs
index aaa58a6..f3809c3 100644
--- a/src/log_collector/watcher/mock.rs
+++ b/src/log_collector/watcher/mock.rs
@@ -1,6 +1,7 @@
 // src/log_collector/watcher/mock.rs
 //! Mock [`Watcher`](crate::log_collector::watcher::Watcher) implementation.
 use std::cell::RefCell;
+use std::fs::{File, OpenOptions};
 use std::io;
 use std::path::{Path, PathBuf};
 use std::rc::Rc;
@@ -60,28 +61,49 @@ impl Watcher {
         }
     }

-    /// Push an event, for later collection by [`read_events`] or [`read_events_blocking`].
-    pub(crate) fn add_event(&mut self, path: PathBuf) {
-        self.mock.borrow_mut().pending_events.push(path);
+    /// Simulate a new file appearing in the given watched directory.
+    ///
+    /// The path to a newly created empty file is returned, and an event for the watched directory
+    /// is pushed for later collection by [`read_events`] or [`read_events_blocking`].
+    ///
+    /// # Panics
+    ///
+    /// This will panic if the given `dir_path` is not in `watched_paths`.
+    pub(crate) fn simulate_new_file(&mut self, dir_path: &PathBuf) -> io::Result<PathBuf> {
+        assert!(
+            self.mock.borrow().watched_paths.contains(dir_path),
+            "Can't simulate new file in unwatched path: {:?}",
+            dir_path
+        );
+
+        let path = dir_path.join("test.log");
+        File::create(&path)?;
+        self.mock.borrow_mut().pending_events.push(dir_path.clone());
+
+        Ok(path)
     }

-    /// Assert that certain paths were watched.
+    /// Simulate a write to a watched file.
     ///
-    /// Ordering is significant.
+    /// The given `text` is written to the watched file at `path`, and an event for the file is
+    /// pushed for later collection by [`read_events`] or [`read_events_blocking`].
     ///
     /// # Panics
     ///
-    /// This panics if the watched paths do not match `expected_paths`.
-    pub(crate) fn assert_watched_paths<'a, I, P>(&self, expected_paths: I)
-    where
-        I: IntoIterator<Item = &'a P>,
-        P: AsRef<Path> + 'a,
-    {
-        let expected_paths = expected_paths
-            .into_iter()
-            .map(AsRef::as_ref)
-            .collect::<Vec<_>>();
-        assert_eq!(self.mock.borrow().watched_paths, expected_paths);
+    /// This will panic if the given `path` is not in `watched_paths`.
+    pub(crate) fn simulate_write(&mut self, path: &PathBuf, text: &str) -> io::Result<()> {
+        use std::io::Write;
+
+        assert!(
+            self.mock.borrow().watched_paths.contains(path),
+            "Can't simulate write in unwatched path: {:?}",
+            path
+        );
+
+        write!(OpenOptions::new().append(true).open(path)?, "{}", text)?;
+        self.mock.borrow_mut().pending_events.push(path.clone());
+
+        Ok(())
     }
 }

```

Alright, enough yak-shaving.
What next?

## `log_collector::kubernetes`

In our [last attempt](12-log-collection-part-11-kubernetes-collector.md), we started by copying `log_collector::directory`.
This seems like a good starting point since we want the same file-watching capabilities, but rather than accepting arbitrary paths and annotating them as metadata we want to:

- Use the Kubernetes log directory as the default `root_path`.
- Parse new file names into a container reference (namespace, pod name, container name/ID).
- Look up the pod in Kubernetes, and attach a subset of the information as metadata.

However, as we noticed last time, if we found any bugs in our Kubernetes collector we would need to 'backport' fixes to the directory collector, and vice-versa, as well as maintaining almost identical tests.
It feels like there could be a component between the `watcher` and the `collector`.
Or perhaps that the `watcher` should be a bit higher level, and the current `watcher` could become... `fs_events`?
`fs_notify`?

Another way to slice it could be the way we briefly discussed when [we started looking at metadata](10-log-collection-part-9-metadata.md) – separating a 'source' from a 'transformer'.
We could consider the `directory` collector a source, since it generates `LogEntry`s from file system events.
We could then think about the `kubernetes` collector as a transformer that parses incoming `LogEntry`'s `path` annotation, queries Kubernetes, and writes new metadata.

We could also imagine a more complete API, that would allow the `kubernetes` transformer to react differently to new 'streams' vs. new entries in the stream etc.
At that point we're starting to creep towards the more general 'pipeline' constructs that appear in a lot of existing log collectors.
Since we're not currently interested in diverse log collection behaviour or fancy configuration, we'll keep our `Collector` trait and try to implement `log_collector::kubernetes` as a wrapper around a `log_collector::directory` instance that performs its processing in `Iterator::next`.

### A basic wrapper

Let's start with a really basic module with the basic pieces:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -3,6 +3,7 @@
 //! The interface for log collection in `monitoring-rs`.

 pub mod directory;
+pub mod kubernetes;
 mod watcher;

 use std::io;
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -64,7 +64,7 @@ struct WatchedFile {
     entry_buf: String,
 }

-struct Collector<W: Watcher> {
+pub(super) struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: W::Descriptor,
     watched_files: HashMap<W::Descriptor, WatchedFile>,
@@ -93,7 +93,7 @@ pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
 }

 impl<W: Watcher> Collector<W> {
-    fn initialize(config: Config, mut watcher: W) -> io::Result<Self> {
+    pub(super) fn initialize(config: Config, mut watcher: W) -> io::Result<Self> {
         let Config { root_path } = config;

         debug!("Initialising watch on root path {:?}", root_path);
```

```rust
// src/log_collector/kubernetes.rs
//! A log collector that collects logs from containers on a Kubernetes node.

use std::io;
use std::path::PathBuf;

use crate::log_collector::directory;
use crate::log_collector::watcher::Watcher;
use crate::LogEntry;

const DEFAULT_ROOT_PATH: &str = "/var/log/containers";

/// Configuration for [`initialize`].
pub struct Config {
    /// The root path from which to collect logs.
    ///
    /// This will default to the default Kubernetes log directory (`/var/log/containers`) if empty.
    pub root_path: Option<PathBuf>,
}

/// Initialize a [`Collector`](super::Collector) that collects logs from containers on a Kubernetes
/// node.
///
/// This wraps a [`directory`](super::directory) collector and post-processes
/// collected [`LogEntry`](crate::LogEntry)s to add metadata from the Kubernetes API.
///
/// See [`directory::initialize]`](super::directory::initialize) for more information about the file
/// watching behaviour.
///
/// # Errors
///
/// Propagates any `io::Error`s that occur during initialization.
pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
    let watcher = super::watcher::watcher()?;
    Ok(Collector {
        directory: directory::Collector::initialize(
            directory::Config {
                root_path: config
                    .root_path
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT_PATH)),
            },
            watcher,
        )?,
    })
}

/// A log collector that collects logs from containers on a Kubernetes node.
///
/// Under-the-hood this wraps a [`directory`](super::directory) collector and post-
/// processes collected [`LogEntry`](crate::LogEntry)s to add metadata from the Kubernetes API.
struct Collector<W: Watcher> {
    directory: directory::Collector<W>,
}

impl<W: Watcher> super::Collector for Collector<W> {}

impl<W: Watcher> Iterator for Collector<W> {
    type Item = io::Result<LogEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.directory.next()
    }
}
```

This should already give us enough to wire the `kubernetes` collector into `main`, and see it start up in a container with the directory default applying:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -32,6 +32,7 @@ struct Args {
 arg_enum! {
     enum CollectorArg {
         Directory,
+        Kubernetes,
     }
 }

@@ -75,6 +76,12 @@ fn init_collector(args: Args) -> io::Result<Box<dyn Collector + Send>> {
                 root_path: args.root_path.unwrap(),
             })?))
         }
+        CollectorArg::Kubernetes => {
+            use log_collector::kubernetes::{self, Config};
+            Ok(Box::new(kubernetes::initialize(Config {
+                root_path: args.root_path,
+            })?))
+        }
     }
 }

```

Now if we try to select the `Kubernetes` collector locally we'll get a 'file not found' error:

```
$ cargo run -- --log-collector Kubernetes
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

We can still override the `root_path` to start the Kubernetes collector locally:

```
$ mkdir .logs
$ cargo run -- --log-collector Kubernetes --root-path "$PWD/.logs"
...

# in another tab
$ touch .logs/hi && echo wow >> .logs/hi
$ curl -i "localhost:8000/logs/path/$PWD/.logs/hi"
HTTP/1.1 200 OK
content-length: 7
content-type: application/json
date: Mon, 01 Feb 2021 14:41:14 GMT

["wow"]
```

Very cool.
We should also be able to launch this without an explicit root path in our Docker environment:

```
$ make writer monitoring
...
monitoring_1  | error: The following required arguments were not provided:
monitoring_1  |     --log-collector <log-collector>
monitoring_1  |
monitoring_1  | USAGE:
monitoring_1  |     monitoring-rs --log-collector <log-collector> --root-path <root-path>
monitoring_1  |
monitoring_1  | For more information try --help
...
```

Oops, we never did add the `--log-collector` argument to our `monitoring` service.
Rather than doing so now, let's make `Kubernetes` the default to maintain 'zero configuration' for Kubernetes deployments:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -21,7 +21,7 @@ use monitoring_rs::{api, log_collector};
 #[derive(StructOpt)]
 struct Args {
     /// The log collector to use.
-    #[structopt(long, env, possible_values = &CollectorArg::variants())]
+    #[structopt(long, default_value, env, possible_values = &CollectorArg::variants())]
     log_collector: CollectorArg,

     /// The root path to watch.
@@ -36,6 +36,12 @@ arg_enum! {
     }
 }

+impl Default for CollectorArg {
+    fn default() -> Self {
+        Self::Kubernetes
+    }
+}
+
 #[async_std::main]
 async fn main() -> io::Result<()> {
     env_logger::init();
```

And now we could once again be able to launch with docker:

```
$ make writer monitoring
...
monitoring_1  | [2021-02-01T14:52:30Z DEBUG monitoring_rs::log_collector::directory] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2021-02-01T14:52:30Z DEBUG monitoring_rs::log_collector::directory] Create /var/log/containers/writer.log
monitoring_1  | [2021-02-01T14:52:30Z DEBUG monitoring_rs::log_collector::directory] Append /var/log/containers/writer.log
monitoring_1  | [2021-02-01T14:52:31Z DEBUG monitoring_rs::log_collector::directory] Append /var/log/containers/writer.log
...

# in another tab
$ curl http://localhost:8000/logs/path//var/log/containers/writer.log | jq
[
  "Mon Feb  1 14:52:30 UTC 2021",
  "Mon Feb  1 14:52:31 UTC 2021",
  "Mon Feb  1 14:52:32 UTC 2021",
  ...
]
```

Great.

### Parsing paths

As we've recollected a few times now, we want to parse paths of the form:

```
<pod name>_<namespace>_<container name>-<container ID>.log
```

Let's implement this now in `log_collector::kubernetes` and use the parsed value to update the `LogEntry`'s metadata:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -2,7 +2,7 @@
 //! A log collector that collects logs from containers on a Kubernetes node.

 use std::io;
-use std::path::PathBuf;
+use std::path::{Path, PathBuf};

 use crate::log_collector::directory;
 use crate::log_collector::watcher::Watcher;
@@ -52,12 +52,46 @@ struct Collector<W: Watcher> {
     directory: directory::Collector<W>,
 }

+impl<W: Watcher> Collector<W> {
+    fn parse_path(path: &str) -> [&str; 4] {
+        use std::convert::TryInto;
+
+        // TODO: `unwrap` is not ideal, since we could feasibly have log files without a file stem.
+        let stem = Path::new(path).file_stem().unwrap();
+
+        // `unwrap` is OK since we converted from `str` above.
+        let stem = stem.to_str().unwrap();
+
+        // TODO: `unwrap` is not ideal, since log file names may not have exactly 3 underscores.
+        stem.split('_').collect::<Vec<_>>().try_into().unwrap()
+    }
+}
+
 impl<W: Watcher> super::Collector for Collector<W> {}

 impl<W: Watcher> Iterator for Collector<W> {
     type Item = io::Result<LogEntry>;

     fn next(&mut self) -> Option<Self::Item> {
-        self.directory.next()
+        let entry = self.directory.next()?;
+        Some(entry.map(|mut entry| {
+            // `unwrap` is OK since we know `directory` always sets `path`.
+            let path = entry.metadata.remove("path").unwrap();
+            let [pod_name, namespace, container_name, container_id] = Self::parse_path(&path);
+            entry
+                .metadata
+                .insert("pod_name".to_string(), pod_name.to_string());
+            entry
+                .metadata
+                .insert("namespace".to_string(), namespace.to_string());
+            entry
+                .metadata
+                .insert("container_name".to_string(), container_name.to_string());
+            entry
+                .metadata
+                .insert("container_id".to_string(), container_id.to_string());
+
+            entry
+        }))
     }
 }
```

We're playing very fast and loose with `unwrap`.
At some point we will need to distinguish between fatal and recoverble errors, and make these kinds of error are recoverable, since a single file with an invalid path shouldn't crash the whole collector.

Let's update our `writer` service to write to a file with a parseable name.
Before we do so we must update the Rust version the container uses, since we now depend on `TryFrom<Vec<T>>` for arrays, which was stabilised in `1.48.0`.
We can trivially update ourselves to the latest compiler version with `rustup update stable`, and then update the version number in `Dockerfile` to the same version:

```diff
--- a/Dockerfile
+++ b/Dockerfile
@@ -1,5 +1,5 @@
 # Dockerfile
-FROM rust:1.46.0-alpine as build_base
+FROM rust:1.49.0-alpine as build_base

 WORKDIR /build
 RUN apk add --no-cache musl-dev \
```

Now if we run the existing `writer` and `monitoring` services we'll get a panic:

```
$ make down writer monitoring
...
monitoring_1  | thread 'blocking-1' panicked at 'called `Result::unwrap()` on an `Err` value: ["writer"]', /build/src/log_collector/kubernetes.rs:66:56
monitoring_1  | note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
monitoring_1  | thread 'async-std/runtime' panicked at 'task has failed', /usr/local/cargo/registry/src/github.com-1ecc6299db9ec823/async-task-4.0.3/src/task.rs:368:45
monitoring_1  | thread 'main' panicked at 'task has failed', /usr/local/cargo/registry/src/github.com-1ecc6299db9ec823/async-task-4.0.3/src/task.rs:368:45
monitoring-rs_monitoring_1 exited with code 101
```

This shows that `try_into` is failing to convert the parts of our file stem (`["writer"]`) into a 4-element array.
Let's update our `writer` service to use a well-formed file name:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -31,7 +31,7 @@ services:
     command:
     - sh
     - -c
-    - while true ; do date ; sleep 1 ; done | cat >> /var/log/containers/writer.log
+    - while true ; do date ; sleep 1 ; done | cat >> /var/log/containers/writer_fake_writer_abc123.log

   inspect:
     image: alpine
```

Now we should be able to run our docker services and lookup logs by pod name, namespace, container name, or container ID:

```
$ make down writer monitoring
...
monitoring_1  | [2021-02-01T16:22:50Z DEBUG monitoring_rs::log_collector::directory] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2021-02-01T16:22:50Z DEBUG monitoring_rs::log_collector::directory] Create /var/log/containers/writer_fake_writer_abc123.log
monitoring_1  | [2021-02-01T16:22:51Z DEBUG monitoring_rs::log_collector::directory] Append /var/log/containers/writer_fake_writer_abc123.log
...

# in another tab
$ curl localhost:8000/logs/namespace/fake | jq
[
  "Mon Feb  1 16:22:51 UTC 2021",
  "Mon Feb  1 16:22:52 UTC 2021",
  "Mon Feb  1 16:22:53 UTC 2021",
  ...
]

$ curl localhost:8000/logs/pod_name/writer | jq
[
  "Mon Feb  1 16:22:51 UTC 2021",
  "Mon Feb  1 16:22:52 UTC 2021",
  "Mon Feb  1 16:22:53 UTC 2021",
  ...
]

$ curl localhost:8000/logs/container_name/writer | jq
[
  "Mon Feb  1 16:22:51 UTC 2021",
  "Mon Feb  1 16:22:52 UTC 2021",
  "Mon Feb  1 16:22:53 UTC 2021",
  ...
]

$ curl localhost:8000/logs/container_id/abc123 | jq
[
  "Mon Feb  1 16:22:51 UTC 2021",
  "Mon Feb  1 16:22:52 UTC 2021",
  "Mon Feb  1 16:22:53 UTC 2021",
  ...
]

$ curl -i localhost:8000/logs/container_name/fake
HTTP/1.1 404 Not Found
content-length: 0
date: Mon, 01 Feb 2021 16:25:40 GMT
```

Wow, this is actually getting useful now!

### Query the Kubernetes API

The last step in our first draft Kubernetes log collector is to query the Kubernetes API.
At time of writing, the de facto standard Kubernetes client crate is [`kube`](https://crates.io/crates/kube).

Let's follow the [installation notes](https://crates.io/crates/kube#installation) and add `kube` to our project.
For `k8s-openapi` set the feature to container the version of Kubernetes you're testing against (see `kubectl version`).

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -24,6 +24,9 @@ md5 = "0.7.0"
 serde_json = "1.0.61"
 structopt = "0.3.21"
 clap = "2.33.3"
+kube = "0.48.0"
+kube-runtime = "0.48.0"
+k8s-openapi = { version = "0.11.0", default-features = false, features = ["v1_20"] }

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

If we have a poke around [the documentation](https://docs.rs/kube/0.48.0/) we can see that the recommended way to interact with the Kubernetes API is via the `Api` struct, e.g.:

```rust
use kube::Client;
use kube::api::Api;
use k8s_openapi::api::core::v1::Pod;

let client = Client::try_default().await?;
let pods: Api<Pod> = Api::namespaced(client, "apps");
let p: Pod = pods.get("blog").await?;
```

We would ideally not contruct a new client for every `LogEntry`, but our `LogEntry`s could come from any namespace, and we'd also ideally not construct an arbitrary number of clients depending on namespaces in the cluster.
Thankfully the `Api` struct is a thin abstraction over [`Resource`](https://docs.rs/kube/0.48.0/kube/struct.Resource.html), whose `namespace` field is public and so could be mutated as needed.
If we look at [the source of `Api::get`](https://docs.rs/kube/0.48.0/src/kube/api/typed.rs.html#80-83) we can see it's quite simple to use `Resource` and `client` directly:

```rust
pub async fn get(&self, name: &str) -> Result<K> {
    let req = self.resource.get(name)?;
    self.client.request::<K>(req).await
}
```

So to fetch a pod with a given `Client`, `Resource`, namespace, and name should be as simple as:

```rust
async fn get(client: &Client, resource: &mut Resource, namespace: String, name: &str) -> kube::Result<Pod> {
    resource.namespace = Some(namespace);
    let req = resource.get(name)?;
    client.request(req).await
}
```

We do have another issue, which is that `kube` uses [`tokio`](https://docs.rs/tokio/) as its runtime, whereas we've been using `async-std` so far.
We will sidestep this issue for now by using [`tokio::runtime::Runtime::block_on`](https://docs.rs/tokio/1.1.1/tokio/runtime/struct.Runtime.html#method.block_on) to perform requests from within the non-async context of our `Collector`.
To do so we will need to add `tokio` as well (with the `rt` feature enabled):

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -27,6 +27,7 @@ clap = "2.33.3"
 kube = "0.48.0"
 kube-runtime = "0.48.0"
 k8s-openapi = { version = "0.11.0", default-features = false, features = ["v1_20"] }
+tokio = { version = "1.1.1", features = ["rt"] }

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

Ok, let's start by adding a `tokio::runtime::Runtime`, `kube::Client`, and `kube::Resource` to our `kubernetes::Collector`:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -4,6 +4,8 @@
 use std::io;
 use std::path::{Path, PathBuf};

+use k8s_openapi::api::core::v1::Pod;
+
 use crate::log_collector::directory;
 use crate::log_collector::watcher::Watcher;
 use crate::LogEntry;
@@ -31,8 +33,17 @@ pub struct Config {
 ///
 /// Propagates any `io::Error`s that occur during initialization.
 pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
+    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
+
+    // TODO: `unwrap` is not ideal, but we can't easily recover from bad/missing Kubernetes config,
+    // and it wouldn't be much better to propagate the failure through `io::Error`.
+    let kube_client = runtime.block_on(kube::Client::try_default()).unwrap();
+
     let watcher = super::watcher::watcher()?;
     Ok(Collector {
+        runtime,
+        kube_client,
+        kube_resource: kube::Resource::all::<Pod>(),
         directory: directory::Collector::initialize(
             directory::Config {
                 root_path: config
@@ -49,6 +60,9 @@ pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
 /// Under-the-hood this wraps a [`directory`](super::directory) collector and post-
 /// processes collected [`LogEntry`](crate::LogEntry)s to add metadata from the Kubernetes API.
 struct Collector<W: Watcher> {
+    runtime: tokio::runtime::Runtime,
+    kube_client: kube::Client,
+    kube_resource: kube::Resource,
     directory: directory::Collector<W>,
 }

```

Now we should be able to query the Kubernetes for additional metadata in `next`:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -5,6 +5,7 @@ use std::io;
 use std::path::{Path, PathBuf};

 use k8s_openapi::api::core::v1::Pod;
+use kube::api::Meta;

 use crate::log_collector::directory;
 use crate::log_collector::watcher::Watcher;
@@ -79,6 +80,30 @@ impl<W: Watcher> Collector<W> {
         // TODO: `unwrap` is not ideal, since log file names may not have exactly 3 underscores.
         stem.split('_').collect::<Vec<_>>().try_into().unwrap()
     }
+
+    fn query_pod_metadata(&mut self, namespace: &str, pod_name: &str) -> Vec<(String, String)> {
+        self.kube_resource.namespace = Some(namespace.to_string());
+
+        // TODO: `unwrap` may be OK here, since the only errors that can occur are from constructing
+        // the HTTP request. This could only happen if `Resource::get` built an invalid URL. In our
+        // case, that could only happen if the data in `k8s_openapi` or `namespace` is corrupt. We
+        // couldn't reaasonably handle corruption in `k8s_openapi`, but we should check in future
+        // what would happen for files containing dodgy (i.e. URL-unsafe) namespaces.
+        let request = self.kube_resource.get(pod_name).unwrap();
+
+        // TODO: `unwrap` is not ideal here, since missing pods or transient failures to communicate
+        // with the Kubernetes API probably shouldn't crash the monitor. There's not really anything
+        // better we can do with the current APIs, however (e.g. propagating in `io::Error` wouldn't
+        // be better).
+        let pod = self
+            .runtime
+            .block_on(self.kube_client.request::<Pod>(request))
+            .unwrap();
+
+        let meta = pod.meta();
+
+        todo!()
+    }
 }

 impl<W: Watcher> super::Collector for Collector<W> {}
@@ -105,6 +130,10 @@ impl<W: Watcher> Iterator for Collector<W> {
                 .metadata
                 .insert("container_id".to_string(), container_id.to_string());

+            for (key, value) in self.query_pod_metadata(namespace, pod_name) {
+                entry.metadata.insert(key, value);
+            }
+
             entry
         }))
     }
```

We're continuing to `unwrap` quite aggressively, but taking some care to document considerations for when we one day revisit error handling, or experience a panic in a running system.
We've also still got a `todo!` in our `query_pod_metadata` function – what metadata should we return?
The `meta` variable is an [`ObjectMeta`](https://docs.rs/kube/0.48.0/kube/api/struct.ObjectMeta.html) value, which has a bunch of fields for various metadata "that all persisted resources must have" (though it's all `Option<T>`, so code must deal with its absence).

For now let's keep it simple and use [`labels`](https://docs.rs/kube/0.48.0/kube/api/struct.ObjectMeta.html#structfield.labels).
We'll also be a bit cheap for now and update `query_pod_metadata` to directly return a `BTreeMap`:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -1,6 +1,7 @@
 // src/log_collector/kubernetes.rs
 //! A log collector that collects logs from containers on a Kubernetes node.

+use std::collections::BTreeMap;
 use std::io;
 use std::path::{Path, PathBuf};

@@ -81,7 +82,7 @@ impl<W: Watcher> Collector<W> {
         stem.split('_').collect::<Vec<_>>().try_into().unwrap()
     }

-    fn query_pod_metadata(&mut self, namespace: &str, pod_name: &str) -> Vec<(String, String)> {
+    fn query_pod_metadata(&mut self, namespace: &str, pod_name: &str) -> BTreeMap<String, String> {
         self.kube_resource.namespace = Some(namespace.to_string());

         // TODO: `unwrap` may be OK here, since the only errors that can occur are from constructing
@@ -102,7 +103,7 @@ impl<W: Watcher> Collector<W> {

         let meta = pod.meta();

-        todo!()
+        meta.labels.as_ref().cloned().unwrap_or_default()
     }
 }

```

What would happen if we run our docker services now:

```
$ make down writer monitoring
...
  --- stderr
  thread 'main' panicked at '

  Could not find directory of OpenSSL installation, and this `-sys` crate cannot
  proceed without this knowledge. If OpenSSL is installed and this crate had
  trouble finding it,  you can set the `OPENSSL_DIR` environment variable for the
  compilation process.

  Make sure you also have the development packages of openssl installed.
  For example, `libssl-dev` on Ubuntu or `openssl-devel` on Fedora.

  If you're in a situation where you think the directory *should* be found
  automatically, please open a bug at https://github.com/sfackler/rust-openssl
  and include information about your system as well as this message.

  $HOST = x86_64-unknown-linux-musl
  $TARGET = x86_64-unknown-linux-musl
  openssl-sys = 0.9.60
...
```

Ah, it seems we're missing some OpenSSL artifacts in our container.
Before we go down this route, perhaps we can enable [`rustls`](https://docs.rs/rustls/0.19.0/rustls/):

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -24,8 +24,8 @@ md5 = "0.7.0"
 serde_json = "1.0.61"
 structopt = "0.3.21"
 clap = "2.33.3"
-kube = "0.48.0"
-kube-runtime = "0.48.0"
+kube = { version = "0.48.0", default-features = false, features = ["rustls-tls"] }
+kube-runtime = { version = "0.48.0", default-features = false, features = ["rustls-tls"] }
 k8s-openapi = { version = "0.11.0", default-features = false, features = ["v1_20"] }
 tokio = { version = "1.1.1", features = ["rt"] }

```

Now if we try our docker services again we get:

```
$ make down writer monitoring
...
monitoring_1  | thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value: Kubeconfig(ConfigInferenceExhausted { cluster_env: Kubeconfig(MissingInClusterVariables { hostenv: "KUBERNETES_SERVICE_HOST", portenv: "KUBERNETES_SERVICE_PORT" }), kubeconfig: Kubeconfig(ReadFile { path: "/root/.kube/config", source: Os { code: 2, kind: NotFound, message: "No such file or directory" } }) })', src/log_collector/kubernetes.rs:42:69
...
```

Makes sense – we don't have any Kubernetes configuration inside our container.
Let's quickly mount our local `~/.kube` directory into the container:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -6,6 +6,7 @@ services:
     image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
     volumes:
     - logs:/var/log/containers
+    - /Users/<you>/.kube:/root/.kube
     environment:
     - RUST_LOG=monitoring_rs=debug
     - ROOT_PATH=/var/log/containers
```

```
$ make down writer monitoring
...
monitoring_1  | thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value: Kubeconfig(ConfigInferenceExhausted { cluster_env: Kubeconfig(MissingInClusterVariables { hostenv: "KUBERNETES_SERVICE_HOST", portenv: "KUBERNETES_SERVICE_PORT" }), kubeconfig: Kubeconfig(AuthExecStart(Os { code: 2, kind: NotFound, message: "No such file or directory" })) })', src/log_collector/kubernetes.rs:42:69
...
```

Your mileage may vary here, but since I'm using DigitalOcean, which uses `doctl` for authentication, I'm now getting a `AuthExecStart` error, probably because `doctl` is not present.
Let's back out of that:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -6,7 +6,6 @@ services:
     image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
     volumes:
     - logs:/var/log/containers
-    - /Users/chris/.kube:/root/.kube
     environment:
     - RUST_LOG=monitoring_rs=debug
     - ROOT_PATH=/var/log/containers
```

Let's instead look to `kubectl proxy`.
If we look at the first part of our error message we see:

```
MissingInClusterVariables { hostenv: "KUBERNETES_SERVICE_HOST", portenv: "KUBERNETES_SERVICE_PORT" }
```

Let's add those to our container, pointing to `host.docker.internal:8001` which is where we'll set up our Kubernetes API proxy:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -9,6 +9,8 @@ services:
     environment:
     - RUST_LOG=monitoring_rs=debug
     - ROOT_PATH=/var/log/containers
+    - KUBERNETES_SERVICE_HOST=host.docker.internal
+    - KUBERNETES_SERVICE_PORT=8001
     ports:
     - 8000:8000

```

Now we can start a Kubernetes API proxy (note that this is pretty insecure if your system is on an untrusted network):

```
$ kubectl proxy --address 0.0.0.0 --accept-hosts '^.*$'
```

And let's see if it blends:

```
$ make down monitoring
monitoring_1  | thread 'main' panicked at 'called `Result::unwrap()` on an `Err` value: Kubeconfig(ConfigInferenceExhausted { cluster_env: Kubeconfig(InvalidInClusterNamespace(Kubeconfig(ReadFile { path: "/var/run/secrets/kubernetes.io/serviceaccount/namespace", source: Os { code: 2, kind: NotFound, message: "No such file or directory" } }))), kubeconfig: Kubeconfig(ReadFile { path: "/root/.kube/config", source: Os { code: 2, kind: NotFound, message: "No such file or directory" } }) })', src/log_collector/kubernetes.rs:42:69
```

Boo, the `kube::Config` inference is digging further into the expected Kubernetes environment, which we don't want to have to set up.
We could capitulate and go straight to Kubernetes, but it would be better to have a local test case.

Let's do things slightly differently and run locally:

```
$ rm -rf .logs && mkdir .logs
$ cargo run -- --root-path .logs
...
```

Now find a pod in the cluster for which we will simulate a log file (ideally one with some labels and active logs).
We can construct the right log file name using some `jq` shenanigans (using a `coredns` pod in this example):

```
$ kubectl -n kube-system get pods coredns-6b6854dcbf-s5kb6 -o json \
  | jq -r '"\(.metadata.name)_\(.metadata.namespace)_\(.status.containerStatuses[0].name)_\(.status.containerStatuses[0].containerID | sub("docker://"; "")).log"'
coredns-6b6854dcbf-s5kb6_kube-system_coredns_c14b6cbbdf1385fb53170775fdbacb81a77da96d2932128f3d5c57392437287e.log
```

OK, now we should be able to `touch` then write to that file:

```
$ touch .logs/coredns-6b6854dcbf-s5kb6_kube-system_coredns_c14b6cbbdf1385fb53170775fdbacb81a77da96d2932128f3d5c57392437287e.log
$ echo hello? >> .logs/coredns-6b6854dcbf-s5kb6_kube-system_coredns_c14b6cbbdf1385fb53170775fdbacb81a77da96d2932128f3d5c57392437287e.log
```

And... our monitor crashes :(

```
thread 'blocking-1' panicked at 'A Tokio 1.x context was found, but timers are disabled. Call `enable_time` on the runtime builder to enable timers.', /Users/chris/.cargo/registry/src/github.com-1ecc6299db9ec823/tokio-1.1.1/src/time/driver/handle.rs:50:18
```

Let's try and do what it says:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -35,7 +35,9 @@ pub struct Config {
 ///
 /// Propagates any `io::Error`s that occur during initialization.
 pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
-    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
+    let runtime = tokio::runtime::Builder::new_current_thread()
+        .enable_time()
+        .build()?;

     // TODO: `unwrap` is not ideal, but we can't easily recover from bad/missing Kubernetes config,
     // and it wouldn't be much better to propagate the failure through `io::Error`.
```

And if we run it again:

```
$ cargo run -- --root-path .logs
...
```

And write again:

```
$ echo hello? >> .logs/coredns-6b6854dcbf-s5kb6_kube-system_coredns_c14b6cbbdf1385fb53170775fdbacb81a77da96d2932128f3d5c57392437287e.log
```

We get a different panic :D

```
thread 'blocking-1' panicked at 'A Tokio 1.x context was found, but IO is disabled. Call `enable_io` on the runtime builder to enable IO.', /Users/chris/.cargo/registry/src/github.com-1ecc6299db9ec823/tokio-1.1.1/src/io/driver/mod.rs:262:50
```

Let's follow the instructions once again:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -36,6 +36,7 @@ pub struct Config {
 /// Propagates any `io::Error`s that occur during initialization.
 pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
     let runtime = tokio::runtime::Builder::new_current_thread()
+        .enable_io()
         .enable_time()
         .build()?;

```

And repeat our test steps:

```
$ cargo run -- --root-path .logs
...

# in another tab
$ echo hello? >> .logs/coredns-6b6854dcbf-s5kb6_kube-system_coredns_c14b6cbbdf1385fb53170775fdbacb81a77da96d2932128f3d5c57392437287e.log
```

...and it's still running!
But did it save anything?

```
$ curl localhost:8000/logs/k8s-app/kube-dns
["hello?"]
```

Nice.

## Wrapping up

We've definitely got a lot of cleaning up still to do.
Some possibly low handing fruit could be:

- Introduce some caching into `kubernetes::Collector` to avoid re-parsing file names and re-querying metadata.
- Introduce some deployment commands to the `Makefile`, to allow us to actually deploy our monitor to a cluster and see how it performs.

Beyond that, we could now start to consider:

- Taking another look at the domain model.
  The caching needed for the Kubernetes collector would look similar to the `WatchedFile` construct in the directory collector.
  If we go 'all in' on the separation between a log file (/source/whatever) and log entries, will the resulting system be cleaner (/more robust/more efficient)?
  This might need to wait until we have some operational data, or perhaps until we start to think about metrics.
- Taking a deeper look at error handling.
  This might want to wait until we revise the model.
- Using a 'real database', such as [`sled`](https://crates.io/crates/sled).
- Start thinking about metric collection!
  It would be particularly cool if we could reuse some of the logging infrastructure, perhaps by fitting both into the same 'domain model', e.g. 'streams' (log files, timeseries) of 'events' (log entries, data points)?

Fune times ahead!
