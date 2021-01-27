# Log collection (part 11 – Kubernetes collector)

Let's take a stab at adding a `log_collector::Collector` implementation for Kubernetes logs and metadata.

## Configuration

Our binary is currently hard-coded to run the `directory` collector, and moreover to collect from the `/var/log/containers` directory.
Let's work towards allowing this to be configured via CLI flags and/or environment variables, starting with introducing a `Config` struct for `directory`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -11,6 +11,12 @@ use crate::LogEntry;

 use super::watcher::{self, watcher, Watcher};

+/// Configuration for [`initialize`].
+pub struct Config {
+    /// The root path from which to collect logs.
+    pub root_path: PathBuf,
+}
+
 #[derive(Debug)]
 enum Event<'collector> {
     Create { path: PathBuf },
@@ -57,21 +63,28 @@ struct Collector<W: Watcher> {
     entry_buf: std::vec::IntoIter<LogEntry>,
 }

+/// Initialize a `Collector` that watches a directory of log files.
+///
+/// This will start a watch (using `inotify` or `kqueue`) on `config.root_path` and any files
+/// therein. Whenever the files change, new lines are emitted as `LogEntry` records.
+///
 /// # Errors
 ///
 /// Propagates any `io::Error`s that occur during initialization.
-pub fn initialize(root_path: &Path) -> io::Result<impl super::Collector> {
+pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
     let watcher = watcher()?;
-    Collector::initialize(root_path, watcher)
+    Collector::initialize(config, watcher)
 }

 impl<W: Watcher> Collector<W> {
-    fn initialize(root_path: &Path, mut watcher: W) -> io::Result<Self> {
+    fn initialize(config: Config, mut watcher: W) -> io::Result<Self> {
+        let Config { root_path } = config;
+
         debug!("Initialising watch on root path {:?}", root_path);
-        let root_wd = watcher.watch_directory(root_path)?;
+        let root_wd = watcher.watch_directory(&root_path)?;

         let mut collector = Self {
-            root_path: root_path.to_path_buf(),
+            root_path,
             root_wd,
             live_files: HashMap::new(),
             watched_files: HashMap::new(),
@@ -79,7 +92,7 @@ impl<W: Watcher> Collector<W> {
             entry_buf: vec![].into_iter(),
         };

-        for entry in fs::read_dir(root_path)? {
+        for entry in fs::read_dir(&collector.root_path)? {
             let entry = entry?;
             let path = fs::canonicalize(entry.path())?;

@@ -237,12 +250,15 @@ mod tests {
     use crate::log_collector::watcher::watcher;
     use crate::test::{self, log_entry};

-    use super::Collector;
+    use super::{Collector, Config};

     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
-        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+        let config = Config {
+            root_path: tempdir.path().to_path_buf(),
+        };
+        let mut collector = Collector::initialize(config, watcher()?)?;

         create_log_file(&tempdir)?;

@@ -256,7 +272,10 @@ mod tests {
     #[test]
     fn collect_entries_nonempty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
-        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+        let config = Config {
+            root_path: tempdir.path().to_path_buf(),
+        };
+        let mut collector = Collector::initialize(config, watcher()?)?;

         let (file_path, mut file) = create_log_file(&tempdir)?;

@@ -280,7 +299,10 @@ mod tests {
     #[test]
     fn iterator_yields_entries() -> test::Result {
         let tempdir = tempfile::tempdir()?;
-        let mut collector = Collector::initialize(tempdir.path(), watcher()?)?;
+        let config = Config {
+            root_path: tempdir.path().to_path_buf(),
+        };
+        let mut collector = Collector::initialize(config, watcher()?)?;

         let (file_path, mut file) = create_log_file(&tempdir)?;

```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -2,7 +2,6 @@
 use std::env;
 use std::fs;
 use std::io;
-use std::path::Path;
 use std::sync::Arc;

 use async_std::prelude::FutureExt;
@@ -24,14 +23,18 @@ async fn main() -> io::Result<()> {
             env::VarError::NotPresent => Ok(DEFAULT_CONTAINER_LOG_DIRECTORY.to_string()),
             error => Err(error),
         })
-        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;
+        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?
+        .into();
+    let collector_config = log_collector::directory::Config {
+        root_path: container_log_directory,
+    };

     let database = init_database()?;

     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

     let collector_handle = task::spawn(blocking::unblock(move || {
-        init_collector(container_log_directory.as_ref(), database)
+        init_collector(collector_config, database)
     }));

     api_handle.try_join(collector_handle).await?;
@@ -50,10 +53,10 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
 }

 fn init_collector(
-    container_log_directory: &Path,
+    collector_config: log_collector::directory::Config,
     database: Arc<RwLock<Database>>,
 ) -> io::Result<()> {
-    let collector = log_collector::directory::initialize(container_log_directory)?;
+    let collector = log_collector::directory::initialize(collector_config)?;
     for entry in collector {
         let entry = entry?;
         let mut database = task::block_on(database.write());
```

Not the most ergonomic API, but it'll do for now.

Next we'll think about the CLI interface.
Let's go ahead and add `structopt` to our dependencies, which will let us define our CLI arguments as a struct.

```
$ cargo add structopt
...
      Adding structopt v0.3.21 to dependencies
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -22,6 +22,7 @@ async-std = { version = "1.7.0", features = ["attributes"] }
 blocking = "1.0.2"
 md5 = "0.7.0"
 serde_json = "1.0.61"
+structopt = "0.3.21"

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

We can now create a stub `Args` struct in `main.rs` which will already get us some basic CLI chrome:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -7,6 +7,7 @@ use std::sync::Arc;
 use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
 use async_std::task;
+use structopt::StructOpt;

 use monitoring_rs::log_database::{self, Database};
 use monitoring_rs::{api, log_collector};
@@ -14,10 +15,15 @@ use monitoring_rs::{api, log_collector};
 const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
 const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

+#[derive(StructOpt)]
+struct Args {}
+
 #[async_std::main]
 async fn main() -> io::Result<()> {
     env_logger::init();

+    let _args = Args::from_args();
+
     let container_log_directory = env::var(VAR_CONTAINER_LOG_DIRECTORY)
         .or_else(|error| match error {
             env::VarError::NotPresent => Ok(DEFAULT_CONTAINER_LOG_DIRECTORY.to_string()),
```

```
$ cargo run -- --help
monitoring-rs 0.1.0

USAGE:
    monitoring-rs

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

Let's add a little bit of a description:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -15,6 +15,7 @@ use monitoring_rs::{api, log_collector};
 const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
 const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

+/// Minimal Kubernetes monitoring pipeline.
 #[derive(StructOpt)]
 struct Args {}

```

```
$ cargo run -- --help
monitoring-rs 0.1.0
Minimal Kubernetes monitoring pipeline

USAGE:
    monitoring-rs

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information
```

Great.
Now we need to add an argument to specify a collector and the root path:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -2,6 +2,7 @@
 use std::env;
 use std::fs;
 use std::io;
+use std::path::PathBuf;
 use std::sync::Arc;

 use async_std::prelude::FutureExt;
@@ -12,28 +13,44 @@ use structopt::StructOpt;
 use monitoring_rs::log_database::{self, Database};
 use monitoring_rs::{api, log_collector};

-const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
-const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
-
 /// Minimal Kubernetes monitoring pipeline.
 #[derive(StructOpt)]
-struct Args {}
+struct Args {
+    /// The log collector to use.
+    #[structopt(long, env)]
+    log_collector: CollectorArg,
+
+    /// The root path to watch.
+    #[structopt(long, env, required_if("log-collector", "directory"))]
+    root_path: Option<PathBuf>,
+}
+
+enum CollectorArg {
+    Directory,
+}
+
+impl std::str::FromStr for CollectorArg {
+    type Err = &'static str;
+
+    fn from_str(s: &str) -> Result<Self, Self::Err> {
+        match s {
+            "directory" => Ok(CollectorArg::Directory),
+            _ => Err("must be one of: directory"),
+        }
+    }
+}

 #[async_std::main]
 async fn main() -> io::Result<()> {
     env_logger::init();

-    let _args = Args::from_args();
+    let args = Args::from_args();

-    let container_log_directory = env::var(VAR_CONTAINER_LOG_DIRECTORY)
-        .or_else(|error| match error {
-            env::VarError::NotPresent => Ok(DEFAULT_CONTAINER_LOG_DIRECTORY.to_string()),
-            error => Err(error),
-        })
-        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?
-        .into();
-    let collector_config = log_collector::directory::Config {
-        root_path: container_log_directory,
+    let collector_config = match args.log_collector {
+        CollectorArg::Directory => log_collector::directory::Config {
+            // We can `unwrap` because we expect presence to be validated by structopt.
+            root_path: args.root_path.unwrap(),
+        },
     };

     let database = init_database()?;
```

```
$ cargo run -- --help
monitoring-rs 0.1.0
Minimal Kubernetes monitoring pipeline

USAGE:
    monitoring-rs [OPTIONS] --log-collector <log-collector>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --log-collector <log-collector>    The log collector to use
        --root-path <root-path>            The root path to watch

$ cargo run -- --log-collector hello
error: Invalid value for '--log-collector <log-collector>': must be one of: directory

$ cargo run -- --log-collector directory
error: The following required arguments were not provided:
    --root-path <root-path>

USAGE:
    monitoring-rs --log-collector <log-collector> --root-path <root-path>

For more information try --help

$ cargo run -- --log-collector directory --root-path .logs
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

There's slightly more boilerplate in our implementation than is ideal.
It seems that there isn't a great solution to this kind of configuration (turning on/off arguments depending on an `enum` 'selector' argument).
We can cut down on boilerplate slightly, and improve our help output, by depending directly on `clap` and using the [`arg_enum`](https://docs.rs/clap/%5E2.33.3/clap/macro.arg_enum.html) macro.

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -23,6 +23,7 @@ blocking = "1.0.2"
 md5 = "0.7.0"
 serde_json = "1.0.61"
 structopt = "0.3.21"
+clap = "2.33.3"

 [target.'cfg(target_os = "linux")'.dependencies]
 inotify = { version = "0.8.3", default-features = false }
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,4 +1,7 @@
 // main.rs
+#[macro_use]
+extern crate clap;
+
 use std::env;
 use std::fs;
 use std::io;
@@ -17,26 +20,17 @@ use monitoring_rs::{api, log_collector};
 #[derive(StructOpt)]
 struct Args {
     /// The log collector to use.
-    #[structopt(long, env)]
+    #[structopt(long, env, possible_values = &CollectorArg::variants())]
     log_collector: CollectorArg,

     /// The root path to watch.
-    #[structopt(long, env, required_if("log-collector", "directory"))]
+    #[structopt(long, env, required_if("log-collector", "Directory"))]
     root_path: Option<PathBuf>,
 }

-enum CollectorArg {
-    Directory,
-}
-
-impl std::str::FromStr for CollectorArg {
-    type Err = &'static str;
-
-    fn from_str(s: &str) -> Result<Self, Self::Err> {
-        match s {
-            "directory" => Ok(CollectorArg::Directory),
-            _ => Err("must be one of: directory"),
-        }
+arg_enum! {
+    enum CollectorArg {
+        Directory,
     }
 }

```

Now we get:

```
$ cargo run -- --help
monitoring-rs 0.1.0
Minimal Kubernetes monitoring pipeline

USAGE:
    monitoring-rs [OPTIONS] --log-collector <log-collector>

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
        --log-collector <log-collector>    The log collector to use [env: LOG_COLLECTOR=]  [possible values: Directory]
        --root-path <root-path>            The root path to watch [env: ROOT_PATH=]

$ cargo run -- --log-collctor directory
error: 'directory' isn't a valid value for '--log-collector <log-collector>'
  [possible values: Directory]

  Did you mean 'Directory'?

USAGE:
    monitoring-rs [OPTIONS] --log-collector <log-collector>

For more information try --help

$ cargo run -- --log-collector Directory
error: The following required arguments were not provided:
    --root-path <root-path>

USAGE:
    monitoring-rs --log-collector <log-collector> --root-path <root-path>

For more information try --help

$ cargo run -- --log-collector Directory --root-path .logs
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

Fantastisch.
We should also update our `docker-compose` and `Makefile` to pass in the appropriate root path:

```diff
--- a/docker-compose.yaml
+++ b/docker-compose.yaml
@@ -8,6 +8,7 @@ services:
     - logs:/var/log/containers
     environment:
     - RUST_LOG=monitoring_rs=debug
+    - ROOT_PATH=/var/log/containers
     ports:
     - 8000:8000

```

```diff
--- a/Makefile
+++ b/Makefile
@@ -36,6 +36,7 @@ kuberun: push
  @kubectl run monitoring-rs \
      --image $(DOCKER_IMAGE) \
      --env RUST_LOG=monitoring_rs=info \
+     --env ROOT_PATH=/var/log/containers \
      --restart Never \
      --dry-run=client \
      --output json \
```

## `log_collector::kubernetes`

Let's stub out a `log_collector::kubernetes` module, to the point where we can get an `unimplemented` panic from `cargo run`:

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

```rust
// src/log_collector/kubernetes.rs
//! A log collector that watches log files on a Kubernetes node.
use std::io;

use crate::LogEntry;

/// # Errors
///
/// Propagates any `io::Error` encountered during initialization.
pub fn initialize() -> io::Result<impl super::Collector> {
    Collector::initialize()
}

struct Collector;

impl Collector {
    fn initialize() -> io::Result<Self> {
        unimplemented!()
    }
}

impl super::Collector for Collector {}

impl Iterator for Collector {
    type Item = io::Result<LogEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        unimplemented!()
    }
}
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -13,6 +13,7 @@ use async_std::sync::RwLock;
 use async_std::task;
 use structopt::StructOpt;

+use monitoring_rs::log_collector::Collector;
 use monitoring_rs::log_database::{self, Database};
 use monitoring_rs::{api, log_collector};

@@ -31,6 +32,7 @@ struct Args {
 arg_enum! {
     enum CollectorArg {
         Directory,
+        Kubernetes,
     }
 }

@@ -40,11 +42,18 @@ async fn main() -> io::Result<()> {

     let args = Args::from_args();

-    let collector_config = match args.log_collector {
-        CollectorArg::Directory => log_collector::directory::Config {
-            // We can `unwrap` because we expect presence to be validated by structopt.
-            root_path: args.root_path.unwrap(),
-        },
+    let collector: Box<dyn Collector + Send> = match args.log_collector {
+        CollectorArg::Directory => {
+            use log_collector::directory::{self, Config};
+            Box::new(directory::initialize(Config {
+                // We can `unwrap` because we expect presence to be validated by structopt.
+                root_path: args.root_path.unwrap(),
+            })?)
+        }
+        CollectorArg::Kubernetes => {
+            use log_collector::kubernetes;
+            Box::new(kubernetes::initialize()?)
+        }
     };

     let database = init_database()?;
@@ -52,7 +61,7 @@ async fn main() -> io::Result<()> {
     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

     let collector_handle = task::spawn(blocking::unblock(move || {
-        init_collector(collector_config, database)
+        run_collector(collector, database)
     }));

     api_handle.try_join(collector_handle).await?;
@@ -70,11 +79,7 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     Ok(Arc::new(RwLock::new(database)))
 }

-fn init_collector(
-    collector_config: log_collector::directory::Config,
-    database: Arc<RwLock<Database>>,
-) -> io::Result<()> {
-    let collector = log_collector::directory::initialize(collector_config)?;
+fn run_collector(collector: Box<dyn Collector>, database: Arc<RwLock<Database>>) -> io::Result<()> {
     for entry in collector {
         let entry = entry?;
         let mut database = task::block_on(database.write());
```

```
$ cargo run -- --log-collector Kubernetes
thread 'main' panicked at 'not implemented', src/log_collector/kubernetes.rs:18:9
```

Let's also tidy up the collector initialization into a function:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -42,19 +42,7 @@ async fn main() -> io::Result<()> {

     let args = Args::from_args();

-    let collector: Box<dyn Collector + Send> = match args.log_collector {
-        CollectorArg::Directory => {
-            use log_collector::directory::{self, Config};
-            Box::new(directory::initialize(Config {
-                // We can `unwrap` because we expect presence to be validated by structopt.
-                root_path: args.root_path.unwrap(),
-            })?)
-        }
-        CollectorArg::Kubernetes => {
-            use log_collector::kubernetes;
-            Box::new(kubernetes::initialize()?)
-        }
-    };
+    let collector = init_collector(args)?;

     let database = init_database()?;

@@ -79,6 +67,22 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     Ok(Arc::new(RwLock::new(database)))
 }

+fn init_collector(args: Args) -> io::Result<Box<dyn Collector + Send>> {
+    match args.log_collector {
+        CollectorArg::Directory => {
+            use log_collector::directory::{self, Config};
+            Ok(Box::new(directory::initialize(Config {
+                // We can `unwrap` because we expect presence to be validated by structopt.
+                root_path: args.root_path.unwrap(),
+            })?))
+        }
+        CollectorArg::Kubernetes => {
+            use log_collector::kubernetes;
+            Ok(Box::new(kubernetes::initialize()?))
+        }
+    }
+}
+
 fn run_collector(collector: Box<dyn Collector>, database: Arc<RwLock<Database>>) -> io::Result<()> {
     for entry in collector {
         let entry = entry?;
```

That's a bit better.

### `log_collector::kubernetes::initialize`

If we think about how `log_collector::kubernetes` should work, we could think of it as a variation of `log_collector::directory` with the following differences:

- The `root_path` should default to the Kubernetes node log path (`/var/log/containers`).
- Discovered log files should have a specific naming convention (`<pod name>_<namespace>_<container name>-<container ID>.log`), or be rejected.
- Log file names should be parsed, and the Kubernetes API should be used to query for metadata to attach to log entries.
- Configuration should allow specifying how to connect to the Kubernetes API.

Let's start with something trivial – copy & pasting the `log_collector::directory` module (this will overwrite our stub module above):

```
$ cp src/log_collector/directory.rs src/log_collector/kubernetes.rs
```

Now we should at least update the documentation:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -1,4 +1,4 @@
-//! A log collector that watches a directory of log files.
+//! A log collector that watches log files on a Kubernetes node.

 use std::collections::HashMap;
 use std::fs::{self, File};
@@ -63,7 +63,7 @@ struct Collector<W: Watcher> {
     entry_buf: std::vec::IntoIter<LogEntry>,
 }

-/// Initialize a `Collector` that watches a directory of log files.
+/// Initialize a `Collector` that watches log files on a Kubernetes node.
 ///
 /// This will start a watch (using `inotify` or `kqueue`) on `config.root_path` and any files
 /// therein. Whenever the files change, new lines are emitted as `LogEntry` records.
```

Now let's just go through our requirements and see how we get on.

### Default `root_path`

Let's make `Config`'s `root_path` optional:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -11,10 +11,14 @@ use crate::LogEntry;

 use super::watcher::{self, watcher, Watcher};

+const DEFAULT_ROOT_PATH: &str = "/var/log/containers";
+
 /// Configuration for [`initialize`].
 pub struct Config {
     /// The root path from which to collect logs.
-    pub root_path: PathBuf,
+    ///
+    /// Defaults to `/var/log/containers` if `None`.
+    pub root_path: Option<PathBuf>,
 }

 #[derive(Debug)]
@@ -78,7 +82,9 @@ pub fn initialize(config: Config) -> io::Result<impl super::Collector> {

 impl<W: Watcher> Collector<W> {
     fn initialize(config: Config, mut watcher: W) -> io::Result<Self> {
-        let Config { root_path } = config;
+        let root_path = config
+            .root_path
+            .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT_PATH));

         debug!("Initialising watch on root path {:?}", root_path);
         let root_wd = watcher.watch_directory(&root_path)?;
@@ -256,7 +262,7 @@ mod tests {
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let config = Config {
-            root_path: tempdir.path().to_path_buf(),
+            root_path: Some(tempdir.path().to_path_buf()),
         };
         let mut collector = Collector::initialize(config, watcher()?)?;

@@ -273,7 +279,7 @@ mod tests {
     fn collect_entries_nonempty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let config = Config {
-            root_path: tempdir.path().to_path_buf(),
+            root_path: Some(tempdir.path().to_path_buf()),
         };
         let mut collector = Collector::initialize(config, watcher()?)?;

@@ -300,7 +306,7 @@ mod tests {
     fn iterator_yields_entries() -> test::Result {
         let tempdir = tempfile::tempdir()?;
         let config = Config {
-            root_path: tempdir.path().to_path_buf(),
+            root_path: Some(tempdir.path().to_path_buf()),
         };
         let mut collector = Collector::initialize(config, watcher()?)?;

```

Now we should also update `main` to supply the appropriate config:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -77,8 +77,10 @@ fn init_collector(args: Args) -> io::Result<Box<dyn Collector + Send>> {
             })?))
         }
         CollectorArg::Kubernetes => {
-            use log_collector::kubernetes;
-            Ok(Box::new(kubernetes::initialize()?))
+            use log_collector::kubernetes::{self, Config};
+            Ok(Box::new(kubernetes::initialize(Config {
+                root_path: args.root_path,
+            })?))
         }
     }
 }
```

Let's also double check our copied tests pass:

```
$ cargo test
...
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

All good.

### Log file naming convention

Let's look at `kubernetes::Collector::handle_event_create`:

```rust
fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
    let wd = self.watcher.watch_file(&path)?;
    let mut reader = BufReader::new(File::open(&path)?);
    reader.seek(io::SeekFrom::End(0))?;

    self.live_files.insert(
        wd.clone(),
        LiveFile {
            path: path.clone(),
            reader,
            entry_buf: String::new(),
        },
    );
    self.watched_files.insert(path, wd.clone());
    Ok(self.live_files.get_mut(&wd).unwrap())
}
```

It would be nice to validate `path` at this point, since it will always be called to add to the `live_files` map.
However, if we look at the call-sites:

```rust
for entry in fs::read_dir(&collector.root_path)? {
    let entry = entry?;
    let path = fs::canonicalize(entry.path())?;
    ...
    collector.handle_event_create(path)?;
}
```

And:

```rust
let path = fs::canonicalize(entry.path())?;

if !self.watched_files.contains_key(&path) {
    events.push(Event::Create { path });
}
...
for event in self.check_event(&watcher_event)? {
    ...
    match event {
        Event::Create { path } => {
            new_paths.push(path);
            ...
        }
    }
    ...
}
...
for path in new_paths {
    let live_file = self.handle_event_create(path)?;
    ...
}
```

We can see that the `path` given to `handle_event_create` will always be canonicalized, which will resolve symlinks and normalize paths to their underlying `/var/lib/docker/containers` form.
This only includes the container ID, which is not sufficient information to find the host pod (without iterating every pod in every namespace – no thank you).
Although the Kubernetes list API supports a 'field selector', only a limited set of fields are supported (which is only documented [in code](https://github.com/kubernetes/kubernetes/blob/v1.18.15/pkg/apis/core/v1/conversion.go#L34-L59)).

So, we need to rework this logic slightly.
Ideally, `handle_event_create` should be called with a path nested in `root_path` (`/var/log/containers`), and be canonicalized therein, if necessary.

For starters, let's remind ourselves *why* we canonicalize.
We skimmed over this in [Log collection (part 4 – minikube)](05-log-collection-part-4-minikube.md), where we implemented the original `handle_event_create` as:

```rust
fn handle_event_create(&mut self, path: PathBuf) -> io::Result<()> {
    let realpath = fs::canonicalize(&path)?;

    let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
    let mut file = File::open(realpath)?;
    file.seek(io::SeekFrom::End(0))?;

    self.live_files.insert(wd, LiveFile { path, file });

    Ok(())
}
```

This was necessary because `inotify.add_watch` (or indeed the underlying `inotify_add_watch` syscall) performs no symlink resolution – so adding a watch on a symlink would watch the symlink itself, no the target file.

So – we *do* need to call `fs::canonicalize` before we call `Watcher::watch_file`.
Thankfully this call is also in `handle_event_create`, so this seems like a good place to start:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -100,10 +100,14 @@ impl<W: Watcher> Collector<W> {

         for entry in fs::read_dir(&collector.root_path)? {
             let entry = entry?;
-            let path = fs::canonicalize(entry.path())?;

-            debug!("{}", Event::Create { path: path.clone() });
-            collector.handle_event_create(path)?;
+            debug!(
+                "{}",
+                Event::Create {
+                    path: entry.path().to_path_buf()
+                }
+            );
+            collector.handle_event_create(&entry.path())?;
         }

         Ok(collector)
@@ -158,7 +162,7 @@ impl<W: Watcher> Collector<W> {
             }

             for path in new_paths {
-                let live_file = self.handle_event_create(path)?;
+                let live_file = self.handle_event_create(&path)?;
                 read_file(live_file)?;
             }
         }
@@ -175,7 +179,9 @@ impl<W: Watcher> Collector<W> {
                 let path = fs::canonicalize(entry.path())?;

                 if !self.watched_files.contains_key(&path) {
-                    events.push(Event::Create { path });
+                    events.push(Event::Create {
+                        path: entry.path().to_path_buf(),
+                    });
                 }
             }

@@ -203,7 +209,9 @@ impl<W: Watcher> Collector<W> {
         }
     }

-    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
+    fn handle_event_create(&mut self, path: &Path) -> io::Result<&mut LiveFile> {
+        let path = fs::canonicalize(path)?;
+
         let wd = self.watcher.watch_file(&path)?;
         let mut reader = BufReader::new(File::open(&path)?);
         reader.seek(io::SeekFrom::End(0))?;
```

This is a start, but it's not ideal.
In particular, we still call `fs::canonicalize` twice: once to insert into the `watched_files` map in `handle_event_create`, and again to read the `watched_files` map in `check_event`.

However, before we start fiddling with things to validate this behaviour, we're getting into quite subtle territory.
Let's add a test to ensure that symlinked log files are collected properly:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -257,7 +257,7 @@ impl<W: Watcher> Iterator for Collector<W> {
 mod tests {
     use std::fs::{self, File};
     use std::io::{self, Write};
-    use std::path::PathBuf;
+    use std::path::{Path, PathBuf};

     use tempfile::TempDir;

@@ -338,6 +338,40 @@ mod tests {
         Ok(())
     }

+    #[cfg(unix)]
+    #[test]
+    fn symlinked() -> test::Result {
+        use std::os::unix;
+
+        let root_tempdir = tempfile::tempdir()?;
+        let logs_tempdir = tempfile::tempdir()?;
+
+        let (src_path, mut src_file) = create_log_file(&logs_tempdir)?;
+        let sym_path: PathBuf = [root_tempdir.path(), Path::new("linked.log")]
+            .iter()
+            .collect();
+        unix::fs::symlink(&src_path, &sym_path)?;
+
+        let config = Config {
+            root_path: Some(root_tempdir.path().to_path_buf()),
+        };
+        let mut collector = Collector::initialize(config, watcher()?)?;
+
+        writeln!(src_file, "hello?")?;
+        writeln!(src_file, "world!")?;
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            entries,
+            vec![
+                log_entry("hello?", &[("path", src_path.to_str().unwrap())]),
+                log_entry("world!", &[("path", src_path.to_str().unwrap())]),
+            ]
+        );
+
+        Ok(())
+    }
+
     fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
         let mut path = fs::canonicalize(tempdir.path())?;
         path.push("test.log");
```

```
$ cargo test log_collector::kubernetes::tests::symlinked
...
running 1 test
test log_collector::kubernetes::tests::symlinked ... ok
...
```

OK, so right now the 'visible' path for log files is the resolved path (`src_path` in the test).
Let's update the test to expect the symlinked path (`sym_path`):

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -364,8 +364,8 @@ mod tests {
         assert_eq!(
             entries,
             vec![
-                log_entry("hello?", &[("path", src_path.to_str().unwrap())]),
-                log_entry("world!", &[("path", src_path.to_str().unwrap())]),
+                log_entry("hello?", &[("path", sym_path.to_str().unwrap())]),
+                log_entry("world!", &[("path", sym_path.to_str().unwrap())]),
             ]
         );

```

Now we get a failure:

```
$ cargo test log_collector::kubernetes::tests::symlinked
...
running 1 test
test log_collector::kubernetes::tests::symlinked ... FAILED

failures:

---- log_collector::kubernetes::tests::symlinked stdout ----
thread 'log_collector::kubernetes::tests::symlinked' panicked at 'assertion failed: `(left == right)`
  left: `[LogEntry { line: "hello?", metadata: {"path": "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpEjxBkm/test.log"} }, LogEntry { line: "world!", metadata: {"path": "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpEjxBkm/test.log"} }]`,
 right: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpQWslys/linked.log"} }, LogEntry { line: "world!", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpQWslys/linked.log"} }]`', src/log_collector/kubernetes.rs:364:9
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
...
```

Very clear (or not).
We can use a bit more of the `tempfile` API to get a more descriptive error:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -343,8 +343,8 @@ mod tests {
     fn symlinked() -> test::Result {
         use std::os::unix;

-        let root_tempdir = tempfile::tempdir()?;
-        let logs_tempdir = tempfile::tempdir()?;
+        let root_tempdir = tempfile::Builder::new().suffix("-root").tempdir()?;
+        let logs_tempdir = tempfile::Builder::new().suffix("-logs").tempdir()?;

         let (src_path, mut src_file) = create_log_file(&logs_tempdir)?;
         let sym_path: PathBuf = [root_tempdir.path(), Path::new("linked.log")]
```

Now we get the following test error:

```
$ cargo test log_collector::kubernetes::tests::symlinked
...
thread 'log_collector::kubernetes::tests::symlinked' panicked at 'assertion failed: `(left == right)`
  left: `[LogEntry { line: "hello?", metadata: {"path": "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpfNMKbA-logs/test.log"} }, LogEntry { line: "world!", metadata: {"path": "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpfNMKbA-logs/test.log"} }]`,
 right: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpon4lSe-root/linked.log"} }, LogEntry { line: "world!", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpon4lSe-root/linked.log"} }]`', src/log_collector/kubernetes.rs:364:9
...
```

Still not brilliant, but we can at least see `-logs` in the `left` (actual) value, and `-root` in the `right` (expected) value.
Now let's try and get it to pass:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -209,22 +209,23 @@ impl<W: Watcher> Collector<W> {
         }
     }

-    fn handle_event_create(&mut self, path: &Path) -> io::Result<&mut LiveFile> {
-        let path = fs::canonicalize(path)?;
+    fn handle_event_create(&mut self, orig_path: &Path) -> io::Result<&mut LiveFile> {
+        let canon_path = fs::canonicalize(orig_path)?;

-        let wd = self.watcher.watch_file(&path)?;
-        let mut reader = BufReader::new(File::open(&path)?);
+        let wd = self.watcher.watch_file(&canon_path)?;
+        let mut reader = BufReader::new(File::open(&canon_path)?);
         reader.seek(io::SeekFrom::End(0))?;

         self.live_files.insert(
             wd.clone(),
             LiveFile {
-                path: path.clone(),
+                path: orig_path.to_path_buf(),
                 reader,
                 entry_buf: String::new(),
             },
         );
-        self.watched_files.insert(path, wd.clone());
+        self.watched_files
+            .insert(orig_path.to_path_buf(), wd.clone());
         Ok(self.live_files.get_mut(&wd).unwrap())
     }

```

With this, our test passes:

```
$ cargo test log_collector::kubernetes::tests::symlinked
...
test log_collector::kubernetes::tests::symlinked ... ok
...
```

However, we've broken our other `kubernetes` tests:

```
$ cargo test log_collector::kubernetes
...
failures:
    log_collector::kubernetes::tests::collect_entries_nonempty_file
    log_collector::kubernetes::tests::iterator_yields_entries

test result: FAILED. 2 passed; 2 failed; 0 ignored; 0 measured; 10 filtered out
```

We can resolve this by removing the call to `fs::canonicalize` from the `create_log_file` helper:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -256,7 +256,7 @@ impl<W: Watcher> Iterator for Collector<W> {

 #[cfg(test)]
 mod tests {
-    use std::fs::{self, File};
+    use std::fs::File;
     use std::io::{self, Write};
     use std::path::{Path, PathBuf};

@@ -374,7 +374,7 @@ mod tests {
     }

     fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
-        let mut path = fs::canonicalize(tempdir.path())?;
+        let mut path = tempdir.path().to_path_buf();
         path.push("test.log");

         let file = File::create(&path)?;
```

```
$ cargo test log_collector::kubernetes
...
running 4 tests
test log_collector::kubernetes::tests::collect_entries_empty_file ... ok
test log_collector::kubernetes::tests::iterator_yields_entries ... ok
test log_collector::kubernetes::tests::collect_entries_nonempty_file ... ok
test log_collector::kubernetes::tests::symlinked ... ok
...
```

However, there may still be a problem (two problems really, since our tests don't detect it).
We are inserting `orig_path` (non-canonicalized) into `watched_files`, but later checking if the canonicalized path is contained.
In theory, this would cause every `watcher::Event` to be treated as a `kubernetes::Event::Create`.
We should be able to see this if we run one of our tests with logging and `nocapture` enabled:

```
$ RUST_LOG=monitoring_rs=trace cargo test log_collector::kubernetes::tests::collect_entries_nonempty_file -- --nocapture
...
running 1 test
test log_collector::kubernetes::tests::collect_entries_nonempty_file ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out
...
```

But no logs :(
This is probably because we're not initialising any logger in test – let's hack something in for now:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -286,6 +286,8 @@ mod tests {

     #[test]
     fn collect_entries_nonempty_file() -> test::Result {
+        env_logger::init();
+
         let tempdir = tempfile::tempdir()?;
         let config = Config {
             root_path: Some(tempdir.path().to_path_buf()),
```

```
$ RUST_LOG=monitoring_rs=trace cargo test log_collector::kubernetes::tests::collect_entries_nonempty_file -- --nocapture
...
running 1 test
[2021-01-24T15:44:38Z DEBUG monitoring_rs::log_collector::kubernetes] Initialising watch on root path "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpAs0wW1"
[2021-01-24T15:44:38Z TRACE monitoring_rs::log_collector::kubernetes] Received inotify event: Event { descriptor: Descriptor(4) }
[2021-01-24T15:44:38Z DEBUG monitoring_rs::log_collector::kubernetes] Create /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpAs0wW1/test.log
[2021-01-24T15:44:38Z TRACE monitoring_rs::log_collector::kubernetes] Received inotify event: Event { descriptor: Descriptor(6) }
[2021-01-24T15:44:38Z DEBUG monitoring_rs::log_collector::kubernetes] Append /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpAs0wW1/test.log
test log_collector::kubernetes::tests::collect_entries_nonempty_file ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 13 filtered out
...
```

Except... everything looks fine.
Let's add some more `debug!` logs to check our sanity:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -178,6 +178,7 @@ impl<W: Watcher> Collector<W> {
                 let entry = entry?;
                 let path = fs::canonicalize(entry.path())?;

+                debug!("watched_files.contains_key {:?}", &path);
                 if !self.watched_files.contains_key(&path) {
                     events.push(Event::Create {
                         path: entry.path().to_path_buf(),
@@ -224,6 +225,7 @@ impl<W: Watcher> Collector<W> {
                 entry_buf: String::new(),
             },
         );
+        debug!("watched_files.insert {:?}", orig_path);
         self.watched_files
             .insert(orig_path.to_path_buf(), wd.clone());
         Ok(self.live_files.get_mut(&wd).unwrap())
```

```
$ RUST_LOG=monitoring_rs=trace cargo test log_collector::kubernetes::tests::collect_entries_nonempty_file -- --nocapture
...
... watched_files.contains_key "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpSWyKkr/test.log"
...
... watched_files.insert "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpSWyKkr/test.log"

```

Interestingly, we only see `contains_key` once, which would occur the first time `check_event` is called after the first `inotify` event (corroborated by our logs).
On the second `inotify` event, we go recognise the event as `Append`, without looking at `watched_files`.
This actually makes sense – the `watched_files.contains_key` code path is within a `watcher_event.descriptor == self.root_wd` branch, which corresponds to `inotify` events from the root directory, which should only ever correspond to new files.

This seems pretty watertight if we look at the `inotify` definition of `watch_directory`:

```rust
fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
    let descriptor = self.inner.add_watch(path, WatchMask::CREATE)?;
    Ok(super::Descriptor(descriptor))
}
```

The `IN_CREATE` flag is described in [the `inotify_add_watch` documentation](http://refspecs.linux-foundation.org/LSB_4.0.0/LSB-Core-generic/LSB-Core-generic/baselib-inotify-add-watch.html) as:

> File or directory was created in a watched directory.

It's not quite as obviously true for the `kqueue` definition of `watch_directory`:

```rust
fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
    self.add_watch(path)
}
...
fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
    let file = File::open(path)?;
    let fd = file.into_raw_fd();
    self.inner
        .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
    self.inner.watch()?;
    Ok(super::Descriptor(fd))
}
```

This has no special handling for directories, so what does [the `kqueue` documentation](https://www.freebsd.org/cgi/man.cgi?query=kqueue&sektion=2) have to say about `EVFILT_VNODE`:

> Takes a file descriptor as the  identifier and the events to watch for in `fflags`, and returns when one or more of the requested events occurs on the descriptor.

And for the `NOTE_WRITE` `fflag`:

> A write occurred on the file referenced by the descriptor.

Nothing specifically mentioning directories.
In fact, if we `ctrl+f` for 'director' we see it's referenced in a note about renames in `NOTE_EXTEND` and symlinks in `NOTE_LINK`.
In fact, if we look back at [Log collection (part 8 – multi-platform)](09-log-collection-part-8-multi-platform.md#) we said:

> We're hoping that new directory entries will be treated as a 'write' against the directory's file descriptor.

And we've been coasting on that hope ever since, although it indeed seems to work as we expect.
For now, let's add some documentation notes in `watcher` to save us from this goose chase in future:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -19,10 +19,35 @@ pub(crate) trait Watcher {
     where
         Self: Sized;

+    /// Watch a directory for newly created files.
+    ///
+    /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever a file
+    /// is created in the directory at the given `path`. It is the caller's responsibility to ensure
+    /// that `path` points to a directory.
+    ///
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` caused when attempting to register the watch.
     fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor>;

+    /// Watch a file for writes.
+    ///
+    /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever the file
+    /// at the given `path` is written to. It is the caller's responsibility to ensure that `path`
+    /// points to a file. Note also that symlinks may not be followed.
+    ///
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` caused when attempting to register the watch.
     fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor>;

+    /// Read some events about the registered directories and files.
+    ///
+    /// This may block forever if no events have been registered, or if no events occur.
+    ///
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` caused when attempting to read events.
     fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
 }

@@ -95,9 +120,16 @@ mod imp {
         fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             let file = File::open(path)?;
             let fd = file.into_raw_fd();
+
+            // kqueue has quite limited fidelity for file watching – the best we can do for both
+            // files and directories is to register the `EVFILT_VNODE` and `NOTE_WRITE` flags, which
+            // is described as "A write occurred on the file referenced by the descriptor.".
+            // Observationally this seems to correspond with what we want: events for files created
+            // in watched directories, and writes to watched files.
             self.inner
                 .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
             self.inner.watch()?;
+
             Ok(super::Descriptor(fd))
         }
     }
```

Now that we've established these 'contracts' in `Watcher`, let's take another look at the relevant branch in `kubernetes::Collector::check_event`:

```rust
if watcher_event.descriptor == self.root_wd {
    let mut events = Vec::new();

    for entry in fs::read_dir(&self.root_path)? {
        let entry = entry?;
        let path = fs::canonicalize(entry.path())?;

        debug!("watched_files.contains_key {:?}", &path);
        if !self.watched_files.contains_key(&path) {
            events.push(Event::Create {
                path: entry.path().to_path_buf(),
            });
        }
    }

    return Ok(events);
}
```

So, we're actually using `watched_files` to determine which paths are already being watched.
The test we actually want, then, is one with two log files:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -261,6 +261,7 @@ mod tests {
     use std::fs::File;
     use std::io::{self, Write};
     use std::path::{Path, PathBuf};
+    use std::sync::atomic::{AtomicU8, Ordering};

     use tempfile::TempDir;

@@ -269,6 +270,8 @@ mod tests {

     use super::{Collector, Config};

+    static FILE_ID: AtomicU8 = AtomicU8::new(0);
+
     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
@@ -288,8 +291,6 @@ mod tests {

     #[test]
     fn collect_entries_nonempty_file() -> test::Result {
-        env_logger::init();
-
         let tempdir = tempfile::tempdir()?;
         let config = Config {
             root_path: Some(tempdir.path().to_path_buf()),
@@ -315,6 +316,37 @@ mod tests {
         Ok(())
     }

+    #[test]
+    fn collect_entries_multiple_files() -> test::Result {
+        env_logger::init();
+
+        let tempdir = tempfile::tempdir()?;
+        let config = Config {
+            root_path: Some(tempdir.path().to_path_buf()),
+        };
+        let mut collector = Collector::initialize(config, watcher()?)?;
+
+        let (file1_path, mut file1) = create_log_file(&tempdir)?;
+        collector.collect_entries()?;
+
+        let (file2_path, mut file2) = create_log_file(&tempdir)?;
+        collector.collect_entries()?;
+
+        writeln!(file1, "hello?")?;
+        writeln!(file2, "world!")?;
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            entries,
+            vec![
+                log_entry("hello?", &[("path", file1_path.to_str().unwrap())]),
+                log_entry("world!", &[("path", file2_path.to_str().unwrap())]),
+            ]
+        );
+
+        Ok(())
+    }
+
     #[test]
     fn iterator_yields_entries() -> test::Result {
         let tempdir = tempfile::tempdir()?;
@@ -378,8 +410,9 @@ mod tests {
     }

     fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
+        let file_id = FILE_ID.fetch_add(1, Ordering::SeqCst);
         let mut path = tempdir.path().to_path_buf();
-        path.push("test.log");
+        path.push(format!("test-{}.log", file_id));

         let file = File::create(&path)?;

```

Note that we've added a `static FILE_ID: AtomicU8`, which we use as an incrementing suffix in `create_log_file`.
Now if we run `collect_entries_multiple_files` with logs we get:

```
$ RUST_LOG=monitoring_rs=trace cargo test log_collector::kubernetes::tests::collect_entries_multiple_files -- --nocapture
[2021-01-24T16:59:29Z TRACE monitoring_rs::log_collector::kubernetes] Received inotify event: Event { descriptor: Descriptor(4) }
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.contains_key "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log"
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] Create /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.insert "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log"
[2021-01-24T16:59:29Z TRACE monitoring_rs::log_collector::kubernetes] Received inotify event: Event { descriptor: Descriptor(4) }
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.contains_key "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-1.log"
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.contains_key "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log"
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] Create /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-1.log
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] Create /var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.insert "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-1.log"
[2021-01-24T16:59:29Z DEBUG monitoring_rs::log_collector::kubernetes] watched_files.insert "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log"
```

This shows that we indeed get duplicate `Create` events for `test-0.log`.
Is there some way that we could observe this in the collector's behaviour, such that we can test for it?
Since we've called `watch_file` twice, we might hope to get duplicate `LogEntry`s for `test-0.log` in this case.
Sadly, this is difficult to test for since:

- Our `kqueue`-based watcher will only ever return a single event at a time.
- If there are no more events, the watcher will block forever.

This makes it impossible to test for '2 `LogEntry`s and no more', since attempting to verify 'no more' would block forever.
Let's introduce a new method on `Watcher` to get available events *without* blocking:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -41,6 +41,15 @@ pub(crate) trait Watcher {
     /// Propagates any `io::Error` caused when attempting to register the watch.
     fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor>;

+    /// Read some events about the registered directories and files.
+    ///
+    /// This will never block, and will just return an empty `Vec` if no events are ready.
+    ///
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` caused when attempting to read events.
+    fn read_events(&mut self) -> io::Result<Vec<Event>>;
+
     /// Read some events about the registered directories and files.
     ///
     /// This may block forever if no events have been registered, or if no events occur.
@@ -69,6 +78,16 @@ mod imp {
         buffer: [u8; INOTIFY_BUFFER_SIZE],
     }

+    impl Watcher {
+        fn map_events(inotify_events: inotify::Events) -> Vec<Event> {
+            inotify_events
+                .map(|event| Event {
+                    descriptor: super::Descriptor(event.wd),
+                })
+                .collect()
+        }
+    }
+
     impl super::Watcher for Watcher {
         fn new() -> io::Result<Self> {
             let inner = Inotify::init()?;
@@ -88,13 +107,14 @@ mod imp {
             Ok(super::Descriptor(descriptor))
         }

+        fn read_events(&mut self) -> io::Result<Vec<Event>> {
+            let inotify_events = self.inner.read_events(&mut self.buffer)?;
+            Ok(Self::map_events(inotify_events))
+        }
+
         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
             let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
-            let events = inotify_events.map(|event| Event {
-                descriptor: super::Descriptor(event.wd),
-            });
-
-            Ok(events.collect())
+            Ok(Self::map_events(inotify_events))
         }
     }
 }
@@ -105,8 +125,9 @@ mod imp {
     use std::io;
     use std::os::unix::io::{IntoRawFd, RawFd};
     use std::path::Path;
+    use std::time::Duration;

-    use kqueue::{EventData, EventFilter, FilterFlag, Ident, Vnode};
+    use kqueue::{self, EventData, EventFilter, FilterFlag, Ident, Vnode};

     use super::Event;

@@ -132,6 +153,22 @@ mod imp {

             Ok(super::Descriptor(fd))
         }
+
+        /// Map a [`kqueue::Event`] into an [`Event`](super::Event).
+        ///
+        /// # Panics
+        ///
+        /// This will panic if `kq_event` does not correspond to the filter passed in `add_watch`,
+        /// i.e. if it does not correspond to a file descriptor or it's not a write event.
+        fn map_event(kq_event: &kqueue::Event) -> Event {
+            let fd = match (&kq_event.ident, &kq_event.data) {
+                (&Ident::Fd(fd), &EventData::Vnode(Vnode::Write)) => fd,
+                _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
+            };
+            Event {
+                descriptor: super::Descriptor(fd),
+            }
+        }
     }

     impl super::Watcher for Watcher {
@@ -148,18 +185,16 @@ mod imp {
             self.add_watch(path)
         }

+        fn read_events(&mut self) -> io::Result<Vec<Event>> {
+            let kq_event = self.inner.poll(Some(Duration::new(0, 0)));
+            let event = kq_event.as_ref().map(Self::map_event);
+
+            Ok(event.into_iter().collect())
+        }
+
         fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
             let kq_event = self.inner.iter().next();
-
-            let event = kq_event.map(|kq_event| {
-                let fd = match (&kq_event.ident, &kq_event.data) {
-                    (&Ident::Fd(fd), &EventData::Vnode(Vnode::Write)) => fd,
-                    _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
-                };
-                Event {
-                    descriptor: super::Descriptor(fd),
-                }
-            });
+            let event = kq_event.as_ref().map(Self::map_event);

             Ok(event.into_iter().collect())
         }
```

We've done a little bit of refactoring to deduplicate transforming events from their underlying representation, but otherwise this is quite straightforward.
Now we need to add a non-blocking method to `kubernetes::Collector`:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -115,7 +115,15 @@ impl<W: Watcher> Collector<W> {

     fn collect_entries(&mut self) -> io::Result<Vec<LogEntry>> {
         let watcher_events = self.watcher.read_events_blocking()?;
+        self.handle_events(watcher_events)
+    }
+
+    fn collect_entries_nonblocking(&mut self) -> io::Result<Vec<LogEntry>> {
+        let watcher_events = self.watcher.read_events()?;
+        self.handle_events(watcher_events)
+    }

+    fn handle_events(&mut self, watcher_events: Vec<watcher::Event>) -> io::Result<Vec<LogEntry>> {
         let mut entries = Vec::new();
         let mut read_file = |live_file: &mut LiveFile| -> io::Result<()> {
             while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
```

Now let's update our tests to assert against all available events:

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -273,8 +273,9 @@ mod tests {

     use tempfile::TempDir;

-    use crate::log_collector::watcher::watcher;
+    use crate::log_collector::watcher::{watcher, Watcher};
     use crate::test::{self, log_entry};
+    use crate::LogEntry;

     use super::{Collector, Config};

@@ -343,7 +344,7 @@ mod tests {
         writeln!(file1, "hello?")?;
         writeln!(file2, "world!")?;

-        let entries = collector.collect_entries()?;
+        let entries = collect_all_entries(&mut collector)?;
         assert_eq!(
             entries,
             vec![
@@ -417,6 +418,17 @@ mod tests {
         Ok(())
     }

+    fn collect_all_entries<W: Watcher>(collector: &mut Collector<W>) -> io::Result<Vec<LogEntry>> {
+        let mut entries = collector.collect_entries_nonblocking()?;
+        loop {
+            let entries_ = collector.collect_entries_nonblocking()?;
+            if entries_.is_empty() {
+                break Ok(entries);
+            }
+            entries.extend(entries_);
+        }
+    }
+
     fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
         let file_id = FILE_ID.fetch_add(1, Ordering::SeqCst);
         let mut path = tempdir.path().to_path_buf();
```

Now we get the error we hoped for:

```
$ cargo test log_collector::kubernetes::tests::collect_entries_multiple_files
...
failures:

---- log_collector::kubernetes::tests::collect_entries_multiple_files stdout ----
thread 'log_collector::kubernetes::tests::collect_entries_multiple_files' panicked at 'assertion failed: `(left == right)`
  left: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpebK9sw/test-0.log"} }, LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpebK9sw/test-0.log"} }, LogEntry { line: "world!", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpebK9sw/test-1.log"} }]`,
 right: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpebK9sw/test-0.log"} }, LogEntry { line: "world!", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpebK9sw/test-1.log"} }]`', src/log_collector/kubernetes.rs:348:9
...
```

We can see that there are two entries for `hello?` in `test-0.log`.
Before we go into fixing this, let's see what happens in Docker:

```
$ make dockertest
...
test_1        | test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

Huh, so we don't get duplicates in Docker.
Let's see what [the `inotify_add_watch` documentation](https://man7.org/linux/man-pages/man2/inotify_add_watch.2.html) has to say about this:

> If the filesystem object was already being watched (perhaps via a different link to the same object), then the descriptor for the existing watch is returned.

So `inotify` doesn't create duplicate watches...
However, we would still see two `Create` events passing by, except that we've not done a good job of isolating specific situations in our tests.
Specifically our `collect_entries_multiple_files` in fact depends on both multiple files *and* symlinks to exhibit the behaviour we've been seeing, e.g.:

```
watched_files.insert "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-0.log"
watched_files.contains_key "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpWj5xv4/test-1.log"
```

It seems like all our `tempdir`s are symlinked on Mac.

### Normalising tests

We're getting a bit bogged down here, and furthermore all the changes we're making here are equally applicable to `log_collector::directory`.
Let's take a step back and think about our test coverage.
In particular, we'd like to get some more assurance on consistent behaviour of our `Watcher` implementations, as well as better coverage of different scenarios in our `Collector` implementations.
In the process, we might consider factoring out some behaviour between `directory::Collector` and `kubernetes::Collector` – although it's still not quite clear where we might need to diverge, so perhaps not.

#### `watcher`

We've encountered an interesting consideration for our `watcher`, which is the behaviour on duplicate calls to `watch_directory` or `watch_file` for a single path.
Right now, this is 'implementation defined' – `inotify` deduplicates whilst `kqueue` will duplicate the watch (and the events).
We have a couple of choices in what to do here:

- Leave it as is – this allows `Collector` implementations to handle normalising the behaviour in a way that's optimal for them.
  For example, the `Collector`s we've written so far already maintain a set of watched file paths, and so should be capable of deduplicating themselves without adding overhead to `watcher`.

- Mandate a specific behaviour in `Watcher` – this would require one of our implementations to add logic to either deduplicate watches for multiple paths, record duplicate watches and repeat them, or else catch duplicate watches and return an error (or panic).

Since we're not interested in duplicating behaviour, and we would expect `Collector` implementations to need to keep track of which files are being watched, the two options that would make the most sense are:

1. Leave it implementation defined, but note in comments that callers should not make duplicate calls.
1. Mandate duplicate detection, and panic on duplicate registrations.

Option 1. would have minimal cost (no additional tracking in `watcher`), but would put the burden of consistency on `Collector` implementations.
Option 2. would require both implementations to keep track of watched paths and/or registered watch descriptors, and panic when called with a duplicate.

Given this is all internal, we'll just continue to leave it implementation defined, but let's boost our comments a bit so we don't get caught out in future:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -22,8 +22,19 @@ pub(crate) trait Watcher {
     /// Watch a directory for newly created files.
     ///
     /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever a file
-    /// is created in the directory at the given `path`. It is the caller's responsibility to ensure
-    /// that `path` points to a directory.
+    /// is created in the directory at the given `path`.
+    ///
+    /// # Callee responsibilities
+    ///
+    /// It is the caller's responsibility to ensure that:
+    ///
+    /// - `path` points to a directory.
+    /// - `path` is canonical (e.g. implementations may not resolve symlinks, and may watch the
+    ///   symlink itself).
+    /// - `path` has not already been watched.
+    ///
+    /// The behaviour if any of these points are violated is implementation defined, and so specific
+    /// behaviour should not be relied upon.
     ///
     /// # Errors
     ///
@@ -33,8 +44,19 @@ pub(crate) trait Watcher {
     /// Watch a file for writes.
     ///
     /// Calling this function should cause the target `Watcher` to emit [`Event`]s whenever the file
-    /// at the given `path` is written to. It is the caller's responsibility to ensure that `path`
-    /// points to a file. Note also that symlinks may not be followed.
+    /// at the given `path` is written to.
+    ///
+    /// # Callee responsibilities
+    ///
+    /// It is the caller's responsibility to ensure that:
+    ///
+    /// - `path` points to a file.
+    /// - `path` is canonical (e.g. implementations may not resolve symlinks, and may watch the
+    ///   symlink itself).
+    /// - `path` has not already been watched.
+    ///
+    /// The behaviour if any of these points are violated is implementation defined, and so specific
+    /// behaviour should not be relied upon.
     ///
     /// # Errors
     ///
@@ -43,7 +65,7 @@ pub(crate) trait Watcher {

     /// Read some events about the registered directories and files.
     ///
-    /// This will never block, and will just return an empty `Vec` if no events are ready.
+    /// This must never block, and should just return an empty `Vec` if no events are ready.
     ///
     /// # Errors
     ///
@@ -97,13 +119,45 @@ mod imp {
             })
         }

+        /// Watch a directory for newly created files.
+        ///
+        /// # Callee responsibilities
+        ///
+        /// It is the caller's responsibility to ensure that:
+        ///
+        /// - `path` points to a directory.
+        /// - `path` is canonical (symlinks are not dereferenced).
+        /// - The inode behind `path` has not already been watched. `inotify` merges duplicate
+        ///   watches for the same path, and returns the `Descriptor` of the original watch.
+        ///
+        /// # Errors
+        ///
+        /// Propagates any `io::Error` caused when attempting to register the watch.
         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            let descriptor = self.inner.add_watch(path, WatchMask::CREATE)?;
+            let descriptor = self
+                .inner
+                .add_watch(path, WatchMask::CREATE | WatchMask::DONT_FOLLOW)?;
             Ok(super::Descriptor(descriptor))
         }

+        /// Watch a file for writes.
+        ///
+        /// # Callee responsibilities
+        ///
+        /// It is the caller's responsibility to ensure that:
+        ///
+        /// - `path` points to a file.
+        /// - `path` is canonical (symlinks are not dereferenced).
+        /// - The inode behind `path` has not already been watched. `inotify` merges duplicate
+        ///   watches for the same path, and returns the `Descriptor` of the original watch.
+        ///
+        /// # Errors
+        ///
+        /// Propagates any `io::Error` caused when attempting to register the watch.
         fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
-            let descriptor = self.inner.add_watch(path, WatchMask::MODIFY)?;
+            let descriptor = self
+                .inner
+                .add_watch(path, WatchMask::MODIFY | WatchMask::DONT_FOLLOW)?;
             Ok(super::Descriptor(descriptor))
         }

@@ -138,15 +192,29 @@ mod imp {
     }

     impl Watcher {
+        /// Watch a file for writes.
+        ///
+        /// `kqueue` has quite limited fidelity for file watching – the best we can do for both
+        /// files and directories is to register the `EVFILT_VNODE` and `NOTE_WRITE` flags, which is
+        /// described as "A write occurred on the file referenced by the descriptor.".
+        /// Observationally this seems to correspond with what we want: events for files created
+        /// in watched directories, and writes to watched files.
+        ///
+        /// # Callee responsibilities
+        ///
+        /// It is the caller's responsibility to ensure that:
+        ///
+        /// - `path` is canonical (symlinks are not dereferenced).
+        /// - The inode behind `path` has not already been watched. `kqueue` will happily register
+        ///   duplicate watches for the same path, and emit duplicate events.
+        ///
+        /// # Errors
+        ///
+        /// Propagates any `io::Error` caused when attempting to register the watch.
         fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             let file = File::open(path)?;
             let fd = file.into_raw_fd();

-            // kqueue has quite limited fidelity for file watching – the best we can do for both
-            // files and directories is to register the `EVFILT_VNODE` and `NOTE_WRITE` flags, which
-            // is described as "A write occurred on the file referenced by the descriptor.".
-            // Observationally this seems to correspond with what we want: events for files created
-            // in watched directories, and writes to watched files.
             self.inner
                 .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
             self.inner.watch()?;
@@ -177,10 +245,34 @@ mod imp {
             Ok(Watcher { inner })
         }

+        /// Watch a directory for newly created files.
+        ///
+        /// # Caller responsibilities
+        ///
+        /// It is the caller's responsibility to ensure that:
+        ///
+        /// - `path` points to a directory.
+        /// - See the notes on [`Watcher::add_watch`] for additional caveats.
+        ///
+        /// # Errors
+        ///
+        /// Propagates any `io::Error` caused when attempting to register the watch.
         fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             self.add_watch(path)
         }

+        /// Watch a file for writes.
+        ///
+        /// # Caller responsibilities
+        ///
+        /// It is the caller's responsibility to ensure that:
+        ///
+        /// - `path` points to a file.
+        /// - See the notes on [`Watcher::add_watch`] for additional caveats.
+        ///
+        /// # Errors
+        ///
+        /// Propagates any `io::Error` caused when attempting to register the watch.
         fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
             self.add_watch(path)
         }
```

That's a lot of repetition, but hopefully anyone arriving at the `watcher` source code will now have a chance to know what's expected for the trait, and how each implementation behaves.
For 'belt and braces', let's also add some comments to `watcher`, `watcher::watcher` and `watcher::Watcher` to really make sure we're signposting these caveats.

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -1,4 +1,13 @@
 // src/log_collector/watcher.rs
+//! Platform-agnostic file and directory watcher.
+//!
+//! The [`Watcher`] trait defines a platform-agnostic interface for a file watcher, and the
+//! [`watcher`] function returns an implementation of `Watcher` for the target platform.
+//!
+//! The [`Watcher`] interface leaves a lot of behaviour 'implementation defined'. See the caveats in
+//! the [`Watcher`] documentation for more details.
+//!
+//! The [`imp`] module contains the `Watcher` implementation for the target platform.
 use std::io;
 use std::path::Path;

@@ -14,7 +23,22 @@ pub(crate) struct Event {
     pub(crate) descriptor: Descriptor,
 }

+/// A platform-agnostic file and directory watching API.
+///
+/// This API is intended to be used to drive log collectors, specifically:
+///
+/// - Generate events when new files are added to a directory (see [`Self::watch_directory`]).
+/// - Generate events when new content is written to a file (see [`Self::watch_file`]).
+///
+/// The API is necessarily very 'lowest common denominator', and leaves a lot of behaviour
+/// implementation-defined. See the notes on callee responsibilities in [`Self::watch_directory`]
+/// and [`Self::watch_file`] for specifics.
 pub(crate) trait Watcher {
+    /// Construct a new instance of the `Watcher`.
+    ///
+    /// # Errors
+    ///
+    /// Propagates any `io::Error` caused when attempting to create the watcher.
     fn new() -> io::Result<Self>
     where
         Self: Sized;
@@ -82,6 +106,7 @@ pub(crate) trait Watcher {
     fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
 }

+/// [`Watcher`] implementation for linux, based on `inotify`.
 #[cfg(target_os = "linux")]
 mod imp {
     use std::io;
@@ -173,6 +198,7 @@ mod imp {
     }
 }

+/// [`Watcher`] implementation for `MacOS`, based on `kqueue`.
 #[cfg(target_os = "macos")]
 mod imp {
     use std::fs::File;
```

OK – hopefully we've done enough now to keep us right in future.

#### `log_collector::directory`

Back to tests.
Now that we've clarified the caller responsibilities for `watcher`, we should introduce tests to `log_collector::directory` for the various points of interest:

- Initialising the watcher with a `root_path` that's a symlink.
- Creating or writing to symlinks in `root_path`, pointing to log files elsewhere in the file system.
- Creating or writing to symlinks in `root_path`, pointing to another log file in `root_path`.

Our general expectation in each are:

- We adhere to the 'callee responsibilities' of the watcher API, specifically that we call the right one of `watch_directory` or `watch_file`, that we pass the canonical path to the file, and furthermore that we only call once per canonical path.
- We annotate log entries with the `path` based in `root_path`, e.g. rather than a canonical path that's outside of `root_path`.

There are a couple of different ways we could test this:

- As we do now, test the observable behaviour with the platform watcher.
  If the tests under both platforms, we can assume we're respecting the behaviour.
- Test using a mock implementation of `Watcher` that validates the callee responsibilities.

The platform-specific approach has the limitation of only being able to test observable behaviour.
As we explored above, detecting things like duplicate watches can only be determined through logs.
We could explore an approach of mocking the logger, and testing the generated logs, but that feels like a bit of a rabbithole, so let's start with a mock `Watcher` that performs some assertions:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -332,4 +332,76 @@ mod tests {

         Ok((path, file))
     }
+
+    mod mock {
+        use std::collections::HashSet;
+        use std::io;
+        use std::path::{Path, PathBuf};
+
+        use crate::log_collector::watcher::{Descriptor, Event, Watcher};
+
+        struct MockWatcher {
+            watched_paths: HashSet<PathBuf>,
+        }
+
+        impl Watcher for MockWatcher {
+            fn new() -> io::Result<Self> {
+                Ok(Self {
+                    watched_paths: HashSet::new(),
+                })
+            }
+
+            fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor> {
+                let canonical_path = path.canonicalize()?;
+
+                assert_eq!(
+                    path, canonical_path,
+                    "called watch_directory with link {:?} to {:?}",
+                    path, canonical_path
+                );
+                assert!(
+                    canonical_path.is_dir(),
+                    "called watch_directory with file path {:?}",
+                    path
+                );
+                assert!(
+                    !self.watched_paths.contains(&canonical_path),
+                    "called watch_directory with duplicate path {:?}",
+                    path
+                );
+
+                todo!()
+            }
+
+            fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor> {
+                let canonical_path = path.canonicalize()?;
+
+                assert_eq!(
+                    path, canonical_path,
+                    "called watch_directory with link {:?} to {:?}",
+                    path, canonical_path
+                );
+                assert!(
+                    canonical_path.is_file(),
+                    "called watch_directory with file path {:?}",
+                    path
+                );
+                assert!(
+                    !self.watched_paths.contains(&canonical_path),
+                    "called watch_directory with duplicate path {:?}",
+                    path
+                );
+
+                todo!()
+            }
+
+            fn read_events(&mut self) -> io::Result<Vec<Event>> {
+                todo!()
+            }
+
+            fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+                todo!()
+            }
+        }
+    }
 }
```

We've taken a stab at it, but have ran into a problem: `watcher::Descriptor` and `watcher::Event` are defined in terms of `imp`, so we currently have no way of mocking them:

```rust
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Descriptor(imp::Descriptor);

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Event {
    pub(crate) descriptor: Descriptor,
}
```

There are two ways we could move forward:

1. Make `Descriptor` an associated type of `Watcher`, e.g.

   ```rust
   trait Watcher {
       type Descriptor: Clone + Debug + Eq + Hash + PartialEq;
   }
   ```

   We'd also have to make `watcher::Event` generic over `Watcher`:

   ```rust
   struct Event<W: Watcher> {
       descriptor: W::Descriptor
   }
   ```

   This would have quite some effect on interactions with `watcher`, but shouldn't be visible outside of that (since `directory::Collector` is already generic over `Watcher`, and doesn't emit `watcher::Events` directly).

1. Move `MockWatcher` into `watcher.rs`, and use it as the `imp` under `cfg(test)`.
   This would mean that all tests would use the mock watcher, and we would have to use integration tests to validate the platform behaviour.

Both of these solutions sound reasonable, and in fact we could choose to do both.
For now, making `Descriptor` an associated type seems like it will have the least impact (e.g. it won't break existing tests), so let's start there:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -8,19 +8,28 @@
 //! the [`Watcher`] documentation for more details.
 //!
 //! The [`imp`] module contains the `Watcher` implementation for the target platform.
+use std::fmt::Debug;
+use std::hash::Hash;
 use std::io;
 use std::path::Path;

-pub(crate) fn watcher() -> io::Result<impl Watcher> {
+pub(super) fn watcher() -> io::Result<impl Watcher> {
     imp::Watcher::new()
 }

-#[derive(Clone, Debug, Eq, Hash, PartialEq)]
-pub(crate) struct Descriptor(imp::Descriptor);
+/// A platform-agnostic description of a watched file descriptor.
+///
+/// The [`Watcher`] API depends on being able to use `Descriptor`s as identifiers to correlate calls
+/// to `watch_*` with events emitted by the `Watcher`. This trait is thus just a collection of other
+/// traits that allow use as an identifier.
+pub(super) trait Descriptor: Clone + Debug + Eq + Hash + PartialEq + Send {}

-#[derive(Debug, Eq, PartialEq)]
-pub(crate) struct Event {
-    pub(crate) descriptor: Descriptor,
+/// A platform-agnostic interface to file system events.
+///
+/// This currently only exposes the `Descriptor` of the registered watch. Clients can use this to
+/// to correlate events with the corresponding `watch_*` call.
+pub(super) trait Event<D: Descriptor>: Debug {
+    fn descriptor(&self) -> &D;
 }

 /// A platform-agnostic file and directory watching API.
@@ -33,7 +42,20 @@ pub(crate) struct Event {
 /// The API is necessarily very 'lowest common denominator', and leaves a lot of behaviour
 /// implementation-defined. See the notes on callee responsibilities in [`Self::watch_directory`]
 /// and [`Self::watch_file`] for specifics.
-pub(crate) trait Watcher {
+pub(super) trait Watcher {
+    /// An opaque reference to a watched directory or file.
+    ///
+    /// Instances of this type are returned by [`watch_directory`](Self::watch_directory) and
+    /// [`watch_file`](Self::watch_file). They are also included in [`Event`]s emitted by the
+    /// watcher, and so can be used by callers to correlate events to watched files.
+    type Descriptor: Descriptor;
+
+    /// The type of events emitted by this watcher.
+    ///
+    /// The only requirement on this type is that it implements [`Event`], which allows the
+    /// associated `Descriptor` to be retrieved.
+    type Event: Event<Self::Descriptor>;
+
     /// Construct a new instance of the `Watcher`.
     ///
     /// # Errors
@@ -63,7 +85,7 @@ pub(crate) trait Watcher {
     /// # Errors
     ///
     /// Propagates any `io::Error` caused when attempting to register the watch.
-    fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor>;
+    fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor>;

     /// Watch a file for writes.
     ///
@@ -85,7 +107,7 @@ pub(crate) trait Watcher {
     /// # Errors
     ///
     /// Propagates any `io::Error` caused when attempting to register the watch.
-    fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor>;
+    fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor>;

     /// Read some events about the registered directories and files.
     ///
@@ -94,7 +116,7 @@ pub(crate) trait Watcher {
     /// # Errors
     ///
     /// Propagates any `io::Error` caused when attempting to read events.
-    fn read_events(&mut self) -> io::Result<Vec<Event>>;
+    fn read_events(&mut self) -> io::Result<Vec<Self::Event>>;

     /// Read some events about the registered directories and files.
     ///
@@ -103,7 +125,7 @@ pub(crate) trait Watcher {
     /// # Errors
     ///
     /// Propagates any `io::Error` caused when attempting to read events.
-    fn read_events_blocking(&mut self) -> io::Result<Vec<Event>>;
+    fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>>;
 }

 /// [`Watcher`] implementation for linux, based on `inotify`.
@@ -114,28 +136,38 @@ mod imp {

     use inotify::{Inotify, WatchDescriptor, WatchMask};

-    use super::Event;
-
     const INOTIFY_BUFFER_SIZE: usize = 1024;

-    pub(crate) type Descriptor = WatchDescriptor;
+    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
+    pub(super) struct Descriptor(WatchDescriptor);

-    pub(crate) struct Watcher {
-        inner: Inotify,
-        buffer: [u8; INOTIFY_BUFFER_SIZE],
+    impl super::Descriptor for Descriptor {}
+
+    #[derive(Debug)]
+    pub(super) struct Event(Descriptor);
+
+    impl super::Event<Descriptor> for Event {
+        fn descriptor(&self) -> &Descriptor {
+            &self.0
+        }
     }

-    impl Watcher {
-        fn map_events(inotify_events: inotify::Events) -> Vec<Event> {
-            inotify_events
-                .map(|event| Event {
-                    descriptor: super::Descriptor(event.wd),
-                })
-                .collect()
+    impl<S> From<inotify::Event<S>> for Event {
+        fn from(inotify_event: inotify::Event<S>) -> Self {
+            Event(Descriptor(inotify_event.wd))
         }
     }

+    pub(super) struct Watcher {
+        inner: Inotify,
+        buffer: [u8; INOTIFY_BUFFER_SIZE],
+    }
+
     impl super::Watcher for Watcher {
+        type Descriptor = Descriptor;
+
+        type Event = Event;
+
         fn new() -> io::Result<Self> {
             let inner = Inotify::init()?;
             Ok(Watcher {
@@ -158,11 +190,11 @@ mod imp {
         /// # Errors
         ///
         /// Propagates any `io::Error` caused when attempting to register the watch.
-        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+        fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
             let descriptor = self
                 .inner
                 .add_watch(path, WatchMask::CREATE | WatchMask::DONT_FOLLOW)?;
-            Ok(super::Descriptor(descriptor))
+            Ok(Descriptor(descriptor))
         }

         /// Watch a file for writes.
@@ -179,21 +211,21 @@ mod imp {
         /// # Errors
         ///
         /// Propagates any `io::Error` caused when attempting to register the watch.
-        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+        fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
             let descriptor = self
                 .inner
                 .add_watch(path, WatchMask::MODIFY | WatchMask::DONT_FOLLOW)?;
-            Ok(super::Descriptor(descriptor))
+            Ok(Descriptor(descriptor))
         }

-        fn read_events(&mut self) -> io::Result<Vec<Event>> {
+        fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
             let inotify_events = self.inner.read_events(&mut self.buffer)?;
-            Ok(Self::map_events(inotify_events))
+            Ok(inotify_events.map(Event::from).collect())
         }

-        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+        fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
             let inotify_events = self.inner.read_events_blocking(&mut self.buffer)?;
-            Ok(Self::map_events(inotify_events))
+            Ok(inotify_events.map(Event::from).collect())
         }
     }
 }
@@ -209,11 +241,40 @@ mod imp {

     use kqueue::{self, EventData, EventFilter, FilterFlag, Ident, Vnode};

-    use super::Event;
+    /// A wrapper for `kqueue` watch descriptors, which are just [`RawFd`]s.
+    ///
+    /// This is just a wrapper to hide the underlying [`RawFd`] type.
+    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
+    pub(super) struct Descriptor(RawFd);
+
+    impl super::Descriptor for Descriptor {}
+
+    /// A wrapper for [`kqueue::Event`]s.
+    ///
+    /// This is a wrapper to hide the underlying [`kqueue::Event`] type.
+    #[derive(Debug)]
+    pub(super) struct Event(Descriptor);
+
+    impl super::Event<Descriptor> for Event {
+        fn descriptor(&self) -> &Descriptor {
+            &self.0
+        }
+    }

-    pub(crate) type Descriptor = RawFd;
+    impl From<kqueue::Event> for Event {
+        /// Translate a [`kqueue::Event`] into an [`Event`].
+        ///
+        /// This will panic if the event's flags don't correspond with the filters supplied in
+        /// [`Watcher::add_watch`], e.g. if the event is not for a file, or it is not a write event.
+        fn from(kq_event: kqueue::Event) -> Self {
+            match (&kq_event.ident, &kq_event.data) {
+                (Ident::Fd(fd), EventData::Vnode(Vnode::Write)) => Event(Descriptor(*fd)),
+                _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
+            }
+        }
+    }

-    pub(crate) struct Watcher {
+    pub(super) struct Watcher {
         inner: kqueue::Watcher,
     }

@@ -237,7 +298,7 @@ mod imp {
         /// # Errors
         ///
         /// Propagates any `io::Error` caused when attempting to register the watch.
-        fn add_watch(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+        fn add_watch(&mut self, path: &Path) -> io::Result<<Self as super::Watcher>::Descriptor> {
             let file = File::open(path)?;
             let fd = file.into_raw_fd();

@@ -245,27 +306,14 @@ mod imp {
                 .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
             self.inner.watch()?;

-            Ok(super::Descriptor(fd))
-        }
-
-        /// Map a [`kqueue::Event`] into an [`Event`](super::Event).
-        ///
-        /// # Panics
-        ///
-        /// This will panic if `kq_event` does not correspond to the filter passed in `add_watch`,
-        /// i.e. if it does not correspond to a file descriptor or it's not a write event.
-        fn map_event(kq_event: &kqueue::Event) -> Event {
-            let fd = match (&kq_event.ident, &kq_event.data) {
-                (&Ident::Fd(fd), &EventData::Vnode(Vnode::Write)) => fd,
-                _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
-            };
-            Event {
-                descriptor: super::Descriptor(fd),
-            }
+            Ok(Descriptor(fd))
         }
     }

     impl super::Watcher for Watcher {
+        type Descriptor = Descriptor;
+        type Event = Event;
+
         fn new() -> io::Result<Self> {
             let inner = kqueue::Watcher::new()?;
             Ok(Watcher { inner })
@@ -283,7 +331,7 @@ mod imp {
         /// # Errors
         ///
         /// Propagates any `io::Error` caused when attempting to register the watch.
-        fn watch_directory(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+        fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
             self.add_watch(path)
         }

@@ -299,22 +347,18 @@ mod imp {
         /// # Errors
         ///
         /// Propagates any `io::Error` caused when attempting to register the watch.
-        fn watch_file(&mut self, path: &Path) -> io::Result<super::Descriptor> {
+        fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
             self.add_watch(path)
         }

-        fn read_events(&mut self) -> io::Result<Vec<Event>> {
+        fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
             let kq_event = self.inner.poll(Some(Duration::new(0, 0)));
-            let event = kq_event.as_ref().map(Self::map_event);
-
-            Ok(event.into_iter().collect())
+            Ok(kq_event.into_iter().map(Event::from).collect())
         }

-        fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+        fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
             let kq_event = self.inner.iter().next();
-            let event = kq_event.as_ref().map(Self::map_event);
-
-            Ok(event.into_iter().collect())
+            Ok(kq_event.into_iter().map(Event::from).collect())
         }
     }
 }
@@ -342,7 +386,8 @@ mod tests {
         let events = watcher
             .read_events_blocking()
             .expect("failed to read events");
-        assert_eq!(events, vec![Event { descriptor }]);
+        let event_descriptors: Vec<_> = events.iter().map(Event::descriptor).collect();
+        assert_eq!(event_descriptors, vec![&descriptor]);
     }

     #[test]
@@ -362,6 +407,7 @@ mod tests {
         let events = watcher
             .read_events_blocking()
             .expect("failed to read events");
-        assert_eq!(events, vec![Event { descriptor }]);
+        let event_descriptors: Vec<_> = events.iter().map(Event::descriptor).collect();
+        assert_eq!(event_descriptors, vec![&descriptor]);
     }
 }
```

Quite a few changes, and more documentation.
One slight hoop we're jumping through is using 'newtypes' for `imp::Descriptor` and `imp::Event` on both platforms.
This is to ensure that code in one platform doesn't end up depending on any platform-specific behaviour that might be visible through the `W::Descriptor` and `W::Event` associated types.
In an ideal world we'd be able to say something like `type Descriptor: impl Descriptor` to make it opaque to clients, but this will do for now.

Now the call-sites:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -9,7 +9,7 @@ use log::{debug, trace, warn};

 use crate::LogEntry;

-use super::watcher::{self, watcher, Watcher};
+use super::watcher::{watcher, Event as _, Watcher};

 /// Configuration for [`initialize`].
 pub struct Config {
@@ -56,9 +56,9 @@ struct LiveFile {

 struct Collector<W: Watcher> {
     root_path: PathBuf,
-    root_wd: watcher::Descriptor,
-    live_files: HashMap<watcher::Descriptor, LiveFile>,
-    watched_files: HashMap<PathBuf, watcher::Descriptor>,
+    root_wd: W::Descriptor,
+    live_files: HashMap<W::Descriptor, LiveFile>,
+    watched_files: HashMap<PathBuf, W::Descriptor>,
     watcher: W,
     entry_buf: std::vec::IntoIter<LogEntry>,
 }
@@ -160,8 +160,8 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event(&mut self, watcher_event: &watcher::Event) -> io::Result<Vec<Event>> {
-        if watcher_event.descriptor == self.root_wd {
+    fn check_event(&mut self, watcher_event: &W::Event) -> io::Result<Vec<Event>> {
+        if watcher_event.descriptor() == &self.root_wd {
             let mut events = Vec::new();

             for entry in fs::read_dir(&self.root_path)? {
@@ -176,7 +176,7 @@ impl<W: Watcher> Collector<W> {
             return Ok(events);
         }

-        let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
+        let live_file = match self.live_files.get_mut(watcher_event.descriptor()) {
             None => {
                 warn!(
                     "Received event for unregistered watch descriptor: {:?}",
@@ -338,20 +338,34 @@ mod tests {
         use std::io;
         use std::path::{Path, PathBuf};

-        use crate::log_collector::watcher::{Descriptor, Event, Watcher};
+        use crate::log_collector::watcher::{self, Watcher};
+
+        type Descriptor = PathBuf;
+        type Event = PathBuf;
+
+        impl watcher::Descriptor for Descriptor {}
+
+        impl watcher::Event<Descriptor> for Event {
+            fn descriptor(&self) -> &Descriptor {
+                &self
+            }
+        }

         struct MockWatcher {
             watched_paths: HashSet<PathBuf>,
         }

         impl Watcher for MockWatcher {
+            type Descriptor = PathBuf;
+            type Event = PathBuf;
+
             fn new() -> io::Result<Self> {
                 Ok(Self {
                     watched_paths: HashSet::new(),
                 })
             }

-            fn watch_directory(&mut self, path: &Path) -> io::Result<Descriptor> {
+            fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
                 let canonical_path = path.canonicalize()?;

                 assert_eq!(
@@ -373,7 +387,7 @@ mod tests {
                 todo!()
             }

-            fn watch_file(&mut self, path: &Path) -> io::Result<Descriptor> {
+            fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
                 let canonical_path = path.canonicalize()?;

                 assert_eq!(
@@ -395,11 +409,11 @@ mod tests {
                 todo!()
             }

-            fn read_events(&mut self) -> io::Result<Vec<Event>> {
+            fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
                 todo!()
             }

-            fn read_events_blocking(&mut self) -> io::Result<Vec<Event>> {
+            fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
                 todo!()
             }
         }
```

```diff
--- a/src/log_collector/kubernetes.rs
+++ b/src/log_collector/kubernetes.rs
@@ -9,7 +9,7 @@ use log::{debug, trace, warn};

 use crate::LogEntry;

-use super::watcher::{self, watcher, Watcher};
+use super::watcher::{watcher, Event as _, Watcher};

 const DEFAULT_ROOT_PATH: &str = "/var/log/containers";

@@ -60,9 +60,9 @@ struct LiveFile {

 struct Collector<W: Watcher> {
     root_path: PathBuf,
-    root_wd: watcher::Descriptor,
-    live_files: HashMap<watcher::Descriptor, LiveFile>,
-    watched_files: HashMap<PathBuf, watcher::Descriptor>,
+    root_wd: W::Descriptor,
+    live_files: HashMap<W::Descriptor, LiveFile>,
+    watched_files: HashMap<PathBuf, W::Descriptor>,
     watcher: W,
     entry_buf: std::vec::IntoIter<LogEntry>,
 }
@@ -123,7 +123,7 @@ impl<W: Watcher> Collector<W> {
         self.handle_events(watcher_events)
     }

-    fn handle_events(&mut self, watcher_events: Vec<watcher::Event>) -> io::Result<Vec<LogEntry>> {
+    fn handle_events(&mut self, watcher_events: Vec<W::Event>) -> io::Result<Vec<LogEntry>> {
         let mut entries = Vec::new();
         let mut read_file = |live_file: &mut LiveFile| -> io::Result<()> {
             while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
@@ -152,7 +152,7 @@ impl<W: Watcher> Collector<W> {
             let mut new_paths = Vec::new();

             for event in self.check_event(&watcher_event)? {
-                debug!("{}", event);
+                debug!("{:?}", event);

                 let live_file = match event {
                     Event::Create { path } => {
@@ -178,8 +178,8 @@ impl<W: Watcher> Collector<W> {
         Ok(entries)
     }

-    fn check_event(&mut self, watcher_event: &watcher::Event) -> io::Result<Vec<Event>> {
-        if watcher_event.descriptor == self.root_wd {
+    fn check_event(&mut self, watcher_event: &W::Event) -> io::Result<Vec<Event>> {
+        if watcher_event.descriptor() == &self.root_wd {
             let mut events = Vec::new();

             for entry in fs::read_dir(&self.root_path)? {
@@ -197,7 +197,7 @@ impl<W: Watcher> Collector<W> {
             return Ok(events);
         }

-        let live_file = match self.live_files.get_mut(&watcher_event.descriptor) {
+        let live_file = match self.live_files.get_mut(&watcher_event.descriptor()) {
             None => {
                 warn!(
                     "Received event for unregistered watch descriptor: {:?}",
```

This is all pretty mechanical.
The most annoying thing is having to import `watcher::Event` in order to access the `descriptor()` method.
Oh well.

Also, we may be mistaken about whether details of `W::Descriptor` and `W::Event` can leak – surely given all we know about `W` is `W: Watcher`, we would only be able to rely on the bounds therein.
What happens if we strip them out:

```diff
--- a/src/log_collector/watcher.rs
+++ b/src/log_collector/watcher.rs
@@ -138,13 +138,12 @@ mod imp {

     const INOTIFY_BUFFER_SIZE: usize = 1024;

-    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
-    pub(super) struct Descriptor(WatchDescriptor);
+    type Descriptor = WatchDescriptor;

     impl super::Descriptor for Descriptor {}

     #[derive(Debug)]
-    pub(super) struct Event(Descriptor);
+    pub(super) struct Event(WatchDescriptor);

     impl super::Event<Descriptor> for Event {
         fn descriptor(&self) -> &Descriptor {
@@ -154,7 +153,7 @@ mod imp {

     impl<S> From<inotify::Event<S>> for Event {
         fn from(inotify_event: inotify::Event<S>) -> Self {
-            Event(Descriptor(inotify_event.wd))
+            Self(inotify_event.wd)
         }
     }

@@ -194,7 +193,7 @@ mod imp {
             let descriptor = self
                 .inner
                 .add_watch(path, WatchMask::CREATE | WatchMask::DONT_FOLLOW)?;
-            Ok(Descriptor(descriptor))
+            Ok(descriptor)
         }

         /// Watch a file for writes.
@@ -215,7 +214,7 @@ mod imp {
             let descriptor = self
                 .inner
                 .add_watch(path, WatchMask::MODIFY | WatchMask::DONT_FOLLOW)?;
-            Ok(Descriptor(descriptor))
+            Ok(descriptor)
         }

         fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
@@ -241,35 +240,23 @@ mod imp {

     use kqueue::{self, EventData, EventFilter, FilterFlag, Ident, Vnode};

-    /// A wrapper for `kqueue` watch descriptors, which are just [`RawFd`]s.
-    ///
-    /// This is just a wrapper to hide the underlying [`RawFd`] type.
-    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
-    pub(super) struct Descriptor(RawFd);
+    type Descriptor = RawFd;

     impl super::Descriptor for Descriptor {}

-    /// A wrapper for [`kqueue::Event`]s.
-    ///
-    /// This is a wrapper to hide the underlying [`kqueue::Event`] type.
-    #[derive(Debug)]
-    pub(super) struct Event(Descriptor);
+    type Event = kqueue::Event;

     impl super::Event<Descriptor> for Event {
-        fn descriptor(&self) -> &Descriptor {
-            &self.0
-        }
-    }
-
-    impl From<kqueue::Event> for Event {
-        /// Translate a [`kqueue::Event`] into an [`Event`].
+        /// Get the `RawFd` for a [`kqueue::Event`].
+        ///
+        /// # Panics
         ///
         /// This will panic if the event's flags don't correspond with the filters supplied in
         /// [`Watcher::add_watch`], e.g. if the event is not for a file, or it is not a write event.
-        fn from(kq_event: kqueue::Event) -> Self {
-            match (&kq_event.ident, &kq_event.data) {
-                (Ident::Fd(fd), EventData::Vnode(Vnode::Write)) => Event(Descriptor(*fd)),
-                _ => panic!("kqueue returned an unexpected event: {:?}", kq_event),
+        fn descriptor(&self) -> &Descriptor {
+            match (&self.ident, &self.data) {
+                (Ident::Fd(fd), EventData::Vnode(Vnode::Write)) => fd,
+                _ => panic!("kqueue returned an unexpected event: {:?}", self),
             }
         }
     }
@@ -306,7 +293,7 @@ mod imp {
                 .add_fd(fd, EventFilter::EVFILT_VNODE, FilterFlag::NOTE_WRITE)?;
             self.inner.watch()?;

-            Ok(Descriptor(fd))
+            Ok(fd)
         }
     }

@@ -353,12 +340,12 @@ mod imp {

         fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
             let kq_event = self.inner.poll(Some(Duration::new(0, 0)));
-            Ok(kq_event.into_iter().map(Event::from).collect())
+            Ok(kq_event.into_iter().collect())
         }

         fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
             let kq_event = self.inner.iter().next();
-            Ok(kq_event.into_iter().map(Event::from).collect())
+            Ok(kq_event.into_iter().collect())
         }
     }
 }
```

That seems a bit neater.

#### Back to `MockWatcher`

That was quite the diversion, but we should be able to finish our `MockWatcher` now:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -384,7 +384,7 @@ mod tests {
                     path
                 );

-                todo!()
+                Ok(canonical_path)
             }

             fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
@@ -406,7 +406,7 @@ mod tests {
                     path
                 );

-                todo!()
+                Ok(canonical_path)
             }

             fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
```

So `watch_directory` and `watch_file` were easy, but how should we handle `read_events` and `read_events_blocking`?
For now, let's add a `Vec<PathBuf>` to `MockWatcher` and return it from `read_events`.
We'll also add a method to `MockWatcher` itself to push an event:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -351,8 +351,15 @@ mod tests {
             }
         }

-        struct MockWatcher {
+        pub(super) struct MockWatcher {
             watched_paths: HashSet<PathBuf>,
+            pending_events: Vec<PathBuf>,
+        }
+
+        impl MockWatcher {
+            pub(super) fn add_event(&mut self, path: PathBuf) {
+                self.pending_events.push(path);
+            }
         }

         impl Watcher for MockWatcher {
@@ -362,6 +369,7 @@ mod tests {
             fn new() -> io::Result<Self> {
                 Ok(Self {
                     watched_paths: HashSet::new(),
+                    pending_events: Vec::new(),
                 })
             }

@@ -410,11 +418,16 @@ mod tests {
             }

             fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
-                todo!()
+                let events = std::mem::replace(&mut self.pending_events, Vec::new());
+                Ok(events)
             }

             fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
-                todo!()
+                let events = self.read_events()?;
+                if events.is_empty() {
+                    panic!("called read_events_blocking with no events prepared, this will block forever");
+                }
+                Ok(events)
             }
         }
     }
```

#### `log_collector::directory` tests for real this time

Great, now let's remind ourselves of the conditions we wanted to test:

> - Initialising the watcher with a `root_path` that's a symlink.
> - Creating or writing to symlinks in `root_path`, pointing to log files elsewhere in the file system.
> - Creating or writing to symlinks in `root_path`, pointing to another log file in `root_path`.

Let's start with the first:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -243,7 +243,9 @@ impl<W: Watcher> Iterator for Collector<W> {
 mod tests {
     use std::fs::{self, File};
     use std::io::{self, Write};
+    use std::os::unix;
     use std::path::PathBuf;
+    use std::rc::Rc;

     use tempfile::TempDir;

@@ -252,6 +254,52 @@ mod tests {

     use super::{Collector, Config};

+    #[test]
+    fn initialize_with_symlink() -> test::Result {
+        env_logger::init();
+
+        let root_dir_parent = tempfile::tempdir()?;
+        let logs_dir = tempfile::tempdir()?;
+
+        let root_path = root_dir_parent.path().join("logs");
+        unix::fs::symlink(logs_dir.path(), &root_path)?;
+
+        let config = Config {
+            root_path: root_path.clone(),
+        };
+        let watcher = mock::MockWatcher::new();
+        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+
+        let (file_path, mut file) = create_log_file(&logs_dir)?;
+        watcher.borrow_mut().add_event(root_path.canonicalize()?);
+
+        collector.collect_entries()?; // refresh known files
+
+        writeln!(file, "hello?")?;
+        watcher.borrow_mut().add_event(file_path.clone());
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            watcher.borrow().watched_paths(),
+            &vec![root_path.canonicalize()?, file_path.clone()]
+        );
+        assert_eq!(
+            entries,
+            vec![log_entry(
+                "hello?",
+                &[(
+                    "path",
+                    root_path
+                        .join(file_path.file_name().unwrap())
+                        .to_str()
+                        .unwrap()
+                )]
+            )]
+        );
+
+        Ok(())
+    }
+
     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
@@ -334,9 +382,10 @@ mod tests {
     }

     mod mock {
-        use std::collections::HashSet;
+        use std::cell::RefCell;
         use std::io;
         use std::path::{Path, PathBuf};
+        use std::rc::Rc;

         use crate::log_collector::watcher::{self, Watcher};

@@ -352,11 +401,19 @@ mod tests {
         }

         pub(super) struct MockWatcher {
-            watched_paths: HashSet<PathBuf>,
+            watched_paths: Vec<PathBuf>,
             pending_events: Vec<PathBuf>,
         }

         impl MockWatcher {
+            pub(super) fn new() -> Rc<RefCell<Self>> {
+                Rc::new(RefCell::new(<Self as Watcher>::new().unwrap()))
+            }
+
+            pub(super) fn watched_paths(&self) -> &Vec<PathBuf> {
+                &self.watched_paths
+            }
+
             pub(super) fn add_event(&mut self, path: PathBuf) {
                 self.pending_events.push(path);
             }
@@ -368,7 +425,7 @@ mod tests {

             fn new() -> io::Result<Self> {
                 Ok(Self {
-                    watched_paths: HashSet::new(),
+                    watched_paths: Vec::new(),
                     pending_events: Vec::new(),
                 })
             }
@@ -391,7 +448,7 @@ mod tests {
                     "called watch_directory with duplicate path {:?}",
                     path
                 );
-
+                self.watched_paths.push(canonical_path.clone());
                 Ok(canonical_path)
             }

@@ -413,7 +470,7 @@ mod tests {
                     "called watch_directory with duplicate path {:?}",
                     path
                 );
-
+                self.watched_paths.push(canonical_path.clone());
                 Ok(canonical_path)
             }

@@ -430,5 +487,32 @@ mod tests {
                 Ok(events)
             }
         }
+
+        impl Watcher for Rc<RefCell<MockWatcher>> {
+            type Descriptor = <MockWatcher as Watcher>::Descriptor;
+            type Event = <MockWatcher as Watcher>::Event;
+
+            fn new() -> io::Result<Self> {
+                <MockWatcher as Watcher>::new()
+                    .map(RefCell::new)
+                    .map(Rc::new)
+            }
+
+            fn watch_directory(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
+                self.borrow_mut().watch_directory(path)
+            }
+
+            fn watch_file(&mut self, path: &Path) -> io::Result<Self::Descriptor> {
+                self.borrow_mut().watch_file(path)
+            }
+
+            fn read_events(&mut self) -> io::Result<Vec<Self::Event>> {
+                self.borrow_mut().read_events()
+            }
+
+            fn read_events_blocking(&mut self) -> io::Result<Vec<Self::Event>> {
+                self.borrow_mut().read_events_blocking()
+            }
+        }
     }
 }
```

We've had to make some small changes to `MockWatcher`, including:

- Add a `new()` associated function to get an `Rc<RefCell<MockWatcher>>`.
- Implement `Watcher` for `Rc<RefCell<MockWatcher>>`.
- Add a getter for `watched_paths`.
- Make `watched_paths` a `Vec`, since order is important.

If we run our new test, we're immediately greeted with an exception:

```
$ cargo test log_collector::directory::tests::initialize_with_symlink
...
running 1 test
test log_collector::directory::tests::initialize_with_symlink ... FAILED

failures:

---- log_collector::directory::tests::initialize_with_symlink stdout ----
thread 'log_collector::directory::tests::initialize_with_symlink' panicked at 'assertion failed: `(left == right)`
  left: `"/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpwrNj4p/logs"`,
 right: `"/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpVPBpRl"`: called watch_directory with link "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpwrNj4p/logs" to "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpVPBpRl"', src/log_collector/directory.rs:436:17
...
```

So we're triggering the assertion in `MockWatcher` about registering a watch with a symlink.
Let's fix this simply for now by canonicalizing `root_path` before calling `watch_directory`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -81,7 +81,7 @@ impl<W: Watcher> Collector<W> {
         let Config { root_path } = config;

         debug!("Initialising watch on root path {:?}", root_path);
-        let root_wd = watcher.watch_directory(&root_path)?;
+        let root_wd = watcher.watch_directory(&root_path.canonicalize()?)?;

         let mut collector = Self {
             root_path,
```

Now if we run our test we get a new failure:

```
$ cargo test log_collector::directory::tests::initialize_with_symlink
...
running 1 test
test log_collector::directory::tests::initialize_with_symlink ... FAILED

failures:

---- log_collector::directory::tests::initialize_with_symlink stdout ----
thread 'log_collector::directory::tests::initialize_with_symlink' panicked at 'assertion failed: `(left == right)`
  left: `[LogEntry { line: "hello?", metadata: {"path": "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmp1mrPr9/test.log"} }]`,
 right: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmp9I2GyC/logs/test.log"} }]`', src/log_collector/directory.rs:286:9
...
```

In this case the difference is the `path` of the `LogEntry`.
We would like paths to be reported relative to the given `root_path`, rather than resolved.
Let's remove the calls to `fs::canonicalize` that affect logs file paths:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -94,7 +94,7 @@ impl<W: Watcher> Collector<W> {

         for entry in fs::read_dir(&collector.root_path)? {
             let entry = entry?;
-            let path = fs::canonicalize(entry.path())?;
+            let path = entry.path().to_path_buf();

             debug!("{}", Event::Create { path: path.clone() });
             collector.handle_event_create(path)?;
@@ -166,7 +166,7 @@ impl<W: Watcher> Collector<W> {

             for entry in fs::read_dir(&self.root_path)? {
                 let entry = entry?;
-                let path = fs::canonicalize(entry.path())?;
+                let path = entry.path().to_path_buf();

                 if !self.watched_files.contains_key(&path) {
                     events.push(Event::Create { path });
```

Now we might expect an error about calling `watch_file` with a symlink, and indeed:

```
$ cargo test log_collector::directory::tests::initialize_with_symlink
...
running 1 test
test log_collector::directory::tests::initialize_with_symlink ... FAILED

failures:

---- log_collector::directory::tests::initialize_with_symlink stdout ----
thread 'log_collector::directory::tests::initialize_with_symlink' panicked at 'assertion failed: `(left == right)`
  left: `"/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpba6ISC/logs/test.log"`,
 right: `"/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpEnKmmd/test.log"`: called watch_directory with link "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpba6ISC/logs/test.log" to "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpEnKmmd/test.log"', src/log_collector/directory.rs:458:17
...
```

Wait, `watch_directory`?
Oops, copy-paste error:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -457,17 +457,17 @@ mod tests {

                 assert_eq!(
                     path, canonical_path,
-                    "called watch_directory with link {:?} to {:?}",
+                    "called watch_file with link {:?} to {:?}",
                     path, canonical_path
                 );
                 assert!(
                     canonical_path.is_file(),
-                    "called watch_directory with file path {:?}",
+                    "called watch_file with file path {:?}",
                     path
                 );
                 assert!(
                     !self.watched_paths.contains(&canonical_path),
-                    "called watch_directory with duplicate path {:?}",
+                    "called watch_file with duplicate path {:?}",
                     path
                 );
                 self.watched_paths.push(canonical_path.clone());
```

Try again:

```
$ cargo test log_collector::directory::tests::initialize_with_symlink
...
running 1 test
test log_collector::directory::tests::initialize_with_symlink ... FAILED

failures:

---- log_collector::directory::tests::initialize_with_symlink stdout ----
thread 'log_collector::directory::tests::initialize_with_symlink' panicked at 'assertion failed: `(left == right)`
  left: `"/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpADOunO/logs/test.log"`,
 right: `"/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpuCppKx/test.log"`: called watch_file with link "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpADOunO/logs/test.log" to "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpuCppKx/test.log"', src/log_collector/directory.rs:458:17
...
```

OK, so since we always call `watch_file` in `handle_event_create` we should be able to get past this pretty easily:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -198,7 +198,7 @@ impl<W: Watcher> Collector<W> {
     }

     fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
-        let wd = self.watcher.watch_file(&path)?;
+        let wd = self.watcher.watch_file(&path.canonicalize()?)?;
         let mut reader = BufReader::new(File::open(&path)?);
         reader.seek(io::SeekFrom::End(0))?;

```

And now our test passes:

```
$ cargo test log_collector::directory::tests::initialize_with_symlink
...
running 1 test
test log_collector::directory::tests::initialize_with_symlink ... ok
...
```

We might expect this will also cover our next condition, "Creating or writing to symlinks in `root_path`, pointing to log files elsewhere in the file system." – but we should add a test anyway:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -279,27 +279,55 @@ mod tests {
         watcher.borrow_mut().add_event(file_path.clone());

         let entries = collector.collect_entries()?;
+        let expected_path = root_path.join(file_path.file_name().unwrap());
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_path.canonicalize()?, file_path.clone()]
+            &vec![root_path.canonicalize()?, file_path]
         );
         assert_eq!(
             entries,
             vec![log_entry(
                 "hello?",
-                &[(
-                    "path",
-                    root_path
-                        .join(file_path.file_name().unwrap())
-                        .to_str()
-                        .unwrap()
-                )]
+                &[("path", expected_path.to_str().unwrap())]
             )]
         );

         Ok(())
     }

+    #[test]
+    fn file_with_symlink() -> test::Result {
+        env_logger::init();
+
+        let root_dir = tempfile::tempdir()?;
+        let logs_dir = tempfile::tempdir()?;
+
+        let (src_path, mut file) = create_log_file(&logs_dir)?;
+        let dst_path = root_dir.path().join(src_path.file_name().unwrap());
+        unix::fs::symlink(&src_path, &dst_path)?;
+
+        let config = Config {
+            root_path: root_dir.path().to_path_buf(),
+        };
+        let watcher = mock::MockWatcher::new();
+        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+
+        writeln!(file, "hello?")?;
+        watcher.borrow_mut().add_event(src_path.clone());
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            watcher.borrow().watched_paths(),
+            &vec![root_dir.path().canonicalize()?, src_path]
+        );
+        assert_eq!(
+            entries,
+            vec![log_entry("hello?", &[("path", dst_path.to_str().unwrap())])]
+        );
+
+        Ok(())
+    }
+
     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
```

And indeed, the test passes:

```
$ cargo test log_collector::directory::tests::file_with_symlink
...
running 1 test
test log_collector::directory::tests::file_with_symlink ... ok
...
```

Our last condition may be more interesting:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -296,7 +296,7 @@ mod tests {
     }

     #[test]
-    fn file_with_symlink() -> test::Result {
+    fn file_with_external_symlink() -> test::Result {
         env_logger::init();

         let root_dir = tempfile::tempdir()?;
@@ -328,6 +328,38 @@ mod tests {
         Ok(())
     }

+    #[test]
+    fn file_with_internal_symlink() -> test::Result {
+        env_logger::init();
+
+        let root_dir = tempfile::tempdir()?;
+
+        let (src_path, mut file) = create_log_file(&root_dir)?;
+        let dst_path = root_dir.path().join("linked.log");
+        unix::fs::symlink(&src_path, &dst_path)?;
+
+        let config = Config {
+            root_path: root_dir.path().to_path_buf(),
+        };
+        let watcher = mock::MockWatcher::new();
+        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+
+        writeln!(file, "hello?")?;
+        watcher.borrow_mut().add_event(src_path.clone());
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            watcher.borrow().watched_paths(),
+            &vec![root_dir.path().canonicalize()?, src_path]
+        );
+        assert_eq!(
+            entries,
+            vec![log_entry("hello?", &[("path", dst_path.to_str().unwrap())])]
+        );
+
+        Ok(())
+    }
+
     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
```

Our new test fails due to a duplicate watch on the log file:

```
$ cargo test log_collector::directory::tests::file_with_internal_symlink
...
running 1 test
test log_collector::directory::tests::file_with_internal_symlink ... FAILED

failures:

---- log_collector::directory::tests::file_with_internal_symlink stdout ----
thread 'log_collector::directory::tests::file_with_internal_symlink' panicked at 'called watch_file with duplicate path "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpXF7vTa/test.log"', src/log_collector/directory.rs:528:17
...
```

As a reminder, we want to prevent duplicate calls to `watch_file` because the behaviour in that case cannot be relied upon: `kqueue` will register a duplicate watch, whereas `inotify` will combine watches.
So, what should we do?

We could store the canonical path somewhere in `directory::Collector`, and panic if we see it come up again in `handle_event_create`.
That wouldn't be ideal though, since our monitor would blow up in the face of such symlinks.

We could store canonical paths in `watched_files`, which would lead to us calling `handle_event_create` only for the first linked path that we see.
Ultimately this would result in log entries being recorded against the first path we see, and this could change between restarts (since directory iteration order is generally unreliable, and new symlinks might appear before pre-existing ones).

Finally, we could record multiple paths for each `LiveFile`, and record a `LogEntry` for each.
This would probably be the most 'correct' implementation, in that each linked path would have its own stream of log entries.
It would, however, complicate the implementation.

For now, let's take the easy way out and adopt the 2nd approach, and document that caveat against `directory::Collector`:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -18,10 +18,18 @@ pub struct Config {
 }

 #[derive(Debug)]
+#[allow(variant_size_differences)]
 enum Event<'collector> {
-    Create { path: PathBuf },
-    Append { live_file: &'collector mut LiveFile },
-    Truncate { live_file: &'collector mut LiveFile },
+    Create {
+        path: PathBuf,
+        canonical_path: PathBuf,
+    },
+    Append {
+        live_file: &'collector mut LiveFile,
+    },
+    Truncate {
+        live_file: &'collector mut LiveFile,
+    },
 }

 impl Event<'_> {
@@ -35,7 +43,7 @@ impl Event<'_> {

     fn path(&self) -> &Path {
         match self {
-            Event::Create { path } => path,
+            Event::Create { path, .. } => path,
             Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => &live_file.path,
         }
     }
@@ -68,6 +76,12 @@ struct Collector<W: Watcher> {
 /// This will start a watch (using `inotify` or `kqueue`) on `config.root_path` and any files
 /// therein. Whenever the files change, new lines are emitted as `LogEntry` records.
 ///
+/// # Caveats
+///
+/// This collector does not reliably handle symlinks in the `root_path` to other files in the
+/// `root_path`. In that situation, `LogEntry` records will have just one of the paths, and the
+/// chosen path might change after restarts.
+///
 /// # Errors
 ///
 /// Propagates any `io::Error`s that occur during initialization.
@@ -95,9 +109,18 @@ impl<W: Watcher> Collector<W> {
         for entry in fs::read_dir(&collector.root_path)? {
             let entry = entry?;
             let path = entry.path().to_path_buf();
-
-            debug!("{}", Event::Create { path: path.clone() });
-            collector.handle_event_create(path)?;
+            let canonical_path = path.canonicalize()?;
+
+            if !collector.watched_files.contains_key(&canonical_path) {
+                debug!(
+                    "{}",
+                    Event::Create {
+                        path: path.clone(),
+                        canonical_path: canonical_path.clone()
+                    }
+                );
+                collector.handle_event_create(path, canonical_path)?;
+            }
         }

         Ok(collector)
@@ -137,8 +160,11 @@ impl<W: Watcher> Collector<W> {
                 debug!("{}", event);

                 let live_file = match event {
-                    Event::Create { path } => {
-                        new_paths.push(path);
+                    Event::Create {
+                        path,
+                        canonical_path,
+                    } => {
+                        new_paths.push((path, canonical_path));
                         continue;
                     }
                     Event::Append { live_file } => live_file,
@@ -151,8 +177,8 @@ impl<W: Watcher> Collector<W> {
                 read_file(live_file)?;
             }

-            for path in new_paths {
-                let live_file = self.handle_event_create(path)?;
+            for (path, canonical_path) in new_paths {
+                let live_file = self.handle_event_create(path, canonical_path)?;
                 read_file(live_file)?;
             }
         }
@@ -167,9 +193,13 @@ impl<W: Watcher> Collector<W> {
             for entry in fs::read_dir(&self.root_path)? {
                 let entry = entry?;
                 let path = entry.path().to_path_buf();
+                let canonical_path = path.canonicalize()?;

-                if !self.watched_files.contains_key(&path) {
-                    events.push(Event::Create { path });
+                if !self.watched_files.contains_key(&canonical_path) {
+                    events.push(Event::Create {
+                        path,
+                        canonical_path,
+                    });
                 }
             }

@@ -197,20 +227,24 @@ impl<W: Watcher> Collector<W> {
         }
     }

-    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<&mut LiveFile> {
-        let wd = self.watcher.watch_file(&path.canonicalize()?)?;
+    fn handle_event_create(
+        &mut self,
+        path: PathBuf,
+        canonical_path: PathBuf,
+    ) -> io::Result<&mut LiveFile> {
+        let wd = self.watcher.watch_file(&canonical_path)?;
         let mut reader = BufReader::new(File::open(&path)?);
         reader.seek(io::SeekFrom::End(0))?;

         self.live_files.insert(
             wd.clone(),
             LiveFile {
-                path: path.clone(),
+                path,
                 reader,
                 entry_buf: String::new(),
             },
         );
-        self.watched_files.insert(path, wd.clone());
+        self.watched_files.insert(canonical_path, wd.clone());
         Ok(self.live_files.get_mut(&wd).unwrap())
     }

```

This isn't super wonderful, and in fact our test is still broken:

```
$ cargo test log_collector::directory::tests::file_with_internal_symlink
...
running 1 test
test log_collector::directory::tests::file_with_internal_symlink ... FAILED

failures:

---- log_collector::directory::tests::file_with_internal_symlink stdout ----
thread 'log_collector::directory::tests::file_with_internal_symlink' panicked at 'assertion failed: `(left == right)`
  left: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpwal8SO/test.log"} }]`,
 right: `[LogEntry { line: "hello?", metadata: {"path": "/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpwal8SO/linked.log"} }]`', src/log_collector/directory.rs:389:9
...
```

Indeed, it's not entirely clear how we should test for an 'arbitrary' path.
For now let's just do some shenanigans:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -384,12 +384,26 @@ mod tests {
         let entries = collector.collect_entries()?;
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path]
-        );
-        assert_eq!(
-            entries,
-            vec![log_entry("hello?", &[("path", dst_path.to_str().unwrap())])]
+            &vec![root_dir.path().canonicalize()?, src_path.clone()]
         );
+        assert_eq!(entries.len(), 1);
+        {
+            let crate::LogEntry { line, metadata } = &entries[0];
+            assert_eq!(line, "hello?");
+            assert_eq!(metadata.len(), 1);
+            assert!(metadata.contains_key("path"));
+
+            let actual_path = metadata.get("path").unwrap();
+            assert!(
+                actual_path
+                    == root_dir
+                        .path()
+                        .join(src_path.file_name().unwrap())
+                        .to_str()
+                        .unwrap()
+                    || actual_path == dst_path.to_str().unwrap()
+            );
+        }

         Ok(())
     }
```

Christ, this is grim.
Let's take a shot towards tracking multiple links instead:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -1,6 +1,6 @@
 //! A log collector that watches a directory of log files.

-use std::collections::HashMap;
+use std::collections::{HashMap, HashSet};
 use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};
@@ -44,7 +44,9 @@ impl Event<'_> {
     fn path(&self) -> &Path {
         match self {
             Event::Create { path, .. } => path,
-            Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => &live_file.path,
+            Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => {
+                &live_file.paths[0].as_ref()
+            }
         }
     }
 }
@@ -57,7 +59,7 @@ impl std::fmt::Display for Event<'_> {

 #[derive(Debug)]
 struct LiveFile {
-    path: PathBuf,
+    paths: Vec<String>,
     reader: BufReader<File>,
     entry_buf: String,
 }
@@ -66,7 +68,7 @@ struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: W::Descriptor,
     live_files: HashMap<W::Descriptor, LiveFile>,
-    watched_files: HashMap<PathBuf, W::Descriptor>,
+    watched_files: HashSet<PathBuf>,
     watcher: W,
     entry_buf: std::vec::IntoIter<LogEntry>,
 }
@@ -101,7 +103,7 @@ impl<W: Watcher> Collector<W> {
             root_path,
             root_wd,
             live_files: HashMap::new(),
-            watched_files: HashMap::new(),
+            watched_files: HashSet::new(),
             watcher,
             entry_buf: vec![].into_iter(),
         };
@@ -111,12 +113,12 @@ impl<W: Watcher> Collector<W> {
             let path = entry.path().to_path_buf();
             let canonical_path = path.canonicalize()?;

-            if !collector.watched_files.contains_key(&canonical_path) {
+            if !collector.watched_files.contains(&canonical_path) {
                 debug!(
                     "{}",
                     Event::Create {
                         path: path.clone(),
-                        canonical_path: canonical_path.clone()
+                        canonical_path: canonical_path.clone(),
                     }
                 );
                 collector.handle_event_create(path, canonical_path)?;
@@ -134,16 +136,15 @@ impl<W: Watcher> Collector<W> {
             while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
                 if live_file.entry_buf.ends_with('\n') {
                     live_file.entry_buf.pop();
+
                     let mut metadata = HashMap::new();
-                    metadata.insert(
-                        "path".to_string(),
-                        live_file.path.to_string_lossy().into_owned(),
-                    );
-                    let entry = LogEntry {
-                        line: live_file.entry_buf.clone(),
-                        metadata,
-                    };
-                    entries.push(entry);
+                    for path in &live_file.paths {
+                        metadata.insert("path".to_string(), path.clone());
+                        entries.push(LogEntry {
+                            line: live_file.entry_buf.clone(),
+                            metadata: metadata.clone(),
+                        });
+                    }

                     live_file.entry_buf.clear();
                 }
@@ -195,7 +196,7 @@ impl<W: Watcher> Collector<W> {
                 let path = entry.path().to_path_buf();
                 let canonical_path = path.canonicalize()?;

-                if !self.watched_files.contains_key(&canonical_path) {
+                if !self.watched_files.contains(&canonical_path) {
                     events.push(Event::Create {
                         path,
                         canonical_path,
@@ -233,19 +234,24 @@ impl<W: Watcher> Collector<W> {
         canonical_path: PathBuf,
     ) -> io::Result<&mut LiveFile> {
         let wd = self.watcher.watch_file(&canonical_path)?;
-        let mut reader = BufReader::new(File::open(&path)?);
+
+        let mut reader = BufReader::new(File::open(&canonical_path)?);
         reader.seek(io::SeekFrom::End(0))?;

-        self.live_files.insert(
-            wd.clone(),
-            LiveFile {
-                path,
-                reader,
-                entry_buf: String::new(),
-            },
-        );
-        self.watched_files.insert(canonical_path, wd.clone());
-        Ok(self.live_files.get_mut(&wd).unwrap())
+        let mut paths = vec![path.to_string_lossy().to_string()];
+        if canonical_path.starts_with(&self.root_path) {
+            paths.push(canonical_path.to_string_lossy().to_string());
+        }
+        let live_file = LiveFile {
+            paths,
+            reader,
+            entry_buf: String::new(),
+        };
+
+        self.watched_files.insert(path);
+        self.watched_files.insert(canonical_path);
+
+        Ok(self.live_files.entry(wd).or_insert(live_file))
     }

     fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
```

This is actually not too bad.
Now let's fix the tests on that basis:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -1,6 +1,6 @@
 //! A log collector that watches a directory of log files.

-use std::collections::{HashMap, HashSet};
+use std::collections::HashMap;
 use std::fs::{self, File};
 use std::io::{self, BufRead, BufReader, Seek};
 use std::path::{Path, PathBuf};
@@ -68,7 +68,7 @@ struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: W::Descriptor,
     live_files: HashMap<W::Descriptor, LiveFile>,
-    watched_files: HashSet<PathBuf>,
+    watched_paths: HashMap<PathBuf, W::Descriptor>,
     watcher: W,
     entry_buf: std::vec::IntoIter<LogEntry>,
 }
@@ -103,7 +103,7 @@ impl<W: Watcher> Collector<W> {
             root_path,
             root_wd,
             live_files: HashMap::new(),
-            watched_files: HashSet::new(),
+            watched_paths: HashMap::new(),
             watcher,
             entry_buf: vec![].into_iter(),
         };
@@ -113,16 +113,14 @@ impl<W: Watcher> Collector<W> {
             let path = entry.path().to_path_buf();
             let canonical_path = path.canonicalize()?;

-            if !collector.watched_files.contains(&canonical_path) {
-                debug!(
-                    "{}",
-                    Event::Create {
-                        path: path.clone(),
-                        canonical_path: canonical_path.clone(),
-                    }
-                );
-                collector.handle_event_create(path, canonical_path)?;
-            }
+            debug!(
+                "{}",
+                Event::Create {
+                    path: path.clone(),
+                    canonical_path: canonical_path.clone(),
+                }
+            );
+            collector.handle_event_create(path, canonical_path)?;
         }

         Ok(collector)
@@ -193,15 +191,16 @@ impl<W: Watcher> Collector<W> {

             for entry in fs::read_dir(&self.root_path)? {
                 let entry = entry?;
+                if self.watched_paths.contains_key(&entry.path()) {
+                    continue;
+                }
+
                 let path = entry.path().to_path_buf();
                 let canonical_path = path.canonicalize()?;
-
-                if !self.watched_files.contains(&canonical_path) {
-                    events.push(Event::Create {
-                        path,
-                        canonical_path,
-                    });
-                }
+                events.push(Event::Create {
+                    path,
+                    canonical_path,
+                });
             }

             return Ok(events);
@@ -233,25 +232,35 @@ impl<W: Watcher> Collector<W> {
         path: PathBuf,
         canonical_path: PathBuf,
     ) -> io::Result<&mut LiveFile> {
-        let wd = self.watcher.watch_file(&canonical_path)?;
+        if let Some(wd) = self.watched_paths.get(&canonical_path) {
+            let wd = wd.clone();

-        let mut reader = BufReader::new(File::open(&canonical_path)?);
-        reader.seek(io::SeekFrom::End(0))?;
+            // unwrap is safe because we any `wd` in `watched_paths` must be present in `live_files`
+            let live_file = self.live_files.get_mut(&wd).unwrap();
+            live_file.paths.push(path.to_string_lossy().to_string());

-        let mut paths = vec![path.to_string_lossy().to_string()];
-        if canonical_path.starts_with(&self.root_path) {
-            paths.push(canonical_path.to_string_lossy().to_string());
-        }
-        let live_file = LiveFile {
-            paths,
-            reader,
-            entry_buf: String::new(),
-        };
+            self.watched_paths.insert(path, wd);
+            Ok(live_file)
+        } else {
+            let wd = self.watcher.watch_file(&canonical_path)?;

-        self.watched_files.insert(path);
-        self.watched_files.insert(canonical_path);
+            let mut reader = BufReader::new(File::open(&canonical_path)?);
+            reader.seek(io::SeekFrom::End(0))?;

-        Ok(self.live_files.entry(wd).or_insert(live_file))
+            let mut paths = vec![path.to_string_lossy().to_string()];
+            if canonical_path.starts_with(&self.root_path) {
+                paths.push(canonical_path.to_string_lossy().to_string());
+            }
+
+            self.watched_paths.insert(path, wd.clone());
+            self.watched_paths.insert(canonical_path, wd.clone());
+
+            Ok(self.live_files.entry(wd).or_insert(LiveFile {
+                paths,
+                reader,
+                entry_buf: String::new(),
+            }))
+        }
     }

     fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
@@ -281,7 +290,7 @@ impl<W: Watcher> Iterator for Collector<W> {

 #[cfg(test)]
 mod tests {
-    use std::fs::{self, File};
+    use std::fs::File;
     use std::io::{self, Write};
     use std::os::unix;
     use std::path::PathBuf;
@@ -296,8 +305,6 @@ mod tests {

     #[test]
     fn initialize_with_symlink() -> test::Result {
-        env_logger::init();
-
         let root_dir_parent = tempfile::tempdir()?;
         let logs_dir = tempfile::tempdir()?;

@@ -311,18 +318,19 @@ mod tests {
         let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;

         let (file_path, mut file) = create_log_file(&logs_dir)?;
+        let file_path_canonical = file_path.canonicalize()?;
         watcher.borrow_mut().add_event(root_path.canonicalize()?);

         collector.collect_entries()?; // refresh known files

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(file_path.clone());
+        watcher.borrow_mut().add_event(file_path_canonical.clone());

         let entries = collector.collect_entries()?;
         let expected_path = root_path.join(file_path.file_name().unwrap());
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_path.canonicalize()?, file_path]
+            &vec![root_path.canonicalize()?, file_path_canonical]
         );
         assert_eq!(
             entries,
@@ -337,12 +345,11 @@ mod tests {

     #[test]
     fn file_with_external_symlink() -> test::Result {
-        env_logger::init();
-
         let root_dir = tempfile::tempdir()?;
         let logs_dir = tempfile::tempdir()?;

         let (src_path, mut file) = create_log_file(&logs_dir)?;
+        let src_path_canonical = src_path.canonicalize()?;
         let dst_path = root_dir.path().join(src_path.file_name().unwrap());
         unix::fs::symlink(&src_path, &dst_path)?;

@@ -353,12 +360,12 @@ mod tests {
         let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(src_path.clone());
+        watcher.borrow_mut().add_event(src_path_canonical.clone());

         let entries = collector.collect_entries()?;
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path]
+            &vec![root_dir.path().canonicalize()?, src_path_canonical]
         );
         assert_eq!(
             entries,
@@ -370,11 +377,10 @@ mod tests {

     #[test]
     fn file_with_internal_symlink() -> test::Result {
-        env_logger::init();
-
         let root_dir = tempfile::tempdir()?;

         let (src_path, mut file) = create_log_file(&root_dir)?;
+        let src_path_canonical = src_path.canonicalize()?;
         let dst_path = root_dir.path().join("linked.log");
         unix::fs::symlink(&src_path, &dst_path)?;

@@ -385,31 +391,31 @@ mod tests {
         let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;

         writeln!(file, "hello?")?;
-        watcher.borrow_mut().add_event(src_path.clone());
+        watcher.borrow_mut().add_event(src_path_canonical.clone());

         let entries = collector.collect_entries()?;
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path.clone()]
+            &vec![root_dir.path().canonicalize()?, src_path_canonical]
+        );
+
+        assert_eq!(entries.len(), 2);
+
+        let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
+        assert!(
+            entries.contains(&entry),
+            "expected entry {:?}, but found: {:#?}",
+            entry,
+            entries
+        );
+
+        let entry = log_entry("hello?", &[("path", src_path.to_str().unwrap())]);
+        assert!(
+            entries.contains(&entry),
+            "expected entry {:?}, but found: {:#?}",
+            entry,
+            entries
         );
-        assert_eq!(entries.len(), 1);
-        {
-            let crate::LogEntry { line, metadata } = &entries[0];
-            assert_eq!(line, "hello?");
-            assert_eq!(metadata.len(), 1);
-            assert!(metadata.contains_key("path"));
-
-            let actual_path = metadata.get("path").unwrap();
-            assert!(
-                actual_path
-                    == root_dir
-                        .path()
-                        .join(src_path.file_name().unwrap())
-                        .to_str()
-                        .unwrap()
-                    || actual_path == dst_path.to_str().unwrap()
-            );
-        }

         Ok(())
     }
@@ -487,9 +493,7 @@ mod tests {
     }

     fn create_log_file(tempdir: &TempDir) -> io::Result<(PathBuf, File)> {
-        let mut path = fs::canonicalize(tempdir.path())?;
-        path.push("test.log");
-
+        let path = tempdir.path().join("test.log");
         let file = File::create(&path)?;

         Ok((path, file))
```

In the process, we've also fixes some issues in our link handling.

As a final yak shave, let's rename:

- `LiveFile` -> `WatchedFile`
- `live_files` -> `watched_files`

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -25,10 +25,10 @@ enum Event<'collector> {
         canonical_path: PathBuf,
     },
     Append {
-        live_file: &'collector mut LiveFile,
+        watched_file: &'collector mut WatchedFile,
     },
     Truncate {
-        live_file: &'collector mut LiveFile,
+        watched_file: &'collector mut WatchedFile,
     },
 }

@@ -44,8 +44,8 @@ impl Event<'_> {
     fn path(&self) -> &Path {
         match self {
             Event::Create { path, .. } => path,
-            Event::Append { live_file, .. } | Event::Truncate { live_file, .. } => {
-                &live_file.paths[0].as_ref()
+            Event::Append { watched_file, .. } | Event::Truncate { watched_file, .. } => {
+                &watched_file.paths[0].as_ref()
             }
         }
     }
@@ -58,7 +58,7 @@ impl std::fmt::Display for Event<'_> {
 }

 #[derive(Debug)]
-struct LiveFile {
+struct WatchedFile {
     paths: Vec<String>,
     reader: BufReader<File>,
     entry_buf: String,
@@ -67,7 +67,7 @@ struct LiveFile {
 struct Collector<W: Watcher> {
     root_path: PathBuf,
     root_wd: W::Descriptor,
-    live_files: HashMap<W::Descriptor, LiveFile>,
+    watched_files: HashMap<W::Descriptor, WatchedFile>,
     watched_paths: HashMap<PathBuf, W::Descriptor>,
     watcher: W,
     entry_buf: std::vec::IntoIter<LogEntry>,
@@ -102,7 +102,7 @@ impl<W: Watcher> Collector<W> {
         let mut collector = Self {
             root_path,
             root_wd,
-            live_files: HashMap::new(),
+            watched_files: HashMap::new(),
             watched_paths: HashMap::new(),
             watcher,
             entry_buf: vec![].into_iter(),
@@ -130,21 +130,21 @@ impl<W: Watcher> Collector<W> {
         let watcher_events = self.watcher.read_events_blocking()?;

         let mut entries = Vec::new();
-        let mut read_file = |live_file: &mut LiveFile| -> io::Result<()> {
-            while live_file.reader.read_line(&mut live_file.entry_buf)? != 0 {
-                if live_file.entry_buf.ends_with('\n') {
-                    live_file.entry_buf.pop();
+        let mut read_file = |watched_file: &mut WatchedFile| -> io::Result<()> {
+            while watched_file.reader.read_line(&mut watched_file.entry_buf)? != 0 {
+                if watched_file.entry_buf.ends_with('\n') {
+                    watched_file.entry_buf.pop();

                     let mut metadata = HashMap::new();
-                    for path in &live_file.paths {
+                    for path in &watched_file.paths {
                         metadata.insert("path".to_string(), path.clone());
                         entries.push(LogEntry {
-                            line: live_file.entry_buf.clone(),
+                            line: watched_file.entry_buf.clone(),
                             metadata: metadata.clone(),
                         });
                     }

-                    live_file.entry_buf.clear();
+                    watched_file.entry_buf.clear();
                 }
             }
             Ok(())
@@ -158,7 +158,7 @@ impl<W: Watcher> Collector<W> {
             for event in self.check_event(&watcher_event)? {
                 debug!("{}", event);

-                let live_file = match event {
+                let watched_file = match event {
                     Event::Create {
                         path,
                         canonical_path,
@@ -166,19 +166,19 @@ impl<W: Watcher> Collector<W> {
                         new_paths.push((path, canonical_path));
                         continue;
                     }
-                    Event::Append { live_file } => live_file,
-                    Event::Truncate { live_file } => {
-                        Self::handle_event_truncate(live_file)?;
-                        live_file
+                    Event::Append { watched_file } => watched_file,
+                    Event::Truncate { watched_file } => {
+                        Self::handle_event_truncate(watched_file)?;
+                        watched_file
                     }
                 };

-                read_file(live_file)?;
+                read_file(watched_file)?;
             }

             for (path, canonical_path) in new_paths {
-                let live_file = self.handle_event_create(path, canonical_path)?;
-                read_file(live_file)?;
+                let watched_file = self.handle_event_create(path, canonical_path)?;
+                read_file(watched_file)?;
             }
         }

@@ -206,7 +206,7 @@ impl<W: Watcher> Collector<W> {
             return Ok(events);
         }

-        let live_file = match self.live_files.get_mut(watcher_event.descriptor()) {
+        let watched_file = match self.watched_files.get_mut(watcher_event.descriptor()) {
             None => {
                 warn!(
                     "Received event for unregistered watch descriptor: {:?}",
@@ -214,16 +214,16 @@ impl<W: Watcher> Collector<W> {
                 );
                 return Ok(vec![]);
             }
-            Some(live_file) => live_file,
+            Some(watched_file) => watched_file,
         };

-        let metadata = live_file.reader.get_ref().metadata()?;
-        let seekpos = live_file.reader.seek(io::SeekFrom::Current(0))?;
+        let metadata = watched_file.reader.get_ref().metadata()?;
+        let seekpos = watched_file.reader.seek(io::SeekFrom::Current(0))?;

         if seekpos <= metadata.len() {
-            Ok(vec![Event::Append { live_file }])
+            Ok(vec![Event::Append { watched_file }])
         } else {
-            Ok(vec![Event::Truncate { live_file }])
+            Ok(vec![Event::Truncate { watched_file }])
         }
     }

@@ -231,16 +231,16 @@ impl<W: Watcher> Collector<W> {
         &mut self,
         path: PathBuf,
         canonical_path: PathBuf,
-    ) -> io::Result<&mut LiveFile> {
+    ) -> io::Result<&mut WatchedFile> {
         if let Some(wd) = self.watched_paths.get(&canonical_path) {
             let wd = wd.clone();

-            // unwrap is safe because we any `wd` in `watched_paths` must be present in `live_files`
-            let live_file = self.live_files.get_mut(&wd).unwrap();
-            live_file.paths.push(path.to_string_lossy().to_string());
+            // unwrap is safe because we any `wd` in `watched_paths` must be present in `watched_files`
+            let watched_file = self.watched_files.get_mut(&wd).unwrap();
+            watched_file.paths.push(path.to_string_lossy().to_string());

             self.watched_paths.insert(path, wd);
-            Ok(live_file)
+            Ok(watched_file)
         } else {
             let wd = self.watcher.watch_file(&canonical_path)?;

@@ -255,7 +255,7 @@ impl<W: Watcher> Collector<W> {
             self.watched_paths.insert(path, wd.clone());
             self.watched_paths.insert(canonical_path, wd.clone());

-            Ok(self.live_files.entry(wd).or_insert(LiveFile {
+            Ok(self.watched_files.entry(wd).or_insert(WatchedFile {
                 paths,
                 reader,
                 entry_buf: String::new(),
@@ -263,9 +263,9 @@ impl<W: Watcher> Collector<W> {
         }
     }

-    fn handle_event_truncate(live_file: &mut LiveFile) -> io::Result<()> {
-        live_file.reader.seek(io::SeekFrom::Start(0))?;
-        live_file.entry_buf.clear();
+    fn handle_event_truncate(watched_file: &mut WatchedFile) -> io::Result<()> {
+        watched_file.reader.seek(io::SeekFrom::Start(0))?;
+        watched_file.entry_buf.clear();
         Ok(())
     }
 }
```

## Coming up for air

We've gone down a bit of a rabbit hole here.
We've also quite significantly changed `directory`, and we'd probably want to propagate those changes to our `kubernetes` collector.
For now, let's remove `log_collector::kubernetes` and finish up:

```
$ rm src/log_collector/kubernetes.rs
```

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -3,7 +3,6 @@
 //! The interface for log collection in `monitoring-rs`.

 pub mod directory;
-pub mod kubernetes;
 mod watcher;

 use std::io;
```

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -32,7 +32,6 @@ struct Args {
 arg_enum! {
     enum CollectorArg {
         Directory,
-        Kubernetes,
     }
 }

@@ -76,12 +75,6 @@ fn init_collector(args: Args) -> io::Result<Box<dyn Collector + Send>> {
                 root_path: args.root_path.unwrap(),
             })?))
         }
-        CollectorArg::Kubernetes => {
-            use log_collector::kubernetes::{self, Config};
-            Ok(Box::new(kubernetes::initialize(Config {
-                root_path: args.root_path,
-            })?))
-        }
     }
 }

```

And now all tests are passing again:

```
$ cargo test
...
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

But what about Linux?

```
$ make dockertest
...
test_1        | failures:
test_1        |
test_1        | ---- log_collector::directory::tests::file_with_internal_symlink stdout ----
test_1        | thread 'log_collector::directory::tests::file_with_internal_symlink' panicked at 'assertion failed: `(left == right)`
test_1        |   left: `3`,
test_1        |  right: `2`', src/log_collector/directory.rs:404:9
...
```

Drama!
Let's take a guess that this is related to the fact that, on MacOS, the canonical path is never under the `root_path` due to the `/private` symlink shenanigans.
We can update our 'internal symlink' test case to make sure this is covered properly:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -380,15 +380,14 @@ mod tests {
     #[test]
     fn file_with_internal_symlink() -> test::Result {
         let root_dir = tempfile::tempdir()?;
+        let root_path = root_dir.path().canonicalize()?;

         let (src_path, mut file) = create_log_file(&root_dir)?;
         let src_path_canonical = src_path.canonicalize()?;
-        let dst_path = root_dir.path().join("linked.log");
+        let dst_path = root_path.join("linked.log");
         unix::fs::symlink(&src_path, &dst_path)?;

-        let config = Config {
-            root_path: root_dir.path().to_path_buf(),
-        };
+        let config = Config { root_path };
         let watcher = mock::MockWatcher::new();
         let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;

@@ -398,7 +397,7 @@ mod tests {
         let entries = collector.collect_entries()?;
         assert_eq!(
             watcher.borrow().watched_paths(),
-            &vec![root_dir.path().canonicalize()?, src_path_canonical]
+            &vec![root_dir.path().canonicalize()?, src_path_canonical.clone()]
         );

         assert_eq!(entries.len(), 2);
@@ -411,7 +410,7 @@ mod tests {
             entries
         );

-        let entry = log_entry("hello?", &[("path", src_path.to_str().unwrap())]);
+        let entry = log_entry("hello?", &[("path", src_path_canonical.to_str().unwrap())]);
         assert!(
             entries.contains(&entry),
             "expected entry {:?}, but found: {:#?}",
```

We've updated our `Config` to use the canonicalized temporary directory, and updated our assertions to match.
Let's see what happens:

```
$ cargo test log_collector::directory::tests::file_with_internal_symlink
...
running 1 test
test log_collector::directory::tests::file_with_internal_symlink ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out
...
```

Huh.
And yet still:

```
$ make dockertest
...
test_1        | failures:
test_1        |
test_1        | ---- log_collector::directory::tests::file_with_internal_symlink stdout ----
test_1        | thread 'log_collector::directory::tests::file_with_internal_symlink' panicked at 'assertion failed: `(left == right)`
test_1        |   left: `3`,
test_1        |  right: `2`', src/log_collector/directory.rs:403:9
...
```

What is the meaning of this?
We're going to have to resort to some crude `dbg!`ing:

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -232,6 +232,8 @@ impl<W: Watcher> Collector<W> {
         path: PathBuf,
         canonical_path: PathBuf,
     ) -> io::Result<&mut WatchedFile> {
+        dbg!(("handle_event_create", &path, &canonical_path));
+
         if let Some(wd) = self.watched_paths.get(&canonical_path) {
             let wd = wd.clone();

```

Now what do we get locally:

```
$ cargo test log_collector::directory::tests::file_with_internal_symlink -- --nocapture
...
running 1 test
[src/log_collector/directory.rs:235] ("handle_event_create", &path, &canonical_path) = (
    "handle_event_create",
    "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpr28x4c/test.log",
    "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpr28x4c/test.log",
)
[src/log_collector/directory.rs:235] ("handle_event_create", &path, &canonical_path) = (
    "handle_event_create",
    "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpr28x4c/linked.log",
    "/private/var/folders/7d/70xzdwfd0rq27yy8c5pm88s00000gn/T/.tmpr28x4c/test.log",
)
test log_collector::directory::tests::file_with_internal_symlink ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out
...
```

This seems sensible – when iterating the files in `root_path` we first see `test.log`, which is also the canonical path.
Then, we see `linked.log`, whose canonical path is `test.log`.

On the first call to `handle_event_create`, `watched_paths` will be empty so we will definitely take the second branch and register the watch.
Furthermore, since `canonical_path` and `path` are the same, we only `path` to `watched_file.paths` and `watched_paths`.

On the second call to `handle_event_create`, `watched_paths` *will* contain the `canonical_path` so we'll take the first branch.
We assume the `path` and `canonical_path` are different, and go ahead and add `path` to `watched_file.paths` and `watched_paths`.

This assumption could be making an ass out of you and me, but let's first check what we see on Linux:

```
$ make dockertest
...
test_1        | failures:
test_1        |
test_1        | ---- log_collector::directory::tests::file_with_internal_symlink stdout ----
test_1        | [src/log_collector/directory.rs:235] ("handle_event_create", &path, &canonical_path) = (
test_1        |     "handle_event_create",
test_1        |     "/tmp/.tmpI8YiKc/linked.log",
test_1        |     "/tmp/.tmpI8YiKc/test.log",
test_1        | )
test_1        | [src/log_collector/directory.rs:235] ("handle_event_create", &path, &canonical_path) = (
test_1        |     "handle_event_create",
test_1        |     "/tmp/.tmpI8YiKc/test.log",
test_1        |     "/tmp/.tmpI8YiKc/test.log",
test_1        | )
test_1        | thread 'log_collector::directory::tests::file_with_internal_symlink' panicked at 'assertion failed: `(left == right)`
test_1        |   left: `3`,
test_1        |  right: `2`', src/log_collector/directory.rs:405:9
...
```

In this case, the files are iterated in a different order – we first see `linked.log` (whose canonical path is `test.log`), followed by `test.log` itself.

On the first call to `handle_event_create`, `watched_paths` will be empty so we will definitely take the second branch and register the watch.
This time however, `canonical_path` and `path` are different (and `canonical_path` starts with `root_path`), so we will push both `path` and `canonical_path` to `watched_file.paths` and `watched_paths`.

On the second call to `handle_event_create`, `watched_paths` *will* contain the `canonical_path` so we'll take the first branch.
Again we seem to assume that `path` and `canonical_path` are different, and go ahead an add `path` to `watched_file.paths` and `watched_paths`.

So indeed, it seems we cannot assume that `path` and `canonical_path` are different in the first branch.
But where has that assumption come from?
Probably from where we generate `Create` events:

```rust
for entry in fs::read_dir(&self.root_path)? {
    let entry = entry?;
    if self.watched_paths.contains_key(&entry.path()) {
        continue;
    }

    let path = entry.path().to_path_buf();
    let canonical_path = path.canonicalize()?;
    events.push(Event::Create {
        path,
        canonical_path,
    });
}
```

So when responding to a `watcher::Event`, we only push `Event::Create` if `watched_paths` does not already contain the path.
Given that condition, it becomes OK to assume in `handle_event_create` that `canonical_path` and `path` are different in the first branch (otherwise we would have seen it in `watched_paths` and ignored the file).

So, a simple fix would be to add an equivalent check to `initialize` (we can also remove our `dbg!`):

```diff
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -110,6 +110,10 @@ impl<W: Watcher> Collector<W> {

         for entry in fs::read_dir(&collector.root_path)? {
             let entry = entry?;
+            if collector.watched_paths.contains_key(&entry.path()) {
+                continue;
+            }
+
             let path = entry.path().to_path_buf();
             let canonical_path = path.canonicalize()?;

@@ -232,8 +236,6 @@ impl<W: Watcher> Collector<W> {
         path: PathBuf,
         canonical_path: PathBuf,
     ) -> io::Result<&mut WatchedFile> {
-        dbg!(("handle_event_create", &path, &canonical_path));
-
         if let Some(wd) = self.watched_paths.get(&canonical_path) {
             let wd = wd.clone();

```

Let's check things still work locally:

```
$ cargo test log_collector::directory::tests::file_with_internal_symlink
...
running 1 test
test log_collector::directory::tests::file_with_internal_symlink ... ok
...
```

Good good.
Now what about Linux?

```
$ make dockertest
...
test_1        | test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

Woohoo 🎉

Before we extricate ourselves from this rabbit hole, let's also add a similar test where the `root_path` is a symlink, to make sure we're covered in both cases:

```diff
diff --git a/src/log_collector/directory.rs b/src/log_collector/directory.rs
index 5e45d56..08efc32 100644
--- a/src/log_collector/directory.rs
+++ b/src/log_collector/directory.rs
@@ -425,6 +425,56 @@ mod tests {
         Ok(())
     }

+    #[test]
+    fn initialize_with_symlink_and_file_with_internal_symlink() -> test::Result {
+        let root_dir_parent = tempfile::tempdir()?;
+        let logs_dir = tempfile::tempdir()?;
+
+        let root_path = root_dir_parent.path().join("logs");
+        unix::fs::symlink(logs_dir.path(), &root_path)?;
+
+        let (src_path, mut file) = create_log_file(&logs_dir)?;
+        let src_path_canonical = src_path.canonicalize()?;
+        let dst_path = root_path.join("linked.log");
+        unix::fs::symlink(&src_path, &dst_path)?;
+
+        let config = Config {
+            root_path: root_path.clone(),
+        };
+        let watcher = mock::MockWatcher::new();
+        let mut collector = Collector::initialize(config, Rc::clone(&watcher))?;
+
+        writeln!(file, "hello?")?;
+        watcher.borrow_mut().add_event(src_path_canonical.clone());
+
+        let entries = collector.collect_entries()?;
+        assert_eq!(
+            watcher.borrow().watched_paths(),
+            &vec![logs_dir.path().canonicalize()?, src_path_canonical]
+        );
+
+        assert_eq!(entries.len(), 2);
+
+        let entry = log_entry("hello?", &[("path", dst_path.to_str().unwrap())]);
+        assert!(
+            entries.contains(&entry),
+            "expected entry {:?}, but found: {:#?}",
+            entry,
+            entries
+        );
+
+        let path = root_path.join(src_path.file_name().unwrap());
+        let entry = log_entry("hello?", &[("path", path.to_str().unwrap())]);
+        assert!(
+            entries.contains(&entry),
+            "expected entry {:?}, but found: {:#?}",
+            entry,
+            entries
+        );
+
+        Ok(())
+    }
+
     #[test]
     fn collect_entries_empty_file() -> test::Result {
         let tempdir = tempfile::tempdir()?;
```

Will it blend?

```
$ cargo test
...
test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

```
$ make dockertest
...
test_1        | test result: ok. 14 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
...
```

Roight.
Let's pack it up.

## Conclusions

We started trying to introduce a new `Collector` implementation for Kubernetes that could associate pod metadata with collected `LogEntry`s.
In pursuit of this objective, we introduced `structopt` and used it to define arguments for choosing and configuring a log collector.

We attempted to kickstart the `log_collector::kubernetes` module by copying the `log_collector::directory` module and making changes from there.
However, we quickly ran into some issues with differences between our `kqueue` and `inotify` `Watcher` implementations, as well as several gaps in our testing around behaviour with symlinked log files.

Then we went down a bit of a rabbit hole, but in the process we wrote a lot of documentation for `watcher` and more clearly articulated the expectations.
We also reworked the API to allow multiple `Watcher` implementations to work with `watcher::Descriptor` and `watcher::Event`, which allowed us to create a mock watcher that verified expectations.
We then introduced some tests to `log_collector::directory` which used the mock watcher to find issues with our handling of symlinks, and we fixed those issues.

Finally, we cleared up our brief attempt at `log_collector::kubernetes`, which we can pick up again in the next post!
