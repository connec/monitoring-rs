# End-to-end

Now that we have a vague impression of what pieces we want to build, and a space in our project in which to build them, let's write a naive implementation of our `log_database` and `api`, and hook them up.

## `log_database`

Let's initialize an empty `log_database` module:

```
mkdir src/log_database
touch src/log_database/mod.rs
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -3,6 +3,7 @@
 extern crate log;

 mod log_collector;
+mod log_database;

 use std::io;

```

What should go in our module?
We know we need to be able to write entries to the database as they are collected, and retrieve entries when read by the API.
We would like to be able to test our database implementation, so we would like to be able to configure interactions with external dependencies like the file system.
Let's start with a skeleton `Database` struct with the methods we might predict we will need:

```rust
// src/log_database/mod.rs
pub struct Database;

impl Database {
    pub fn read(/* ??? */) /* -> ??? */
    {
        unimplemented!()
    }

    pub fn write(/* ??? */) /* -> ??? */
    {
        unimplemented!()
    }
}
```

Let's also set ourselves up to handle growing configuration surface area by having a separate `Config` struct, and a `Database::open(config)` method:

```rust
// src/log_database/mod.rs
pub struct Config;

pub struct Database;

impl Database {
    pub fn open(config: Config) /* -> ??? */
    {
        unimplemented!()
    }

    pub fn read(/* ??? */) /* -> ??? */
    {
        unimplemented!()
    }

    pub fn write(/* ??? */) /* -> ??? */
    {
        unimplemented!()
    }
}
```

Now, let's consider the `???` in the interface.

### `Database::open` API

Since we expect the database to be stored in the file system, the return value of `Database::open` should account for potential failures.
Let's put off significant thinking around error types for now, and settle for returning an `io::Result<Database>`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -1,11 +1,12 @@
 // src/log_database/mod.rs
