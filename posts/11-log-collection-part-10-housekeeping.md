# Log collection (part 10 – housekeeping)

Let's spend a bit of time tidying up our code and adding some tests before pushing on with more features.
This post will consist mostly of diffs, and commentary on why the changes were made.
It would probably be better as a `git log`, but hey-oh, here we go.

## Simplifying tests

Our tests are all quite noisy just now, mostly due to:

- Lots of `.expect`s to raise meaningful panics from our many `Result`-returning functions.
- Complicates construction of `LogEntry` (especially `metadata`).

We'll attempt to clean this up by introducing some helper functions in a conditionally-compiled `test` module.

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -6,6 +6,9 @@ pub mod api;
 pub mod log_collector;
 pub mod log_database;

+#[cfg(test)]
+pub mod test;
+
 use std::collections::HashMap;

 #[derive(Debug, PartialEq)]
```

```rust
// src/test.rs
use std::io;

use tempfile::TempDir;

use crate::log_database::{self, Database};
use crate::LogEntry;

/// A convenient alias to use `?` in tests.
///
/// There is a blanket `impl From<E: Error> for Box<dyn Error>`, meaning anything that implements
/// [`std::error::Error`] can be propagated using `?`.
pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;

/// Open a database in a temporary directory.
///
/// This returns the handle to the temporary directory as well as the database, since the directory
/// will be unlinked when the `TempDir` value is dropped.
pub fn temp_database() -> io::Result<(TempDir, Database)> {
    let tempdir = tempfile::tempdir()?;
    let config = log_database::Config {
        data_directory: tempdir.path().to_path_buf(),
    };
    Ok((tempdir, Database::open(config)?))
}

