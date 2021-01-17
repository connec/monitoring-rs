# Log collection (part 9 – metadata)

With a skeleton log collector up and running and testable locally, it's time to take aim at adding metadata to log messages.

Our end goal would see our log collector annotating all log lines with information about the source container, including things like the container name, the name of the container's pod, the pod's namespace, and ideally labels on the pod.
This metadata would make it much easier to find relevant logs when searching.

## Collecting JSON

Before considering how to actually collect this data, let's convert our 'pipeline' to transport structured data, rather than `String`s.
Although we only have one field just now (the log message), this will get us into a position where we can start to add fields in the collector without changing the database/API (and later improve the database/API to support querying metadata).

To keep things simple, we'll change our interfaces to send and receive `serde_json::Value`s, rather than `String`s.
Let's start by adding `serde_json`:

```sh
$ cargo add serde_json
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding serde_json v1.0.61 to dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -13,6 +13,7 @@ tide = "0.15.0"
 async-std = { version = "1.7.0", features = ["attributes"] }
 blocking = "1.0.2"
 md5 = "0.7.0"
+serde_json = "1.0.61"

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

Let's start by introducing structure in `Collector`, as opposed to `Watcher`.
We might hope to later add metadata collection to `Collector`, perhaps using the `LiveFile` structure for caching.

We'll change our `LogEntry` struct to carry a `metadata: HashMap<String, serde_json::Value>` field:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -50,6 +50,7 @@ struct LiveFile {
 pub struct LogEntry {
     pub path: PathBuf,
     pub line: String,
+    pub metadata: HashMap<String, serde_json::Value>,
 }

 pub struct Collector<W: Watcher> {
@@ -100,6 +101,7 @@ impl<W: Watcher> Collector<W> {
                     let entry = LogEntry {
                         path: live_file.path.clone(),
                         line: live_file.entry_buf.clone(),
+                        metadata: HashMap::new(),
                     };
                     entries.push(entry);

```

So far so easy.
Next we need to decide how we want metadata to be stored in our database.

## Storing JSON

Our current database format is a flat file of log lines per key (log file path), with byte `147` as a separator between lines.
If we want to eventually run metadata-based queries performantly, we will likely want to store the metadata as an index against the flat file of log entries.

First, let's use this to motivate a new signature for `Database::write`, which currently looks like:

```rust
pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
    ...
}
```

There are two obvious ways to get our `metadata` in:

- As an additional `metadata: HashMap<String, serde_json::Value>` argument.
- By replacing `line` (and possibly `key` as well) with a struct containing all three.

The latter option is interesting, since we already have such a struct: `LogEntry`.
If we build our interfaces around that struct, it should be more efficient than splitting and combining the same pieces of data at each interface point.

In order to avoid introducing additional dependences on the `log_collector` module, we can promote the `LogEntry` struct up to `lib.rs`:

```rust
// lib.rs
#[macro_use]
extern crate log;

pub mod api;
pub mod log_collector;
pub mod log_database;

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug)]
pub struct LogEntry {
    pub path: PathBuf,
    pub line: String,
    pub metadata: HashMap<String, serde_json::Value>,
}
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,11 +1,4 @@
 // main.rs
-#[macro_use]
-extern crate log;
-
-mod api;
-mod log_collector;
-mod log_database;
-
 use std::env;
 use std::fs;
 use std::io;
@@ -16,7 +9,8 @@ use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
 use async_std::task;

-use log_database::Database;
+use monitoring_rs::log_database::{self, Database};
+use monitoring_rs::{api, log_collector};

 const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
 const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
```

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -8,6 +8,8 @@ use std::path::{Path, PathBuf};

 use watcher::{watcher, Watcher};

+use crate::LogEntry;
+
 #[derive(Debug)]
 enum Event<'collector> {
     Create { path: PathBuf },
@@ -46,13 +48,6 @@ struct LiveFile {
     entry_buf: String,
 }

-#[derive(Debug)]
-pub struct LogEntry {
-    pub path: PathBuf,
-    pub line: String,
-    pub metadata: HashMap<String, serde_json::Value>,
-}
-
 pub struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: watcher::Descriptor,
```

We might want to change the name/type of `LogEntry`'s `path` field at some point to be more convenient for retrieval, but we'll cross that bridge when we come to it.
For now, we have what we need to update `Database::write` to accept metadata via `LogEntry`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -5,6 +5,8 @@ use std::fs::{self, File, OpenOptions};
 use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
 use std::path::PathBuf;

+use crate::LogEntry;
+
 const DATA_FILE_EXTENSION: &str = "dat";
 const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

@@ -95,8 +97,8 @@ impl Database {
         Ok(Some(lines))
     }

-    pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
-        let key_hash = Self::hash(key);
+    pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
+        let key_hash = Self::hash(&entry.path.to_string_lossy());
         let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
             Some(file) => (file, true),
             None => {
@@ -122,7 +124,7 @@ impl Database {
         if needs_delimeter {
             file.write_all(&[DATA_FILE_RECORD_SEPARATOR])?;
         }
-        file.write_all(line.as_ref())?;
+        file.write_all(entry.line.as_ref())?;

         Ok(())
     }
@@ -158,6 +160,10 @@ pub mod test {

 #[cfg(test)]
 mod tests {
+    use std::collections::HashMap;
+
+    use crate::LogEntry;
+
     use super::test::open_temp_database;
     use super::{Config, Database};

@@ -171,7 +177,11 @@ mod tests {
         );

         database
-            .write("foo", "line1")
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line1".into(),
+                metadata: HashMap::new(),
+            })
             .expect("unable to write to database");
         assert_eq!(
             database.read("foo").expect("unable to read from database"),
@@ -179,7 +189,11 @@ mod tests {
         );

         database
-            .write("foo", "line2")
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line2".into(),
+                metadata: HashMap::new(),
+            })
             .expect("unable to write to database");
         assert_eq!(
             database.read("foo").expect("unable to read from database"),
@@ -193,10 +207,18 @@ mod tests {
         let data_directory = database.data_directory.clone();

         database
-            .write("foo", "line1")
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line1".into(),
+                metadata: HashMap::new(),
+            })
             .expect("failed to write to database");
         database
-            .write("foo", "line2")
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line2".into(),
+                metadata: HashMap::new(),
+            })
             .expect("failed to write to database");
         drop(database);

```

We also need to update `main.rs` and `api::tests` to speak the new interface:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -58,8 +58,7 @@ fn init_collector(
         let entries = collector.collect_entries()?;
         let mut database = task::block_on(database.write());
         for entry in entries {
-            let key = entry.path.to_string_lossy();
-            database.write(&key, &entry.line)?;
+            database.write(&entry)?;
         }
     }
 }
```

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -30,11 +30,13 @@ async fn read_logs(req: tide::Request<State>) -> tide::Result {
 #[cfg(test)]
 mod tests {
     use async_std::sync::RwLock;
+    use std::collections::HashMap;
     use std::sync::Arc;

     use tide_testing::TideTestingExt;

     use crate::log_database::test::open_temp_database;
+    use crate::LogEntry;

     #[async_std::test]
     async fn read_logs_non_existent_key() {
@@ -49,8 +51,20 @@ mod tests {
     #[async_std::test]
     async fn read_logs_existing_key() {
         let (mut database, _tempdir) = open_temp_database();
-        database.write("/foo", "hello").unwrap();
-        database.write("/foo", "world").unwrap();
+        database
+            .write(&LogEntry {
+                path: "/foo".into(),
+                line: "hello".into(),
+                metadata: HashMap::new(),
+            })
+            .unwrap();
+        database
+            .write(&LogEntry {
+                path: "/foo".into(),
+                line: "world".into(),
+                metadata: HashMap::new(),
+            })
+            .unwrap();

         let api = super::server(Arc::new(RwLock::new(database)));

```

This is not the most ergonomic interface in tests, but we can improve things later if desired.

Rather than spending too much time considering how metadata will be stored, let's cheap out in order to focus on collection first:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -98,6 +98,10 @@ impl Database {
     }

     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