+use std::io;
+
 pub struct Config;

 pub struct Database;

 impl Database {
-    pub fn open(config: Config) /* -> ??? */
-    {
+    pub fn open(config: Config) -> io::Result<Self> {
         unimplemented!()
     }

```

### `Database::read` & `Database::write` API

For `read` and `write`, we need to think about what we're storing, how we're storing it, and more generally how `Database` will be used.
Let's consider the last point first.

#### How will the database be used?

We want a database in order to persist collected log entries and allow them to be retrieved by operators via an API.
We made some educated guesses about the use-cases for log retrieval during [discovery](0-discovery.md#retrieval):

> 1. Following the live logs for a specific service or component in order to debug it or otherwise observe its behaviour.
>   For this use-case, latency and specificity are key – obtaining the latest log entries based on precise criteria needs to be fast.
>   Latency is not just a retrieval issue, but the retrieval performance would affect the overall latency.
>   Reliably filtering logs to those from a specific source would require field-based filtering, rather than text-based filtering which may lead to false positives.
>
> 1. Retrieving logs with potentially vague criteria in order to understand an alert with limited context – perhaps only the service name and a rough timeframe, or even just a string of text from an error message.
>   For this use-case, query flexibility and plain-text search are important.
>   The known criteria should be combineable into a single query which can be further refined to exclude noise and hone in on relevant entries.
>
> 1. Tracing activity through multiple services based on structured log data (e.g. a transaction ID or user ID).
>   For this use-case, it's important that there are minimal or no required constraints when searching logs.
>   Relevant logs may be from arbitrary time periods and sources, and all must be retrieved in order to offer a complete answer to the query.

We conclude with the following requirements and a helpful summary:

> - Support for storing and querying structured documents, with efficient text storage.
> - Support for complex queries, including conjunction ("and"), disjunction ("or"), negation ("not"), and free-text search.
> - Support for cross-partition queries.
> - Optimised for append-only writes.
> - Optimised for time-ordered, filtered reads with simple (`key=value`) criteria.
> - Reasonably performant for complex or cross-partition queries.
> - Support for reasonable retention/archiving strategies, due to append-only operation.
>
> Ooft.
> This seems quite idealistic.

Indeed, this is far too much for our initial end-to-end implementation.
Let's go extremely bare-bones for now to get our system running end-to-end and consider the following two simplified use-cases:

1. Following the live logs for a specific container, identified by an implementation-convenient key.
1. Obtain the most recent `n` log entries for a specific container, identified by an implementation-convenient key.

Essentially we want to be able to mimic the behaviour of `kubectl logs`, with a specific (and probably not very useable) key that is convenient for our implementation, such as the name of the log file.

This gives us our answer to "how will the database be used?".
Our database will be used to...

- Persist incoming log entries.
- Retrieve last `n` log entries by key (let's assume log file name).

#### What we're storing & how we're storing it

This also allows us to answer our "what we're storing" & "how we're storing it" questions:

- We're storing log lines as plain strings.
- We're storing them by appending them to a different file.

This is clearly not very useful, after all we could just have our `api` read from the source log volumes!
However, we're assuming that we will extend this in future to include features such as more powerful filtering, log parsing/text search, and retention management.
Having a "database" component of some kind, fed by our log collector, will help us get to a point where we can develop and test database features without making changes to the collector or API.

#### Filling in the blanks

Now we can take a better guess at the API for `read` and `write`:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -10,13 +10,11 @@ impl Database {
         unimplemented!()
     }

-    pub fn read(/* ??? */) /* -> ??? */
-    {
+    pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
         unimplemented!()
     }

-    pub fn write(/* ??? */) /* -> ??? */
-    {
+    pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
         unimplemented!()
     }
 }
```

### Implementation

With that scaffold in place we can imagine the following behaviour:

- `Config` should include a directory into which data will be written.
- `Database` should include a mapping of keys to file handles, based on the names of files in the directory.
- `read` should lookup the file handle for the given `key`, and parse the contents to a list of strings if present (returning `None` if a file does not exist for the `key`).
- `write` should lookup the file handle for the given `key`, create the file and add it to the mapping if it doesn't already exist, and finally write the line to the file.

We need a serialization scheme to allow us to read and write log lines to a file without getting tripped up by potentially embedded line breaks.
We would ideally use a scheme that allows us to write new lines by simply appending to the file.
For now, let's separate our records with the byte 147, which is invalid UTF-8 and so could never be in an incoming `&str`.

Let's see how that looks:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -1,20 +1,127 @@
 // src/log_database/mod.rs
-use std::io;
+use std::collections::HashMap;
+use std::ffi::OsStr;
+use std::fs::{self, File, OpenOptions};
+use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
+use std::path::PathBuf;

-pub struct Config;
+const DATA_FILE_EXTENSION: &str = "json";
+const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

-pub struct Database;
+pub struct Config {
+    data_directory: PathBuf,
+}
+
+pub struct Database {
+    data_directory: PathBuf,
+    files: HashMap<String, File>,
+}

 impl Database {
     pub fn open(config: Config) -> io::Result<Self> {
-        unimplemented!()
+        let mut files = HashMap::new();
+        for entry in fs::read_dir(&config.data_directory)? {
+            let entry = entry?;
+            let path = entry.path();
+
+            if path.extension().and_then(OsStr::to_str) != Some(DATA_FILE_EXTENSION) {
+                return Err(Self::error(format!(
+                    "invalid data file {}: extension must be `json`",
+                    path.display()
+                )));
+            }
+
+            let metadata = fs::metadata(&path)?;
+            if !metadata.is_file() {
+                return Err(Self::error(format!(
+                    "invalid data file {}: not a file",
+                    path.display()
+                )));
+            }
+
+            let key = path.file_stem().ok_or_else(|| {
+                Self::error(format!(
+                    "invalid data file name {}: empty file stem",
+                    path.display()
+                ))
+            })?;
+
+            let key = key.to_str().ok_or_else(|| {
+                Self::error(format!(
+                    "invalid data file name {}: non-utf8 file name",
+                    path.display()
+                ))
+            })?;
+
+            let file = OpenOptions::new().append(true).read(true).open(&path)?;
+
+            files.insert(key.to_string(), file);
+        }
+        Ok(Database {
+            data_directory: config.data_directory,
+            files,
+        })
     }

     pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
-        unimplemented!()
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
     }

     pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
-        unimplemented!()
+        let (mut file, needs_delimeter) = match self.files.get(key) {
+            Some(file) => (file, true),
+            None => {
+                let mut path = self.data_directory.clone();
+                path.push(key);
+                path.set_extension(DATA_FILE_EXTENSION);
+
+                let file = OpenOptions::new()
+                    .append(true)
+                    .create(true)
+                    .read(true)
+                    .open(&path)?;
+
+                self.files.insert(key.to_string(), file);
+                (&self.files[key], false)
+            }
+        };
+
+        if needs_delimeter {
+            file.write_all(&[DATA_FILE_RECORD_SEPARATOR])?;
+        }
+        file.write_all(line.as_ref())?;
+
+        Ok(())
+    }
+
+    fn error(message: String) -> io::Error {
+        io::Error::new(io::ErrorKind::Other, message)
     }
 }
```

This ain't all that beautiful, but it should get us started.

### Tests

Let's write some simple tests.
We'll keep them very high-level for now, so let's add a dev dependency on `tempfile` so that we can create temporary database directories:

```
$ cargo add tempfile --dev
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding tempfile v3.1.0 to dev-dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -10,3 +10,6 @@ edition = "2018"
 env_logger = "0.8.1"
 inotify = { version = "0.8.3", default-features = false }
 log = "0.4.11"