/// Construct a `LogEntry` with the given `line` and `metadata`.
///
/// This is a convenience function to avoid having to build a `HashMap` for metadata.
pub fn log_entry(line: &str, metadata: &[(&str, &str)]) -> LogEntry {
    LogEntry {
        line: line.to_string(),
        metadata: metadata
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}
```

See the comments in the file for a description of each item.

We can then use these helpers throughout our tests to simplify them:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -30,52 +30,42 @@ async fn read_logs(req: tide::Request<State>) -> tide::Result {

 #[cfg(test)]
 mod tests {
-    use std::collections::HashMap;
     use std::sync::Arc;

     use async_std::sync::RwLock;
     use tide_testing::TideTestingExt;

-    use crate::log_database::test::open_temp_database;
-    use crate::LogEntry;
+    use crate::test::{self, log_entry, temp_database};

     #[async_std::test]
-    async fn read_logs_non_existent_key() {
-        let (database, _tempdir) = open_temp_database();
+    async fn read_logs_non_existent_key() -> test::Result {
+        let (_tempdir, database) = temp_database()?;
         let api = super::server(Arc::new(RwLock::new(database)));

-        let response = api.get("/logs/foo/bar").await.unwrap();
+        let response = api.get("/logs/foo/bar").await?;

         assert_eq!(response.status(), 404);
+
+        Ok(())
     }

     #[async_std::test]
-    async fn read_logs_existing_key() {
-        let (mut database, _tempdir) = open_temp_database();
-        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
-            .into_iter()
-            .collect();
-        database
-            .write(&LogEntry {
-                line: "hello".into(),
-                metadata: metadata.clone(),
-            })
-            .unwrap();
-        database
-            .write(&LogEntry {
-                line: "world".into(),
-                metadata,
-            })
-            .unwrap();
+    async fn read_logs_existing_key() -> test::Result {
+        let (_tempdir, mut database) = temp_database()?;
+
+        database.write(&log_entry("hello", &[("foo", "bar")]))?;
+        database.write(&log_entry("world", &[("foo", "bar")]))?;

         let api = super::server(Arc::new(RwLock::new(database)));

-        let mut response = api.get("/logs/foo/bar").await.unwrap();
+        let mut response = api.get("/logs/foo/bar").await?;

         assert_eq!(response.status(), 200);
         assert_eq!(
-            response.body_json::<Vec<String>>().await.unwrap(),
+            response.body_json::<Vec<String>>().await?,
             vec!["hello".to_string(), "world".to_string()]
         );
+
+        Ok(())
     }
 }
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -222,100 +222,85 @@ impl<W: Watcher> Iterator for Collector<W> {
 #[cfg(test)]
 mod tests {
     use std::fs::{self, File};
-    use std::io::Write;
+    use std::io::{self, Write};
+    use std::path::PathBuf;
+
+    use tempfile::TempDir;

     use crate::log_collector::watcher::watcher;
-    use crate::LogEntry;
+    use crate::test::{self, log_entry};

     use super::Collector;

     #[test]
-    fn collect_entries_empty_file() {
-        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-        let mut collector =
-            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
-                .expect("unable to initialize collector");
-
-        let mut file_path = tempdir.path().to_path_buf();
-        file_path.push("test.log");
-        File::create(file_path).expect("failed to create temp file");
-
-        let entries = collector
-            .collect_entries()
-            .expect("failed to collect entries");
-        assert_eq!(entries, Vec::<LogEntry>::new());
+    fn collect_entries_empty_file() -> test::Result {
+        let tempdir = tempfile::tempdir()?;
+        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+
+        create_log_file(&tempdir)?;
+
+        // A new file will trigger an event but return no entries.
+        let entries = collector.collect_entries()?;
+        assert_eq!(entries, vec![]);
+
+        Ok(())
     }

     #[test]
-    fn collect_entries_nonempty_file() {
-        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-        let mut collector =
-            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
-                .expect("unable to initialize collector");
-
-        let mut file_path = tempdir.path().to_path_buf();
-        file_path.push("test.log");
-        let mut file = File::create(&file_path).expect("failed to create temp file");
-
-        collector
-            .collect_entries()
-            .expect("failed to collect entries");
-
-        writeln!(file, "hello?").expect("failed to write to file");
-        writeln!(file, "world!").expect("failed to write to file");
-
-        let entries = collector
-            .collect_entries()
-            .expect("failed to collect entries");
-        let expected_path = fs::canonicalize(file_path)
-            .unwrap()
-            .to_string_lossy()
-            .into_owned();
-        let expected_entries = vec![
-            LogEntry {
-                line: "hello?".to_string(),
-                metadata: vec![("path".to_string(), expected_path.clone())]
-                    .into_iter()
-                    .collect(),
-            },
-            LogEntry {
-                line: "world!".to_string(),
-                metadata: vec![("path".to_string(), expected_path)]
-                    .into_iter()
-                    .collect(),
-            },
-        ];
-        assert_eq!(entries, expected_entries);
+    fn collect_entries_nonempty_file() -> test::Result {
+        let tempdir = tempfile::tempdir()?;
+        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+
+        let (file_path, mut file) = create_log_file(&tempdir)?;
+
+        collector.collect_entries()?;
+
+        writeln!(file, "hello?")?;
+        writeln!(file, "world!")?;
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            entries,
+            vec![
+                log_entry("hello?", &[("path", file_path.to_str().unwrap())]),
+                log_entry("world!", &[("path", file_path.to_str().unwrap())]),
+            ]
+        );
+
+        Ok(())
     }

     #[test]
-    fn iterator_yields_entries() {
-        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-        let mut collector =
-            Collector::initialize(&tempdir.path(), watcher().expect("failed to start watcher"))
-                .expect("unable to initialize collector");
-
-        let mut file_path = tempdir.path().to_path_buf();
-        file_path.push("test.log");
-        let mut file = File::create(file_path).expect("failed to create temp file");
-
-        collector
-            .collect_entries()
-            .expect("failed to collect entries");
-
-        writeln!(file, "hello?").expect("failed to write to file");
-        writeln!(file, "world!").expect("failed to write to file");
-
-        let entry = collector
-            .next()
-            .expect("expected at least 1 entry")
-            .expect("failed to collect entries");
-        assert_eq!(entry.line, "hello?".to_string());
-
-        let entry = collector
-            .next()
-            .expect("expected at least 2 entries")
-            .expect("failed to collect entries");
-        assert_eq!(entry.line, "world!".to_string());
+    fn iterator_yields_entries() -> test::Result {
+        let tempdir = tempfile::tempdir()?;
+        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+
+        let (file_path, mut file) = create_log_file(&tempdir)?;
+
+        collector.collect_entries()?;
+
+        writeln!(file, "hello?")?;
+        writeln!(file, "world!")?;
+
+        assert_eq!(
+            collector.next().expect("expected at least 1 entry")?,
+            log_entry("hello?", &[("path", file_path.to_str().unwrap())])
+        );
+
+        assert_eq!(
+            collector.next().expect("expected at least 2 entries")?,
+            log_entry("world!", &[("path", file_path.to_str().unwrap())])
+        );
+
+        Ok(())
+    }
+
+    fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
+        let mut path = fs::canonicalize(tempdir.path())?;
+        path.push("test.log");
+
+        let file = File::create(&path)?;
+
+        Ok((path, file))
     }
 }
```

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -217,142 +217,67 @@ impl Database {
     }
 }