+        if !entry.metadata.is_empty() {
+            unimplemented!("Database::write with non-empty metadata is not implemented")
+        }
+
         let key_hash = Self::hash(&entry.path.to_string_lossy());
         let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
             Some(file) => (file, true),
@@ -230,4 +234,22 @@ mod tests {
             Some(vec!["line1".to_string(), "line2".to_string()])
         );
     }
+
+    #[test]
+    #[should_panic(
+        expected = "not implemented: Database::write with non-empty metadata is not implemented"
+    )]
+    fn test_write_metadata() {
+        let (mut database, _tempdir) = open_temp_database();
+
+        database
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line1".into(),
+                metadata: vec![("hello".to_string(), serde_json::json!("world"))]
+                    .into_iter()
+                    .collect(),
+            })
+            .expect("failed to write to database");
+    }
 }
```

We have added a test so we can be sure our panic applies, and to remind us of that behaviour!

## Collecting metadata

We now want to populate our `metadata` field with some... metadata.
This will require us to query the Kubernetes API to retrieve metadata about the container.
Since containers are not first-class in the Kubernetes API, this will require us to retrieve metadata about the pod the container us running in.

If we take a look at the Kubernetes API documentation, we can find the [read operations for Pod v1 core](https://kubernetes.io/docs/reference/generated/kubernetes-api/v1.20/#-strong-read-operations-pod-v1-core-strong-).
This shows us that we would need to determine the container's pod's name and namespace in order to perform a ["Read" operation](https://kubernetes.io/docs/reference/generated/kubernetes-api/v1.20/#read-pod-v1-core).
If we recall our learnings from [Log collction (part 1)](01-log-collection-part-1.md), we found that:

> The log files in `/var/log/containers` have the following form:
>
> ```
> <pod name>_<namespace>_<container name>-<container ID>.log
> ```

Since pod names, namespaces, and container names must be DNS labels, we can safely split on `_` to separate the components.
However, we do need to remember that our tests and local environments don't currently follow this naming format, and we will have to deal with their absence somehow.
Hrm.

## Collector configuration

Our hope with this entire project is to keep the configuration surface area as minimal as possible, and particularly to follow a 'convention over configuration' approach optimised for deployment on Kubernetes.
However, one way we could allow ourselves to experiment with metadata without having to test in Kubernetes would be to introduce some collector configuration, instructing the collector on how to find metadata.

We should start by thinking about our 'domain model' a little bit.
We're considering how to attach metadata to log entries, but for our Kubernetes case the metadata really pertains to the log files themselves.
We could be a bit generic here and consider a *log source* to be a domain object through which our collector can be notified of new log entries, with relevant metadata.
We could then think about two different log sources for now:

- Our ideal Kubernetes log source, which monitors the Kubernetes log directory for new log files, parses their names to lookup metadata, and feeds log entries and metadata to the collector.
- A 'basic' log source, which monitors an arbitrary directory for new log files, and feeds log entries and configured metadata to the collector.

With this idea, we start to see why existing log collectors have the range of configuration they do, and typically follow a 'pipeline' model with 'sources' and 'transformers' (though perhaps by different names).
With that terminology we could imagine:

- The Kubernetes pipeline could be built from a (zero-configuration) 'Kubernetes' source, metadata could be added by a (zero-configuration) 'Kubernetes metadata' transformer.
  We could imagine variants with optional configuration to override the default Kubernetes log directory, API server, etc.
- The 'basic' pipeline could be built from a 'log directory' source, metadata could be added by a 'metadata' transformer.
  The log directory source would need to be configured with a path, and the metadata transformer would be configured with hard-coded metadata.

A hypothetical pipeline configuration could look like:

```toml
[[pipelines]]
name = "varlog"
source = "directory"
transformer = "metadata"

[varlog.source]
path = "/var/log"

[varlog.transformer]
custom_field = "hello"

[[pipelines]]
name = "kubernetes"
source = "kubernetes"
transformer = "kubernetes"
```

To support zero-configuration for Kubernetes, the default configuration might then simply be:

```toml
[[pipelines]]
name = "kubernetes"
source = "kubernetes"
transformer = "kubernetes"
```

Of course, "pipeline" feels like a bit of a misnomer if we only allow a single transformer.
It's also worth considering that this hypothetical configuration format allows arbitrary combinations of sources and transformers (and/or some kind of dependency mechanism to prevent that) – such as a "directory" source with a "kubernetes" transformer.
Whilst this could make sense when testing, it increases the surface area of viable configuration which would need to be tested and supported, even though it may be an extremely marginal situation.

For now, we could opt for something much simpler – we could consider multiple implementations of our log collector, which could be chosen and configured via configuration, e.g.:

```toml
log_collector = "kubernetes"

# or

[log_collector]
name = "directory"
path = "/var/log"

[log_collector.metadata]
custom_key = "hello"
```

That way, we only have to validate the two log collector implementations.

## Refactoring `log_collector`

Let's work towards having a `log_collector::Collector` trait, and move our current `LogCollector` implementation into a `log_collector::directory::Collector` struct, which should implement `Collector`.

We can start by moving our current `Collector` to `directory::Collector` to prevent naming conflicts.

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/directory.rs
@@ -1,15 +1,12 @@
-// log_collector/mod.rs
-mod watcher;
-
-use std::collections::hash_map::HashMap;
+use std::collections::HashMap;
 use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};

-use watcher::{watcher, Watcher};
-
 use crate::LogEntry;

+use super::watcher::{self, watcher, Watcher};
+
 #[derive(Debug)]
 enum Event<'collector> {
     Create { path: PathBuf },
```

```rust
// log_collector/mod.rs
pub mod directory;
mod watcher;
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -53,7 +53,7 @@ fn init_collector(
     container_log_directory: &Path,
     database: Arc<RwLock<Database>>,
 ) -> io::Result<()> {
-    let mut collector = log_collector::initialize(container_log_directory)?;
+    let mut collector = log_collector::directory::initialize(container_log_directory)?;
     loop {
         let entries = collector.collect_entries()?;
         let mut database = task::block_on(database.write());
```

Now we can introduce a `log_collector::Collector` trait.
We'll keep this super minimal for now, and just say that a `Collector` must implement `Iterator<Item = LogEntry>`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -1,3 +1,7 @@
 // log_collector/mod.rs
 pub mod directory;
 mod watcher;
+
+use crate::LogEntry;
+
+pub trait Collector: Iterator<Item = LogEntry> {}
```

We could introduce a blanket implementation on anything implementing `Iterator<Item = LogEntry>`, e.g.:

```rust
impl<T> Collector for T where T: Iterator<Item = LogEntry> {}
```

For now we will not do this, however, meaning `Collector` will behave as a 'marker trait' for types that are intended to be used as collectors.
Let's implement it for `Directory`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -196,6 +196,8 @@ impl<W: Watcher> Collector<W> {
     }
 }

+impl<W: Watcher> super::Collector for Collector<W> {}
+
 #[cfg(test)]
 mod tests {
     use std::fs::File;
```

Of course, we now get a compiler error:

```
$ cargo check
...
error[E0277]: `log_collector::directory::Collector<W>` is not an iterator
   --> src/log_collector/directory.rs:199:18
    |
199 | impl<W: Watcher> super::Collector for Collector<W> {}
    |                  ^^^^^^^^^^^^^^^^ `log_collector::directory::Collector<W>` is not an iterator
    |
   ::: src/log_collector/mod.rs:7:22
    |
7   | pub trait Collector: Iterator<Item = LogEntry> {}
    |                      ------------------------- required by this bound in `log_collector::Collector`
    |
    = help: the trait `std::iter::Iterator` is not implemented for `log_collector::directory::Collector<W>`
...
```

So we need to implement `Iterator`, let's stub that out:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -198,6 +198,14 @@ impl<W: Watcher> Collector<W> {

 impl<W: Watcher> super::Collector for Collector<W> {}