+
+[dev-dependencies]
+tempfile = "3.1.0"
```

Now the tests:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -125,3 +125,69 @@ impl Database {
         io::Error::new(io::ErrorKind::Other, message)
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use super::{Config, Database};
+
+    #[test]
+    fn test_new_db() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+        let config = Config {
+            data_directory: tempdir.path().to_path_buf(),
+        };
+        let mut database = Database::open(config).expect("unable to open database");
+
+        assert_eq!(
+            database.read("foo").expect("unable to read from database"),
+            None
+        );
+
+        database
+            .write("foo", "line1")
+            .expect("unable to write to database");
+        assert_eq!(
+            database.read("foo").expect("unable to read from database"),
+            Some(vec!["line1".to_string()])
+        );
+
+        database
+            .write("foo", "line2")
+            .expect("unable to write to database");
+        assert_eq!(
+            database.read("foo").expect("unable to read from database"),
+            Some(vec!["line1".to_string(), "line2".to_string()])
+        );
+    }
+
+    #[test]
+    fn test_existing_db() {
+        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
+
+        {
+            let config = Config {
+                data_directory: tempdir.path().to_path_buf(),
+            };
+            let mut database = Database::open(config).expect("unable to open database");
+
+            database
+                .write("foo", "line1")
+                .expect("failed to write to database");
+            database
+                .write("foo", "line2")
+                .expect("failed to write to database");
+
+            drop(database);
+        }
+
+        let config = Config {
+            data_directory: tempdir.path().to_path_buf(),
+        };
+        let database = Database::open(config).expect("unable to open database");
+
+        assert_eq!(
+            database.read("foo").expect("unable to read from database"),
+            Some(vec!["line1".to_string(), "line2".to_string()])
+        );
+    }
+}
```

And now we can get some reassuring green text!

```
$ cargo test
...
running 2 tests
test log_database::tests::test_new_db ... ok
test log_database::tests::test_existing_db ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

## `api`

Now let's consider the API.
The first thing we need to do is choose an HTTP server library.
There are several options now available for Rust HTTP server crates, the most popular of which is probably [`actix-web`](https://crates.io/crates/actix-web).

For now though, let's experiment with [Tide](https://crates.io/crates/tide) since this looks like one of the lighter-weight options.
Let's start by adding the dependency:

```
$ cargo add tide
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding tide v0.15.0 to dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -10,6 +10,7 @@ edition = "2018"
 env_logger = "0.8.1"
 inotify = { version = "0.8.3", default-features = false }
 log = "0.4.11"
+tide = "0.15.0"

 [dev-dependencies]
 tempfile = "3.1.0"
```

Now let's create a new module:

```
mkdir src/api
touch src/api/mod.rs
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -2,6 +2,7 @@
 #[macro_use]
 extern crate log;

+mod api;
 mod log_collector;
 mod log_database;

```

Let's start with a "hello, world" and then look to get things up-and-running in `main.rs`.

```rust
// api/mod.rs
pub type Server = tide::Server<()>;

pub fn server() -> Server {
    let mut app = tide::new();
    app.at("/").get(|_| async { Ok("Hello, world") });
    app
}
```

`tide::Server` has a generic placeholder for the app state, which we will likely have to change.
We create an `api::Server` type alias so that the consumer doesn't have to care about that changing.
Let's update `main.rs` to run the server (we will restore our log collection soon):

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -12,13 +12,19 @@ use log_collector::Collector;

 const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

-fn main() -> io::Result<()> {
+#[async_std::main]
+async fn main() -> io::Result<()> {
     env_logger::init();

-    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+    let api_server = api::server();
+    api_server.listen("0.0.0.0:8000").await?;

-    let mut buffer = [0; 1024];
-    loop {
-        collector.handle_events(&mut buffer)?;
-    }
+    Ok(())
+
+    // let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+
+    // let mut buffer = [0; 1024];
+    // loop {
+    //     collector.handle_events(&mut buffer)?;
+    // }
 }
```

For this to work we need to add `async-std` as a direct dependency as well.
`cargo add` is insufficient for this task as we also need the `attributes` feature, so we have to edit `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -11,6 +11,7 @@ env_logger = "0.8.1"
 inotify = { version = "0.8.3", default-features = false }
 log = "0.4.11"
 tide = "0.15.0"
+async-std = { version = "1.7.0", features = ["attributes"] }

 [dev-dependencies]
 tempfile = "3.1.0"
```

We can now `cargo run`:

```
$ cargo run
...
     Running `target/debug/monitoring-rs`
```

Et voila – an HTTP endpoint has appeared:

```
# in another tab
$ curl -i 127.0.0.1:8000
HTTP/1.1 200 OK
content-length: 12
content-type: text/plain;charset=utf-8
date: Mon, 07 Dec 2020 18:06:34 GMT

Hello, world
```

We should also update our docker-compose file so that we can access the server when running `make monitoring`:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -8,6 +8,8 @@ services:
     - logs:/var/log/containers
     environment:
     - RUST_LOG=monitoring_rs=debug
+    ports:
+    - 8000:8000

   writer:
     image: alpine
```

## Integrating everything

Let's try and get everything working in harmony.

### Open a database

First, we want to open a database:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -6,9 +6,12 @@ mod api;
 mod log_collector;
 mod log_database;

+use std::env;
+use std::fs;
 use std::io;

 use log_collector::Collector;
+use log_database::Database;

 const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

@@ -16,6 +19,12 @@ const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
 async fn main() -> io::Result<()> {
     env_logger::init();

+    let mut data_directory = env::current_dir()?;
+    data_directory.push(".data");
+    fs::create_dir_all(&data_directory)?;
+
+    let database = Database::open(log_database::Config { data_directory })?;
+
     let api_server = api::server();
     api_server.listen("0.0.0.0:8000").await?;

```