-#[cfg(test)]
-pub mod test {
-    use tempfile::TempDir;
-
-    use super::Config;
-    use super::Database;
-
-    pub fn open_temp_database() -> (Database, TempDir) {
-        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-        let config = Config {
-            data_directory: tempdir.path().to_path_buf(),
-        };
-        (
-            Database::open(config).expect("unable to open database"),
-            tempdir,
-        )
-    }
-}
-
 #[cfg(test)]
 mod tests {
-    use std::collections::HashMap;
+    use crate::test::{self, log_entry, temp_database};

-    use crate::LogEntry;
-
-    use super::test::open_temp_database;
     use super::{Config, Database};

     #[test]
-    fn test_new_db() {
-        let (mut database, _tempdir) = open_temp_database();
-        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
-            .into_iter()
-            .collect();
+    fn test_new_db() -> test::Result {
+        let (_tempdir, mut database) = temp_database()?;

-        assert_eq!(
-            database
-                .query("foo", "bar")
-                .expect("unable to read from database"),
-            None
-        );
+        assert_eq!(database.query("foo", "bar")?, None);

-        database
-            .write(&LogEntry {
-                line: "line1".into(),
-                metadata: metadata.clone(),
-            })
-            .expect("unable to write to database");
+        database.write(&log_entry("line1", &[("foo", "bar")]))?;
         assert_eq!(
-            database
-                .query("foo", "bar")
-                .expect("unable to read from database"),
+            database.query("foo", "bar")?,
             Some(vec!["line1".to_string()])
         );

-        database
-            .write(&LogEntry {
-                line: "line2".into(),
-                metadata,
-            })
-            .expect("unable to write to database");
+        database.write(&log_entry("line2", &[("foo", "bar")]))?;
         assert_eq!(
-            database
-                .query("foo", "bar")
-                .expect("unable to read from database"),
+            database.query("foo", "bar")?,
             Some(vec!["line1".to_string(), "line2".to_string()])
         );
+
+        Ok(())
     }

     #[test]
-    fn test_existing_db() {
-        let (mut database, _tempdir) = open_temp_database();
-        let data_directory = database.data_directory.clone();
-        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
-            .into_iter()
-            .collect();
-
-        database
-            .write(&LogEntry {
-                line: "line1".into(),
-                metadata: metadata.clone(),
-            })
-            .expect("failed to write to database");
-        database
-            .write(&LogEntry {
-                line: "line2".into(),
-                metadata,
-            })
-            .expect("failed to write to database");
+    fn test_existing_db() -> test::Result {
+        let (tempdir, mut database) = temp_database()?;
+
+        database.write(&log_entry("line1", &[("foo", "bar")]))?;
+        database.write(&log_entry("line2", &[("foo", "bar")]))?;
         drop(database);

-        let config = Config { data_directory };
-        let database = Database::open(config).expect("unable to open database");
+        let config = Config {
+            data_directory: tempdir.path().to_path_buf(),
+        };
+        let database = Database::open(config)?;

         assert_eq!(
-            database
-                .query("foo", "bar")
-                .expect("unable to read from database"),
+            database.query("foo", "bar")?,
             Some(vec!["line1".to_string(), "line2".to_string()])
         );
+
+        Ok(())
     }

     #[test]
-    fn test_metadata() {
-        let (mut database, _tempdir) = open_temp_database();
-
-        database
-            .write(&LogEntry {
-                line: "line1".into(),
-                metadata: HashMap::new(),
-            })
-            .expect("failed to write to database");
-
-        database
-            .write(&LogEntry {
-                line: "line2".into(),
-                metadata: vec![("hello".to_string(), "world".to_string())]
-                    .into_iter()
-                    .collect(),
-            })
-            .expect("failed to write to database");
-
-        database
-            .write(&LogEntry {
-                line: "line3".into(),
-                metadata: vec![("hello".to_string(), "foo".to_string())]
-                    .into_iter()
-                    .collect(),
-            })
-            .expect("failed to write to database");
+    fn test_query_metadata() -> test::Result {
+        let (_tempdir, mut database) = temp_database()?;
+
+        database.write(&log_entry("line1", &[]))?;
+        database.write(&log_entry("line2", &[("hello", "world")]))?;
+        database.write(&log_entry("line2", &[("hello", "foo")]))?;

         assert_eq!(
-            database
-                .query("hello", "world")
-                .expect("failed to query database"),
+            database.query("hello", "world")?,
             Some(vec!["line2".to_string()])
         );
+
+        Ok(())
     }
 }
```

## Pedantic lints

The inclusion of a linting framework in the default toolchain is one of the many things that makes Rust so pleasant to work with.
Let's make sure we're ticking all possible boxes by turning on some more lints:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,6 +1,22 @@
 // lib.rs
-#[macro_use]
-extern crate log;
+#![warn(
+    explicit_outlives_requirements,
+    macro_use_extern_crate,
+    meta_variable_misuse,
+    missing_crate_level_docs,
+    missing_doc_code_examples,
+    missing_docs,
+    private_doc_tests,
+    single_use_lifetimes,
+    trivial_casts,
+    trivial_numeric_casts,
+    unreachable_pub,
+    unused_extern_crates,
+    unused_lifetimes,
+    variant_size_differences,
+    clippy::cargo,
+    clippy::pedantic
+)]

 pub mod api;
 pub mod log_collector;
```

We now get **41** warnings across our codebase.
Let's tidy them up:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1,9 +1,16 @@
 [package]
 name = "monitoring-rs"
+description = "An adventure in building a minimal monitoring pipeline, in Rust."
+
 version = "0.1.0"
 authors = ["Chris Connelly <chris@connec.co.uk>"]
+license = "GPL-3.0-only"
 edition = "2018"

+categories = ["command-line-utilities"]
+keywords = ["logging", "metrics", "monitoring"]
+repository = "https://github.com/connec/monitoring-rs"
+
 # See more keys and their definitions at https://doc.rust-lang.org/cargo/reference/manifest.html

 [dependencies]
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -3,6 +3,8 @@ use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};

+use log::{debug, trace, warn};
+
 use crate::LogEntry;

 use super::watcher::{self, watcher, Watcher};
@@ -26,8 +28,7 @@ impl Event<'_> {
     fn path(&self) -> &Path {
         match self {
             Event::Create { path } => path,
-            Event::Append { live_file, .. } => &live_file.path,
-            Event::Truncate { live_file, .. } => &live_file.path,
+            Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => &live_file.path,
         }
     }
 }
@@ -54,6 +55,9 @@ struct Collector<W: Watcher> {
     entry_buf: std::vec::IntoIter<LogEntry>,
 }

+/// # Errors
+///
+/// Propagates any `io::Error`s that occur during initialization.
 pub fn initialize(root_path: &Path) -> io::Result<impl super::Collector> {
     let watcher = watcher()?;
     Collector::initialize(root_path, watcher)
@@ -114,7 +118,7 @@ impl<W: Watcher> Collector<W> {

             let mut new_paths = Vec::new();

-            for event in self.check_event(watcher_event)? {
+            for event in self.check_event(&watcher_event)? {
                 debug!("{}", event);

                 let live_file = match event {
@@ -141,7 +145,7 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event(&mut self, watcher_event: watcher::Event) -> io::Result<Vec<Event>> {
+    fn check_event(&mut self, watcher_event: &watcher::Event) -> io::Result<Vec<Event>> {
         if watcher_event.descriptor == self.root_wd {
             let mut events = Vec::new();

```

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -2,19 +2,19 @@
 use std::io;
 use std::path::Path;

-pub fn watcher() -> io::Result<impl Watcher> {
+pub(crate) fn watcher() -> io::Result<impl Watcher> {
     imp::Watcher::new()
 }

 #[derive(Clone, Debug, Eq, Hash, PartialEq)]
-pub struct Descriptor(imp::Descriptor);
+pub(crate) struct Descriptor(imp::Descriptor);

 #[derive(Debug, Eq, PartialEq)]
-pub struct Event {
-    pub descriptor: Descriptor,
+pub(crate) struct Event {
+    pub(crate) descriptor: Descriptor,
 }

-pub trait Watcher {
+pub(crate) trait Watcher {
     fn new() -> io::Result<Self>
     where
         Self: Sized;
@@ -37,9 +37,9 @@ mod imp {

     const INOTIFY_BUFFER_SIZE: usize = 1024;

-    pub type Descriptor = WatchDescriptor;
+    pub(crate) type Descriptor = WatchDescriptor;

-    pub struct Watcher {
+    pub(crate) struct Watcher {
         inner: Inotify,
         buffer: [u8; INOTIFY_BUFFER_SIZE],
     }
@@ -85,9 +85,9 @@ mod imp {

     use super::Event;

-    pub type Descriptor = RawFd;
+    pub(crate) type Descriptor = RawFd;

-    pub struct Watcher {
+    pub(crate) struct Watcher {
         inner: kqueue::Watcher,
     }

```

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -27,6 +27,9 @@ pub struct Database {
 }

 impl Database {
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` that ocurrs when opening the database.
     pub fn open(config: Config) -> io::Result<Self> {
         let mut files = HashMap::new();
         let mut index = HashMap::new();
@@ -79,7 +82,7 @@ impl Database {
                     let metadata = serde_json::from_reader(file)?;
                     let key = Self::hash(&metadata);

-                    for meta in metadata.into_iter() {
+                    for meta in metadata {
                         let keys = index
                             .entry((meta.0.to_string(), meta.1.to_string()))
                             .or_insert_with(|| HashSet::with_capacity(1));
@@ -98,6 +101,9 @@ impl Database {
         })
     }

+    /// # Errors
+    ///
+    /// Propagates any `io::Error` that occurs when querying the database.
     pub fn query(&self, key: &str, value: &str) -> io::Result<Option<Vec<String>>> {
         let keys = match self.index.get(&(key.to_string(), value.to_string())) {
             None => return Ok(None),
@@ -114,10 +120,13 @@ impl Database {
         Ok(Some(lines))
     }

+    /// # Errors
+    ///
+    /// Propagates any `io::Error` that occurs when querying the database.
     pub fn write(&mut self, entry: &LogEntry) -> io::Result<()> {
         let key = Self::hash(&entry.metadata);

-        for meta in entry.metadata.iter() {
+        for meta in &entry.metadata {
             let keys = self
                 .index
                 .entry((meta.0.to_string(), meta.1.to_string()))
@@ -130,32 +139,31 @@ impl Database {
             }
         }

-        let (file, needs_delimeter) = match self.files.get_mut(&key) {
-            Some(file) => (file, true),
-            None => {
-                let mut entry_path = self.data_directory.clone();
-                entry_path.push(&key);
+        let (file, needs_delimeter) = if let Some(file) = self.files.get_mut(&key) {
+            (file, true)
+        } else {
+            let mut entry_path = self.data_directory.clone();
+            entry_path.push(&key);

-                let mut metadata_path = entry_path;
-                metadata_path.set_extension(METADATA_FILE_EXTENSION);
-                fs::write(&metadata_path, serde_json::to_vec(&entry.metadata)?)?;
+            let mut metadata_path = entry_path;
+            metadata_path.set_extension(METADATA_FILE_EXTENSION);
+            fs::write(&metadata_path, serde_json::to_vec(&entry.metadata)?)?;

-                let mut data_path = metadata_path;
-                data_path.set_extension(DATA_FILE_EXTENSION);
+            let mut data_path = metadata_path;
+            data_path.set_extension(DATA_FILE_EXTENSION);

-                let file = OpenOptions::new()
-                    .append(true)
-                    .create(true)
-                    .read(true)
-                    .open(&data_path)?;
+            let file = OpenOptions::new()
+                .append(true)
+                .create(true)
+                .read(true)
+                .open(&data_path)?;

-                // Using `.or_insert` here is annoying since we know there is no entry, but
-                // `hash_map::entry::insert` is unstable
-                // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
-                let file = self.files.entry(key).or_insert(file);
+            // Using `.or_insert` here is annoying since we know there is no entry, but
+            // `hash_map::entry::insert` is unstable
+            // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
+            let file = self.files.entry(key).or_insert(file);

-                (file, false)
-            }
+            (file, false)
         };

         if needs_delimeter {
@@ -198,7 +206,7 @@ impl Database {
     }

     fn hash(metadata: &HashMap<String, String>) -> String {
-        let mut digest = [0u8; 16];
+        let mut digest = [0_u8; 16];
         for (key, value) in metadata.iter() {
             let mut context = md5::Context::new();
             context.consume(key);
```

```diff
--- a/src/test.rs
+++ b/src/test.rs
@@ -16,6 +16,10 @@ pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
 ///
 /// This returns the handle to the temporary directory as well as the database, since the directory
 /// will be unlinked when the `TempDir` value is dropped.
+///
+/// # Errors
+///
+/// Propagates any `io::Error`s that occur when opening the database.
 pub fn temp_database() -> io::Result<(TempDir, Database)> {
     let tempdir = tempfile::tempdir()?;
     let config = log_database::Config {
@@ -27,12 +31,13 @@ pub fn temp_database() -> io::Result<(TempDir, Database)> {
 /// Construct a `LogEntry` with the given `line` and `metadata`.
 ///
 /// This is a convenience function to avoid having to build a `HashMap` for metadata.
+#[must_use]
 pub fn log_entry(line: &str, metadata: &[(&str, &str)]) -> LogEntry {
     LogEntry {
         line: line.to_string(),
         metadata: metadata
             .iter()
-            .map(|(k, v)| (k.to_string(), v.to_string()))
+            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
             .collect(),
     }
 }
```

This leaves us with 14 warnings due to missing documentation, so let's go through and add some:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,7 @@
 // lib.rs
+
+//! The elements that drive the `monitoring-rs` binary.
+
 #![warn(
     explicit_outlives_requirements,
     macro_use_extern_crate,
@@ -27,8 +30,12 @@ pub mod test;

 use std::collections::HashMap;

+/// A log entry that can be processed by the various parts of this library.
 #[derive(Debug, PartialEq)]
 pub struct LogEntry {
+    /// A line of text in the log.
     pub line: String,
+
+    /// Metadata associated with this log line.
     pub metadata: HashMap<String, String>,
 }
```

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -1,4 +1,7 @@
 // api/mod.rs
+
+//! Types and functions for initialising the `monitoring-rs` HTTP API.
+
 use std::sync::Arc;

 use async_std::sync::RwLock;
@@ -7,8 +10,13 @@ use crate::log_database::Database;

 type State = Arc<RwLock<Database>>;

+/// An instance of the `monitoring-rs` HTTP API.
+///
+/// This is aliased to save typing out the entire `State` type. In future it could be replaced by an
+/// opaque `impl Trait` type.
 pub type Server = tide::Server<State>;

+/// Initialise an instance of the `monitoring-rs` HTTP API.
 pub fn server(database: State) -> Server {
     let mut app = tide::Server::with_state(database);
     app.at("/logs/:key/*value").get(read_logs);
```

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -1,4 +1,7 @@
 // log_collector/mod.rs
+
+//! The interface for log collection in `monitoring-rs`.
+
 pub mod directory;
 mod watcher;

@@ -6,4 +9,7 @@ use std::io;

 use crate::LogEntry;

+/// A log collector can be any type that can be used as an `Iterator` of [`LogEntry`]s.
+///
+/// This is currently just a marker trait, but this could change as new log collectors are added.
 pub trait Collector: Iterator<Item = Result<LogEntry, io::Error>> {}
```

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -1,3 +1,5 @@
+//! A log collector that watches a directory of log files.
+
 use std::collections::HashMap;
 use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
```

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -1,4 +1,7 @@
 // src/log_database/mod.rs
+
+//! The interface for log storage in `monitoring-rs`.
+
 use std::collections::{HashMap, HashSet};
 use std::ffi::OsStr;
 use std::fs::{self, File, OpenOptions};
@@ -11,7 +14,9 @@ const DATA_FILE_EXTENSION: &str = "dat";
 const METADATA_FILE_EXTENSION: &str = "json";
 const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

+/// The configuration needed to open a database.
 pub struct Config {
+    /// The directory in which the database should store its data.
     pub data_directory: PathBuf,
 }

@@ -20,6 +25,24 @@ enum FileType {
     MetadataFile,
 }

+/// A log database supporting key-value rerieval.
+///
+/// **Note:** the functionality of this database is extremely minimal just now, and is missing vital
+/// features like retention management.
+///
+/// That said, it should be decently fast for storing and querying UTF-8 log entries with key-value
+/// metadata (via [`LogEntry`](crate::LogEntry)).
+///
+/// - Log lines are stored in a flat file named with a hash of the entry's metadata. Log entry
+///   metadata is stored in JSON files with the same base name. Handles to all log files are kept
+///   open in memory. An in-memory index is maintained for all `(key, value)` pairs of metadata to
+///   the set of log files that include that metadata.
+/// - Writes append a new line to the relevant file, creating a new log file and metadata file if
+///   necessary (and updating the index if so).
+/// - Reads are performed using a `key=value` pair. The index is used to identify the files that
+///   contain relevant records, and these files are then scanned in their entirety.
+///
+/// The structure, interface, and storage approach of the database is likely to change in future.
 pub struct Database {
     data_directory: PathBuf,
     files: HashMap<String, File>,
```

This fixes all the warnings reported by `cargo clippy --tests` – but what about
`missing_doc_code_examples`?
Since we haven't added any, we should expect to have some warnings for that.

It turns out some warnings only shows when using `cargo doc`, and furthermore only on nightly.
We would probably want to build documentation using rustdoc nightly anyway, in order to support
[linking to items by name](https://doc.rust-lang.org/rustdoc/linking-to-items-by-name.html).

```
$ cargo +nightly doc --no-deps
...
warning: 67 warnings emitted
```

Some more work to do then...
Or not.
It turns out there's presumably some work to do on the `missing_doc_code_examples` lint, since it currently checks for documentation on private items as well as public items (putting it in conflict with `private_doc_tests`), so we will leave that out for now:

```diff
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7,7 +7,6 @@
     macro_use_extern_crate,
     meta_variable_misuse,
     missing_crate_level_docs,
-    missing_doc_code_examples,
     missing_docs,
     private_doc_tests,
     single_use_lifetimes,
```

And this means we're done with lints!

```
$ cargo +nightly doc --no-deps
...
    Finished dev [unoptimized + debuginfo] target(s) in 0.25s
```

Or, we're almost done at least.
Since we have some platform-dependent code we should probably introduce a means to run `clippy` in docker as well:

```diff
--- a/Makefile
+++ b/Makefile
@@ -12,6 +12,9 @@ monitoring: build-monitoring
 dockertest:
  @docker-compose up --build --force-recreate test

+dockerlint:
+ @docker-compose up --build --force-recreate lint
+
 writer:
  @docker-compose up -d writer

```

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -17,6 +17,12 @@ services:
       target: builder
     command: [cargo, test, --release]

+  lint:
+    build:
+      context: .
+      target: builder
+    command: [cargo, clippy, --tests]
+
   writer:
     image: alpine
     volumes:
```

```diff
--- a/Dockerfile
+++ b/Dockerfile
@@ -2,7 +2,7 @@
 FROM rust:1.46.0-alpine as build_base

 WORKDIR /build
-RUN apk add --no-cache musl-dev && cargo install cargo-chef
+RUN apk add --no-cache musl-dev && rustup component add clippy && cargo install cargo-chef


 FROM build_base as planner
```

An, in fact, we've missed a bit:

```
$ make dockerlint
...
lint_1        | warning: package `monitoring-rs` is missing `package.readme` metadata
...
lint_1        | warning: useless conversion to the same type
lint_1        |   --> src/log_collector/watcher.rs:68:26
lint_1        |    |
lint_1        | 68 |             let events = inotify_events.into_iter().map(|event| Event {
lint_1        |    |                          ^^^^^^^^^^^^^^^^^^^^^^^^^^ help: consider removing `.into_iter()`: `inotify_events`
...
```

It's odd that `package.readme` is firing when it doesn't locally – but if we read [the docs](https://doc.rust-lang.org/cargo/reference/manifest.html#the-readme-field) we can see why:

> If no value is specified for this field, and a file named README.md, README.txt or README exists in the package root, then the name of that file will be used.

Since we exclude `README.md` in our `.dockerignore`, it's not present for `dockerlint` and so `package.readme` doesn't get its default value.
The docs go on to say:

> If the field is set to true, a default value of README.md will be assumed.

So, what if we just set `package.readme = true`?

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -7,6 +7,7 @@ authors = ["Chris Connelly <chris@connec.co.uk>"]
 license = "GPL-3.0-only"
 edition = "2018"

+readme = true
 categories = ["command-line-utilities"]
 keywords = ["logging", "metrics", "monitoring"]
 repository = "https://github.com/connec/monitoring-rs"
```

```
$ make dockerlint
...
Caused by:
    0: invalid type: boolean `true`, expected a string for key `package.readme` at line 10 column 10
    1: invalid type: boolean `true`, expected a string for key `package.readme` at line 10 column 10
ERROR: Service 'lint' failed to build : The command '/bin/sh -c cargo chef prepare' returned a non-zero code: 1
make: *** [dockerlint] Error 1
```

Ah, this looks like an issue in `cargo-chef`...
A [quick PR](https://github.com/LukeMathWalker/cargo-manifest/pull/6) later and we're able to build by installing `cargo-chef` from git:

```diff
--- a/Dockerfile
+++ b/Dockerfile
@@ -2,7 +2,9 @@
 FROM rust:1.46.0-alpine as build_base

 WORKDIR /build
-RUN apk add --no-cache musl-dev && rustup component add clippy && cargo install cargo-chef
+RUN apk add --no-cache musl-dev \
+  && rustup component add clippy \
+  && cargo install --git https://github.com/LukeMathWalker/cargo-chef --branch main


 FROM build_base as planner
```

```
$ make dockerlint
...
lint_1        | warning: useless conversion to the same type
lint_1        |   --> src/log_collector/watcher.rs:68:26
lint_1        |    |
lint_1        | 68 |             let events = inotify_events.into_iter().map(|event| Event {
lint_1        |    |                          ^^^^^^^^^^^^^^^^^^^^^^^^^^ help: consider removing `.into_iter()`: `inotify_events`
lint_1        |    |
lint_1        |    = note: `#[warn(clippy::useless_conversion)]` on by default
lint_1        |    = help: for further information visit https://rust-lang.github.io/rust-clippy/master/index.html#useless_conversion
```

Great, so one small code change:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -65,7 +65,7 @@ mod imp {

         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
             let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
-            let events = inotify_events.into_iter().map(|event| Event {
+            let events = inotify_events.map(|event| Event {
                 descriptor: super::Descriptor(event.wd),
             });

```

And, at last:

```
$ make dockerlint
monitoring-rs_lint_1 exited with code 0
```

## Running in Docker

Let's make sure our latest changes still run in Docker:

```
$ make writer monitoring
...
monitoring_1  | [2021-01-19T20:06:47Z DEBUG monitoring_rs::log_collector::directory] Initialising watch on root path "/var/log/containers"

# in another tab
$ curl -sD /dev/stderr localhost:8000/logs/path//var/log/containers/writer.log | jq
[
  "Tue Jan 19 20:06:47 UTC 2021",
  "Tue Jan 19 20:06:48 UTC 2021",
  "Tue Jan 19 20:06:49 UTC 2021",
  "Tue Jan 19 20:06:50 UTC 2021",
  "Tue Jan 19 20:06:51 UTC 2021",
  "Tue Jan 19 20:06:52 UTC 2021"
]
```

Reassuring!

```
$ make down
```

## Running in Kubernetes

If we run in Kubernetes:

```
$ make kubecleanup kuberun
...
```

We get no output, as expected (though we could make some things easier for ourselves with some basic periodic output).
We can expose the API with:

```
$ kubectl port-forward monitoring-rs 8000:8000
Forwarding from 127.0.0.1:8000 -> 8000
Forwarding from [::1]:8000 -> 8000
```

We can then identify a log file to query with:

```
$ kubectl exec -it monitoring-rs -- sh -c \
  'for f in $(ls -d /var/log/containers/*) ; do readlink -f $f ; done'
...
```

However, no matter which file we try, they all return `404` and no logs.

Let's turn up the log verbosity with:

```diff
--- a/Makefile
+++ b/Makefile
@@ -35,7 +35,7 @@ push: build-monitoring
 kuberun: push
  @kubectl run monitoring-rs \
      --image $(DOCKER_IMAGE) \
-     --env RUST_LOG=monitoring_rs=info \
+     --env RUST_LOG=monitoring_rs=trace \
      --restart Never \
      --dry-run=client \
      --output json \
```

And if we `kuberun` now we get:

```
$ make kubecleanup kuberun
...
[2021-01-19T20:35:10Z DEBUG monitoring_rs::log_collector::directory] Create [...]
[2021-01-19T20:35:10Z DEBUG monitoring_rs::log_collector::directory] Create [...]
[2021-01-19T20:35:10Z DEBUG monitoring_rs::log_collector::directory] Create [...]
```

After a number of `Create` events, presumably corresponding to the log files that exist when we start up, no more events come in.
Given that not even our `trace` is showing, we must be stuck in `read_events_blocking`?

```rust
let watcher_events = self.watcher.read_events_blocking()?;
...
for watcher_event in watcher_events {
    trace!("Received inotify event: {:?}", watcher_event);
    ...
}
```

The main chage we've made here has been introducing the `Iterator` interface:

```rust
impl<W: Watcher> Iterator for Collector<W> {
    type Item = Result<LogEntry, io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.entry_buf.len() == 0 {
            let entries = match self.collect_entries() {
                Ok(entries) => entries,
                Err(error) => return Some(Err(error)),
            };
            self.entry_buf = entries.into_iter();
        }
        Some(Ok(self.entry_buf.next()?))
    }
}
```

What if, for some reason, `collect_entries` returns an empty `Vec`?
In fact, this could easily happen if `watcher.read_events_blocking` only returns `Create` events, uncorrelated events, or itself returns an empty `Vec`.

To get the behaviour we want we can change `if` to `while`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -214,14 +214,15 @@ impl<W: Watcher> Iterator for Collector<W> {
     type Item = Result<LogEntry, io::Error>;

     fn next(&mut self) -> Option<Self::Item> {
-        if self.entry_buf.len() == 0 {
+        while self.entry_buf.len() == 0 {
             let entries = match self.collect_entries() {
                 Ok(entries) => entries,
                 Err(error) => return Some(Err(error)),
             };
             self.entry_buf = entries.into_iter();
         }
-        Some(Ok(self.entry_buf.next()?))
+        // `unwrap` because we've refilled `entry_buf`
+        Some(Ok(self.entry_buf.next().unwrap()))
     }
 }

```

We've also added an `unwrap` to ensure we crash noisily if we ever exit with an empty `entry_buf`, rather than silently ending the loop (we could also loop in `main.rs`, but this will do for now).

Let's revert our `Makefile` and try again:

```diff
--- a/Makefile
+++ b/Makefile
@@ -35,7 +35,7 @@ push: build-monitoring
 kuberun: push
  @kubectl run monitoring-rs \
      --image $(DOCKER_IMAGE) \
-     --env RUST_LOG=monitoring_rs=trace \
+     --env RUST_LOG=monitoring_rs=info \
      --restart Never \
      --dry-run=client \
      --output json \
```

```
$ make kubecleanup kuberun
...

# in another tab
$ kubectl port-forward monitoring-rs 8000:8000

# find a container that has recent logs and get its ID

# in another tab
$ curl -sD/dev/stderr localhost:8000/logs/path//var/lib/docker/containers/<id>/<id>-json.log | jq
HTTP/1.1 200 OK
content-length: 12376
content-type: application/json
date: Tue, 19 Jan 2021 21:22:06 GMT

[
  ...
]
```

Phew, sorted.

### Should we add a test for this?

It would be nice to test that our `Iterator` implementation iterates forever, but this is easier said than done since 'forever' is a long time to wait for tests.
This could be a reason to introduce our own API to the `Collector` trait that can return `!`, indicating that the function diverges (never returns), but we will worry about this in future.

## That'll do

Let's close this out here.
We've:

- Simplified our tests.
- Turned on more lints.
- Used our Kubernetes environment to find and fix a bug.

Good job, us.
Next up it might be time to consider the "Kubernetes" collector, so that we can add Kubernetes metadata and more easily verify expected behaviour in that environment.