+impl<W: Watcher> Iterator for Collector<W> {
+    type Item = LogEntry;
+
+    fn next(&mut self) -> Option<Self::Item> {
+        todo!()
+    }
+}
+
 #[cfg(test)]
 mod tests {
     use std::fs::File;
```

Everything now compiles again, but if we introduce a test for iteration we will get a panic:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -255,4 +255,28 @@ mod tests {
             .collect();
         assert_eq!(entries, vec!["hello?".to_string(), "world!".to_string()]);
     }
+
+    #[test]
+    fn iterator_yields_entries() {
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
+        let entry = collector.next().expect("expected at least 1 entry");
+        assert_eq!(entry.line, "hello?".to_string());
+
+        let entry = collector.next().expect("expected at least 2 entries");
+        assert_eq!(entry.line, "world!".to_string());
+    }
 }
```

```
$ cargo test
---- log_collector::directory::tests::iterator_yields_entries stdout ----
thread 'log_collector::directory::tests::iterator_yields_entries' panicked at 'not yet implemented', src/log_collector/directory.rs:205:9
```

Let's get this test passing.
We'll start with something silly: calling `collect_entries` and returning the first item in the `Vec`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -202,7 +202,7 @@ impl<W: Watcher> Iterator for Collector<W> {
     type Item = LogEntry;

     fn next(&mut self) -> Option<Self::Item> {
-        todo!()
+        self.collect_entries().unwrap().into_iter().next()
     }
 }

```

Now our test hangs 😱

```
$ cargo test log_collector::directory::tests::iterator_yields_entries
    Finished test [unoptimized + debuginfo] target(s) in 0.14s
     Running target/debug/deps/monitoring_rs-ae9379cb3ab46a15

running 1 test
...
```

This is due to the blocking nature of `collect_entries`, which has no events to collect the second time we call it.
In order to return a single item from `next`, we're going to have to add an 'event buffer' to `Collector` and have `next` fill the buffer from `collect_entries`, and return the next event from it:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -51,6 +51,7 @@ pub struct Collector<W: Watcher> {
     live_files: HashMap<watcher::Descriptor, LiveFile>,
     watched_files: HashMap<PathBuf, watcher::Descriptor>,
     watcher: W,
+    entry_buf: std::vec::IntoIter<LogEntry>,
 }

 pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
@@ -69,6 +70,7 @@ impl<W: Watcher> Collector<W> {
             live_files: HashMap::new(),
             watched_files: HashMap::new(),
             watcher,
+            entry_buf: vec![].into_iter(),
         };

         for entry in fs::read_dir(root_path)? {
@@ -202,7 +204,10 @@ impl<W: Watcher> Iterator for Collector<W> {
     type Item = LogEntry;

     fn next(&mut self) -> Option<Self::Item> {
-        self.collect_entries().unwrap().into_iter().next()
+        if self.entry_buf.len() == 0 {
+            self.entry_buf = self.collect_entries().unwrap().into_iter();
+        }
+        self.entry_buf.next()
     }
 }

```

And now our new test passes:

```
$ cargo test
...
test log_collector::directory::tests::iterator_yields_entries ... ok
...
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

One final issue with our `Iterator` implementation is the `unwrap` after `collect_entries`.
`unwrap` should generally be considered a smell, and should at least have a comment explaining why `unwrap` is OK in a particular scenario (e.g. if you just pushed to a `Vec`, `vec.get(0)` will always be `Some(_)`).
In this case it's definitely not OK – `next` will panic if `collect_entries` encounters any IO error.
Although our current implementation bubbles up IO errors, and would crash anyway, we might in future decide that IO errors when interacting with a file should just be logged, and collection should continue to unaffected files.
This change would be much easier to make if we're handling errors consistently – in our case bubbling them up.

In order to get a `Result` out of our `Iterator` implementation, we'll have to change the trait bound for `Collector` to use `Iterator<Item = Result<LogEntry>>`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -2,6 +2,8 @@
 pub mod directory;
 mod watcher;

+use std::io;
+
 use crate::LogEntry;

-pub trait Collector: Iterator<Item = LogEntry> {}
+pub trait Collector: Iterator<Item = Result<LogEntry, io::Error>> {}
```

