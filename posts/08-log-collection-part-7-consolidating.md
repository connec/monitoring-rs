# Log collection (part 7 – consolidating)

After a bit of a slog we have built ourselves an end-to-end log collector:

```
$ make writer monitoring
...

# in another tab
$ curl 127.0.0.1:8000/logs//var/log/containers/writer.log | jq
...
[
  "Mon Dec 14 20:10:20 UTC 2020",
  "Mon Dec 14 20:10:21 UTC 2020",
  "Mon Dec 14 20:10:22 UTC 2020"
]
```

We have taken quite a few shortcuts to get here, including:

- `docker build`s are getting veerry long (\~10mins on my machine!).
- `main` is quite sprawling.
- The concurrency patterns are confusing (`main` and `api` are async, but `database` and `collector` are sync).
- We have written very few tests.
  Our 'development loop' has involved builing and running the binary in Docker, and manually validating expected log/request output.

And this is just technical debt, we are also missing significant features:

- Storing Kubernetes metadata with logs.
- Useful querying (by time and/or label).
- Retention management.

All of these are critical features for the log collector to be usable in a realistic scenario.

## Technical debt

Let's pay down some of our technical debt to free us from heavy interest payments.
This will free up more income to spend on additional features.

### `cargo chef`

Enough metaphors.
[`cargo-chef`](https://crates.io/crates/cargo-chef) is a `cargo` sub-command specifically intended to speed up Docker builds (developed as part of the [Zero to Production](https://www.zero2prod.com/) tutorial series).
It's designed to be installed on the container when building, in order to pre-compile dependencies.

We can follow the example from the `cargo-chef` docs mostly verbatim:

```Dockerfile
# Dockerfile
FROM rust:1.46.0-alpine as build_base

WORKDIR /build
RUN apk add --no-cache musl-dev && cargo install cargo-chef


FROM build_base as planner

COPY . .
RUN cargo chef prepare


FROM build_base as cacher

COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release


FROM build_base as builder

COPY . .
COPY --from=cacher /build/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
RUN cargo build --release


FROM alpine as runtime

RUN apk add --no-cache tini

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["monitoring-rs"]

COPY --from=builder /build/target/release/monitoring-rs /usr/local/bin
```

Let's give it a try:

```
$ make monitoring
...
<some time late>
...
monitoring_1  | [2020-12-18T16:11:11Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
```

This first build may take a while, but lets make a small change in `main.rs` and see how the build behaves:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -36,6 +36,7 @@ async fn main() -> io::Result<()> {
     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

     let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
+        debug!("Collector started");
         let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
         let mut buffer = [0; 1024];
         loop {
```

```
$ make down monitoring
...
   Compiling monitoring-rs v0.1.0 (/build)
    Finished release [optimized] target(s) in 24.12s
...
monitoring_1  | [2020-12-18T16:14:37Z DEBUG monitoring_rs] Collector started
monitoring_1  | [2020-12-18T16:14:37Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
```

This time, the only compilation is for `monitoring-rs` itself, none of the dependencies!
However, `COPY --from=cacher /build/target target` still seems to take a few seconds on my builds, perhaps we can collapse `cacher` and `builder`?

```diff
--- a/Dockerfile
+++ b/Dockerfile
@@ -11,17 +11,12 @@ COPY . .
 RUN cargo chef prepare


-FROM build_base as cacher
+FROM build_base as builder

 COPY --from=planner /build/recipe.json recipe.json
 RUN cargo chef cook --release

-
-FROM build_base as builder
-
 COPY . .
-COPY --from=cacher /build/target target
-COPY --from=cacher /usr/local/cargo /usr/local/cargo
 RUN cargo build --release


```

```
$ make down monitoring
...
   Compiling monitoring-rs v0.1.0 (/build)
    Finished release [optimized] target(s) in 23.83s
...
monitoring_1  | [2020-12-18T16:18:50Z DEBUG monitoring_rs] Collector started
monitoring_1  | [2020-12-18T16:18:50Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
```

Nice.
And what if we revert our logging and try again?

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -36,7 +36,6 @@ async fn main() -> io::Result<()> {
     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

     let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
-        debug!("Collector started");
         let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
         let mut buffer = [0; 1024];
         loop {
```

```
$ make down monitoring
...
   Compiling monitoring-rs v0.1.0 (/build)
    Finished release [optimized] target(s) in 26.18s
...
monitoring_1  | [2020-12-18T16:20:29Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
```

Beautiful.
Let's leave this here for now.

### `main()`scaping

Let's take a critical look at `main()` and see if we can propose some improvements.

```rust
let mut data_directory = env::current_dir()?;
data_directory.push(".data");
fs::create_dir_all(&data_directory)?;

let database = Arc::new(RwLock::new(Database::open(log_database::Config {
    data_directory,
})?));
```

These 6 lines of code are preparing a directory for our database, and then initializing a database with the necessary wrappers to allow us to share it between threads.
We could pull these into an `init_database` function.

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -25,13 +25,7 @@ const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
 async fn main() -> io::Result<()> {
     env_logger::init();

-    let mut data_directory = env::current_dir()?;
-    data_directory.push(".data");
-    fs::create_dir_all(&data_directory)?;
-
-    let database = Arc::new(RwLock::new(Database::open(log_database::Config {
-        data_directory,
-    })?));
+    let database = init_database()?;

     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

@@ -53,3 +47,13 @@ async fn main() -> io::Result<()> {

     Ok(())
 }
+
+fn init_database() -> io::Result<Arc<RwLock<Database>>> {
+    let mut data_directory = env::current_dir()?;
+    data_directory.push(".data");
+    fs::create_dir_all(&data_directory)?;
+
+    let config = log_database::Config { data_directory };
+    let database = Database::open(config)?;
+    Ok(Arc::new(RwLock::new(database)))
+}
```

We've also split apart the single construction statement to make it clearer what's going on.

Next up we have our `api_handle` assignment:

```rust
let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");
```

This is already pretty clean.
We could create an `init_api` function for consistency, but let's not for now.

Next we have our `collector_handle` setup:

```rust
let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
    let mut buffer = [0; 1024];
    loop {
        let entries = collector.collect_entries(&mut buffer)?;
        let mut database = block_on(database.write());
        for entry in entries {
            let key = entry.path.to_string_lossy();
            database.write(&key, &entry.line)?;
        }
    }
});
let collector_handle = blocking::unblock(|| collector_thread.join().unwrap());
```

This is the heaviest part of `main()` currently and we would like to split it out.
Let's start by just moving the whole thing to an `init_collector` function.

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -8,6 +8,7 @@ mod log_database;

 use std::env;
 use std::fs;
+use std::future::Future;
 use std::io;
 use std::sync::Arc;
 use std::thread;
@@ -29,19 +30,7 @@ async fn main() -> io::Result<()> {

     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

-    let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
-        let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
-        let mut buffer = [0; 1024];
-        loop {
-            let entries = collector.collect_entries(&mut buffer)?;
-            let mut database = block_on(database.write());
-            for entry in entries {
-                let key = entry.path.to_string_lossy();
-                database.write(&key, &entry.line)?;
-            }
-        }
-    });
-    let collector_handle = blocking::unblock(|| collector_thread.join().unwrap());
+    let collector_handle = init_collector(database);

     api_handle.try_join(collector_handle).await?;

@@ -57,3 +46,19 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     let database = Database::open(config)?;
     Ok(Arc::new(RwLock::new(database)))
 }
+
+fn init_collector(database: Arc<RwLock<Database>>) -> impl Future<Output = io::Result<()>> {
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
+    blocking::unblock(|| collector_thread.join().unwrap())
+}
```

Easy peasy.
Though... something seems odd about our thread handling.
We spawn a thread with `thread::spawn`, and then transform the `collector_thread.join()` into an asynchronous operation using [`blocking::unblock`](https://docs.rs/blocking/1.0.2/blocking/fn.unblock.html), which notes:

> Runs blocking code on a thread pool.

So, we're spawning a thread in a threadpool in order to wait for another thread to finish?
That seems a bit wasteful.
We should be able to use [`async_std::task::spawn_blocking`](https://docs.rs/async-std/1.8.0/async_std/task/fn.spawn_blocking.html) which is exactly intended for spawning long-running threads, returning an `.await`able `JoinHandle`.
Sadly this is only available on 'unstable', which we don't want to use.
However, if we look at [the source](https://docs.rs/async-std/1.8.0/src/async_std/task/spawn_blocking.rs.html#33-39), we can see that this just `async_std::task::spawn(blocking::unblock(...))`, so we can simplify `init_collector` with what we already have available:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -11,11 +11,10 @@ use std::fs;
 use std::future::Future;
 use std::io;
 use std::sync::Arc;
-use std::thread;

 use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
-use async_std::task::block_on;
+use async_std::task;

 use log_collector::Collector;
 use log_database::Database;
@@ -48,17 +47,16 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
 }

 fn init_collector(database: Arc<RwLock<Database>>) -> impl Future<Output = io::Result<()>> {
-    let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
+    task::spawn(blocking::unblock(move || {
         let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
         let mut buffer = [0; 1024];
         loop {
             let entries = collector.collect_entries(&mut buffer)?;
-            let mut database = block_on(database.write());
+            let mut database = task::block_on(database.write());
             for entry in entries {
                 let key = entry.path.to_string_lossy();
                 database.write(&key, &entry.line)?;
             }
         }
-    });
-    blocking::unblock(|| collector_thread.join().unwrap())
+    }))
 }
```

In fact, let's make `init_collector` actually run the loop, and perform the asynchronisation in `main`:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -8,7 +8,6 @@ mod log_database;

 use std::env;
 use std::fs;
-use std::future::Future;
 use std::io;
 use std::sync::Arc;

@@ -29,7 +28,7 @@ async fn main() -> io::Result<()> {

     let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

-    let collector_handle = init_collector(database);
+    let collector_handle = task::spawn(blocking::unblock(move || init_collector(database)));

     api_handle.try_join(collector_handle).await?;

@@ -46,17 +45,15 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     Ok(Arc::new(RwLock::new(database)))
 }

-fn init_collector(database: Arc<RwLock<Database>>) -> impl Future<Output = io::Result<()>> {
-    task::spawn(blocking::unblock(move || {
-        let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
-        let mut buffer = [0; 1024];
-        loop {
-            let entries = collector.collect_entries(&mut buffer)?;
-            let mut database = task::block_on(database.write());
-            for entry in entries {
-                let key = entry.path.to_string_lossy();
-                database.write(&key, &entry.line)?;
-            }
+fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {
+    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
+    let mut buffer = [0; 1024];
+    loop {
+        let entries = collector.collect_entries(&mut buffer)?;
+        let mut database = task::block_on(database.write());
+        for entry in entries {
+            let key = entry.path.to_string_lossy();
+            database.write(&key, &entry.line)?;
         }
-    }))
+    }
 }
```

This seems nicer.
It feels like we may be able to add an `Iterator` (or `Stream` if async) interface to our collector, but let's worry about that another time.

### Confusing concurrency

Our `task::spawn`ing shenanigans have probably done enough to make the concurrency clearer now – `api_handle` and `collector_handle` are `Future`s that we can then join concurrently.

### Tests

We've left the biggest omission 'til last – only our `Database` has any tests.
We should be able to create some high level tests for our API and log collector.
Let's proceed in alphabetical order.

#### Tests for `api`

Let's aim for a couple of fairly high level tests that start a `Server` and:

- Check that non-existent keys return 404.
- Check that existing keys return 200 with the lines in the DB.

These would qualify as 'integration tests' normally, since we will use a 'real' `Server` (and `Database`), rather than mocking or omitting those dependencies.
For starters, let's expose an `open_temp_database` helper function from `log_database::test`.

```diff
--- a/src/log_database/mod.rs
+++ b/src/log_database/mod.rs
@@ -138,16 +138,32 @@ impl Database {
 }

 #[cfg(test)]
-mod tests {
-    use super::{Config, Database};
+pub mod test {
+    use tempfile::TempDir;

-    #[test]
-    fn test_new_db() {
+    use super::Config;
+    use super::Database;
+
+    pub fn open_temp_database() -> (Database, TempDir) {
         let tempdir = tempfile::tempdir().expect("unable to create tempdir");
         let config = Config {
             data_directory: tempdir.path().to_path_buf(),
         };
-        let mut database = Database::open(config).expect("unable to open database");
+        (
+            Database::open(config).expect("unable to open database"),
+            tempdir,
+        )
+    }
+}
+
+#[cfg(test)]
+mod tests {
+    use super::test::open_temp_database;
+    use super::{Config, Database};
+
+    #[test]
+    fn test_new_db() {
+        let (mut database, _tempdir) = open_temp_database();

         assert_eq!(
             database.read("foo").expect("unable to read from database"),
@@ -173,27 +189,18 @@ mod tests {

     #[test]
     fn test_existing_db() {
-        let tempdir = tempfile::tempdir().expect("unable to create tempdir");
-
-        {
-            let config = Config {
-                data_directory: tempdir.path().to_path_buf(),
-            };
-            let mut database = Database::open(config).expect("unable to open database");
+        let (mut database, _tempdir) = open_temp_database();
+        let data_directory = database.data_directory.clone();

-            database
-                .write("foo", "line1")
-                .expect("failed to write to database");
-            database
-                .write("foo", "line2")
-                .expect("failed to write to database");
-
-            drop(database);
-        }
+        database
+            .write("foo", "line1")
+            .expect("failed to write to database");
+        database
+            .write("foo", "line2")
+            .expect("failed to write to database");
+        drop(database);

-        let config = Config {
-            data_directory: tempdir.path().to_path_buf(),
-        };
+        let config = Config { data_directory };
         let database = Database::open(config).expect("unable to open database");

         assert_eq!(
```

Note that we have to return the `TempDir` instance from `open_temp_database`.
It would otherwise be dropped, and `TempDir::drop` deletes the directory!

We should double check everything's still green:

```
$ cargo test
...
running 2 tests
test log_database::tests::test_new_db ... ok
test log_database::tests::test_existing_db ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Very nice!

We've done this to make it easier for us to create temporary databases in tests for our other modules, starting with `api`:

```diff
--- a/src/api/mod.rs
+++ b/src/api/mod.rs
@@ -26,3 +26,40 @@ async fn read_logs(req: tide::Request<State>) -> tide::Result {
         None => tide::Response::new(tide::StatusCode::NotFound),
     })
 }
+
+#[cfg(test)]
+mod tests {
+    use async_std::sync::RwLock;
+    use std::sync::Arc;
+
+    use tide_testing::TideTestingExt;
+
+    use crate::log_database::test::open_temp_database;
+
+    #[async_std::test]
+    async fn read_logs_non_existent_key() {
+        let (database, _tempdir) = open_temp_database();
+        let api = super::server(Arc::new(RwLock::new(database)));
+
+        let response = api.get("/logs//foo").await.unwrap();
+
+        assert_eq!(response.status(), 404);
+    }
+
+    #[async_std::test]
+    async fn read_logs_existing_key() {
+        let (mut database, _tempdir) = open_temp_database();
+        database.write("/foo", "hello").unwrap();
+        database.write("/foo", "world").unwrap();
+
+        let api = super::server(Arc::new(RwLock::new(database)));
+
+        let mut response = api.get("/logs//foo").await.unwrap();
+
+        assert_eq!(response.status(), 200);
+        assert_eq!(
+            response.body_json::<Vec<String>>().await.unwrap(),
+            vec!["hello".to_string(), "world".to_string()]
+        );
+    }
+}
```

We've added [`tide-testing`](https://crates.io/crates/tide-testing), which let's us directly call `.get` (and [other methods](https://docs.rs/tide-testing/0.1.2/tide_testing/trait.TideTestingExt.html#provided-methods)) on our `Server`:

```
$ cargo add -D tide-testing
```

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -17,3 +17,4 @@ md5 = "0.7.0"

 [dev-dependencies]
 tempfile = "3.1.0"
+tide-testing = "0.1.2"
```

And now let's enjoy them passing:

```
$ cargo test
...
running 4 tests
test log_database::tests::test_existing_db ... ok
test log_database::tests::test_new_db ... ok
test api::tests::read_logs_non_existent_key ... ok
test api::tests::read_logs_existing_key ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Beautiful.

#### Tests for `log_collector`

Next up, `log_collector`.
Let's start with a simple test to check that initializing against a non-existent directory fails:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -193,3 +193,20 @@ impl Collector {
         Ok(())
     }
 }
+
+#[cfg(test)]
+mod tests {
+    use std::path::PathBuf;
+
+    use super::Collector;
+
+    #[test]
+    fn initialize_non_existent_directory() {
+        let mut path = PathBuf::new();
+        path.push("/fakedirectory");
+
+        let collector_result = Collector::initialize(&path);
+
+        assert_eq!(collector_result.is_err(), true);
+    }
+}
```

And...

```
$ cargo test
...
<explosion>
...
  = note: Undefined symbols for architecture x86_64:
            "_inotify_init", referenced from:
                inotify::inotify::Inotify::init::hb89b5218f26a1dc7 in libinotify-8f703d9a3d855ebc.rlib(inotify-8f703d9a3d855ebc.inotify.7915tgwq-cgu.5.rcgu.o)
            "_inotify_add_watch", referenced from:
                inotify::inotify::Inotify::add_watch::ha146228cc51e1008 in monitoring_rs-a81dd6a39c5668ec.3o1xxpz21drjol5b.rcgu.o
                inotify::inotify::Inotify::add_watch::hf57567310d49a925 in monitoring_rs-a81dd6a39c5668ec.3o1xxpz21drjol5b.rcgu.o
          ld: symbol(s) not found for architecture x86_64
          clang: error: linker command failed with exit code 1 (use -v to see invocation)
```

Aha, of course `inotify` isn't available on Mac.
How can we deal with this?
We *could* use conditional compilation to offer an `Inotify` alternative on Mac (perhaps only when also in test), but if we do so we need to be aware that any tests involving that functionality will not be representative of the 'real world'.

For now, let's back out that test:

```diff
--- a/src/log_collector/mod.rs
+++ b/src/log_collector/mod.rs
@@ -193,20 +193,3 @@ impl Collector {
         Ok(())
     }
 }
-
-#[cfg(test)]
-mod tests {
-    use std::path::PathBuf;
-
-    use super::Collector;
-
-    #[test]
-    fn initialize_non_existent_directory() {
-        let mut path = PathBuf::new();
-        path.push("/fakedirectory");
-
-        let collector_result = Collector::initialize(&path);
-
-        assert_eq!(collector_result.is_err(), true);
-    }
-}
```

Let's start by making the `inotify` dependency platform-specific:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -8,13 +8,15 @@ edition = "2018"

 [dependencies]
 env_logger = "0.8.1"
-inotify = { version = "0.8.3", default-features = false }
 log = "0.4.11"
 tide = "0.15.0"
 async-std = { version = "1.7.0", features = ["attributes"] }
 blocking = "1.0.2"
 md5 = "0.7.0"

+[target.'cfg(target_os = "linux")'.dependencies]
+inotify = { version = "0.8.3", default-features = false }
+
 [dev-dependencies]
 tempfile = "3.1.0"
 tide-testing = "0.1.2"
```

Now we get an expected error when `cargo check`ing:

```
$ cargo check
...
error[E0432]: unresolved import `inotify`
 --> src/log_collector/mod.rs:8:5
  |
8 | use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
  |     ^^^^^^^ use of undeclared type or module `inotify`

error[E0433]: failed to resolve: use of undeclared type or module `inotify`
   --> src/log_collector/mod.rs:125:24
    |
125 |         inotify_event: inotify::Event<&'ev OsStr>,
    |                        ^^^^^^^ use of undeclared type or module `inotify`

error: aborting due to 2 previous errors
```

Boohoo.
There are at least three different ways we could proceed:

1. Use conditional compilation to implement a compatible API for MacOS.
   This could use a native MacOS mechanism or simply behave as a stub, since we only need it for testing (we are assuming the operating environment will always be Linux).
   Equivalently, we could use a crate for cross-platform file watching, such as [`notify`](https://docs.rs/notify/4.0.15/notify/).

   Sadly, file watching is notoriously different between platforms, so behaviour on one platform may differ subtly on other platforms in a way that defies good testing.
   For example, MacOS' `fsevent` API (which `notify` uses) doesn't emit 'modify' events until the file is closed – which is a fairly critical feature for a log collector!

1. Use conditional compilation to run tests involving `inotify` only on Linux (and introduce a `make` target to run tests in a container).

   Conditional compilation has the obvious drawback of requiring us to use Docker to run all our tests (with the slower feedback loop that implies).

1. Refactor the log watcher to maximise the API surface that does not depend on `inotify`.

   This leaves the `inotify` interactions untested.

In an ideal world, the 1st option would seem to be the best, as this would let us write representative tests and run them locally.
Given the disparities between platforms, particularly fsevent's limitations, this is not sufficient for us.

Using conditionally compiled tests gives us a way to test the platform-specific behaviour, and so is something we should leverage.
However, we would like to test as much as possible natively in order keep a fast feedback loop.
Hence, we should take a two-pronged approach:

1. Refactor the `log_collector` API to separate file system events from collector behaviour.
1. Use conditionally compiled tests to verify the limited remaining platform dependent behaviour.

#### Solving today's problems tomorrow

This could be a chunk of work, so let's consider it separately in the next episode of Reinventing the Wheel.
For now let's just tidy up a little, so that `cargo check` fails with a clearer error when `inotify` isn't available:

```diff
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,11 +1,13 @@
 // main.rs
-#[macro_use]
+#[cfg_attr(target_os = "linux", macro_use)]
 extern crate log;

 mod api;
-mod log_collector;
 mod log_database;

+#[cfg(target_os = "linux")]
+mod log_collector;
+
 use std::env;
 use std::fs;
 use std::io;
@@ -15,9 +17,11 @@ use async_std::prelude::FutureExt;
 use async_std::sync::RwLock;
 use async_std::task;

+#[cfg(target_os = "linux")]
 use log_collector::Collector;
 use log_database::Database;

+#[cfg(target_os = "linux")]
 const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

 #[async_std::main]
@@ -45,6 +49,7 @@ fn init_database() -> io::Result<Arc<RwLock<Database>>> {
     Ok(Arc::new(RwLock::new(database)))
 }

+#[cfg(target_os = "linux")]
 fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {
     let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
     let mut buffer = [0; 1024];
@@ -57,3 +62,16 @@ fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {
         }
     }
 }
+
+#[cfg(not(test))]
+#[cfg(not(target_os = "linux"))]
+fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
+    compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
+    unreachable!()
+}
+
+#[cfg(test)]
+#[cfg(not(target_os = "linux"))]
+fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
+    panic!("log_collector is only available on Linux due to dependency on `inotify`")
+}
```

Now if we try to run `cargo check` natively we see:

```
$ cargo check
    Checking monitoring-rs v0.1.0
error: log_collector is only available on Linux due to dependency on `inotify`
  --> src/main.rs:69:5
   |
69 |     compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
   |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

error: aborting due to previous error

error: could not compile `monitoring-rs`.

To learn more, run the command again with --verbose.
```

`cargo test` still works, however, since we generate a run-time panic in that context:

```
$ cargo test
    Finished test [unoptimized + debuginfo] target(s) in 0.14s
     Running target/debug/deps/monitoring_rs-1aac4ad619643289

running 4 tests
test log_database::tests::test_new_db ... ok
test log_database::tests::test_existing_db ... ok
test api::tests::read_logs_non_existent_key ... ok
test api::tests::read_logs_existing_key ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Finally, make monitoring will still work:

```
$ make down writer monitoring
...
Attaching to monitoring-rs_monitoring_1
monitoring_1  | [2020-12-20T16:11:36Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-20T16:11:36Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-20T16:11:36Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
^C

$ make down
```

Looks good, and this is where we shall stop for now.

[Back to the README](../README.md#posts)
