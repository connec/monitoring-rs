# Log collection (part 3 – rotation)

## Rotators gonna rotate (again)

We tried to tackle log rotation during our aborted second foray into log collection in [Log collection (part 2 – aborted)](2-log-collection-part-2-aborted.md).
Let's add a new service to our `docker-compose.yaml` to perform a rotation on our writer's log file.

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -25,5 +25,22 @@ services:
     - -c
     - cat /var/log/containers/*

+  rotate:
+    image: alpine
+    volumes:
+    - logs:/var/log/containers
+    command:
+    - sh
+    - -c
+    - |
+      apk add --no-cache logrotate
+      cat <<EOF > test.config
+      /var/log/containers/writer.log {
+        copytruncate
+        size 1
+      }
+      EOF
+      logrotate --verbose test.config
+
 volumes:
   logs:
```

We should also add a new target to our `Makefile`:

```diff
--- a/Makefile
+++ b/Makefile
@@ -1,5 +1,5 @@
 # Makefile
-.PHONY: monitoring writer inspect down reset
+.PHONY: monitoring writer inspect rotate down reset

 monitoring:
  @docker-compose up --build --force-recreate monitoring
@@ -10,6 +10,9 @@ writer:
 inspect:
  @docker-compose up inspect

+rotate:
+ @docker-compose up rotate
+
 down:
  @docker-compose down --timeout 0 --volumes

```

Let's see what happens when we perform a rotation whilst monitoring:

```
$ make reset monitoring
...
monitoring_1  | Mon Oct 19 19:42:45 UTC 2020
monitoring_1  | Mon Oct 19 19:42:46 UTC 2020
monitoring_1  | Mon Oct 19 19:42:47 UTC 2020
monitoring_1  | Mon Oct 19 19:42:48 UTC 2020
monitoring_1  | Mon Oct 19 19:42:49 UTC 2020

# in another tab
$ make rotate
...
rotate_1      | reading config file test.config
rotate_1      | Reading state from file: /var/lib/logrotate.status
rotate_1      | Allocating hash table for state file, size 64 entries
rotate_1      |
rotate_1      | Handling 1 logs
rotate_1      |
rotate_1      | rotating pattern: /var/log/containers/writer.log  1 bytes (no old logs will be kept)
rotate_1      | empty log files are rotated, old logs are removed
rotate_1      | considering log /var/log/containers/writer.log
rotate_1      | Creating new state
rotate_1      |   Now: 2020-10-19 19:42
rotate_1      |   Last rotated at 2020-10-19 19:00
rotate_1      |   log needs rotating
rotate_1      | rotating log /var/log/containers/writer.log, log->rotateCount is 0
rotate_1      | dateext suffix '-20201019'
rotate_1      | glob pattern '-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
rotate_1      | renaming /var/log/containers/writer.log.1 to /var/log/containers/writer.log.2 (rotatecount 1, logstart 1, i 1),
rotate_1      | old log /var/log/containers/writer.log.1 does not exist
rotate_1      | renaming /var/log/containers/writer.log.0 to /var/log/containers/writer.log.1 (rotatecount 1, logstart 1, i 0),
rotate_1      | old log /var/log/containers/writer.log.0 does not exist
rotate_1      | log /var/log/containers/writer.log.2 doesn't exist -- won't try to dispose of it
rotate_1      | copying /var/log/containers/writer.log to /var/log/containers/writer.log.1
rotate_1      | truncating /var/log/containers/writer.log
monitoring-rs_rotate_1 exited with code 0

# back in our original tab
...
monitoring_1  | Mon Oct 19 19:42:58 UTC 2020
monitoring_1  | Mon Oct 19 19:42:59 UTC 2020
monitoring_1  | Mon Oct 19 19:43:00 UTC 2020
monitoring_1  | Mon Oct 19 19:43:01 UTC 2020
```

Note the gap in timestamps when we perform rotation, from `Mon Oct 19 19:42:49 UTC 2020` to `Mon Oct 19 19:42:58 UTC 2020` (obviously your dates and times will be different, and the gap may be longer or shorter).
This happens because `logrotate` truncates our file, but our monitoring process has already seeked some way into it.
Until the writer 'catches up' to that seek position, our monitoring process will be trying to copy a range beyond the end of the file, which ends up copying nothing.

How can we deal with this?
The simplest option would be to `stat` the file, and check if our offset is now greater than the length of the file.
This seems potentially expensive, but it's not clear that there's any better way (and, indeed, it's the approach that [Fluent Bit uses](https://github.com/fluent/fluent-bit/blob/master/plugins/in_tail/tail_fs_inotify.c#L245-L246)).

Let's add the to `Collector::handle_event_modify`:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -2,7 +2,7 @@
 use std::collections::HashMap;
 use std::ffi::OsStr;
 use std::fs::File;
-use std::io::{self, Stdout};
+use std::io::{self, Seek, Stdout};
 use std::path::{Path, PathBuf};

 use inotify::{EventMask, Inotify, WatchMask};
@@ -70,11 +70,10 @@ impl Collector {

     fn handle_event_modify(&mut self, path: PathBuf) -> io::Result<()> {
         if let Some(file) = self.live_files.get_mut(&path) {
+            Self::handle_truncation(file)?;
             io::copy(file, &mut self.stdout)?;
         } else {
             let mut file = File::open(&path)?;
-
-            use std::io::Seek;
             file.seek(io::SeekFrom::End(0))?;

             self.live_files.insert(path, file);
@@ -82,6 +81,17 @@ impl Collector {

         Ok(())
     }
+
+    fn handle_truncation(file: &mut File) -> io::Result<()> {
+        let metadata = file.metadata()?;
+        let seekpos = file.seek(io::SeekFrom::Current(0))?;
+
+        if seekpos > metadata.len() {
+            file.seek(io::SeekFrom::Start(0))?;
+        }
+
+        Ok(())
+    }
 }

 fn main() -> io::Result<()> {
```

Does this solve our rotation problem?

```
$ make reset monitoring
...
monitoring_1  | Mon Oct 19 20:27:11 UTC 2020
monitoring_1  | Mon Oct 19 20:27:12 UTC 2020
monitoring_1  | Mon Oct 19 20:27:13 UTC 2020
...

# in a new tab
$ make rotate
...

# back in our original tab
monitoring_1  | Mon Oct 19 20:27:14 UTC 2020
monitoring_1  | Mon Oct 19 20:27:15 UTC 2020
monitoring_1  | Mon Oct 19 20:27:16 UTC 2020
```

Et voila!
Our container is happily logging consecutive timestamps, all thanks to our simple truncation detection.
At least, we think so...
Maybe we should add some debug logging so we can more easily see what our program is doing as it develops.

## Logging

Let's add [`log`](https://crates.io/crates/log) and [`env_logger`](https://crates.io/crates/env_logger) to our project so we can see more of what our program is up to.
`log` will give us [macros](https://docs.rs/log/0.4.11/log/#macros) to log messages at different levels, and `env_logger` provides a logging implementation that writes to `stderr` and allows verbosity to be controlled by environment variables.

```
$ cargo add env_logger log
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding env_logger v0.8.1 to dependencies
      Adding log v0.4.11 to dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -7,4 +7,6 @@ edition = "2018"
 # See more keys and their definitions at https://doc.rust-lang.org/cargo/reference/manifest.html

 [dependencies]
+env_logger = "0.8.1"
 inotify = { version = "0.8.3", default-features = false }
+log = "0.4.11"
```

We can then initialise the logger in `main`, and add some useful messages:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,7 @@
 // main.rs
+#[macro_use]
+extern crate log;
+
 use std::collections::HashMap;
 use std::ffi::OsStr;
 use std::fs::File;
@@ -9,11 +12,13 @@ use inotify::{EventMask, Inotify, WatchMask};

 const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

+#[derive(Debug)]
 struct Event {
     event_type: EventType,
     path: PathBuf,
 }

+#[derive(Debug)]
 enum EventType {
     Modify,
 }
@@ -28,6 +33,8 @@ struct Collector {
 impl Collector {
     pub fn new(path: &Path) -> io::Result<Self> {
         let mut inotify = Inotify::init()?;
+
+        debug!("Initialising watch on path {:?}", &path);
         inotify.add_watch(path, WatchMask::MODIFY)?;

         Ok(Self {
@@ -42,7 +49,9 @@ impl Collector {
         let events = self.inotify.read_events_blocking(buffer)?;

         for event in events {
+            debug!("Received event: {:?}", event);
             if let Some(event) = self.check_event(event) {
+                debug!("Handling event: {:?}", event);
                 let handler = match event.event_type {
                     EventType::Modify => Self::handle_event_modify,
                 };
@@ -70,9 +79,10 @@ impl Collector {

     fn handle_event_modify(&mut self, path: PathBuf) -> io::Result<()> {
         if let Some(file) = self.live_files.get_mut(&path) {
-            Self::handle_truncation(file)?;
+            Self::handle_truncation(&path, file)?;
             io::copy(file, &mut self.stdout)?;
         } else {
+            debug!("Opening new log file {:?}", &path);
             let mut file = File::open(&path)?;
             file.seek(io::SeekFrom::End(0))?;

@@ -82,11 +92,12 @@ impl Collector {
         Ok(())
     }

-    fn handle_truncation(file: &mut File) -> io::Result<()> {
+    fn handle_truncation(path: &Path, file: &mut File) -> io::Result<()> {
         let metadata = file.metadata()?;
         let seekpos = file.seek(io::SeekFrom::Current(0))?;

         if seekpos > metadata.len() {
+            debug!("File {:?} was truncated, resetting seek position", path);
             file.seek(io::SeekFrom::Start(0))?;
         }

@@ -95,6 +106,8 @@ impl Collector {
 }

 fn main() -> io::Result<()> {
+    env_logger::init();
+
     let mut collector = Collector::new(CONTAINER_LOG_DIRECTORY.as_ref())?;

     let mut buffer = [0; 1024];
```

Note that we've added `#[derive(Debug)]` to our `Event` struct in order to be able to log the events we receive.

We should also update our `docker-compose.yaml` to actually show debug logs for our program:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -6,6 +6,8 @@ services:
     image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
     volumes:
     - logs:/var/log/containers
+    environment:
+    - RUST_LOG=monitoring_rs

   writer:
     image: alpine
```

Now let's see what we see:

```
$ make reset monitoring
monitoring_1  | [2020-10-26T10:48:02Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
monitoring_1  | [2020-10-26T10:48:03Z DEBUG monitoring_rs] Received event: Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }
monitoring_1  | [2020-10-26T10:48:03Z DEBUG monitoring_rs] Handling event: Event { event_type: Modify, path: "/var/log/containers/writer.log" }
monitoring_1  | [2020-10-26T10:48:03Z DEBUG monitoring_rs] Opening new log file "/var/log/containers/writer.log"
monitoring_1  | [2020-10-26T10:48:04Z DEBUG monitoring_rs] Received event: Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }
monitoring_1  | [2020-10-26T10:48:04Z DEBUG monitoring_rs] Handling event: Event { event_type: Modify, path: "/var/log/containers/writer.log" }
monitoring_1  | Mon Oct 26 10:48:04 UTC 2020
...

# in a new tab
$ make rotate
...

# back in our original tab
...
monitoring_1  | [2020-10-26T10:49:40Z DEBUG monitoring_rs] File "/var/log/containers/writer.log" was truncated, resetting seek position
...
```

The output is quite noisy now, but we can at least see everything that's going on!
And indeed, we see that our program is handling the truncation as we expect.
Very cool.

Let's reduce the verbosity a little by demoting received events to 'trace' and tidying up the format of handled events:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -49,9 +49,13 @@ impl Collector {
         let events = self.inotify.read_events_blocking(buffer)?;

         for event in events {
-            debug!("Received event: {:?}", event);
+            trace!("Received event: {:?}", event);
             if let Some(event) = self.check_event(event) {
-                debug!("Handling event: {:?}", event);
+                debug!(
+                    "Handling {:?} event for {}",
+                    event.event_type,
+                    event.path.display()
+                );
                 let handler = match event.event_type {
                     EventType::Modify => Self::handle_event_modify,
                 };
```

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -7,7 +7,7 @@ services:
     volumes:
     - logs:/var/log/containers
     environment:
-    - RUST_LOG=monitoring_rs
+    - RUST_LOG=monitoring_rs=debug

   writer:
     image: alpine
```

And now when we run it:

```
$ make reset monitoring
monitoring_1  | [2020-10-26T10:59:20Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
monitoring_1  | [2020-10-26T10:59:20Z DEBUG monitoring_rs] Handling Modify event for /var/log/containers/writer.log
monitoring_1  | [2020-10-26T10:59:20Z DEBUG monitoring_rs] Opening new log file "/var/log/containers/writer.log"
monitoring_1  | [2020-10-26T10:59:21Z DEBUG monitoring_rs] Handling Modify event for /var/log/containers/writer.log
monitoring_1  | Mon Oct 26 10:59:21 UTC 2020
monitoring_1  | [2020-10-26T10:59:22Z DEBUG monitoring_rs] Handling Modify event for /var/log/containers/writer.log
monitoring_1  | Mon Oct 26 10:59:22 UTC 2020
...
```

That's a bit better.

## What's an event, anyway?

Right now, we're doing a very minimal translation from inotify events to our own `EventType` enum.
What would happen if we tried to make our `EventType` more rich, to account for the different branches of logic that currently live inside `handle_event_modify`?
There are three possible paths through `handle_event_modify` just now:

- If the file is not currently in `live_files`, open it and add it.
- If the seek position is beyond the end of the file, seek back to the beginning and copy any available content.
- Otherwise, simply copy any available content.

Let's treat these as separate `Create`, `Truncate`, `Write` events:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -2,7 +2,7 @@
 #[macro_use]
 extern crate log;

-use std::collections::HashMap;
+use std::collections::hash_map::{self, HashMap};
 use std::ffi::OsStr;
 use std::fs::File;
 use std::io::{self, Seek, Stdout};
@@ -13,14 +13,45 @@ use inotify::{EventMask, Inotify, WatchMask};
 const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

 #[derive(Debug)]
-struct Event {
-    event_type: EventType,
-    path: PathBuf,
+enum Event<'collector> {
+    Create {
+        entry: hash_map::VacantEntry<'collector, PathBuf, File>,
+    },
+    Append {
+        entry: hash_map::OccupiedEntry<'collector, PathBuf, File>,
+    },
+    Truncate {
+        entry: hash_map::OccupiedEntry<'collector, PathBuf, File>,
+    },
 }

-#[derive(Debug)]
-enum EventType {
-    Modify,
+impl Event<'_> {
+    fn name(&self) -> &str {
+        match self {
+            Event::Create { .. } => "Create",
+            Event::Append { .. } => "Append",
+            Event::Truncate { .. } => "Truncate",
+        }
+    }
+
+    fn path(&self) -> &Path {
+        match self {
+            Event::Create { entry } => entry.key(),
+            Event::Append { entry } => entry.key(),
+            Event::Truncate { entry } => entry.key(),
+        }
+    }
+}
+
+impl std::fmt::Display for Event<'_> {
+    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
+        write!(f, "{} {}", self.name(), self.path().display())
+    }
+}
+
+struct EventContext<'collector> {
+    event: Event<'collector>,
+    stdout: &'collector mut Stdout,
 }

 struct Collector {
@@ -50,60 +81,83 @@ impl Collector {

         for event in events {
             trace!("Received event: {:?}", event);
-            if let Some(event) = self.check_event(event) {
-                debug!(
-                    "Handling {:?} event for {}",
-                    event.event_type,
-                    event.path.display()
-                );
-                let handler = match event.event_type {
-                    EventType::Modify => Self::handle_event_modify,
+            if let Some(mut context) = self.check_event(event)? {
+                debug!("{}", context.event);
+                match context.event {
+                    Event::Create { entry } => Self::handle_event_create(entry)?,
+                    Event::Append { entry } => {
+                        Self::handle_event_append(entry, &mut context.stdout)?
+                    }
+                    Event::Truncate { entry } => {
+                        Self::handle_event_truncate(entry, &mut context.stdout)?
+                    }
                 };
-                handler(self, event.path)?;
             }
         }

         Ok(())
     }

-    fn check_event<'ev>(&self, event: inotify::Event<&'ev OsStr>) -> Option<Event> {
-        let event_type = if event.mask.contains(EventMask::MODIFY) {
-            Some(EventType::Modify)
-        } else {
-            None
-        }?;
+    fn check_event<'ev>(
+        &mut self,
+        event: inotify::Event<&'ev OsStr>,
+    ) -> io::Result<Option<EventContext>> {
+        if !event.mask.contains(EventMask::MODIFY) {
+            return Ok(None);
+        }

-        let name = event.name?;
+        let name = match event.name {
+            None => return Ok(None),
+            Some(name) => name,
+        };
         let mut path = PathBuf::with_capacity(self.path.capacity() + name.len());
         path.push(&self.path);
         path.push(name);

-        Some(Event { event_type, path })
+        let event = match self.live_files.entry(path) {
+            hash_map::Entry::Vacant(entry) => Event::Create { entry },
+            hash_map::Entry::Occupied(mut entry) => {
+                let metadata = entry.get().metadata()?;
+                let seekpos = entry.get_mut().seek(io::SeekFrom::Current(0))?;
+
+                if seekpos <= metadata.len() {
+                    Event::Append { entry }
+                } else {
+                    Event::Truncate { entry }
+                }
+            }
+        };
+
+        Ok(Some(EventContext {
+            event,
+            stdout: &mut self.stdout,
+        }))
     }

-    fn handle_event_modify(&mut self, path: PathBuf) -> io::Result<()> {
-        if let Some(file) = self.live_files.get_mut(&path) {
-            Self::handle_truncation(&path, file)?;
-            io::copy(file, &mut self.stdout)?;
-        } else {
-            debug!("Opening new log file {:?}", &path);
-            let mut file = File::open(&path)?;
-            file.seek(io::SeekFrom::End(0))?;
+    fn handle_event_create(entry: hash_map::VacantEntry<'_, PathBuf, File>) -> io::Result<()> {
+        let mut file = File::open(entry.key())?;
+        file.seek(io::SeekFrom::End(0))?;

-            self.live_files.insert(path, file);
-        }
+        entry.insert(file);

         Ok(())
     }

-    fn handle_truncation(path: &Path, file: &mut File) -> io::Result<()> {
-        let metadata = file.metadata()?;
-        let seekpos = file.seek(io::SeekFrom::Current(0))?;
+    fn handle_event_append(
+        mut entry: hash_map::OccupiedEntry<'_, PathBuf, File>,
+        stdout: &mut Stdout,
+    ) -> io::Result<()> {
+        io::copy(entry.get_mut(), stdout)?;

-        if seekpos > metadata.len() {
-            debug!("File {:?} was truncated, resetting seek position", path);
-            file.seek(io::SeekFrom::Start(0))?;
-        }
+        Ok(())
+    }
+
+    fn handle_event_truncate(
+        mut entry: hash_map::OccupiedEntry<'_, PathBuf, File>,
+        stdout: &mut Stdout,
+    ) -> io::Result<()> {
+        entry.get_mut().seek(io::SeekFrom::Start(0))?;
+        io::copy(entry.get_mut(), stdout)?;

         Ok(())
     }
```

Significant changes!
Notably:

- Our `Event` is now an enum, with each variant containing a `hash_map::*Entry` struct representing an entry in the `live_files` `HashMap`.
- We've implemented `Display` on `Event` to show it as just `<type> <path>`.
- `check_event` now takes `&mut self` in order to use `live_files::entry` to construct the correct `Event` variant based on whether the entry is already present, and if so whether the seek position is beyond the end of the file.
- `check_event` returns an `EventContext` struct.
  This works around borrowing restrictions that would prevent us from accessing `Collector::stdout` as long as an `Event` exists.
  Rust prevents this because the `Event` returned by `check_event` is tied to the lifetime of its `&mut self` borrow, so as long as the `Event` is around Rust will not allow us to take mutable borrows for any part of `self`.
  It might be possible to avoid this struct by passing only the required parts of `self` into `check_event`, but for now this is fine.

We can re-run our log rotation test and see that output is now a bit clearer:

```
$ make reset monitoring
...
monitoring_1  | [2020-10-26T13:06:50Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
monitoring_1  | [2020-10-26T13:06:51Z DEBUG monitoring_rs] Create /var/log/containers/writer.log
monitoring_1  | [2020-10-26T13:06:52Z DEBUG monitoring_rs] Append /var/log/containers/writer.log
monitoring_1  | Mon Oct 26 13:06:52 UTC 2020
...

# in another tab
$ make rotate

# back in our original tab
...
monitoring_1  | [2020-10-26T13:07:51Z DEBUG monitoring_rs] Truncate /var/log/containers/writer.log
...
```

Nice!

## Validate in Kubernetes

Before going any further, we should validate that our collector will work in Kubernetes.
Ideally we should try and force log rotation to make sure that behaves as expected.
All that in next installment!

[Back to the README](../README.md#posts)