Now we can update our `Iterator` implementation for `Directory`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -201,13 +201,17 @@ impl<W: Watcher> Collector<W> {
 impl<W: Watcher> super::Collector for Collector<W> {}

 impl<W: Watcher> Iterator for Collector<W> {
-    type Item = LogEntry;
+    type Item = Result<LogEntry, io::Error>;

     fn next(&mut self) -> Option<Self::Item> {
         if self.entry_buf.len() == 0 {
-            self.entry_buf = self.collect_entries().unwrap().into_iter();
+            let entries = match self.collect_entries() {
+                Ok(entries) => entries,
+                Err(error) => return Some(Err(error)),
+            };
+            self.entry_buf = entries.into_iter();
         }
-        self.entry_buf.next()
+        Some(Ok(self.entry_buf.next()?))
     }
 }

```

Finally, we need to update our iterator test:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -282,10 +282,16 @@ mod tests {
         writeln!(file, "hello?").expect("failed to write to file");
         writeln!(file, "world!").expect("failed to write to file");

-        let entry = collector.next().expect("expected at least 1 entry");
+        let entry = collector
+            .next()
+            .expect("expected at least 1 entry")
+            .expect("failed to collect entries");
         assert_eq!(entry.line, "hello?".to_string());

-        let entry = collector.next().expect("expected at least 2 entries");
+        let entry = collector
+            .next()
+            .expect("expected at least 2 entries")
+            .expect("failed to collect entries");
         assert_eq!(entry.line, "world!".to_string());
     }
 }
```

Next, we would ideally like to keep `directory::Collector` out of the public API of our `log_collector` module, and instead interact with collectors exclusively via the `Collector` trait.
We can do some compiler-guided refactoring here and start by updating `directory::initialize` to return an `impl Collector`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -54,7 +54,7 @@ pub struct Collector<W: Watcher> {
     entry_buf: std::vec::IntoIter<LogEntry>,
 }

-pub fn initialize(root_path: &Path) -> io::Result<Collector<impl Watcher>> {
+pub fn initialize(root_path: &Path) -> io::Result<impl super::Collector> {
     let watcher = watcher()?;
     Collector::initialize(root_path, watcher)
 }
```

Our first error is from `main`:

```
$ cargo check
...
error[E0599]: no method named `collect_entries` found for opaque type `impl monitoring_rs::log_collector::Collector` in the current scope
  --> src/main.rs:58:33
   |
58 |         let entries = collector.collect_entries()?;
   |                                 ^^^^^^^^^^^^^^^ method not found in `impl monitoring_rs::log_collector::Collector`
...
```

We can update `main` to use the `Iterator` interface instead:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -53,12 +53,11 @@ fn init_collector(
     container_log_directory: &Path,
     database: Arc<RwLock<Database>>,
 ) -> io::Result<()> {
-    let mut collector = log_collector::directory::initialize(container_log_directory)?;
-    loop {
-        let entries = collector.collect_entries()?;
+    let collector = log_collector::directory::initialize(container_log_directory)?;
+    for entry in collector {
+        let entry = entry?;
         let mut database = task::block_on(database.write());
-        for entry in entries {
-            database.write(&entry)?;
-        }
+        database.write(&entry)?;
     }
+    Ok(())
 }
```

Finally, we need to update our tests to use `Collector::initialize` rather than `initialize`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -220,11 +220,16 @@ mod tests {
     use std::fs::File;
     use std::io::Write;

+    use crate::log_collector::watcher::watcher;
+
+    use super::Collector;
+
     #[test]
     fn collect_entries_empty_file() {
         let tempdir = tempfile::tempdir().expect("unable to create tempdir");
         let mut collector =
-            super::initialize(&tempdir.path()).expect("unable to initialize collector");
+            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
+                .expect("unable to initialize collector");

         let mut file_path = tempdir.path().to_path_buf();
         file_path.push("test.log");
@@ -243,7 +248,8 @@ mod tests {
     fn collect_entries_nonempty_file() {
         let tempdir = tempfile::tempdir().expect("unable to create tempdir");
         let mut collector =
-            super::initialize(&tempdir.path()).expect("unable to initialize collector");
+            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
+                .expect("unable to initialize collector");

         let mut file_path = tempdir.path().to_path_buf();
         file_path.push("test.log");
@@ -269,7 +275,8 @@ mod tests {
     fn iterator_yields_entries() {
         let tempdir = tempfile::tempdir().expect("unable to create tempdir");
         let mut collector =
-            super::initialize(&tempdir.path()).expect("unable to initialize collector");
+            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
+                .expect("unable to initialize collector");

         let mut file_path = tempdir.path().to_path_buf();
         file_path.push("test.log");
```

Finally finally, we should 'demote' some `log_collector::directory` items that don't need to be `pub`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -45,7 +45,7 @@ struct LiveFile {
     entry_buf: String,
 }

-pub struct Collector<W: Watcher> {
+struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: watcher::Descriptor,
     live_files: HashMap<watcher::Descriptor, LiveFile>,
@@ -84,7 +84,7 @@ impl<W: Watcher> Collector<W> {
         Ok(collector)
     }

-    pub fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
+    fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
         let watcher_events = self.watcher.read_events_blocking()?;

         let mut entries = Vec::new();
```

## Back to storing metadata

Before we go into allowing the `Collector` to be configured let's revisit storing and retrieving metadata.
(We're meandering around a bit... I guess this is what happens when you work on something for a few hours every 1-2 weeks.)

For now, let's consider how can enable `key=value` queries for metadata.
We can start by updating our panicking test to exercise a stub API:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -97,6 +97,10 @@ impl Database {
         Ok(Some(lines))
     }

+    pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
+        todo!()
+    }
+
     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
         if !entry.metadata.is_empty() {
             unimplemented!("Database::write with non-empty metadata is not implemented")
@@ -236,20 +240,42 @@ mod tests {
     }

     #[test]
-    #[should_panic(
-        expected = "not implemented: Database::write with non-empty metadata is not implemented"
-    )]
-    fn test_write_metadata() {
+    fn test_metadata() {
         let (mut database, _tempdir) = open_temp_database();

         database
             .write(&LogEntry {
                 path: "foo".into(),
                 line: "line1".into(),
+                metadata: HashMap::new(),
+            })
+            .expect("failed to write to database");
+
+        database
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line2".into(),
                 metadata: vec![("hello".to_string(), serde_json::json!("world"))]
                     .into_iter()
                     .collect(),
             })
             .expect("failed to write to database");
+
+        database
+            .write(&LogEntry {
+                path: "foo".into(),
+                line: "line3".into(),
+                metadata: vec![("hello".to_string(), serde_json::json!("foo"))]
+                    .into_iter()
+                    .collect(),
+            })
+            .expect("failed to write to database");
+
+        assert_eq!(
+            database
+                .query("hello", "world")
+                .expect("failed to query database"),
+            Some(vec!["line2".to_string()])
+        );
     }
 }
```

But how can we implement `Database::query`?
There are quite a few possibilities here, including:

- If we store metadata in entries in `Database::write`, then `Database::query` could then scan all files for matching entries.
  This would be quite inefficient – having to scan the entirety of every file, and parse every entry.

- If we create separate data files for each `key=value` pair in `Database::write`, then `Database::query` could simply read the data file for the given `key` and `value`.
  This would be quite efficient for querying, since it would only scan relevant entries.
  However it would be quite storage inefficient, leading to duplicate entries for every distinct `key=value` entry in metadata.

- We could do something in-between, such as maintaining a `hash(metadata)` index of entries with each unique combination of metadata, as well as an index from each `key=value` in the metadata to the `hash(metadata)` files.
  `Database::query` could then lookup the possible `hash(metadata)` files for the given `key=value`, and merge the results from the files.
  This would have less duplication, and would only need to scan files with relevant records.

This last option could have legs, since we expect each log file to have the same metadata.
This means we would store the same number of files as we do now (if metadata never changes), and will give us roughly the same query behaviour.

We will have to rethink `Database::read`, however, since entries will no longer be recorded by `key`.
We could deal with this quite simply by making the `key` part of the metadata (perhaps more appropriately named `path`).

So we want to work towards the following data layout:

```
.data/
  <hash1>.dat
  <hash2>.dat
  ...
  index.json
```

Let's imagine `index.json` has the following structure:

```json
{
  "key=value1": ["<hash1>.dat", "<hash2>.dat"],
  "key=value2": ["<hash1>.dat"]
}
```

Given a query for `key=value1`, we would first look up `key=value1` in the index and find the two dat files.
We would then merge the entries from both the `<hash1>.dat` and `<hash2>.dat` files.

This structure probably exposes us to integrity issues – a failure to store the index would cause us to lose the mapping to file names.
To protect us from this, we could store the metadata in a 'header' in the file.
In fact, if we store such a header we could recreate the index in memory when the database starts up, and maintain the index exclusively in memory.
Alternatively, we could keep the `.dat` files simple and write the metadata into a separate file (e.g. `<hash>.json`).
It's not immediately obvious what the pros and cons would be of each approach, so let's opt to keep our file formats simple and write the metadata to a separate file.

```
.data/
  <hash1>.json
  <hash1>.dat
  <hash2>.json
  <hash2>.dat
```

With this layout, when our database starts it can scan the `.data` directory for `.json` files and add pointers to the corresponding `.dat` file to the relevant index keys.

### Simplifying metadata

We ambitiously started with `serde_json::Value` as the type for metadata values.
Whilst it would be nice to support this, for now let's simplify this to just `String` values.
This wouldn't prevent us from storing JSON strings, but correctly suggests we won't be able to query within documents.

This is a fairly simple change across the codebase:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -13,5 +13,5 @@ use std::path::PathBuf;
 pub struct LogEntry {
     pub path: PathBuf,
     pub line: String,
-    pub metadata: HashMap<String, serde_json::Value>,
+    pub metadata: HashMap<String, String>,
 }
```

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -255,7 +255,7 @@ mod tests {
             .write(&LogEntry {
                 path: "foo".into(),
                 line: "line2".into(),
-                metadata: vec![("hello".to_string(), serde_json::json!("world"))]
+                metadata: vec![("hello".to_string(), "world".to_string())]
                     .into_iter()
                     .collect(),
             })
@@ -265,7 +265,7 @@ mod tests {
             .write(&LogEntry {
                 path: "foo".into(),
                 line: "line3".into(),
-                metadata: vec![("hello".to_string(), serde_json::json!("foo"))]
+                metadata: vec![("hello".to_string(), "foo".to_string())]
                     .into_iter()
                     .collect(),
             })
```

### Adding `key` to `metadata`

If we're identifying data files by `hash(metadata)`, we no longer need a specific `key` value.
However, we would still like to include the path to the log file in the metadata.

**Note:** this makes sense for our `directory` collector, but perhaps not for our `kubernetes` collector, which might rather discard the path and instead include Kubernetes identifiers such as container ID.

If we think about how we might document the `directory` collector, we could end up with something like:

> Collect logs from a directory containing log files.
>
> This log collector watches a target directory, and all files in the directory (using `inotify` or `kqueue`, depending on platform).
> Every line written to any file in the directory will be collected as a log entry.
>
> Log entries will be annotated with the `path` to the log file containing the entry.

This reads alright, so let's go ahead and put `path` in the metadata in `directory::Collector`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -92,10 +92,15 @@ impl<W: Watcher> Collector<W> {
             while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
                 if live_file.entry_buf.ends_with('\n') {
                     live_file.entry_buf.pop();
+                    let mut metadata = HashMap::new();
+                    metadata.insert(
+                        "path".to_string(),
+                        live_file.path.to_string_lossy().into_owned(),
+                    );
                     let entry = LogEntry {
                         path: live_file.path.clone(),
                         line: live_file.entry_buf.clone(),
-                        metadata: HashMap::new(),
+                        metadata,
                     };
                     entries.push(entry);

```

We might have expected some tests to fail when we did this, but still only our `test_metadata` test is failing.

```
$ cargo test
...
failures:
    log_database::tests::test_metadata

test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Let's update the tests now to check that `path` is present in `metadata`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -222,10 +222,11 @@ impl<W: Watcher> Iterator for Collector<W> {

 #[cfg(test)]
 mod tests {
-    use std::fs::File;
+    use std::fs::{self, File};
     use std::io::Write;

     use crate::log_collector::watcher::watcher;
+    use crate::LogEntry;

     use super::Collector;

@@ -240,13 +241,10 @@ mod tests {
         file_path.push("test.log");
         File::create(file_path).expect("failed to create temp file");

-        let entries: Vec<String> = collector
+        let entries = collector
             .collect_entries()
-            .expect("failed to collect entries")
-            .into_iter()
-            .map(|entry| entry.line)
-            .collect();
-        assert_eq!(entries, Vec::<String>::new());
+            .expect("failed to collect entries");
+        assert_eq!(entries, Vec::<LogEntry>::new());
     }

     #[test]
@@ -258,7 +256,7 @@ mod tests {

         let mut file_path = tempdir.path().to_path_buf();
         file_path.push("test.log");
-        let mut file = File::create(file_path).expect("failed to create temp file");
+        let mut file = File::create(&file_path).expect("failed to create temp file");

         collector
             .collect_entries()
@@ -267,13 +265,33 @@ mod tests {
         writeln!(file, "hello?").expect("failed to write to file");
         writeln!(file, "world!").expect("failed to write to file");

-        let entries: Vec<String> = collector
+        let entries = collector
             .collect_entries()
-            .expect("failed to collect entries")
-            .into_iter()
-            .map(|entry| entry.line)
-            .collect();
-        assert_eq!(entries, vec!["hello?".to_string(), "world!".to_string()]);
+            .expect("failed to collect entries");
+        let expected_path = fs::canonicalize(file_path).unwrap();
+        let expected_entries = vec![
+            LogEntry {
+                path: expected_path.clone(),
+                line: "hello?".to_string(),
+                metadata: vec![(
+                    "path".to_string(),
+                    expected_path.to_string_lossy().into_owned(),
+                )]
+                .into_iter()
+                .collect(),
+            },
+            LogEntry {
+                path: expected_path.clone(),
+                line: "world!".to_string(),
+                metadata: vec![(
+                    "path".to_string(),
+                    expected_path.to_string_lossy().into_owned(),
+                )]
+                .into_iter()
+                .collect(),
+            },
+        ];
+        assert_eq!(entries, expected_entries);
     }

     #[test]
```

We also need to derive `PartialEq` on `LogEntry` in order to use them in `assert_eq!`:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -9,7 +9,7 @@ pub mod log_database;
 use std::collections::HashMap;
 use std::path::PathBuf;

-#[derive(Debug)]
+#[derive(Debug, PartialEq)]
 pub struct LogEntry {
     pub path: PathBuf,
     pub line: String,
```

### Removing `key`

Let's start by removing `path` from `LogEntry`:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7,11 +7,9 @@ pub mod log_collector;
 pub mod log_database;

 use std::collections::HashMap;
-use std::path::PathBuf;

 #[derive(Debug, PartialEq)]
 pub struct LogEntry {
-    pub path: PathBuf,
     pub line: String,
     pub metadata: HashMap<String, String>,
 }
```

We can now play a few rounds of compiler error whac-a-mole until we're just left with the calculation of `key_hash`:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -53,14 +53,12 @@ mod tests {
         let (mut database, _tempdir) = open_temp_database();
         database
             .write(&LogEntry {
-                path: "/foo".into(),
                 line: "hello".into(),
                 metadata: HashMap::new(),
             })
             .unwrap();
         database
             .write(&LogEntry {
-                path: "/foo".into(),
                 line: "world".into(),
                 metadata: HashMap::new(),
             })
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -98,7 +98,6 @@ impl<W: Watcher> Collector<W> {
                         live_file.path.to_string_lossy().into_owned(),
                     );
                     let entry = LogEntry {
-                        path: live_file.path.clone(),
                         line: live_file.entry_buf.clone(),
                         metadata,
                     };
@@ -268,27 +267,22 @@ mod tests {
         let entries = collector
             .collect_entries()
             .expect("failed to collect entries");
-        let expected_path = fs::canonicalize(file_path).unwrap();
+        let expected_path = fs::canonicalize(file_path)
+            .unwrap()
+            .to_string_lossy()
+            .into_owned();
         let expected_entries = vec![
             LogEntry {
-                path: expected_path.clone(),
                 line: "hello?".to_string(),
-                metadata: vec![(
-                    "path".to_string(),
-                    expected_path.to_string_lossy().into_owned(),
-                )]
-                .into_iter()
-                .collect(),
+                metadata: vec![("path".to_string(), expected_path.clone())]
+                    .into_iter()
+                    .collect(),
             },
             LogEntry {
-                path: expected_path.clone(),
                 line: "world!".to_string(),
-                metadata: vec![(
-                    "path".to_string(),
-                    expected_path.to_string_lossy().into_owned(),
-                )]
-                .into_iter()
-                .collect(),
+                metadata: vec![("path".to_string(), expected_path)]
+                    .into_iter()
+                    .collect(),
             },
         ];
         assert_eq!(entries, expected_entries);
```

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -186,7 +186,6 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line1".into(),
                 metadata: HashMap::new(),
             })
@@ -198,7 +197,6 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line2".into(),
                 metadata: HashMap::new(),
             })
@@ -216,14 +214,12 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line1".into(),
                 metadata: HashMap::new(),
             })
             .expect("failed to write to database");
         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line2".into(),
                 metadata: HashMap::new(),
             })
@@ -245,7 +241,6 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line1".into(),
                 metadata: HashMap::new(),
             })
@@ -253,7 +248,6 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line2".into(),
                 metadata: vec![("hello".to_string(), "world".to_string())]
                     .into_iter()
@@ -263,7 +257,6 @@ mod tests {

         database
             .write(&LogEntry {
-                path: "foo".into(),
                 line: "line3".into(),
                 metadata: vec![("hello".to_string(), "foo".to_string())]
                     .into_iter()
```

```
$ cargo check
...
error[E0609]: no field `path` on type `&LogEntry`
   --> src/log_database/mod.rs:109:42
    |
109 |         let key_hash = Self::hash(&entry.path.to_string_lossy());
    |                                          ^^^^ unknown field
    |
    = note: available fields are: `line`, `metadata`
...
```

Now let's update `Database::hash` to take metadata instead of `&str`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -106,7 +106,7 @@ impl Database {
             unimplemented!("Database::write with non-empty metadata is not implemented")
         }

-        let key_hash = Self::hash(&entry.path.to_string_lossy());
+        let key_hash = Self::hash(&entry.metadata);
         let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
             Some(file) => (file, true),
             None => {
@@ -137,8 +137,8 @@ impl Database {
         Ok(())
     }

-    fn hash(key: &str) -> String {
-        let digest = md5::compute(&key);
+    fn hash(metadata: &HashMap<String, String>) -> String {
+        let digest = md5::compute(&metadata);
         format!("{:x}", digest)
     }

```

Now we need to work out how to compute the md5 of a `HashMap`:

```
$ cargo check
...
error[E0277]: the trait bound `std::collections::HashMap<std::string::String, std::string::String>: std::convert::AsRef<[u8]>` is not satisfied
   --> src/log_database/mod.rs:141:35
    |
141 |         let digest = md5::compute(&metadata);
    |                                   ^^^^^^^^^ the trait `std::convert::AsRef<[u8]>` is not implemented for `std::collections::HashMap<std::string::String, std::string::String>`
    |
   ::: /Users/chris/.cargo/registry/src/github.com-1ecc6299db9ec823/md5-0.7.0/src/lib.rs:189:19
    |
189 | pub fn compute<T: AsRef<[u8]>>(data: T) -> Digest {
    |                   ----------- required by this bound in `md5::compute`
    |
    = note: required because of the requirements on the impl of `std::convert::AsRef<[u8]>` for `&std::collections::HashMap<std::string::String, std::string::String>`
    = note: required because of the requirements on the impl of `std::convert::AsRef<[u8]>` for `&&std::collections::HashMap<std::string::String, std::string::String>`
...
```

(There's also an error coming from `Database::read`, but we'll deal with that later.)

We might hope to use the `md5::Context` interface to incrementally hash the contents of the `HashMap`, but this won't work because `HashMap` has no guarantees about iteration order.
There are three ways we could deal with this:

- Use an ordered data structure for `metadata`, such as `BTreeMap`.
- Sort the entries in `metadata` before adding them to `md5::Context`.
- Compute an `md5::Digest` for each entry in the map, and combine them with an order-invariant operator such as (wrapping) addition or exclusive-or.

It's probable that if we want to add more operators than `key=value` in future, we would want a `BTreeMap` at some level of our index.
For now, though, let's combine `(key, value)` hashes using XOR, because it feels computer-sciency:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -138,8 +138,18 @@ impl Database {
     }

     fn hash(metadata: &HashMap<String, String>) -> String {
-        let digest = md5::compute(&metadata);
-        format!("{:x}", digest)
+        let mut digest = [0u8; 16];
+        for (key, value) in metadata.iter() {
+            let mut context = md5::Context::new();
+            context.consume(key);
+            context.consume(value);
+            let entry_digest = context.compute();
+
+            for (digest_byte, entry_byte) in digest.iter_mut().zip(entry_digest.iter()) {
+                *digest_byte ^= entry_byte;
+            }
+        }
+        format!("{:x}", md5::Digest(digest))
     }

     fn error(message: String) -> io::Error {
```

Let's also tidy up `Database::write` a little bit:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -102,16 +102,12 @@ impl Database {
     }

     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
-        if !entry.metadata.is_empty() {
-            unimplemented!("Database::write with non-empty metadata is not implemented")
-        }
-
-        let key_hash = Self::hash(&entry.metadata);
-        let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
+        let key = Self::hash(&entry.metadata);
+        let (file, needs_delimeter) = match self.files.get_mut(&key) {
             Some(file) => (file, true),
             None => {
                 let mut path = self.data_directory.clone();
-                path.push(&key_hash);
+                path.push(&key);
                 path.set_extension(DATA_FILE_EXTENSION);

                 let file = OpenOptions::new()
@@ -123,7 +119,7 @@ impl Database {
                 // Using `.or_insert` here is annoying since we know there is no entry, but
                 // `hash_map::entry::insert` is unstable
                 // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
-                let file = self.files.entry(key_hash).or_insert(file);
+                let file = self.files.entry(key).or_insert(file);

                 (file, false)
             }
```

This leaves us with one remaining compiler error:

```
$ cargo check
...
error[E0308]: mismatched types
  --> src/log_database/mod.rs:70:57
   |
70 |         let mut file = match self.files.get(&Self::hash(key)) {
   |                                                         ^^^ expected struct `std::collections::HashMap`, found `str`
   |
   = note: expected reference `&std::collections::HashMap<std::string::String, std::string::String>`
              found reference `&str`
...
```

There are two ways we could solve the immediate compiler error:

- Remove the `Database::read` function, since the `key` is no longer from the `Database` API.
- Treat the given `key` as the hashed value.

The first option is where we want to end up, but if we do it now we'll have to update a bunch of tests.
So for now we're going to simply remove the call:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -67,7 +67,7 @@ impl Database {
     }

     pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
-        let mut file = match self.files.get(&Self::hash(key)) {
+        let mut file = match self.files.get(key) {
             Some(file) => file,
             None => return Ok(None),
         };
```

Now everything compiles (although we have a lot of failing tests):

```
failures:

---- api::tests::read_logs_existing_key stdout ----
thread 'api::tests::read_logs_existing_key' panicked at 'assertion failed: `(left == right)`
  left: `NotFound`,
 right: `200`', src/api/mod.rs:71:9
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- log_database::tests::test_existing_db stdout ----
thread 'log_database::tests::test_existing_db' panicked at 'assertion failed: `(left == right)`
  left: `None`,
 right: `Some(["line1", "line2"])`', src/log_database/mod.rs:242:9

---- log_database::tests::test_metadata stdout ----
thread 'log_database::tests::test_metadata' panicked at 'not implemented: Database::write with non-empty metadata is not implemented', src/log_database/mod.rs:106:13

---- log_database::tests::test_new_db stdout ----
thread 'log_database::tests::test_new_db' panicked at 'assertion failed: `(left == right)`
  left: `None`,
 right: `Some(["line1"])`', src/log_database/mod.rs:203:9


failures:
    api::tests::read_logs_existing_key
    log_database::tests::test_existing_db
    log_database::tests::test_metadata
    log_database::tests::test_new_db

test result: FAILED. 6 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

### The index structure

Since we're only interested in `key=value` queries for the moment, we'll start with a `HashMap` for the outer structure.
For the index keys, we can keep it simple and use `(String, String)` tuples, but in future we may want to pre-hash the keys and/or values when storing in order to bound the storage requirements by the size of the index, rather than the size of the keys/values.
For the index values, since we will only ever grow the list of `.dat` files (for now) we can just use a `Vec`.

Let's add an `index` field to `Database`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -17,6 +17,7 @@ pub struct Config {
 pub struct Database {
     data_directory: PathBuf,
     files: HashMap<String, File>,
+    index: HashMap<(String, String), Vec<String>>,
 }

 impl Database {
@@ -63,6 +64,7 @@ impl Database {
         Ok(Database {
             data_directory: config.data_directory,
             files,
+            index: HashMap::new(),
         })
     }

```

Now, in `Database::write` we want to insert an entry into the index for each `(key, value)`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -105,6 +105,15 @@ impl Database {

     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
         let key = Self::hash(&entry.metadata);
+
+        for (key, value) in entry.metadata.iter() {
+            let keys = self
+                .index
+                .entry((key.to_string(), value.to_string()))
+                .or_insert_with(|| Vec::with_capacity(1));
+            keys.push(key.clone());
+        }
+
         let (file, needs_delimeter) = match self.files.get_mut(&key) {
             Some(file) => (file, true),
             None => {
```

We're not protected from pushing duplicate entries into the `Vec`, and it would not be efficient to scan the `Vec` every time.
Since the ordering is not important, we can just swap from `Vec` to `HashSet`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -1,5 +1,5 @@
 // src/log_database/mod.rs
-use std::collections::HashMap;
+use std::collections::{HashMap, HashSet};
 use std::ffi::OsStr;
 use std::fs::{self, File, OpenOptions};
 use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
@@ -17,7 +17,7 @@ pub struct Config {
 pub struct Database {
     data_directory: PathBuf,
     files: HashMap<String, File>,
-    index: HashMap<(String, String), Vec<String>>,
+    index: HashMap<(String, String), HashSet<String>>,
 }

 impl Database {
@@ -110,8 +110,13 @@ impl Database {
             let keys = self
                 .index
                 .entry((key.to_string(), value.to_string()))
-                .or_insert_with(|| Vec::with_capacity(1));
-            keys.push(key.clone());
+                .or_insert_with(|| HashSet::with_capacity(1));
+
+            // We'd ideally use `HashSet::get_or_insert_owned`, but it's currently unstable
+            // ([#60896](https://github.com/rust-lang/rust/issues/60896)).
+            if !keys.contains(key) {
+                keys.insert(key.clone());
+            }
         }

         let (file, needs_delimeter) = match self.files.get_mut(&key) {
```

### Implemeting `query`

We should now have all the pieces we need to implement `query` by:

- Reading the `(key, value)` entry from `index`.
- Iterating through the returned keys (if any).
- Merging the records from each `.dat` file.

We should ideally be merging in timestamp order, but we're not currently storing timestamps, so we will just concatenate them:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -100,7 +100,19 @@ impl Database {
     }

     pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
-        todo!()
+        let keys = match self.index.get(&(key.to_string(), value.to_string())) {
+            None => return Ok(None),
+            Some(keys) => keys,
+        };
+
+        let mut lines = Vec::new();
+        for key in keys {
+            if let Some(lines_) = self.read(key)? {
+                lines.extend(lines_);
+            }
+        }
+
+        Ok(Some(lines))
     }

     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
```

Sadly, our metadata test is still failing:

```
$ cargo test
...
---- log_database::tests::test_metadata stdout ----
thread 'log_database::tests::test_metadata' panicked at 'assertion failed: `(left == right)`
  left: `Some([])`,
 right: `Some(["line2"])`', src/log_database/mod.rs:301:9
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
...
```

Let's toss in some `dbg!` and see what's happening:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -100,7 +100,7 @@ impl Database {
     }

     pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
-        let keys = match self.index.get(&(key.to_string(), value.to_string())) {
+        let keys = match dbg!(dbg!(&self.index).get(&(key.to_string(), value.to_string()))) {
             None => return Ok(None),
             Some(keys) => keys,
         };
@@ -108,7 +108,7 @@ impl Database {
         let mut lines = Vec::new();
         for key in keys {
             if let Some(lines_) = self.read(key)? {
-                lines.extend(lines_);
+                lines.extend(dbg!(lines_));
             }
         }

```

We can run our tests with `--nocapture` to see what those values are:

```
$ cargo test log_database::tests::test_metadata -- --nocapture
...
[src/log_database/mod.rs:103] &self.index = {
    (
        "hello",
        "foo",
    ): {
        "hello",
    },
    (
        "hello",
        "world",
    ): {
        "hello",
    },
}
[src/log_database/mod.rs:103] dbg!(& self . index).get(&(key.to_string(), value.to_string())) = Some(
    {
        "hello",
    },
)
...
```

So the entries in our index are a bit suspicious, and if we review our implementation we'll see why:

```rust
let key = Self::hash(&entry.metadata);

for (key, value) in entry.metadata.iter() {
    ...
    if !keys.contains(key) {
        keys.insert(key.clone());
    }
}
```

We have shadowed `key` in our loop, which has got us in a bit of a pickle.
In general, Rust's shadowing can be quite useful in reducing the need to come up with distinct names when variables transition through different types etc.
However, in this case we have pranked ourselves.

Let's navigate ourselves out of this as cheaply as possible:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -100,7 +100,7 @@ impl Database {
     }

     pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
-        let keys = match dbg!(dbg!(&self.index).get(&(key.to_string(), value.to_string()))) {
+        let keys = match self.index.get(&(key.to_string(), value.to_string())) {
             None => return Ok(None),
             Some(keys) => keys,
         };
@@ -108,7 +108,7 @@ impl Database {
         let mut lines = Vec::new();
         for key in keys {
             if let Some(lines_) = self.read(key)? {
-                lines.extend(dbg!(lines_));
+                lines.extend(lines_);
             }
         }

@@ -118,15 +118,15 @@ impl Database {
     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
         let key = Self::hash(&entry.metadata);

-        for (key, value) in entry.metadata.iter() {
+        for meta in entry.metadata.iter() {
             let keys = self
                 .index
-                .entry((key.to_string(), value.to_string()))
+                .entry((meta.0.to_string(), meta.1.to_string()))
                 .or_insert_with(|| HashSet::with_capacity(1));

             // We'd ideally use `HashSet::get_or_insert_owned`, but it's currently unstable
             // ([#60896](https://github.com/rust-lang/rust/issues/60896)).
-            if !keys.contains(key) {
+            if !keys.contains(&key) {
                 keys.insert(key.clone());
             }
         }
```

And now our metadata test passes:

```
$ cargo test log_database::tests::test_metadata -- --nocapture
...
test log_database::tests::test_metadata ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out
...
```

### Making `read` private

Let's remove `read` from the `Database`'s public API:

```diff
diff --git a/src/log_database/mod.rs b/src/log_database/mod.rs
index d6c8d60..c313a99 100644
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -68,37 +68,6 @@ impl Database {
         })
     }

-    pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
-        let mut file = match self.files.get(key) {
-            Some(file) => file,
-            None => return Ok(None),
-        };
-
-        file.seek(SeekFrom::Start(0))?;
-        let mut reader = BufReader::new(file);
-        let mut lines = Vec::new();
-
-        loop {
-            let mut line_bytes = Vec::new();
-            let bytes_read = reader.read_until(DATA_FILE_RECORD_SEPARATOR, &mut line_bytes)?;
-            if bytes_read == 0 {
-                break;
-            }
-            if line_bytes.last() == Some(&DATA_FILE_RECORD_SEPARATOR) {
-                line_bytes.pop();
-            }
-            let line = String::from_utf8(line_bytes).map_err(|error| {
-                Self::error(format!(
-                    "corrupt data file for key {}: invalid utf8: {}",
-                    key, error
-                ))
-            })?;
-            lines.push(line);
-        }
-
-        Ok(Some(lines))
-    }
-
     pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
         let keys = match self.index.get(&(key.to_string(), value.to_string())) {
             None => return Ok(None),
@@ -161,6 +130,37 @@ impl Database {
         Ok(())
     }

+    fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
+        let mut file = match self.files.get(key) {
+            Some(file) => file,
+            None => return Ok(None),
+        };
+
+        file.seek(SeekFrom::Start(0))?;
+        let mut reader = BufReader::new(file);
+        let mut lines = Vec::new();
+
+        loop {
+            let mut line_bytes = Vec::new();
+            let bytes_read = reader.read_until(DATA_FILE_RECORD_SEPARATOR, &mut line_bytes)?;
+            if bytes_read == 0 {
+                break;
+            }
+            if line_bytes.last() == Some(&DATA_FILE_RECORD_SEPARATOR) {
+                line_bytes.pop();
+            }
+            let line = String::from_utf8(line_bytes).map_err(|error| {
+                Self::error(format!(
+                    "corrupt data file for key {}: invalid utf8: {}",
+                    key, error
+                ))
+            })?;
+            lines.push(line);
+        }
+
+        Ok(Some(lines))
+    }
+
     fn hash(metadata: &HashMap<String, String>) -> String {
         let mut digest = [0u8; 16];
         for (key, value) in metadata.iter() {
```

(We've also moved the `read` function below the `pub` methods, since we're such pedants.)

Now we have a compilation error from `api::read_logs`:

```
$ cargo check
...
error[E0624]: associated function `read` is private
  --> src/api/mod.rs:22:23
   |
22 |     Ok(match database.read(key)? {
   |                       ^^^^ private associated function
...
```

### Updating the API

To fix our compiler error, we're going to have to update our API.
For now, to avoid going into parsing etc. we will support an API like:

```
GET /logs/:key/:value
```

Square go:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -11,15 +11,16 @@ pub type Server = tide::Server<State>;

 pub fn server(database: State) -> Server {
     let mut app = tide::Server::with_state(database);
-    app.at("/logs/*key").get(read_logs);
+    app.at("/logs/:key/*value").get(read_logs);
     app
 }

 async fn read_logs(req: tide::Request<State>) -> tide::Result {
     let key = req.param("key")?;
+    let value = req.param("value")?;
     let database = req.state().read().await;

-    Ok(match database.read(key)? {
+    Ok(match database.query(key, value)? {
         Some(logs) => tide::Response::builder(tide::StatusCode::Ok)
             .body(tide::Body::from_json(&logs)?)
             .build(),
@@ -29,10 +30,10 @@ async fn read_logs(req: tide::Request<State>) -> tide::Result {

 #[cfg(test)]
 mod tests {
-    use async_std::sync::RwLock;
     use std::collections::HashMap;
     use std::sync::Arc;

+    use async_std::sync::RwLock;
     use tide_testing::TideTestingExt;

     use crate::log_database::test::open_temp_database;
@@ -43,7 +44,7 @@ mod tests {
         let (database, _tempdir) = open_temp_database();
         let api = super::server(Arc::new(RwLock::new(database)));

-        let response = api.get("/logs//foo").await.unwrap();
+        let response = api.get("/logs/foo/bar").await.unwrap();

         assert_eq!(response.status(), 404);
     }
@@ -51,22 +52,25 @@ mod tests {
     #[async_std::test]
     async fn read_logs_existing_key() {
         let (mut database, _tempdir) = open_temp_database();
+        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
+            .into_iter()
+            .collect();
         database
             .write(&LogEntry {
                 line: "hello".into(),
-                metadata: HashMap::new(),
+                metadata: metadata.clone(),
             })
             .unwrap();
         database
             .write(&LogEntry {
                 line: "world".into(),
-                metadata: HashMap::new(),
+                metadata,
             })
             .unwrap();

         let api = super::server(Arc::new(RwLock::new(database)));

-        let mut response = api.get("/logs//foo").await.unwrap();
+        let mut response = api.get("/logs/foo/bar").await.unwrap();

         assert_eq!(response.status(), 200);
         assert_eq!(
```

Now our api tests pass again:

```
$ cargo test api
...
test api::tests::read_logs_non_existent_key ... ok
test api::tests::read_logs_existing_key ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 8 filtered out
...
```

### Fixing `log_database` tests

We're left with two test failures in `log_database`.
These tests still compile because the `read` method is accessible from the `tests` submodule.
Let's update them to use `query`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -212,31 +212,40 @@ mod tests {
     #[test]
     fn test_new_db() {
         let (mut database, _tempdir) = open_temp_database();
+        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
+            .into_iter()
+            .collect();

         assert_eq!(
-            database.read("foo").expect("unable to read from database"),
+            database
+                .query("foo", "bar")
+                .expect("unable to read from database"),
             None
         );

         database
             .write(&LogEntry {
                 line: "line1".into(),
-                metadata: HashMap::new(),
+                metadata: metadata.clone(),
             })
             .expect("unable to write to database");
         assert_eq!(
-            database.read("foo").expect("unable to read from database"),
+            database
+                .query("foo", "bar")
+                .expect("unable to read from database"),
             Some(vec!["line1".to_string()])
         );

         database
             .write(&LogEntry {
                 line: "line2".into(),
-                metadata: HashMap::new(),
+                metadata,
             })
             .expect("unable to write to database");
         assert_eq!(
-            database.read("foo").expect("unable to read from database"),
+            database
+                .query("foo", "bar")
+                .expect("unable to read from database"),
             Some(vec!["line1".to_string(), "line2".to_string()])
         );
     }
@@ -245,17 +254,20 @@ mod tests {
     fn test_existing_db() {
         let (mut database, _tempdir) = open_temp_database();
         let data_directory = database.data_directory.clone();
+        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
+            .into_iter()
+            .collect();

         database
             .write(&LogEntry {
                 line: "line1".into(),
-                metadata: HashMap::new(),
+                metadata: metadata.clone(),
             })
             .expect("failed to write to database");
         database
             .write(&LogEntry {
                 line: "line2".into(),
-                metadata: HashMap::new(),
+                metadata,
             })
             .expect("failed to write to database");
         drop(database);
@@ -264,7 +276,9 @@ mod tests {
         let database = Database::open(config).expect("unable to open database");

         assert_eq!(
-            database.read("foo").expect("unable to read from database"),
+            database
+                .query("foo", "bar")
+                .expect("unable to read from database"),
             Some(vec!["line1".to_string(), "line2".to_string()])
         );
     }
```

Now we're down to a single failing test: `test_existing_db`.
This makes sense, since we've yet to update `Database::open` to rebuild `index`.
To do this, we first need to persist the metadata.
Since we still have `serde_json`, we'll serialize to JSON for now':

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -8,6 +8,7 @@ use std::path::PathBuf;
 use crate::LogEntry;

 const DATA_FILE_EXTENSION: &str = "dat";
+const METADATA_FILE_EXTENSION: &str = "json";
 const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

 pub struct Config {
@@ -103,15 +104,21 @@ impl Database {
         let (file, needs_delimeter) = match self.files.get_mut(&key) {
             Some(file) => (file, true),
             None => {
-                let mut path = self.data_directory.clone();
-                path.push(&key);
-                path.set_extension(DATA_FILE_EXTENSION);
+                let mut entry_path = self.data_directory.clone();
+                entry_path.push(&key);
+
+                let mut metadata_path = entry_path;
+                metadata_path.set_extension(METADATA_FILE_EXTENSION);
+                fs::write(&metadata_path, serde_json::to_vec(&entry.metadata)?)?;
+
+                let mut data_path = metadata_path;
+                data_path.set_extension(DATA_FILE_EXTENSION);

                 let file = OpenOptions::new()
                     .append(true)
                     .create(true)
                     .read(true)
-                    .open(&path)?;
+                    .open(&data_path)?;

                 // Using `.or_insert` here is annoying since we know there is no entry, but
                 // `hash_map::entry::insert` is unstable
```

Now if we run our tests we'll see our limited 'integrity checking' causing a failure:

```
$ cargo test
...
failures:

---- log_database::tests::test_existing_db stdout ----
thread 'log_database::tests::test_existing_db' panicked at 'unable to open database: Custom { kind: Other, error: "invalid data file /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpa9t7mk/3858f62230ac3c915f300c664312c63f.json: extension must be `dat`" }', src/log_database/mod.rs:283:47
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    log_database::tests::test_existing_db

test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
...
```

Now let's update `Database::open` to accept those files:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -15,6 +15,11 @@ pub struct Config {
     pub data_directory: PathBuf,
 }

+enum FileType {
+    DataFile,
+    MetadataFile,
+}
+
 pub struct Database {
     data_directory: PathBuf,
     files: HashMap<String, File>,
@@ -24,17 +29,24 @@ pub struct Database {
 impl Database {
     pub fn open(config: Config) -> io::Result<Self> {
         let mut files = HashMap::new();
+        let mut index = HashMap::new();
         for entry in fs::read_dir(&config.data_directory)? {
             let entry = entry?;
             let path = entry.path();

-            if path.extension().and_then(OsStr::to_str) != Some(DATA_FILE_EXTENSION) {
-                return Err(Self::error(format!(
-                    "invalid data file {}: extension must be `{}`",
-                    path.display(),
-                    DATA_FILE_EXTENSION
-                )));
-            }
+            let extension = path.extension().and_then(OsStr::to_str);
+            let file_type = match extension {
+                Some(DATA_FILE_EXTENSION) => FileType::DataFile,
+                Some(METADATA_FILE_EXTENSION) => FileType::MetadataFile,
+                _ => {
+                    return Err(Self::error(format!(
+                        "invalid data file {}: extension must be `{}` or `{}`",
+                        path.display(),
+                        DATA_FILE_EXTENSION,
+                        METADATA_FILE_EXTENSION
+                    )))
+                }
+            };

             let metadata = fs::metadata(&path)?;
             if !metadata.is_file() {
@@ -59,13 +71,30 @@ impl Database {
             })?;

             let file = OpenOptions::new().append(true).read(true).open(&path)?;
-
-            files.insert(key_hash.to_string(), file);
+            match file_type {
+                FileType::DataFile => {
+                    files.insert(key_hash.to_string(), file);
+                }
+                FileType::MetadataFile => {
+                    let metadata = serde_json::from_reader(file)?;
+                    let key = Self::hash(&metadata);
+
+                    for meta in metadata.into_iter() {
+                        let keys = index
+                            .entry((meta.0.to_string(), meta.1.to_string()))
+                            .or_insert_with(|| HashSet::with_capacity(1));
+
+                        if !keys.contains(&key) {
+                            keys.insert(key.clone());
+                        }
+                    }
+                }
+            }
         }
         Ok(Database {
             data_directory: config.data_directory,
             files,
-            index: HashMap::new(),
+            index,
         })
     }

```

Our code is crying out for some significant refactoring, but we've got our tests passing again:

```
$ cargo test
...
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running target/debug/deps/monitoring_rs-af4ec7a3b47a9fbf
...
```

## Wrapping up

We've added the capability for log collectors to attach key-value metadata to log entries.
We store this metadata in our database, and allow `key=value` retrieval via the API.

We took a pretty bullish approach to get here, so the next installment will probably feature some refactoring.