This reveals a bug in our `log_database` implementation – we have no way to construct `Config` since the fields are private by default and there's no constructor.
To keep configuration explicit, let's just make the field public for now:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -9,7 +9,7 @@ const DATA_FILE_EXTENSION: &str = "json";
 const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

 pub struct Config {
-    data_directory: PathBuf,
+    pub data_directory: PathBuf,
 }

 pub struct Database {
```

Now if we (re)start our server, a `.data` directory will appear (we will make this configurable later):

```
$ cargo run
...
     Running `target/debug/monitoring-rs`

# in another tab
$ stat -F .data
drwxr-xr-x 2 chris group 64 Dec  7 18:15:24 2020 .data/
```

### Read from the database in the server

Let's create a `/logs/:key` endpoint that will respond with a JSON array of the log entries under the given `key`.
To do so, we will have to pass our `database` to `api::server`.
Let's do that naively for now:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -25,7 +25,7 @@ async fn main() -> io::Result<()> {

     let database = Database::open(log_database::Config { data_directory })?;

-    let api_server = api::server();
+    let api_server = api::server(database);
     api_server.listen("0.0.0.0:8000").await?;

     Ok(())
```

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -1,8 +1,12 @@
 // api/mod.rs
-pub type Server = tide::Server<()>;
+use std::sync::Arc;

-pub fn server() -> Server {
-    let mut app = tide::new();
+use crate::log_database::Database;
+
+pub type Server = tide::Server<Arc<Database>>;
+
+pub fn server(database: Database) -> Server {
+    let mut app = tide::Server::with_state(Arc::new(database));
     app.at("/").get(|_| async { Ok("Hello, world") });
     app
 }

```

Now we can try to implement our endpoint:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -7,6 +7,18 @@ pub type Server = tide::Server<Arc<Database>>;

 pub fn server(database: Database) -> Server {
     let mut app = tide::Server::with_state(Arc::new(database));
-    app.at("/").get(|_| async { Ok("Hello, world") });
+    app.at("/logs/:key").get(read_logs);
     app
 }
+
+async fn read_logs(req: tide::Request<Arc<Database>>) -> tide::Result {
+    let key = req.param("key")?;
+    let database = req.state();
+
+    Ok(match database.read(key)? {
+        Some(logs) => tide::Response::builder(tide::StatusCode::Ok)
+            .body(tide::Body::from_json(&logs)?)
+            .build(),
+        None => tide::Response::new(tide::StatusCode::NotFound),
+    })
+}
```

And if we test it out:

```
$ cargo run
...
     Running `target/debug/monitoring-rs`

# in another tab
$ curl -i 127.0.0.1:8000/foo
HTTP/1.1 404 Not Found
content-length: 0
date: Mon, 07 Dec 2020 20:47:08 GMT

```

Ideal.

### Write to the database from the collector

Now, the last link in the chain is to get our collector to write to the database.
There's actually quite a lot to do here, but we're getting tantalisingly close so let's keep digging...

#### Extract log lines

Up until now, the behaviour of our collector on encountering an event has been to copy the file contents from the last seek position to `stdout`.
We now want to get new log data out of the log collector.
Furthermore, the rest of our system expects to work with log lines, so the collector needs to split lines.

Let's try to augment `LiveFile` such that we can use [`BufRead::read_until`](https://doc.rust-lang.org/stable/std/io/trait.BufRead.html#method.read_until) to read lines from the file:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -2,24 +2,16 @@
 use std::collections::hash_map::HashMap;
 use std::ffi::OsStr;
 use std::fs::{self, File};
-use std::io::{self, Seek, Stdout};
+use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};

 use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

 #[derive(Debug)]
 enum Event<'collector> {
-    Create {
-        path: PathBuf,
-    },
-    Append {
-        stdout: &'collector mut Stdout,
-        live_file: &'collector mut LiveFile,
-    },
-    Truncate {
-        stdout: &'collector mut Stdout,
-        live_file: &'collector mut LiveFile,
-    },
+    Create { path: PathBuf },
+    Append { live_file: &'collector mut LiveFile },
+    Truncate { live_file: &'collector mut LiveFile },
 }

 impl Event<'_> {
@@ -49,13 +41,13 @@ impl std::fmt::Display for Event<'_> {
 #[derive(Debug)]
 struct LiveFile {
     path: PathBuf,
-    file: File,
+    reader: BufReader<File>,
+    entry_buf: String,
 }

 pub struct Collector {
     root_path: PathBuf,
     root_wd: WatchDescriptor,
-    stdout: Stdout,
     live_files: HashMap<WatchDescriptor, LiveFile>,
     inotify: Inotify,
 }
@@ -70,7 +62,6 @@ impl Collector {
         let mut collector = Self {
             root_path: root_path.to_path_buf(),
             root_wd,
-            stdout: io::stdout(),
             live_files: HashMap::new(),
             inotify,
         };
@@ -95,15 +86,22 @@ impl Collector {
             if let Some(event) = self.check_event(inotify_event)? {
                 debug!("{}", event);

-                match event {
-                    Event::Create { path } => self.handle_event_create(path),
-                    Event::Append { stdout, live_file } => {
-                        Self::handle_event_append(stdout, &mut live_file.file)
+                let live_file = match event {
+                    Event::Create { path } => self.handle_event_create(path)?,
+                    Event::Append { live_file } => live_file,
+                    Event::Truncate { live_file } => {
+                        Self::handle_event_truncate(live_file)?;
+                        live_file
                     }
-                    Event::Truncate { stdout, live_file } => {
-                        Self::handle_event_truncate(stdout, &mut live_file.file)
+                };
+
+                while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
+                    if live_file.entry_buf.ends_with('\n') {
+                        live_file.entry_buf.pop();
+                        println!("{}", live_file.entry_buf);
+                        live_file.entry_buf.clear();
                     }
-                }?;
+                }
             }
         }

@@ -138,7 +136,6 @@ impl Collector {
             return Ok(Some(Event::Create { path }));
         }

-        let stdout = &mut self.stdout;
         let live_file = match self.live_files.get_mut(&inotify_event.wd) {
             None => {
                 warn!(
@@ -150,38 +147,37 @@ impl Collector {
             Some(live_file) => live_file,
         };

-        let metadata = live_file.file.metadata()?;
-        let seekpos = live_file.file.seek(io::SeekFrom::Current(0))?;
+        let metadata = live_file.reader.get_ref().metadata()?;
+        let seekpos = live_file.reader.seek(io::SeekFrom::Current(0))?;

         if seekpos <= metadata.len() {
-            Ok(Some(Event::Append { stdout, live_file }))
+            Ok(Some(Event::Append { live_file }))
         } else {
-            Ok(Some(Event::Truncate { stdout, live_file }))
+            Ok(Some(Event::Truncate { live_file }))
         }
     }

-    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<()> {
+    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
         let realpath = fs::canonicalize(&path)?;

         let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
-        let mut file = File::open(realpath)?;
-        file.seek(io::SeekFrom::End(0))?;
-
-        self.live_files.insert(wd, LiveFile { path, file });
-
-        Ok(())
+        let mut reader = BufReader::new(File::open(realpath)?);
+        reader.seek(io::SeekFrom::End(0))?;
+
+        self.live_files.insert(
+            wd.clone(),
+            LiveFile {
+                path,
+                reader,
+                entry_buf: String::new(),
+            },
+        );
+        Ok(self.live_files.get_mut(&wd).unwrap())
     }

-    fn handle_event_append(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
-        io::copy(&mut file, stdout)?;
-
-        Ok(())
-    }
-
-    fn handle_event_truncate(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
-        file.seek(io::SeekFrom::Start(0))?;
-        io::copy(&mut file, stdout)?;
-
+    fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
+        live_file.reader.seek(io::SeekFrom::Start(0))?;
+        live_file.entry_buf.clear();
         Ok(())
     }
 }
```

This is quite a significant diff.
We have done the following:

1. Removed `stdout` from `Event::Append` and `Event::Truncate`.
1. Replace `file` in `LiveFile` with `reader: BufReader<File>` and `entry_buf: String`.
  `reader` gives us access to `read_line`, and `entry_buf` gives us a reusable buffer we can pass to it.
  We can also use `entry_buf` to handle partial lines (when we encounter EOF before line end).
1. Removed `stdout` from `Collector`.
1. Obtained a `LiveFile` in the `handle_events` loop, which we use to repeatedly call `read_line` and print the read line.
1. Updated `handle_event_create` to return a reference to the created `LiveFile`.
1. Removed `handle_event_append`, as it was no longer doing anything.
1. Removed writing to `stdout` from `handle_event_truncate`.

We should restore our old `main.rs` implementation and check things still work (one day we should add some useful tests...):

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -19,21 +19,21 @@ const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
 async fn main() -> io::Result<()> {
     env_logger::init();

-    let mut data_directory = env::current_dir()?;
-    data_directory.push(".data");
-    fs::create_dir_all(&data_directory)?;
+    // let mut data_directory = env::current_dir()?;
+    // data_directory.push(".data");
+    // fs::create_dir_all(&data_directory)?;

-    let database = Database::open(log_database::Config { data_directory })?;
+    // let database = Database::open(log_database::Config { data_directory })?;

-    let api_server = api::server(database);
-    api_server.listen("0.0.0.0:8000").await?;
+    // let api_server = api::server(database);
+    // api_server.listen("0.0.0.0:8000").await?;

-    Ok(())
+    // Ok(())

-    // let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;

-    // let mut buffer = [0; 1024];
-    // loop {
-    //     collector.handle_events(&mut buffer)?;
-    // }
+    let mut buffer = [0; 1024];
+    loop {
+        collector.handle_events(&mut buffer)?;
+    }
 }
```

Let's fire 'er up:

```
$ make writer monitoring
...
  = note: /usr/lib/gcc/x86_64-alpine-linux-musl/9.3.0/../../../../x86_64-alpine-linux-musl/bin/ld: cannot find crti.o: No such file or directory
```

Ruh roh, one of our new dependencies is failing to compile on Alpine.
We can sort this by installing `musl-dev`:

```diff
--- a/Dockerfile
+++ b/Dockerfile
@@ -1,6 +1,8 @@
 # Dockerfile
 FROM rust:1.46.0-alpine

+RUN apk add --no-cache musl-dev
+
 RUN mkdir /build
 ADD . /build/

```

And now:

```
$ make writer monitoring
...
monitoring_1  | [2020-12-07T22:33:03Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-07T22:33:03Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | Mon Dec  7 22:33:03 UTC 2020
^C

$ make down
```

Still works, phew.
Now let's introduce a `LogEntry` struct that we will eventually return from `handle_events` (but for now will continue to print to `stdout`):

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -45,6 +45,12 @@ struct LiveFile {
     entry_buf: String,
 }

+#[derive(Debug)]
+pub struct LogEntry {
+    pub path: PathBuf,
+    pub line: String,
+}
+
 pub struct Collector {
     root_path: PathBuf,
     root_wd: WatchDescriptor,
@@ -98,7 +104,12 @@ impl Collector {
                 while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
                     if live_file.entry_buf.ends_with('\n') {
                         live_file.entry_buf.pop();
-                        println!("{}", live_file.entry_buf);
+                        let entry = LogEntry {
+                            path: live_file.path.clone(),
+                            line: live_file.entry_buf.clone(),
+                        };
+                        println!("{:?}", entry);
+
                         live_file.entry_buf.clear();
                     }
                 }
```

The fields are `pub` to allow us to access them later from `main` without the need for accessors.
All that cloning is probably not as efficient as it could be, but it will do for now.
It will also help us with the next step of returning them from `handle_events`, which we'll rename to `collect_entries`:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -83,8 +83,9 @@ impl Collector {
         Ok(collector)
     }

-    pub fn handle_events(&mut self, buffer: &mut [u8]) -> io::Result<()> {
+    pub fn collect_entries(&mut self, buffer: &mut [u8]) -> io::Result<Vec<LogEntry>> {
         let inotify_events = self.inotify.read_events_blocking(buffer)?;
+        let mut entries = Vec::new();

         for inotify_event in inotify_events {
             trace!("Received inotify event: {:?}", inotify_event);
@@ -108,7 +109,7 @@ impl Collector {
                             path: live_file.path.clone(),
                             line: live_file.entry_buf.clone(),
                         };
-                        println!("{:?}", entry);
+                        entries.push(entry);

                         live_file.entry_buf.clear();
                     }
@@ -116,7 +117,7 @@ impl Collector {
             }
         }

-        Ok(())
+        Ok(entries)
     }

     fn check_event<'ev>(
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -34,6 +34,7 @@ async fn main() -> io::Result<()> {

     let mut buffer = [0; 1024];
     loop {
-        collector.handle_events(&mut buffer)?;
+        let entries = collector.collect_entries(&mut buffer)?;
+        println!("{:?}", entries);
     }
 }
```

Right, now we have `entries` outside of the collector.
Will it blend?

```
$ make writer monitoring
...
monitoring_1  | [2020-12-13T22:54:22Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-13T22:54:22Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-13T22:54:22Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
monitoring_1  | [LogEntry { path: "/var/log/containers/writer.log", line: "Sun Dec 13 22:54:22 UTC 2020" }]
...
^C

$ make down
```

### Final plumbing

Time to plumb it together...

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -1,19 +1,23 @@
 // api/mod.rs
 use std::sync::Arc;

+use async_std::sync::RwLock;
+
 use crate::log_database::Database;

-pub type Server = tide::Server<Arc<Database>>;
+type State = Arc<RwLock<Database>>;
+
+pub type Server = tide::Server<State>;

-pub fn server(database: Database) -> Server {
-    let mut app = tide::Server::with_state(Arc::new(database));
+pub fn server(database: State) -> Server {
+    let mut app = tide::Server::with_state(database);
     app.at("/logs/:key").get(read_logs);
     app
 }

-async fn read_logs(req: tide::Request<Arc<Database>>) -> tide::Result {
+async fn read_logs(req: tide::Request<State>) -> tide::Result {
     let key = req.param("key")?;
-    let database = req.state();
+    let database = req.state().read().await;

     Ok(match database.read(key)? {
         Some(logs) => tide::Response::builder(tide::StatusCode::Ok)
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -9,6 +9,12 @@ mod log_database;
 use std::env;
 use std::fs;
 use std::io;
+use std::sync::Arc;
+use std::thread;
+
+use async_std::prelude::FutureExt;
+use async_std::sync::RwLock;
+use async_std::task::block_on;

 use log_collector::Collector;
 use log_database::Database;
@@ -19,22 +25,31 @@ const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
 async fn main() -> io::Result<()> {
     env_logger::init();

-    // let mut data_directory = env::current_dir()?;
-    // data_directory.push(".data");
-    // fs::create_dir_all(&data_directory)?;
-
-    // let database = Database::open(log_database::Config { data_directory })?;
-
-    // let api_server = api::server(database);
-    // api_server.listen("127.0.0.1:8000").await?;
-
-    // Ok(())
-
-    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
-
-    let mut buffer = [0; 1024];
-    loop {
-        let entries = collector.collect_entries(&mut buffer)?;
-        println!("{:?}", entries);
-    }
+    let mut data_directory = env::current_dir()?;
+    data_directory.push(".data");
+    fs::create_dir_all(&data_directory)?;
+
+    let database = Arc::new(RwLock::new(Database::open(log_database::Config {
+        data_directory,
+    })?));
+
+    let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");
+
+    let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
+        let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+        let mut buffer = [0; 1024];
+        loop {
+            let entries = collector.collect_entries(&mut buffer)?;
+            let mut database = block_on(database.write());
+            for entry in entries {
+                let key = entry.path.to_string_lossy();
+                database.write(&key, &entry.line)?;
+            }
+        }
+    });
+    let collector_handle = blocking::unblock(|| collector_thread.join().unwrap());
+
+    api_handle.try_join(collector_handle).await?;
+
+    Ok(())
 }
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -12,6 +12,7 @@ inotify = { version = "0.8.3", default-features = false }
 log = "0.4.11"
 tide = "0.15.0"
 async-std = { version = "1.7.0", features = ["attributes"] }
+blocking = "1.0.2"

 [dev-dependencies]
 tempfile = "3.1.0"
```

The changes in `main` are substantial.
We now run the collector in a thread, and with a little gymnastics (using [`blocking::unblock`](https://docs.rs/blocking/1.0.2/blocking/fn.unblock.html)) we are able to await the termination of either the collector thread or the api server, ensuring the whole process goes down if one of them crashes.

In order to pass the `Database` to both `api::server` and the collector thread, we've wrapped it in [`Arc`](https://doc.rust-lang.org/stable/std/sync/struct.Arc.html) and [`RwLock`](https://docs.rs/async-std/1.8.0/async_std/sync/struct.RwLock.html) in order to access it from both contexts, one of which needs a mutable reference.
A `Mutex` would also enable this but `RwLock` would at least allow multiple concurrent API requests to be served when the collector isn't writing.
There is likely to be a more efficient option, such as making `Database::write` operate on `&self` rather than `&mut self` and handling locking internally, but we will worry about that later.

For now, let's see if it blends:

```
$ make writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-14T17:35:13Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-14T17:35:13Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-14T17:35:14Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
monitoring_1  | [2020-12-14T17:35:14Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.json
monitoring_1  | [2020-12-14T17:35:15Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
monitoring_1  | [2020-12-14T17:35:15Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.json
monitoring_1  | Error: Custom { kind: InvalidData, error: "stream did not contain valid UTF-8" }
monitoring-rs_monitoring_1 exited with code 1

$ make down
```

Oh.
It looks like our collector is encountering invalid UTF-8 in `/var/log/containers/writer.json`.
In fact, where is `writer.json` coming from!?
Probably from our rediculous `key` handling in `Database::write`:

```rust
...
let mut path = self.data_directory.clone();
path.push(key);
path.set_extension(DATA_FILE_EXTENSION);
...
```

What if, not so hypothetically, `key` was an absolute path?
Well then, [`PathBuf::push`](https://doc.rust-lang.org/stable/std/path/struct.PathBuf.html#method.push) will do its thing and replace the whole path...

> If `path` is absolute, it replaces the current path.

...and we'll end up saving a `.json` version of the file, with non-UTF-8 separators, in the log directory 🤦‍♂️

Let's change where we store our data files to take a safe hash of the `key` instead of using it verbatim:

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -5,7 +5,7 @@ use std::fs::{self, File, OpenOptions};
 use std::io::{self, BufRead, BufReader, Seek, SeekFrom, Write};
 use std::path::PathBuf;

-const DATA_FILE_EXTENSION: &str = "json";
+const DATA_FILE_EXTENSION: &str = "dat";
 const DATA_FILE_RECORD_SEPARATOR: u8 = 147;

 pub struct Config {
@@ -26,8 +26,9 @@ impl Database {

             if path.extension().and_then(OsStr::to_str) != Some(DATA_FILE_EXTENSION) {
                 return Err(Self::error(format!(
-                    "invalid data file {}: extension must be `json`",
-                    path.display()
+                    "invalid data file {}: extension must be `{}`",
+                    path.display(),
+                    DATA_FILE_EXTENSION
                 )));
             }

@@ -39,14 +40,14 @@ impl Database {
                 )));
             }

-            let key = path.file_stem().ok_or_else(|| {
+            let key_hash = path.file_stem().ok_or_else(|| {
                 Self::error(format!(
                     "invalid data file name {}: empty file stem",
                     path.display()
                 ))
             })?;

-            let key = key.to_str().ok_or_else(|| {
+            let key_hash = key_hash.to_str().ok_or_else(|| {
                 Self::error(format!(
                     "invalid data file name {}: non-utf8 file name",
                     path.display()
@@ -55,7 +56,7 @@ impl Database {

             let file = OpenOptions::new().append(true).read(true).open(&path)?;

-            files.insert(key.to_string(), file);
+            files.insert(key_hash.to_string(), file);
         }
         Ok(Database {
             data_directory: config.data_directory,
@@ -64,7 +65,7 @@ impl Database {
     }

     pub fn read(&self, key: &str) -> io::Result<Option<Vec<String>>> {
-        let mut file = match self.files.get(key) {
+        let mut file = match self.files.get(&Self::hash(key)) {
             Some(file) => file,
             None => return Ok(None),
         };
@@ -95,11 +96,12 @@ impl Database {
     }

     pub fn write(&mut self, key: &str, line: &str) -> io::Result<()> {
-        let (mut file, needs_delimeter) = match self.files.get(key) {
+        let key_hash = Self::hash(key);
+        let (file, needs_delimeter) = match self.files.get_mut(&key_hash) {
             Some(file) => (file, true),
             None => {
                 let mut path = self.data_directory.clone();
-                path.push(key);
+                path.push(&key_hash);
                 path.set_extension(DATA_FILE_EXTENSION);

                 let file = OpenOptions::new()
@@ -108,8 +110,12 @@ impl Database {
                     .read(true)
                     .open(&path)?;

-                self.files.insert(key.to_string(), file);
-                (&self.files[key], false)
+                // Using `.or_insert` here is annoying since we know there is no entry, but
+                // `hash_map::entry::insert` is unstable
+                // ([#65225](https://github.com/rust-lang/rust/issues/65225)).
+                let file = self.files.entry(key_hash).or_insert(file);
+
+                (file, false)
             }
         };

@@ -121,6 +127,11 @@ impl Database {
         Ok(())
     }

+    fn hash(key: &str) -> String {
+        let digest = md5::compute(&key);
+        format!("{:x}", digest)
+    }
+
     fn error(message: String) -> io::Error {
         io::Error::new(io::ErrorKind::Other, message)
     }
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -13,6 +13,7 @@ log = "0.4.11"
 tide = "0.15.0"
 async-std = { version = "1.7.0", features = ["attributes"] }
 blocking = "1.0.2"
+md5 = "0.7.0"

 [dev-dependencies]
 tempfile = "3.1.0"
```

We have added the [md5](https://crates.io/crates/md5) crate which we're now using to compute a safe file name from the arbitrary strings passed to `Database::write`.
We've also changed the file extension from `.json` to `.dat`, since the files are not JSON and this will make it more obvious if file are not where they're supposed to be...

Let's give it another run:

```
$ make writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-14T18:02:43Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-14T18:02:43Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-14T18:02:43Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
```

This looks much better!
Now let's see if we can retrieve our logs from the API...

```
# in another tab
$ curl -i 127.0.0.1:8000/logs//var/log/containers/writer.log
HTTP/1.1 404 Not Found
```

Hrm, maybe we should percent-encode our key?

```
$ curl -i 127.0.0.1:8000/logs/%2Fvar%2Flog%2Fcontainers%2Fwriter.log
HTTP/1.1 404 Not Found
```

What now?
Let's add some logging for incoming `/logs/:key` requests:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -17,6 +17,7 @@ pub fn server(database: State) -> Server {

 async fn read_logs(req: tide::Request<State>) -> tide::Result {
     let key = req.param("key")?;
+    debug!("read_logs({})", key);
     let database = req.state().read().await;

     Ok(match database.read(key)? {
```

If we now re-run everything and hit the server and see what's logged:

```
$ make down writer monitoring
...

# in another tab
$ curl -i 127.0.0.1:8000/logs//var/log/containers/writer.log
```

With this we don't see anything in our logs (apart from the steady beat of `Append /var/log/containers/writer.log`).
Let's try percent-encoded:

```
$ curl -i 127.0.0.1:8000/logs/%2Fvar%2Flog%2Fcontainers%2Fwriter.log

# in our monitoring tab
monitoring_1  | [2020-12-14T18:56:02Z DEBUG monitoring_rs::api] read_logs(%2Fvar%2Flog%2Fcontainers%2Fwriter.log)
```

Aha!
So tide does not decode our percent-encoded path.
We could add percent-decoding, but let's instead just change our routing to allow multiple path segments in `key`:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -11,7 +11,7 @@ pub type Server = tide::Server<State>;

 pub fn server(database: State) -> Server {
     let mut app = tide::Server::with_state(database);
-    app.at("/logs/:key").get(read_logs);
+    app.at("/logs/*key").get(read_logs);
     app
 }

```

Let's give it a (hopefully) final spin:

```
$ make down writer monitoring
...

# in another tab
$ curl -i 127.0.0.1:8000/logs/var/log/containers/writer.log
HTTP/1.1 404 Not Found

$ curl -i 127.0.0.1:8000/logs//var/log/containers/writer.log
HTTP/1.1 200 OK
...
["Mon Dec 14 19:05:24 UTC 2020","Mon Dec 14 19:05:25 UTC 2020"]
```

After a final trip-up (we need the `//` to ensure our `key` has a leading `/`), it works! 🎉

#### Tidy up

For now, let's remove the logging from `api::read_logs`:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -17,7 +17,6 @@ pub fn server(database: State) -> Server {

 async fn read_logs(req: tide::Request<State>) -> tide::Result {
     let key = req.param("key")?;
-    debug!("read_logs({})", key);
     let database = req.state().read().await;

     Ok(match database.read(key)? {
```

## Recap

Dayum – this got pretty long.
However, we now have an end-to-end (albeit somewhat horrendous) implementation of a log collector!
Who thought it would get this far?

Next up we're probably due some consolidation of what we have done, including some refactoring, additional tests, and some build optimisations (`make monitoring` is now taking \~10mins due to all the dependencies).

[Back to the README](../README.md#posts)
