# Log collection (part 8 – multi-platform)

We have found ourselves in a bit of a pickle: we're unable to reliably test our log collector natively due to its dependence on `inotify`, which is only available on Linux (and we are all developing on MacOS... right?).
We arrived at a two-pronged approach to refactor the `log_collector` interface in order to increase the surface area of tests that can be performed natively, and testing the 'last mile' of platform specific behaviour using conditionally compiled tests.

We originally ruled out using a cross-platform library like [`notify`](https://crates.io/crates/notify) since differences between platform could make native tests unrepresentative, and thus offer us no additional benefit over running tests in Docker.
This decision was based on [previous experience](2-log-collection-part-2-aborted.md) with the `notify` crate, which uses the `fsevents` API on MacOS.
However, there is another file system event API available on MacOS – [`kqueue`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kqueue.2.html).

## `kqueue`

> **`kqueue`**, **`kevent`** -- kernel event notification mechanism

Let's try and use the [`kqueue` crate](https://crates.io/crates/kqueue) to get file watching working locally.
Before we do, let's add a configuration option we have so far avoided – setting the log directory.

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -11,6 +11,7 @@ mod log_collector;
 use std::env;
 use std::fs;
 use std::io;
+use std::path::Path;
 use std::sync::Arc;

 use async_std::prelude::FutureExt;
@@ -21,18 +22,27 @@ use async_std::task;
 use log_collector::Collector;
 use log_database::Database;

-#[cfg(target_os = "linux")]
-const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
+const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
+const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

 #[async_std::main]
 async fn main() -> io::Result<()> {
     env_logger::init();

+    let container_log_directory = env::var(VAR_CONTAINER_LOG_DIRECTORY)
+        .or_else(|error| match error {
+            env::VarError::NotPresent => Ok(DEFAULT_CONTAINER_LOG_DIRECTORY.to_string()),
+            error => Err(error),
+        })
+        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
+
     let database = init_database()?;

     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

-    let collector_handle = task::spawn(blocking::unblock(move || init_collector(database)));
+    let collector_handle = task::spawn(blocking::unblock(move || {
+        init_collector(container_log_directory.as_ref(), database)
+    }));

     api_handle.try_join(collector_handle).await?;

@@ -50,8 +60,11 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
 }

 #[cfg(target_os = "linux")]
-fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {
-    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+fn init_collector(
+    container_log_directory: &Path,
+    database: Arc<RwLock<Database>>,
+) -> io::Result<()> {
+    let mut collector = Collector::initialize(container_log_directory)?;
     let mut buffer = [0; 1024];
     loop {
         let entries = collector.collect_entries(&mut buffer)?;
@@ -65,13 +78,19 @@ fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {

 #[cfg(not(test))]
 #[cfg(not(target_os = "linux"))]
-fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
+fn init_collector(
+    _container_log_directory: &Path,
+    _database: Arc<RwLock<Database>>,
+) -> io::Result<()> {
     compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
     unreachable!()
 }

 #[cfg(test)]
 #[cfg(not(target_os = "linux"))]
-fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
+fn init_collector(
+    _container_log_directory: &Path,
+    _database: Arc<RwLock<Database>>,
+) -> io::Result<()> {
     panic!("log_collector is only available on Linux due to dependency on `inotify`")
 }
```

We're still unable to compile this locally, so for now let's make a small change so we can verify things in Docker:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -5,9 +5,10 @@ services:
     build: .
     image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
     volumes:
-    - logs:/var/log/containers
+    - logs:/var/log/containers_new
     environment:
     - RUST_LOG=monitoring_rs=debug
+    - CONTAINER_LOG_DIRECTORY=/var/log/containers_new
     ports:
     - 8000:8000

```

We've changed where we mount the `logs` volume, and set the appropriate environment variable in order to watch the new location.
Let's see if it blends:

```
$ make down writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-20T19:33:38Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers_new"
monitoring_1  | [2020-12-20T19:33:38Z DEBUG monitoring_rs::log_collector] Create /var/log/containers_new/writer.log
monitoring_1  | [2020-12-20T19:33:38Z DEBUG monitoring_rs::log_collector] Append /var/log/containers_new/writer.log
```

Beautiful.
Let's revert our `docker-compose.yaml` and validate the default behaviour:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -5,10 +5,9 @@ services:
     build: .
     image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
     volumes:
-    - logs:/var/log/containers_new
+    - logs:/var/log/containers
     environment:
     - RUST_LOG=monitoring_rs=debug
-    - CONTAINER_LOG_DIRECTORY=/var/log/containers_new
     ports:
     - 8000:8000

```

```
$ make down writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-20T19:39:13Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-20T19:39:13Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-20T19:39:14Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
```

All good!
Now we need to get our project to compile locally.
In order to do so, we need to remove our dependence on `inotify`, and allow `kqueue` to take its place on MacOS.

## A `Watcher` of our own

The `notify` crate defines a [`Watcher`](https://docs.rs/notify/4.0.15/notify/trait.Watcher.html) trait with the following API:

```rust
pub trait Watcher {
    fn new_raw(tx: Sender<RawEvent>) -> Result<Self>;

    fn new(tx: Sender<DebouncedEvent>, delay: Duration) -> Result<Self>;

    fn watch<P: AsRef<Path>>(&mut self, path: P, recursive_mode: RecursiveMode) -> Result<()>;

    fn unwatch<P: AsRef<Path>>(&mut self, path: P) -> Result<()>;
}
```

Implementors of this trait are constructed with `new`, passing in a [`std::sync::mpsc::Sender`](https://doc.rust-lang.org/stable/std/sync/mpsc/struct.Sender.html).
Paths can then be `watch`ed and `unwatch`ed.
This interface implies the presence of a thread to operate the underlying event API, and forward events through the given `Sender`.

This isn't quite right for us, for the following reasons:

- The distinction between "raw" and "debounced" events is not useful.
  We can probably get by with a single `Event`-type to serve as the interface between the file system watcher and the log collector.
- `recursive_mode` is not something we're interested in.
  We will assume a flat log directory for the time being.
- Using `AsRef<Path>` instead of `&Path` is a nice convenience for a general purpose library.
  Since ours is an internal API, we should choose whatever makes sense for our call-sites.
- The need for implementors to spawn their own threads etc. is not ideal, since our `Collector` will already
  be running on a separate thread.

Let's instead go the other way – we are currently using the following APIs from `inotify`:

```rust
// Construct a new `Inotify` instance.
let mut inotify = Inotify::init()?;

// Add watches...
let descriptor = inotify.add_watch(path, WatchMask::CREATE)?; // for the log directory
let descriptor = inotify.add_watch(path, WatchMask::MODIFY)?; // for the log files

// Read events.
let events = inotify.read_events_blocking(buffer)?;
```

From this, an interface that doesn't depend on implementation-specific details (like `WatchMask`) could look like:

```rust
#[derive(Copy, Eq, Hash, PartialEq)]
pub struct Descriptor;

#[derive(Debug)]
pub struct Event {
    pub descriptor: Descriptor
};

pub trait Watcher {
    fn new() -> io::Result<Self>;

    fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor>;

    fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor>;

    fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
}
```

This would give us a platform indepent notion of a `Descriptor` (as a copyable, comparable, and hashable unit) and an `Event` (with the `Descriptor` to which it pertains).
Usage would be very similar to `inotify`, e.g.:

```rust
// Construct a new `Watcher`.
let watcher = WatcherImpl::new()?;

// Add watches...
let descriptor = watcher.watch_directory(path)?;
let descriptor = watcher.watch_file(path)?;

// Read events.
let events = watcher.read_events_blocking()?;
```

Let's create a `log_collector::watcher` module and sketch out our definitions:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -8,6 +8,13 @@ mod log_database;
 #[cfg(target_os = "linux")]
 mod log_collector;

+#[cfg(not(target_os = "linux"))]
+mod log_collector {
+    mod watcher;
+
+    pub use watcher::Watcher;
+}
+
 use std::env;
 use std::fs;
 use std::io;
```

```rust
// src/log_collector/watcher.rs
use std::io;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Descriptor;

#[derive(Debug)]
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
```

Our definition compiles so far, which is a good sign (though note we've had to add the `where Self: Sized` bound to `Watcher::new`, since it's required by `Result`).

### `kqueue` imp

Since we're on a journey to improve local development, let's start trying to implement a `kqueue` implementation for `Watcher`.
We will use the `kqueue` crate for this purpose:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -17,6 +17,12 @@ md5 = "0.7.0"
 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }

+[target.'cfg(target_os = "macos")'.dependencies]
+kqueue = "1.0.2"
+
 [dev-dependencies]
 tempfile = "3.1.0"
 tide-testing = "0.1.2"
+
+[patch.crates-io]
+kqueue-sys = { git = "https://gitlab.com/connec/rust-kqueue-sys.git", rev = "263f56cfb022b69643c9307095db1fde910822df" }
```

Note the patched version of `kqueue-sys` due to [a bug](https://gitlab.com/worr/rust-kqueue-sys/-/merge_requests/2) in the published version.

Next we need to get `cargo check` to succeed locally.
Currently we hit our inotify-related compiler error:

```
$ cargo check
    Checking monitoring-rs v0.1.0
error: log_collector is only available on Linux due to dependency on `inotify`
  --> src/main.rs:92:5
   |
92 |     compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

error: aborting due to previous error

error: could not compile `monitoring-rs`.

To learn more, run the command again with --verbose.
```

For now we can just remove the `cfg(not(test))` version of `init_collector` and use the panicking `cfg(test)` version in all cases:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -83,17 +83,6 @@ fn init_collector(
     }
 }

-#[cfg(not(test))]
-#[cfg(not(target_os = "linux"))]
-fn init_collector(
-    _container_log_directory: &Path,
-    _database: Arc<RwLock<Database>>,
-) -> io::Result<()> {
-    compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
-    unreachable!()
-}
-
-#[cfg(test)]
 #[cfg(not(target_os = "linux"))]
 fn init_collector(
     _container_log_directory: &Path,
```

Now we can `cargo check` happily (albeit with a couple of dead code warnings):

```
$ cargo check
...
warning: 2 warnings emitted

    Finished dev [unoptimized + debuginfo] target(s) in 0.67s
```

Back to our `kqueue`-backed `Watcher` implementation, let's introduce a `watcher::imp` module that's conditionally compiled on MacOS:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -21,3 +21,6 @@ pub trait Watcher {

     fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
 }
+
+#[cfg(target_os = "macos")]
+mod imp {}
```

In this module we're going to create a `watcher::imp::Watcher` struct that will implement our `watcher::Watcher` trait.
We're hoping to keep all the contents of `imp` private, so that only the `watcher::Watcher` API can be depended on.
Let's start by stubbing the implementation:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -23,4 +23,29 @@ pub trait Watcher {
 }

 #[cfg(target_os = "macos")]
-mod imp {}
+mod imp {
+    use std::io;
+    use std::path::Path;
+
+    use super::{Descriptor, Event};
+
+    pub struct Watcher;
+
+    impl super::Watcher for Watcher {
+        fn new() -> io::Result<Self> {
+            unimplemented!()
+        }
+
+        fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor> {
+            unimplemented!()
+        }
+
+        fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor> {
+            unimplemented!()
+        }
+
+        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+            unimplemented!()
+        }
+    }
+}
```

So far, so good.
Now we need to look closer at the `kqueue` crate, starting with [`kqueue::Watcher::new`](https://docs.worrbase.com/rust/kqueue/struct.Watcher.html#method.new) which has exactly the same signature as our `Watcher`.
An obvious implementation for `Watcher::new` would therefore be to call `kqueue::Watcher::new` and store the resulting `kqueue::Watcher` in a field of the returned `imp::Watcher` struct:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -29,11 +29,14 @@ mod imp {

     use super::{Descriptor, Event};

-    pub struct Watcher;
+    pub struct Watcher {
+        inner: kqueue::Watcher,
+    }

     impl super::Watcher for Watcher {
         fn new() -> io::Result<Self> {
-            unimplemented!()
+            let inner = kqueue::Watcher::new()?;
+            Ok(Watcher { inner })
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor> {
```

Ok, now for `Watcher::watch_directory` and `Watcher::watch_file`.
`kqueue::Watcher` has three different methods for watching files:

- [`add_filename`](https://docs.worrbase.com/rust/kqueue/struct.Watcher.html#method.add_filename) which takes an `AsRef<Path>`.
  The documentation notes that this opens the path internally, and suggests that `add_fd` or `add_file` should be preferred.
- [`add_fd`](https://docs.worrbase.com/rust/kqueue/struct.Watcher.html#method.add_fd) which takes an [`std::os::unix::RawFd`](https://doc.rust-lang.org/stable/std/os/unix/io/type.RawFd.html).
  These can be obtained from `File`s using [`AsRawFd::as_raw_fd`](https://doc.rust-lang.org/stable/std/os/unix/io/trait.AsRawFd.html#tymethod.as_raw_fd).
- [`add_file`](https://docs.worrbase.com/rust/kqueue/struct.Watcher.html#method.add_file) which takes a `File`.
  This just calls `add_fd` internally with `file.as_raw_fd()`.

Let's stick with `add_fd` since it's one of the preferred options and isn't notably less ergonomic.
So, our `watch_directory` implementation will have to open the given `path` and call `self.inner.add_fd` with the `RawFd`.

We also need to consider the return value, which is meant to be a `Descriptor` that represents the watched file.
The `inotify` crate specifies a [`WatchDescriptor`](https://docs.rs/inotify/0.8.3/inotify/struct.WatchDescriptor.html) struct, but there's no analogue for `kqueue`.
However, the `add_filename` docs note "`kqueue(2)` is an `fd`-based API", so let's try using the `RawFd` as the inner identifier.
Let's update that first:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -3,7 +3,7 @@ use std::io;
 use std::path::Path;

 #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
-pub struct Descriptor;
+pub struct Descriptor(imp::Descriptor);

 #[derive(Debug)]
 pub struct Event {
@@ -25,9 +25,12 @@ pub trait Watcher {
 #[cfg(target_os = "macos")]
 mod imp {
     use std::io;
+    use std::os::unix::io::RawFd;
     use std::path::Path;

-    use super::{Descriptor, Event};
+    use super::Event;
+
+    pub type Descriptor = RawFd;

     pub struct Watcher {
         inner: kqueue::Watcher,
@@ -39,11 +42,11 @@ mod imp {
             Ok(Watcher { inner })
         }

-        fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor> {
+        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             unimplemented!()
         }

-        fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor> {
+        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             unimplemented!()
         }

```

So our outer `Descriptor` now wraps an `imp::Descriptor`.
Since the wrapped value is not `pub`, consumers of the `watcher` module will be unable to access it, which is what we want for our `imp` items.

Before going further with the implementation, let's write a small test for directory modifications:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -5,7 +5,7 @@ use std::path::Path;
 #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
 pub struct Descriptor(imp::Descriptor);

-#[derive(Debug)]
+#[derive(Debug, Eq, PartialEq)]
 pub struct Event {
     pub descriptor: Descriptor,
 }
@@ -54,4 +54,31 @@ mod imp {
             unimplemented!()
         }
     }
+
+    #[cfg(test)]
+    mod tests {
+        use std::fs::File;
+
+        use super::super::Watcher as _;
+        use super::Watcher;
+
+        #[test]
+        fn watch_directory_events() {
+            let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+
+            let mut watcher = Watcher::new().expect("unable to create watcher");
+            watcher
+                .watch_directory(tempdir.path())
+                .expect("unable to watch directory");
+
+            let mut file_path = tempdir.path().to_path_buf();
+            file_path.push("test.log");
+            File::create(file_path).expect("failed to create temp file");
+
+            let events = watcher
+                .read_events_blocking()
+                .expect("failed to read events");
+            assert_eq!(events, vec![]);
+        }
+    }
 }
```

Note that we've had to add `Eq` and `PartialEq` to our derived traits for `Event` in order to use it with `assert_eq`.
We're comparing the `events` we receive with an empty `Vec`, hoping for an assertion error.

Now if we run `cargo test` we will get an "unimplemented" panic:

```
$ cargo test
...
thread 'log_collector::watcher::imp::tests::watch_directory_events' panicked at 'not implemented', src/log_collector/watcher.rs:46:13
...
```

Right, line 46 is `watch_directory` so let's start there:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -24,10 +24,13 @@ pub trait Watcher {

 #[cfg(target_os = "macos")]
 mod imp {
+    use std::fs::File;
     use std::io;
-    use std::os::unix::io::RawFd;
+    use std::os::unix::io::{AsRawFd, RawFd};
     use std::path::Path;

+    use kqueue::{EventFilter, FilterFlag};
+
     use super::Event;

     pub type Descriptor = RawFd;
@@ -43,7 +46,12 @@ mod imp {
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            unimplemented!()
+            let file = File::open(path)?;
+            let fd = file.as_raw_fd();
+            self.inner
+                .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
+            self.inner.watch()?;
+            Ok(super::Descriptor(fd))
         }

         fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
```

We're using the `EVFILT_VNODE` kqueue event filter and `NOTE_WRITE` filter flag.
The man page documentation for `EVFILT_VNODE` states:

> Takes a file descriptor as the identifier and the events to watch for in `fflags`, and returns when one or more of the requested events occurs on the descriptor. The events to monitor are:
>
> - [...]
> - `NOTE_WRITE`: A write occurred on the file referenced by the descriptor.
> - [...]

We're hoping that new directory entries will be treated as a 'write' against the directory's file descriptor.

Also note that we're calling `kqueue::Watcher::watch` after adding the file.
The `add_fd` docs note:

> TODO: Adding new files requires calling `Watcher.watch` again.

So perhaps this will happen internally one day, but for now we will just call it ourselves whenever we call `add_fd`.

Now if we run our tests we'll see they're failing at `read_events_blocking`:

```
$ cargo test
...
thread 'log_collector::watcher::imp::tests::watch_directory_events' panicked at 'not implemented', src/log_collector/watcher.rs:61:13
...
```

For `read_events_blocking` we want to use [`kqueue::Watcher::poll`](https://docs.worrbase.com/rust/kqueue/struct.Watcher.html#method.poll).
Since we also want to block until there's an event we can simply call it without a `timeout`.
Let's see how this looks:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -59,7 +59,21 @@ mod imp {
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
-            unimplemented!()
+            let events = self
+                .inner
+                .poll(None)
+                .map(|event| {
+                    let fd = match event.ident {
+                        kqueue::Ident::Fd(fd) => fd,
+                        _ => panic!("kqueue returned an event with a non-fd ident"),
+                    };
+                    let event = Event {
+                        descriptor: super::Descriptor(fd),
+                    };
+                    vec![event]
+                })
+                .unwrap_or_default();
+            Ok(events)
         }
     }

```

And now if we `cargo test`:

```
$ cargo test
...
test log_collector::watcher::imp::tests::watch_directory_events ... ok
...
```

So, we're not getting any events... even though `kqueue::Watcher::poll` says it will block!
It turns out this is (at least) a bug in the documentation, since [the source](https://docs.worrbase.com/rust/src/kqueue/lib.rs.html#465-472) shows the duration in fact defaults to `0`, and "poll will not block indefinitely".

```rust
// poll will not block indefinitely
// None -> return immediately
match timeout {
    Some(timeout) => get_event(self, Some(timeout)),
    None => get_event(self, Some(Duration::new(0, 0))),
}
```

Great, how do we deal with that?
Well it looks like [`EventIter::next`](https://docs.worrbase.com/rust/src/kqueue/lib.rs.html#648) will do what we want, so let's give that a go:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -61,7 +61,8 @@ mod imp {
         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
             let events = self
                 .inner
-                .poll(None)
+                .iter()
+                .next()
                 .map(|event| {
                     let fd = match event.ident {
                         kqueue::Ident::Fd(fd) => fd,
```

And `cargo test`:

```
$ cargo test
running 5 tests
...
```

The tests hang :(

After some debugging including enabling all the `FilterFlag` bits, writing to the file as well as just creating it, and storing the opened `File` for the directory in the `Watcher` struct, it turns out that the latter is required.
This makes sense when thinking about it – `kqueue` is an `fd`-based API, and Rust's `File` will close the underlying file descriptor on drop.

It turns out there is an API that will let us move on in the short term: [`IntoRawFd::into_raw_fd`](https://doc.rust-lang.org/stable/std/os/unix/io/trait.IntoRawFd.html#tymethod.into_raw_fd).
This is very similar to `AsRawFd`, except it consumes the receiving `File` without closing the descriptor.
With our current API this will constitute a file descriptor leak, since there's no tracking of opened file descriptors, or indeed methods to unwatch directories or files.
We will live with this for now and switch to `into_raw_fd`:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -26,7 +26,7 @@ pub trait Watcher {
 mod imp {
     use std::fs::File;
     use std::io;
-    use std::os::unix::io::{AsRawFd, RawFd};
+    use std::os::unix::io::{IntoRawFd, RawFd};
     use std::path::Path;

     use kqueue::{EventFilter, FilterFlag};
@@ -47,7 +47,7 @@ mod imp {

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             let file = File::open(path)?;
-            let fd = file.as_raw_fd();
+            let fd = file.into_raw_fd();
             self.inner
                 .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
             self.inner.watch()?;
```

And *now* `cargo test`:

```
$ cargo test
thread 'log_collector::watcher::imp::tests::watch_directory_events' panicked at 'assertion failed: `(left == right)`
  left: `[Event { descriptor: Descriptor(4) }]`,
 right: `[]`', src/log_collector/watcher.rs:104:13
```

*Finally* we have an event 🎉
Let's update the condition to get the test to pass:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -82,7 +82,7 @@ mod imp {
     mod tests {
         use std::fs::File;

-        use super::super::Watcher as _;
+        use super::super::{Event, Watcher as _};
         use super::Watcher;

         #[test]
@@ -90,7 +90,7 @@ mod imp {
             let tempdir = tempfile::tempdir().expect("unable to create tempdir");

             let mut watcher = Watcher::new().expect("unable to create watcher");
-            watcher
+            let descriptor = watcher
                 .watch_directory(tempdir.path())
                 .expect("unable to watch directory");

@@ -101,7 +101,7 @@ mod imp {
             let events = watcher
                 .read_events_blocking()
                 .expect("failed to read events");
-            assert_eq!(events, vec![]);
+            assert_eq!(events, vec![Event { descriptor }]);
         }
     }
 }
```

And now our tests pass 🎉

```
$ cargo test
...
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Before moving on, let's tidy up `read_events_blocking` to make clearer assertions:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -29,7 +29,7 @@ mod imp {
     use std::os::unix::io::{IntoRawFd, RawFd};
     use std::path::Path;

-    use kqueue::{EventFilter, FilterFlag};
+    use kqueue::{EventData, EventFilter, FilterFlag, Ident, Vnode};

     use super::Event;

@@ -59,22 +59,19 @@ mod imp {
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
-            let events = self
-                .inner
-                .iter()
-                .next()
-                .map(|event| {
-                    let fd = match event.ident {
-                        kqueue::Ident::Fd(fd) => fd,
-                        _ => panic!("kqueue returned an event with a non-fd ident"),
-                    };
-                    let event = Event {
-                        descriptor: super::Descriptor(fd),
-                    };
-                    vec![event]
-                })
-                .unwrap_or_default();
-            Ok(events)
+            let kq_event = self.inner.iter().next();
+
+            let event = kq_event.map(|kq_event| {
+                let fd = match (&kq_event.ident, &kq_event.data) {
+                    (&Ident::Fd(fd), &EventData::Vnode(Vnode::Write)) => fd,
+                    _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
+                };
+                Event {
+                    descriptor: super::Descriptor(fd),
+                }
+            });
+
+            Ok(event.into_iter().collect())
         }
     }

```

This should cause our watcher to panic if we encounter an unexpected event, and the message will include the details of the event to help us debug.

Let's now implement `watch_file` and a corresponding test before moving on to an `inotify` variant:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -39,6 +39,17 @@ mod imp {
         inner: kqueue::Watcher,
     }

+    impl Watcher {
+        fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+            let file = File::open(path)?;
+            let fd = file.into_raw_fd();
+            self.inner
+                .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
+            self.inner.watch()?;
+            Ok(super::Descriptor(fd))
+        }
+    }
+
     impl super::Watcher for Watcher {
         fn new() -> io::Result<Self> {
             let inner = kqueue::Watcher::new()?;
@@ -46,16 +57,11 @@ mod imp {
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            let file = File::open(path)?;
-            let fd = file.into_raw_fd();
-            self.inner
-                .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
-            self.inner.watch()?;
-            Ok(super::Descriptor(fd))
+            self.add_watch(path)
         }

         fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            unimplemented!()
+            self.add_watch(path)
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
@@ -78,6 +84,7 @@ mod imp {
     #[cfg(test)]
     mod tests {
         use std::fs::File;
+        use std::io::Write;

         use super::super::{Event, Watcher as _};
         use super::Watcher;
@@ -100,5 +107,25 @@ mod imp {
                 .expect("failed to read events");
             assert_eq!(events, vec![Event { descriptor }]);
         }
+
+        #[test]
+        fn watch_file_events() {
+            let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+            let mut file_path = tempdir.path().to_path_buf();
+            file_path.push("test.log");
+            let mut file = File::create(&file_path).expect("failed to create temp file");
+
+            let mut watcher = Watcher::new().expect("unable to create watcher");
+            let descriptor = watcher
+                .watch_file(&file_path)
+                .expect("unable to watch directory");
+
+            file.write_all(b"hello?").expect("unable to write to file");
+
+            let events = watcher
+                .read_events_blocking()
+                .expect("failed to read events");
+            assert_eq!(events, vec![Event { descriptor }]);
+        }
     }
 }
```

Since the logic for watching directories is the same as for watching files, we promote it to an `impl Watcher` block and reuse it from `watch_file`.
The test follows the same pattern as the one for directories.

We now have a hefty 6 passing tests:

```
$ cargo test
...
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## `inotify` imp

Before we get to implementing an `inotify`-driven `Watcher`, let's add a Docker Compose service and `make` target to run the tests in docker:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -11,6 +11,12 @@ services:
     ports:
     - 8000:8000

+  test:
+    build:
+      context: .
+      target: builder
+    command: [cargo, test, --release]
+
   writer:
     image: alpine
     volumes:
```

```diff
--- a/Makefile
+++ b/Makefile
@@ -9,6 +9,9 @@ build-monitoring:
 monitoring: build-monitoring
  @docker-compose up --force-recreate monitoring

+dockertest:
+ @docker-compose up --build --force-recreate test
+
 writer:
  @docker-compose up -d writer

```

```
$ make dockertest
...
test_1        |
test_1        | test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test_1        |
monitoring-rs_test_1 exited with code 0
```

Notice that we only run 4 tests in Docker, because the watcher tests are currently contained in the conditionally compiled `imp` module.
Now on to implementing.

First recall how we declared the `log_collector::watcher` module:

```rust
#[cfg(target_os = "linux")]
mod log_collector;

#[cfg(not(target_os = "linux"))]
mod log_collector {
    mod watcher;

    pub use watcher::Watcher;
}
```

In order to even evaluate the `watcher` module, we first need to declare it in our linux version of `log_collector`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -1,4 +1,6 @@
 // log_collector/mod.rs
+mod watcher;
+
 use std::collections::hash_map::HashMap;
 use std::ffi::OsStr;
 use std::fs::{self, File};
```

Whilst we're in this area, let's also remove the `pub use` from `main.rs` for now, since we will probably be moving things around before that is relevant:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -11,8 +11,6 @@ mod log_collector;
 #[cfg(not(target_os = "linux"))]
 mod log_collector {
     mod watcher;
-
-    pub use watcher::Watcher;
 }

 use std::env;
```

Now let's think about testing.
If we consider the tests we wrote for the `macos` implementation, they actually only depend on the `Watcher` API.
We should be able to promote the tests out from `imp` and into the enclosing `watcher` module where they can use the appropriate `imp` for whichever platform the tests are ran on:

```diff
diff --git a/src/log_collector/watcher.rs b/src/log_collector/watcher.rs
index 7e6019a..acf8c5d 100644
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -80,52 +80,51 @@ mod imp {
             Ok(event.into_iter().collect())
         }
     }
+}

-    #[cfg(test)]
-    mod tests {
-        use std::fs::File;
-        use std::io::Write;
+#[cfg(test)]
+mod tests {
+    use std::fs::File;
+    use std::io::Write;

-        use super::super::{Event, Watcher as _};
-        use super::Watcher;
+    use super::{imp, Event, Watcher as _};

-        #[test]
-        fn watch_directory_events() {
-            let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+    #[test]
+    fn watch_directory_events() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");

-            let mut watcher = Watcher::new().expect("unable to create watcher");
-            let descriptor = watcher
-                .watch_directory(tempdir.path())
-                .expect("unable to watch directory");
+        let mut watcher = imp::Watcher::new().expect("unable to create watcher");
+        let descriptor = watcher
+            .watch_directory(tempdir.path())
+            .expect("unable to watch directory");

-            let mut file_path = tempdir.path().to_path_buf();
-            file_path.push("test.log");
-            File::create(file_path).expect("failed to create temp file");
+        let mut file_path = tempdir.path().to_path_buf();
+        file_path.push("test.log");
+        File::create(file_path).expect("failed to create temp file");

-            let events = watcher
-                .read_events_blocking()
-                .expect("failed to read events");
-            assert_eq!(events, vec![Event { descriptor }]);
-        }
+        let events = watcher
+            .read_events_blocking()
+            .expect("failed to read events");
+        assert_eq!(events, vec![Event { descriptor }]);
+    }

-        #[test]
-        fn watch_file_events() {
-            let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-            let mut file_path = tempdir.path().to_path_buf();
-            file_path.push("test.log");
-            let mut file = File::create(&file_path).expect("failed to create temp file");
+    #[test]
+    fn watch_file_events() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+        let mut file_path = tempdir.path().to_path_buf();
+        file_path.push("test.log");
+        let mut file = File::create(&file_path).expect("failed to create temp file");

-            let mut watcher = Watcher::new().expect("unable to create watcher");
-            let descriptor = watcher
-                .watch_file(&file_path)
-                .expect("unable to watch directory");
+        let mut watcher = imp::Watcher::new().expect("unable to create watcher");
+        let descriptor = watcher
+            .watch_file(&file_path)
+            .expect("unable to watch directory");

-            file.write_all(b"hello?").expect("unable to write to file");
+        file.write_all(b"hello?").expect("unable to write to file");

-            let events = watcher
-                .read_events_blocking()
-                .expect("failed to read events");
-            assert_eq!(events, vec![Event { descriptor }]);
-        }
+        let events = watcher
+            .read_events_blocking()
+            .expect("failed to read events");
+        assert_eq!(events, vec![Event { descriptor }]);
     }
 }
```

Now if we try to run `make dockertest` we should see some problemos:

```
$ make dockertest
...
   Compiling monitoring-rs v0.1.0 (/build)
error[E0433]: failed to resolve: use of undeclared type or module `imp`
 --> src/log_collector/watcher.rs:6:23
  |
6 | pub struct Descriptor(imp::Descriptor);
  |                       ^^^ use of undeclared type or module `imp`
...
```

In fact the problemos begin even sooner, since `watcher::Descriptor` is defined in terms of `imp::Descriptor`.

Since we based our `Watcher` API on our usage of `inotify`, the implementation should be pretty straight forward.
Let's start by specifying `Descriptor` as `WatchDescriptor`:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -2,7 +2,7 @@
 use std::io;
 use std::path::Path;

-#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
+#[derive(Clone, Debug, Eq, Hash, PartialEq)]
 pub struct Descriptor(imp::Descriptor);

 #[derive(Debug, Eq, PartialEq)]
@@ -22,6 +22,13 @@ pub trait Watcher {
     fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
 }

+#[cfg(target_os = "linux")]
+mod imp {
+    use inotify::WatchDescriptor;
+
+    pub type Descriptor = WatchDescriptor;
+}
+
 #[cfg(target_os = "macos")]
 mod imp {
     use std::fs::File;
```

Note that we have are no longer deriving `Copy` for `Descriptor`, since this is not implemented for `WatchDescriptor` and we don't really need it so far.

Now only the tests are failing to compile with `make dockertest`.
Let's again stub all the `Watcher` methods with `unimplemented!` to get them to run:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -24,9 +24,34 @@ pub trait Watcher {

 #[cfg(target_os = "linux")]
 mod imp {
+    use std::io;
+    use std::path::Path;
+
     use inotify::WatchDescriptor;

+    use super::Event;
+
     pub type Descriptor = WatchDescriptor;
+
+    pub struct Watcher;
+
+    impl super::Watcher for Watcher {
+        fn new() -> io::Result<Self> {
+            unimplemented!()
+        }
+
+        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+            unimplemented!()
+        }
+
+        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+            unimplemented!()
+        }
+
+        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+            unimplemented!()
+        }
+    }
 }

 #[cfg(target_os = "macos")]
```

Now when running `make dockertest` we get a panic for 'not implemented'.

Let's again start with `Watcher::new`, and simply call `Inotify::new` and store that in an `inner` field on `imp::Watcher`:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -27,17 +27,20 @@ mod imp {
     use std::io;
     use std::path::Path;

-    use inotify::WatchDescriptor;
+    use inotify::{Inotify, WatchDescriptor};

     use super::Event;

     pub type Descriptor = WatchDescriptor;

-    pub struct Watcher;
+    pub struct Watcher {
+        inner: Inotify,
+    }

     impl super::Watcher for Watcher {
         fn new() -> io::Result<Self> {
-            unimplemented!()
+            let inner = Inotify::init()?;
+            Ok(Watcher { inner })
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
```

Now our `make dockertest` panic has migrated to `watch_directory` and `watch_file`.
Let's flush them out based on the logic we current use in `log_collector::Collector`:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -27,7 +27,7 @@ mod imp {
     use std::io;
     use std::path::Path;

-    use inotify::{Inotify, WatchDescriptor};
+    use inotify::{Inotify, WatchDescriptor, WatchMask};

     use super::Event;

@@ -44,11 +44,13 @@ mod imp {
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            unimplemented!()
+            let descriptor = self.inner.add_watch(path, WatchMask::CREATE)?;
+            Ok(super::Descriptor(descriptor))
         }

         fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            unimplemented!()
+            let descriptor = self.inner.add_watch(path, WatchMask::MODIFY)?;
+            Ok(super::Descriptor(descriptor))
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
```

And now we have our final 'not implemented' from `read_events_blocking`.
This should also translate easily:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -31,16 +31,22 @@ mod imp {

     use super::Event;

+    const INOTIFY_BUFFER_SIZE: usize = 1024;
+
     pub type Descriptor = WatchDescriptor;

     pub struct Watcher {
         inner: Inotify,
+        buffer: [u8; INOTIFY_BUFFER_SIZE],
     }

     impl super::Watcher for Watcher {
         fn new() -> io::Result<Self> {
             let inner = Inotify::init()?;
-            Ok(Watcher { inner })
+            Ok(Watcher {
+                inner,
+                buffer: [0; INOTIFY_BUFFER_SIZE],
+            })
         }

         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
@@ -54,7 +60,12 @@ mod imp {
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
-            unimplemented!()
+            let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
+            let events = inotify_events.into_iter().map(|event| Event {
+                descriptor: super::Descriptor(event.wd),
+            });
+
+            Ok(events.collect())
         }
     }
 }
```

We've added a `buffer` field to `imp::Watcher` in order to reuse the buffer we have to pass to `Inotify::read_events_blocking`, but otherwise this is pretty straightforward.

And now, what happens when we `make dockertest`:

```
$ make dockertest
...
test_1        | test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test_1        |
monitoring-rs_test_1 exited with code 0
```

Woohoo 🎉
We now have a minimal file watcher abstraction that we can test both locally and on our target platform.

## Using `Watcher`

We now have a `log_collector::watcher::imp` module for each of our target platforms.
The next thing to do is to actually use the `Watcher` interface from within `log_collector::Collector`.

First, we can remove the conditional compilation around `log_collector` itself since only `log_collector::watcher::imp` should need to be conditionally compiled:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,17 +1,10 @@
 // main.rs
-#[cfg_attr(target_os = "linux", macro_use)]
+#[macro_use]
 extern crate log;

 mod api;
-mod log_database;
-
-#[cfg(target_os = "linux")]
 mod log_collector;
-
-#[cfg(not(target_os = "linux"))]
-mod log_collector {
-    mod watcher;
-}
+mod log_database;

 use std::env;
 use std::fs;
@@ -23,7 +16,6 @@ use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
 use async_std::task;

-#[cfg(target_os = "linux")]
 use log_collector::Collector;
 use log_database::Database;

@@ -64,7 +56,6 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     Ok(Arc::new(RwLock::new(database)))
 }

-#[cfg(target_os = "linux")]
 fn init_collector(
     container_log_directory: &Path,
     database: Arc<RwLock<Database>>,
@@ -80,11 +71,3 @@ fn init_collector(
         }
     }
 }
-
-#[cfg(not(target_os = "linux"))]
-fn init_collector(
-    _container_log_directory: &Path,
-    _database: Arc<RwLock<Database>>,
-) -> io::Result<()> {
-    panic!("log_collector is only available on Linux due to dependency on `inotify`")
-}
```

We're now back to "use of undeclared type or module `inotify`" errors in `src/log_collector/mod.rs`.
The first error is for our `use inotify::{...}` line:

```rust
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
```

We now have a replacement for `WatchDescriptor` (`watcher::Descriptor`), and `EventMask` and `WatchMask` have become implementation details of the linux `watcher::imp`.
Replacing `Inotify` needs a bit more thought – we've created a `watcher::Watcher` trait, however we have not exposed the `imp::Watcher` structs that implement that trait.
We can't call `watcher::Watcher::new` directly, we have to call it on an implementing type.

We could simply export the `imp::Watcher` type, but apart from creating an awkward naming situation this would 'leak' one of our `imp` structs into the API.
To keep the `imp` details truly private, we can create a `watcher::watcher` function that returns `impl Watcher`:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -2,6 +2,10 @@
 use std::io;
 use std::path::Path;

+pub fn watcher() -> io::Result<impl Watcher> {
+    imp::Watcher::new()
+}
+
 #[derive(Clone, Debug, Eq, Hash, PartialEq)]
 pub struct Descriptor(imp::Descriptor);

```

This will allow us to construct a `Watcher` without having to know specifically which type is implementing it.

Let's update our `use ...` statements in `log_collector` and then work through the affected call-sites:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -7,7 +7,7 @@ use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};

-use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
+use watcher::{watcher, Watcher};

 #[derive(Debug)]
 enum Event<'collector> {
```

Our first errors (by line number) are from our `struct Collector` definition:

```
error[E0412]: cannot find type `WatchDescriptor` in this scope
  --> src/log_collector/mod.rs:58:14
   |
58 |     root_wd: WatchDescriptor,
   |              ^^^^^^^^^^^^^^^ not found in this scope

error[E0412]: cannot find type `WatchDescriptor` in this scope
  --> src/log_collector/mod.rs:59:25
   |
56 | pub struct Collector {
   |                     - help: you might be missing a type parameter: `<WatchDescriptor>`
...
59 |     live_files: HashMap<WatchDescriptor, LiveFile>,
   |                         ^^^^^^^^^^^^^^^ not found in this scope

error[E0412]: cannot find type `Inotify` in this scope
  --> src/log_collector/mod.rs:60:14
   |
60 |     inotify: Inotify,
   |              ^^^^^^^ not found in this scope
```

Fixing `WatcherDescriptor` is easy – it just needs to become `watcher::Descriptor`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -55,8 +55,8 @@ pub struct LogEntry {

 pub struct Collector {
     root_path: PathBuf,
-    root_wd: WatchDescriptor,
-    live_files: HashMap<WatchDescriptor, LiveFile>,
+    root_wd: watcher::Descriptor,
+    live_files: HashMap<watcher::Descriptor, LiveFile>,
     inotify: Inotify,
 }

```

For `Inotify`, we need to replace it with an instance of `Watcher`.
We can't simply replace `Inotify` with `impl Watcher`, but we can add a generic parameter and add a bound on the `Watcher` trait:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -53,14 +53,14 @@ pub struct LogEntry {
     pub line: String,
 }

-pub struct Collector {
+pub struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: watcher::Descriptor,
     live_files: HashMap<watcher::Descriptor, LiveFile>,
-    inotify: Inotify,
+    watcher: W,
 }

-impl Collector {
+impl<W: Watcher> Collector<W> {
     pub fn initialize(root_path: &Path) -> io::Result<Self> {
         let mut inotify = Inotify::init()?;

```

Now let's fix `Collector::initialize` to call `watcher::watcher` and set the new field:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -61,17 +61,17 @@ pub struct Collector<W: Watcher> {
 }

 impl<W: Watcher> Collector<W> {
-    pub fn initialize(root_path: &Path) -> io::Result<Self> {
-        let mut inotify = Inotify::init()?;
+    pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
+        let watcher = watcher()?;

         debug!("Initialising watch on root path {:?}", root_path);
         let root_wd = inotify.add_watch(root_path, WatchMask::CREATE)?;

-        let mut collector = Self {
+        let mut collector = Collector {
             root_path: root_path.to_path_buf(),
             root_wd,
             live_files: HashMap::new(),
-            inotify,
+            watcher,
         };

         for entry in fs::read_dir(root_path)? {
```

Note that we're no longer able to use the `Self` reference, since `Self` refers to the monomorphised type, which the caller could theoretically specify:

```rust
Collector::<MyWatcher>::initialize()
```

We want to propagate the opaque type from `Watcher`, so for now we simply rewrite the signature and constructor, though we may find later that this won't blend.

Let's now fix the final error in `Collector::initialize` and switch from `inotify.add_watch` to `watcher.watch_directory`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -62,10 +62,10 @@ pub struct Collector<W: Watcher> {

 impl<W: Watcher> Collector<W> {
     pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
-        let watcher = watcher()?;
+        let mut watcher = watcher()?;

         debug!("Initialising watch on root path {:?}", root_path);
-        let root_wd = inotify.add_watch(root_path, WatchMask::CREATE)?;
+        let root_wd = watcher.watch_directory(root_path)?;

         let mut collector = Collector {
             root_path: root_path.to_path_buf(),
```

And now `Collector::initialize` has no compilation errors!
We'll now update the two references to `self.inotify` in `collect_entries` and `handle_event_create`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -85,14 +85,14 @@ impl<W: Watcher> Collector<W> {
         Ok(collector)
     }

-    pub fn collect_entries(&mut self, buffer: &mut [u8]) -> io::Result<Vec<LogEntry>> {
-        let inotify_events = self.inotify.read_events_blocking(buffer)?;
+    pub fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
+        let watcher_events = self.watcher.read_events_blocking()?;
         let mut entries = Vec::new();

-        for inotify_event in inotify_events {
-            trace!("Received inotify event: {:?}", inotify_event);
+        for watcher_event in watcher_events {
+            trace!("Received inotify event: {:?}", watcher_event);

-            if let Some(event) = self.check_event(inotify_event)? {
+            if let Some(event) = self.check_event(watcher_event)? {
                 debug!("{}", event);

                 let live_file = match event {
@@ -174,7 +174,7 @@ impl<W: Watcher> Collector<W> {
     fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
         let realpath = fs::canonicalize(&path)?;

-        let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
+        let wd = self.watcher.watch_file(&realpath)?;
         let mut reader = BufReader::new(File::open(realpath)?);
         reader.seek(io::SeekFrom::End(0))?;

```

We've gone ahead and renamed `inotify_event` to `watcher_event` and removed the `buffer` argument as well (we'll fix the call-site later).

This leaves us with two errors in `src/log_collector/mod.rs`, both in `check_event`.
This will be more difficult to update since it currently depends on the `mask` and `name` fields of `inotify::Event`, which are not represented in `watcher::Event`.
Let's start by updating `wd` to `descriptor` and then see what's left:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -122,20 +122,17 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event<'ev>(
-        &mut self,
-        inotify_event: inotify::Event<&'ev OsStr>,
-    ) -> io::Result<Option<Event>> {
-        if inotify_event.wd == self.root_wd {
-            if !inotify_event.mask.contains(EventMask::CREATE) {
+    fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Option<Event>> {
+        if watcher_event.descriptor == self.root_wd {
+            if !watcher_event.mask.contains(EventMask::CREATE) {
                 warn!(
                     "Received unexpected event for root fd: {:?}",
-                    inotify_event.mask
+                    watcher_event.mask
                 );
                 return Ok(None);
             }

-            let name = match inotify_event.name {
+            let name = match watcher_event.name {
                 None => {
                     warn!("Received CREATE event for root fd without a name");
                     return Ok(None);
@@ -150,11 +147,11 @@ impl<W: Watcher> Collector<W> {
             return Ok(Some(Event::Create { path }));
         }

-        let live_file = match self.live_files.get_mut(&inotify_event.wd) {
+        let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
             None => {
                 warn!(
                     "Received event for unregistered watch descriptor: {:?} {:?}",
-                    inotify_event.mask, inotify_event.wd
+                    watcher_event.mask, watcher_event.descriptor
                 );
                 return Ok(None);
             }
```

Now we'll look at each remaining 'no field' error in turn:

```rust
if !watcher_event.mask.contains(EventMask::CREATE) {
    warn!(
        "Received unexpected event for root fd: {:?}",
        watcher_event.mask
    );
    return Ok(None);
}
```

In this block we're asserting that an event we received for the `root_fd` is indeed a `CREATE` event, as we would have expected since we registered the watch with `WatchMask::CREATE`.
Since `kqueue` does not distinguish between file creation and writing to files, this distinction is not available in our `watcher::Event` abstraction.
As it is only a sanity check, it's reasonable for us to simply delete this entire block:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -124,14 +124,6 @@ impl<W: Watcher> Collector<W> {

     fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Option<Event>> {
         if watcher_event.descriptor == self.root_wd {
-            if !watcher_event.mask.contains(EventMask::CREATE) {
-                warn!(
-                    "Received unexpected event for root fd: {:?}",
-                    watcher_event.mask
-                );
-                return Ok(None);
-            }
-
             let name = match watcher_event.name {
                 None => {
                     warn!("Received CREATE event for root fd without a name");
```

Next up we have:

```rust
let name = match watcher_event.name {
    None => {
        warn!("Received CREATE event for root fd without a name");
        return Ok(None);
    }
    Some(name) => name,
};
```

This is where things get a little interesting.
`inotify` very helpfully includes the name of created files when watching a directory, whereas `kqueue` has no such capability (many complaints can be found online about this).
Instead, we will have to re-scan the directory and add any files that are not currently in `live_files`.
We can try to solve this for now by making `check_event` return a `Vec<Event>` and updating the root descriptor handling to re-scan `root_path` and emit events for new files:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -92,7 +92,7 @@ impl<W: Watcher> Collector<W> {
         for watcher_event in watcher_events {
             trace!("Received inotify event: {:?}", watcher_event);

-            if let Some(event) = self.check_event(watcher_event)? {
+            for event in self.check_event(watcher_event)? {
                 debug!("{}", event);

                 let live_file = match event {
@@ -122,21 +122,20 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Option<Event>> {
+    fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
         if watcher_event.descriptor == self.root_wd {
-            let name = match watcher_event.name {
-                None => {
-                    warn!("Received CREATE event for root fd without a name");
-                    return Ok(None);
-                }
-                Some(name) => name,
-            };
+            let events = Vec::new();
+
+            for entry in fs::read_dir(self.root_path)? {
+                let entry = entry?;
+                let path = entry.path();

-            let mut path = PathBuf::with_capacity(self.root_path.capacity() + name.len());
-            path.push(&self.root_path);
-            path.push(name);
+                if !self.live_files.contains_key(unimplemented!()) {
+                    events.push(Event::Create { path });
+                }
+            }

-            return Ok(Some(Event::Create { path }));
+            return Ok(events);
         }

         let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
@@ -145,7 +144,7 @@ impl<W: Watcher> Collector<W> {
                     "Received event for unregistered watch descriptor: {:?} {:?}",
                     watcher_event.mask, watcher_event.descriptor
                 );
-                return Ok(None);
+                return Ok(vec![]);
             }
             Some(live_file) => live_file,
         };
@@ -154,9 +153,9 @@ impl<W: Watcher> Collector<W> {
         let seekpos = live_file.reader.seek(io::SeekFrom::Current(0))?;

         if seekpos <= metadata.len() {
-            Ok(Some(Event::Append { live_file }))
+            Ok(vec![Event::Append { live_file }])
         } else {
-            Ok(Some(Event::Truncate { live_file }))
+            Ok(vec![Event::Truncate { live_file }])
         }
     }

```

Notice we have left an `unimplemented!()` call in the condition.
We cannot currently detect which files are new, because our `live_files` map is keyed by `watcher::Descriptor`, and these are only returned when we actually add a watch.
Our implementation of `Watcher::watch_{directory,file}` does not perform any deduplication, and we currently have no way of deduplicating them ourselves.

We may want to revisit our `Watcher` API in future, but for now lets hack ourselves out of this situation by adding a `watched_paths` hash map to our `Collector` that points a `PathBuf` to a `watcher::Descriptor`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -57,6 +57,7 @@ pub struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: watcher::Descriptor,
     live_files: HashMap<watcher::Descriptor, LiveFile>,
+    watched_files: HashMap<PathBuf, watcher::Descriptor>,
     watcher: W,
 }

@@ -71,12 +72,13 @@ impl<W: Watcher> Collector<W> {
             root_path: root_path.to_path_buf(),
             root_wd,
             live_files: HashMap::new(),
+            watched_files: HashMap::new(),
             watcher,
         };

         for entry in fs::read_dir(root_path)? {
             let entry = entry?;
-            let path = entry.path();
+            let path = fs::canonicalize(entry.path())?;

             debug!("{}", Event::Create { path: path.clone() });
             collector.handle_event_create(path)?;
@@ -128,9 +130,9 @@ impl<W: Watcher> Collector<W> {

             for entry in fs::read_dir(self.root_path)? {
                 let entry = entry?;
-                let path = entry.path();
+                let path = fs::canonicalize(entry.path())?;

-                if !self.live_files.contains_key(unimplemented!()) {
+                if !self.watched_files.contains_key(&path) {
                     events.push(Event::Create { path });
                 }
             }
@@ -160,10 +162,8 @@ impl<W: Watcher> Collector<W> {
     }

     fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
-        let realpath = fs::canonicalize(&path)?;
-
-        let wd = self.watcher.watch_file(&realpath)?;
-        let mut reader = BufReader::new(File::open(realpath)?);
+        let wd = self.watcher.watch_file(&path)?;
+        let mut reader = BufReader::new(File::open(&path)?);
         reader.seek(io::SeekFrom::End(0))?;

         self.live_files.insert(
@@ -174,6 +174,7 @@ impl<W: Watcher> Collector<W> {
                 entry_buf: String::new(),
             },
         );
+        self.watched_files.insert(path, wd.clone());
         Ok(self.live_files.get_mut(&wd).unwrap())
     }

```

Note that we also now `canonicalize` the file path before constructing an `Event::Create` – this is to ensure we are keying `watched_files` by the canonical path of the file.

We now have just one compiler error in `check_event`:

```rust
warn!(
    "Received event for unregistered watch descriptor: {:?} {:?}",
    watcher_event.mask, watcher_event.descriptor
);
```

We're firing a warning in the event that we receive an event for an unknown file descriptor.
The current message is trying to seperately print the `mask` and `descriptor`, but we can simplify it to log the entire event:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -143,8 +143,8 @@ impl<W: Watcher> Collector<W> {
         let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
             None => {
                 warn!(
-                    "Received event for unregistered watch descriptor: {:?} {:?}",
-                    watcher_event.mask, watcher_event.descriptor
+                    "Received event for unregistered watch descriptor: {:?}",
+                    watcher_event
                 );
                 return Ok(vec![]);
             }
```

We're now down to a single compilation error in `main.rs`:

```
error[E0061]: this function takes 0 arguments but 1 argument was supplied
  --> src/main.rs:66:33
   |
66 |         let entries = collector.collect_entries(&mut buffer)?;
   |                                 ^^^^^^^^^^^^^^^ ----------- supplied 1 argument
   |                                 |
   |                                 expected 0 arguments
   |
  ::: src/log_collector/mod.rs:90:5
   |
90 |     pub fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
   |     -------------------------------------------------------------- defined here
```

Let's fix that and see where we end up:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -61,9 +61,8 @@ fn init_collector(
     database: Arc<RwLock<Database>>,
 ) -> io::Result<()> {
     let mut collector = Collector::initialize(container_log_directory)?;
-    let mut buffer = [0; 1024];
     loop {
-        let entries = collector.collect_entries(&mut buffer)?;
+        let entries = collector.collect_entries()?;
         let mut database = task::block_on(database.write());
         for entry in entries {
             let key = entry.path.to_string_lossy();
```

Easy enough.
However, we've now uncovered a further error (presumably from subsequent compiler passes):

```
error[E0282]: type annotations needed for `log_collector::Collector<impl log_collector::watcher::Watcher>`
  --> src/main.rs:63:25
   |
63 |     let mut collector = Collector::initialize(container_log_directory)?;
   |         -------------   ^^^^^^^^^^^^^^^^^^^^^ cannot infer type for type parameter `W`
   |         |
   |         consider giving `collector` the explicit type `log_collector::Collector<impl log_collector::watcher::Watcher>`, where the type parameter `W` is specified
```

Alas, since the `W` parameter is extraneous for `Collector::initialize` Rust's type inference is not happy.
Since we can't name any `impl Watcher` types the suggestion of specifying the `W` parameter also won't work.
For now, let's add a `log_collector::initialize` function that passes an `impl Watcher` into `Collector::initialize`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -61,14 +61,17 @@ pub struct Collector<W: Watcher> {
     watcher: W,
 }

-impl<W: Watcher> Collector<W> {
-    pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
-        let mut watcher = watcher()?;
+pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
+    let watcher = watcher()?;
+    Collector::initialize(root_path, watcher)
+}

+impl<W: Watcher> Collector<W> {
+    fn initialize(root_path: &Path, mut watcher: W) -> io::Result<Self> {
         debug!("Initialising watch on root path {:?}", root_path);
         let root_wd = watcher.watch_directory(root_path)?;

-        let mut collector = Collector {
+        let mut collector = Self {
             root_path: root_path.to_path_buf(),
             root_wd,
             live_files: HashMap::new(),
```

Note that this has also allowed us to restore our use of the `Self` alias.
We've made `Collector::initialize` private for now, since the only way of obtaining a `Watcher` is through the (also private) `watcher` module.

Now we can update the call-site in `main`:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -16,7 +16,6 @@ use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
 use async_std::task;

-use log_collector::Collector;
 use log_database::Database;

 const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
@@ -60,10 +59,9 @@ fn init_collector(
     container_log_directory: &Path,
     database: Arc<RwLock<Database>>,
 ) -> io::Result<()> {
-    let mut collector = Collector::initialize(container_log_directory)?;
-    let mut buffer = [0; 1024];
+    let mut collector = log_collector::initialize(container_log_directory)?;
     loop {
-        let entries = collector.collect_entries(&mut buffer)?;
+        let entries = collector.collect_entries()?;
         let mut database = task::block_on(database.write());
         for entry in entries {
             let key = entry.path.to_string_lossy();
```

Making this change incovers a handful of error related to exclusive references and ownership.
Some of these are every easy to fix:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -129,9 +129,9 @@ impl<W: Watcher> Collector<W> {

     fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
         if watcher_event.descriptor == self.root_wd {
-            let events = Vec::new();
+            let mut events = Vec::new();

-            for entry in fs::read_dir(self.root_path)? {
+            for entry in fs::read_dir(&self.root_path)? {
                 let entry = entry?;
                 let path = fs::canonicalize(entry.path())?;

@@ -172,7 +172,7 @@ impl<W: Watcher> Collector<W> {
         self.live_files.insert(
             wd.clone(),
             LiveFile {
-                path,
+                path: path.clone(),
                 reader,
                 entry_buf: String::new(),
             },
```

A slightly more awkward one is caused when we iterate over the results of `check_event`:

```
error[E0499]: cannot borrow `*self` as mutable more than once at a time
   --> src/log_collector/mod.rs:104:47
    |
100 |             for event in self.check_event(watcher_event)? {
    |                          --------------------------------
    |                          |
    |                          first mutable borrow occurs here
    |                          first borrow later used here
...
104 |                     Event::Create { path } => self.handle_event_create(path)?,
    |                                               ^^^^ second mutable borrow occurs here
```

This happens because the mutable reference to `self` taken by `check_event` lives across the entire `for` loop.
We can fix this by building a list of new file paths and iterating them separately afterwards:

```diff
diff --git a/src/log_collector/mod.rs b/src/log_collector/mod.rs
index 84cbfd7..71a5ff4 100644
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -92,16 +92,37 @@ impl<W: Watcher> Collector<W> {

     pub fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
         let watcher_events = self.watcher.read_events_blocking()?;
+
         let mut entries = Vec::new();
+        let mut read_file = |live_file: &mut LiveFile| -> io::Result<()> {
+            while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
+                if live_file.entry_buf.ends_with('\n') {
+                    live_file.entry_buf.pop();
+                    let entry = LogEntry {
+                        path: live_file.path.clone(),
+                        line: live_file.entry_buf.clone(),
+                    };
+                    entries.push(entry);
+
+                    live_file.entry_buf.clear();
+                }
+            }
+            Ok(())
+        };

         for watcher_event in watcher_events {
             trace!("Received inotify event: {:?}", watcher_event);

+            let mut new_paths = Vec::new();
+
             for event in self.check_event(watcher_event)? {
                 debug!("{}", event);

                 let live_file = match event {
-                    Event::Create { path } => self.handle_event_create(path)?,
+                    Event::Create { path } => {
+                        new_paths.push(path);
+                        continue;
+                    }
                     Event::Append { live_file } => live_file,
                     Event::Truncate { live_file } => {
                         Self::handle_event_truncate(live_file)?;
@@ -109,18 +130,12 @@ impl<W: Watcher> Collector<W> {
                     }
                 };

-                while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
-                    if live_file.entry_buf.ends_with('\n') {
-                        live_file.entry_buf.pop();
-                        let entry = LogEntry {
-                            path: live_file.path.clone(),
-                            line: live_file.entry_buf.clone(),
-                        };
-                        entries.push(entry);
+                read_file(live_file)?;
+            }

-                        live_file.entry_buf.clear();
-                    }
-                }
+            for path in new_paths {
+                let live_file = self.handle_event_create(path)?;
+                read_file(live_file)?;
             }
         }

```

We've used a closure to read entries from a `LiveFile` to save us from repeating ourselves.
We're now down to a couple of warnings which we can quickly sweep up:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -2,7 +2,6 @@
 mod watcher;

 use std::collections::hash_map::HashMap;
-use std::ffi::OsStr;
 use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};
@@ -142,7 +141,7 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event<'ev>(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
+    fn check_event(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
         if watcher_event.descriptor == self.root_wd {
             let mut events = Vec::new();

```

## Did it blend?

Let's first check our tests still pass:

```
$ cargo test
...
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ make dockertest
...
test_1        | test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

So far so good.
Now let's validate the collector still behaves when running in Docker:

```
$ make down writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-21T15:06:26Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-21T15:06:26Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-21T15:06:27Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
```

Wow, this is some real "if it compiles it works" magic!
Finally, we should now be able to see our collector working locally:

```
$ mkdir .logs
$ CONTAINER_LOG_DIRECTORY=.logs RUST_LOG=monitoring_rs=debug cargo run
[2020-12-21T15:08:52Z DEBUG monitoring_rs::log_collector] Initialising watch on root path ".logs"

# in another tab
$ touch .logs/hello.log

# in our original tab
[2020-12-21T15:09:27Z DEBUG monitoring_rs::log_collector] Create /Users/chris/repos/monitoring-rs/.logs/hello.log

# in another tab
$ cat >> .logs/hello.log
hello?
world!

# in our original tab
[2020-12-21T15:10:11Z DEBUG monitoring_rs::log_collector] Append /Users/chris/repos/monitoring-rs/.logs/hello.log
[2020-12-21T15:10:18Z DEBUG monitoring_rs::log_collector] Append /Users/chris/repos/monitoring-rs/.logs/hello.log

# in another tab
$ curl localhost:8000/logs/$(pwd)/.logs/hello.log
["hello?","world!"]
```

And there we have it, our `log_collector` now works in Linux *and* MacOS 🎉

## Adding tests

We originally went down this rabbithole because we want to write tests for `log_collector`.
Let's add some basic tests just to ensure that we can:

```diff
diff --git a/src/log_collector/mod.rs b/src/log_collector/mod.rs
index a232282..e97331a 100644
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -201,3 +201,54 @@ impl<W: Watcher> Collector<W> {
         Ok(())
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use std::fs::File;
+    use std::io::Write;
+
+    #[test]
+    fn collect_entries_empty_file() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+        let mut collector =
+            super::initialize(&tempdir.path()).expect("unable to initialize collector");
+
+        let mut file_path = tempdir.path().to_path_buf();
+        file_path.push("test.log");
+        File::create(file_path).expect("failed to create temp file");
+
+        let entries: Vec<String> = collector
+            .collect_entries()
+            .expect("failed to collect entries")
+            .into_iter()
+            .map(|entry| entry.line)
+            .collect();
+        assert_eq!(entries, Vec::<String>::new());
+    }
+
+    #[test]
+    fn collect_entries_nonempty_file() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+        let mut collector =
+            super::initialize(&tempdir.path()).expect("unable to initialize collector");
+
+        let mut file_path = tempdir.path().to_path_buf();
+        file_path.push("test.log");
+        let mut file = File::create(file_path).expect("failed to create temp file");
+
+        collector
+            .collect_entries()
+            .expect("failed to collect entries");
+
+        writeln!(file, "hello?").expect("failed to write to file");
+        writeln!(file, "world!").expect("failed to write to file");
+
+        let entries: Vec<String> = collector
+            .collect_entries()
+            .expect("failed to collect entries")
+            .into_iter()
+            .map(|entry| entry.line)
+            .collect();
+        assert_eq!(entries, vec!["hello?".to_string(), "world!".to_string()]);
+    }
+}
```

And indeed, for `cargo test` and `make dockertest` we now get 8 passing tests:

```
$ cargo test
...
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ make dockertest
...
test_1        | test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

## Wrapping up

That turned out to be quite a lot of work, but we've achieved what we set out to do:

- Refactored `log_collector` to increase the surface area that can be tested platform-agnostically.
  In fact, all our tests are now platform-agnostic!
- Use conditional compilation to run tests involving `inotify` only on Linux (and introduce a `make` target to run tests in a container).
  We've introduced a `make dockertest` target and although we run the same tests on both MacOS and Linux, conditional compilation is used to change the underlying implementation.

There are still some quite rough edges, but we're now in a better state to resolve them – we can create failing tests and fix them!
We will probably take aim at a couple of such edges in the next post.

[Back to the README](../README.md#posts)
