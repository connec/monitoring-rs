# Log collection (part 5 – recentring)

We've now spent quite a bit of time digging into log collection.
We have arrived at an implementation that successfully detects log writes from all containers in the cluster, and re-logs them (creating quite the explosion of logs!).

Obviously, we don't want to simply re-log the logs of all containers.
We want to store the logs, so that we can later query them.
Furthermore, we would like to be able to query them by Kubernetes metadata (such as the name of the pod, the name of the container, labels on the pod, etc.).

Before digging into the substantial areas of storage and querying, or expanding our collector implementation to account for Kubernetes metadata, let's pause and lay down a target architecture based on what we've learned so far.
This will hopefully allow us to restructure our implementation to allow us to iterate on broader functionality without rebuilding `main.rs` every time!

## Some diagrams

Let's start with a trivial diagram based on the separation we've already talked about: log collection and log persistence:

![First architecture diagram with boxes for `log_collector` and `log_database`, connected by an arrow (from `log_collector` to `log_database`) representing data flow.](../media/6-target-architecture/architecture-1.png)

We imagine two modules within the `monitoring-rs` package:

- `log_collector` will contain the implementation of our log collector, including elements like capturing Kubernetes metadata.
- `log_database` will contain the implementation of our log database.

The arrow represent the data flow of log entries from the collector to the database.

### Enhance!

Representing data flow between the modules begs the question of where data is coming from, and where it later goes.
Let's recall our first high-level requirements from [Discovery](0-discovery.md#Our-requirements):

> - Accept logs/metrics from Kubernetes.
> - Store those logs/metrics in a format suitable for searching and alerting.
> - Provide an API and UI for searching and visualising ingested logs/metrics.
> - Provide an API and UI for configuring alert rules based on incoming logs/metrics.

Focusing on the first 3 for now, we could add "Kubernetes" and "UI" to our diagram.
We can also add `api` as a module.

![Enhanced architecture diagram with "kubernetes" as an external data source (arrow to `log_collector`), an "api" module (arrow from `log_database`), and "ui" as the termination of data ((arrow from `api`)).](../media/6-target-architecture/architecture-2.png)

### Collector detail

We think our `log_collector` module will consume log files from Kubernetes nodes and metadata from the API.
Let's split those out in our diagram:

!["kubernetes" has been split into sub-boxes for "log files" and "api". Both have arrows into `log_collector`.](../media/6-target-architecture/architecture-3.png)

### Deployment topology

We know that Kubernetes writes container logs to the filesystem of the node hosting the container.
This means we will need an instance of our log collector running on every node.
Let's aspirationally aim for a single `DaemonSet` deployment.

Let's rearrange the diagram to separate the Kubernetes node from the Kubernetes API and include a representation of the `DaemonSet`:

!["Kubernetes API" and "UI" now sandwich a "DaemonSet". The "DaemonSet" is underpinned by a "Kubernetes node". The "DaemonSet" includes a box for "log files volume" (arrow from "Kubernettes node") and a box for `monitoring_rs`. `monitoring_rs` has the same contents as before, in reverse order and with "Kubernetes API" and "log files volume" now feeding into `log_collector`.](../media/6-target-architecture/architecture-4.png)

### Was this helpful?

I'm not sure...
It does give us an at-a-glance outline of our system's external dependencies:

- The Kubernetes node's log files volume.
- The Kubernetes API.

It also gives us an idea about how to structure the major pieces of our application, with separate modules for `log_collector`, `log_database`, and `api`.

## Reorganise

Let's reorganise our Rust project in-line with our target modules:

```
src/
  main.rs
  log_collector/mod.rs
```

We can start by simply renaming our existing `main.rs` to `log_collector/mod.rs`:

```sh
mkdir src/log_collector
mv src/main.rs src/log_collector/mod.rs
```

Let's strip `main()`, `extern crate`, and `CONTAINER_LOG_DIRECTORY` from this and expose our `Collector`:

```diff
--- a/src/main.rs
+++ b/src/log_collector/mod.rs
@@ -1,7 +1,4 @@
-// main.rs
-#[macro_use]
-extern crate log;
-
+// log_collector/mod.rs
 use std::collections::hash_map::HashMap;
 use std::ffi::OsStr;
 use std::fs::{self, File};
@@ -10,8 +7,6 @@ use std::path::{Path, PathBuf};

 use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

-const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";
-
 #[derive(Debug)]
 enum Event<'collector> {
     Create {
@@ -57,7 +52,7 @@ struct LiveFile {
     file: File,
 }

-struct Collector {
+pub struct Collector {
     root_path: PathBuf,
     root_wd: WatchDescriptor,
     stdout: Stdout,
@@ -190,14 +185,3 @@ impl Collector {
         Ok(())
     }
 }
-
-fn main() -> io::Result<()> {
-    env_logger::init();
-
-    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
-
-    let mut buffer = [0; 1024];
-    loop {
-        collector.handle_events(&mut buffer)?;
-    }
-}
```

We can now create a new `main.rs` with our old `fn main()` and the requisite imports:

```rust
// main.rs
#[macro_use]
extern crate log;

mod log_collector;

use std::io;

use log_collector::Collector;

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

fn main() -> io::Result<()> {
    env_logger::init();

    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
```

Let's check things still work:

```
$ make writer && make monitoring
...
monitoring_1  | [2020-12-06T23:38:52Z DEBUG monitoring_rs::log_collector] Initialising watch on root path "/var/log/containers"
monitoring_1  | [2020-12-06T23:38:52Z DEBUG monitoring_rs::log_collector] Create /var/log/containers/writer.log
monitoring_1  | [2020-12-06T23:38:52Z DEBUG monitoring_rs::log_collector] Append /var/log/containers/writer.log
monitoring_1  | Sun Dec  6 23:38:52 UTC 2020
...
$ make down
```

All good!
Note that the `log` crate captures the module that log messages come from, and that this is now `monitoring_rs::log_collector`.

## Wrapping up

That'll do for now.
I can't think of anything else to do to procrastinate starting on persistence (`log_database`), so that's up next!

[Back to the README](../README.md#posts)
