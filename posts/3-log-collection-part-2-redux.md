# Log collection (part 2 – redux)

## Recap

What have we done?

- In [Discovery](0-discovery.md) we waxed lyrical about the definition of a 'monitoring pipeline' and wound up with the following high level requirements:

  > - Accept logs/metrics from Kubernetes.
  > - Store those logs/metrics in a format suitable for searching and alerting.
  > - Provide an API and UI for searching and visualising ingested logs/metrics.
  > - Provide an API and UI for configuring alert rules based on incoming logs/metrics.

- In [Log collection part 1](1-log-collection-part-1.md) we made a shallow dive into Kubernetes log collection, including setting up a Rust project with Docker configuration.
  We built a Docker container that we could run in Kubernetes to print the contents of `/var/log` on the node and made a quick study of what we found.

  > - The log files in `/var/log/containers` have the following form:
  >
  >   ```
  >   <pod name>_<namespace>_<container name>-<container ID>.log
  >   ```
  >
  > - The log files in `/var/log/pods` have the following form:
  >
  >   ```
  >   <namespace>_<pod name>_<pod uid>/
  >     <container name>/
  >       <n - 1>.log
  >       <n>.log
  >   ```
  >
  >   `n` starts at zero, and is incremented whenever the container restarts.
  >   Kubernetes only retains the logs of one previous container (`n - 1`).

- In [Log collection part 2 (aborted)](2-log-collection-part-2-aborted.md) we attempted a deeper dive into implementing log collection, in particular aiming to have a binary that would monitor the container logs directory and re-print the log entries to `stdout`.
  We encountered some difficulties with our approach, in particular:

  - We tried to use the `notify` crate for its cross-platform file watching support, in the hope that it would support a convenient native development experience.
    Sadly, the MacOS fsevents API will not deliver events until files are closed, which is totally unsuitable for detecting log file changes, meaning we couldn't develop natively anyway.

  - We used Docker to work around these limitations, but the set up involved wrangling multiple terminal tabs whilst playing whac-a-mole with unexpected behaviour when performing log rotation, ultimately creating an awkward development experience and slow feedback loop.

  We did however reach a better understanding of what our log collector needs to do:

  > It seems like what we want is to maintain a `HashMap` of 'live' log files, which we can scrape from whenever we notice a write.
  > We need to be able to use the incoming events both to trigger us to scrape a file, as well as to trigger updates to the `HashMap` – opening and adding files that become active, whilst pruning files that become inactive.

In this post we will improve our Docker-based workflow and start a new implementation based on the [inotify](https://docs.rs/inotify/0.8.3/inotify/) crate.

## Reset

You might have different content in `src/main.rs` depending on whether or not you followed through Log collection part 2 (aborted).
Since we will be starting again, we can populate that file with a trivial Hello World:

```rust
// main.rs
fn main() {
    println!("Hello, world!");
}
```

We can also clear out our dependencies:

```toml
# Cargo.toml
[package]
# ...

[dependencies]
```

Verify all is well:

```sh
$ cargo run
Hello, world!
```

## Better development using Docker

We can make our lives a little easier by using [Docker Compose](https://docs.docker.com/compose/).
Let's create a basic [compose file](https://docs.docker.com/compose/compose-file/) with a shared log volume and three containers:

```yaml
# docker-compose.yaml
version: '3.8'
services:
  monitoring:
    build: .
    image: registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
    volumes:
    - logs:/var/log/containers

  writer:
    image: alpine
    volumes:
    - logs:/var/log/containers
    command:
    - sh
    - -c
    - while true ; do date ; sleep 1 ; done | cat >> /var/log/containers/writer.log

  inspect:
    image: alpine
    volumes:
    - logs:/var/log/containers
    command:
    - sh
    - -c
    - cat /var/log/containers/*

volumes:
  logs:
```

Let's see what happens if we start our monitoring service:

```
$ docker-compose up --build --force-recreate monitoring
...
Successfully built 0aaf87b15557
Successfully tagged $DOCKER_REGISTRY/monitoring-rs:latest
Recreating monitoring-rs_monitoring_1 ... done
Attaching to monitoring-rs_monitoring_1
monitoring_1  | Hello, world!
monitoring-rs_monitoring_1 exited with code 0
```

Great, so we can run our binary in Docker a bit easier.

Let's try our other services:

```
$ docker-compose up -d writer
Creating monitoring-rs_writer_1 ... done

$ docker-compose up inspect
Creating monitoring-rs_inspect_1 ... done
Attaching to monitoring-rs_inspect_1
inspect_1     | Mon Oct 19 15:19:36 UTC 2020
inspect_1     | Mon Oct 19 15:19:37 UTC 2020
...
```

And we can tear everything down with `docker-compose down`:

```
$ docker-compose down --volumes
Stopping monitoring-rs_writer_1 ... done
Removing monitoring-rs_inspect_1    ... done
Removing monitoring-rs_writer_1     ... done
Removing monitoring-rs_monitoring_1 ... done
Removing network monitoring-rs_default
Removing volume monitoring-rs_logs
```

As a final convenience, let's add a Makefile so we can reduce the number of arguments we need:

```Makefile
# Makefile
.PHONY: monitoring writer inspect down reset

monitoring:
  @docker-compose up --build --force-recreate monitoring

writer:
  @docker-compose up -d writer

inspect:
  @docker-compose up inspect

down:
  @docker-compose down --timeout 0 --volumes

reset: down writer
```

This gives us the following workflow:

```
$ make reset
...

$ make inspect
Creating monitoring-rs_inspect_1 ... done
Attaching to monitoring-rs_inspect_1
inspect_1     | Mon Oct 19 15:32:09 UTC 2020
inspect_1     | Mon Oct 19 15:32:10 UTC 2020
...

$ make monitoring
...
Hello, world!

$ make down
Stopping monitoring-rs_writer_1 ... done
Removing monitoring-rs_monitoring_1 ... done
Removing monitoring-rs_inspect_1    ... done
Removing monitoring-rs_writer_1     ... done
Removing network monitoring-rs_default
Removing volume monitoring-rs_logs
```

That will do!

## Begin again

Let's work towards an implementation based on our high-level description from above:

> It seems like what we want is to maintain a `HashMap` of 'live' log files, which we can scrape from whenever we notice a write.
> We need to be able to use the incoming events both to trigger us to scrape a file, as well as to trigger updates to the `HashMap` – opening and adding files that become active, whilst pruning files that become inactive.

We're going to use [`inotify`](https://docs.rs/inotify/0.8/inotify/) rather than `notify` this time – we want a low-level interface to file system events and don't need cross-platform support, so it doesn't seem like `notify` will give us anything.
We will also disable the default features for this, as we don't want to drag in [`tokio`](https://tokio.rs/).

We can add this directly to `Cargo.toml` or using [`cargo-edit`](https://github.com/killercup/cargo-edit):

```
$ cargo add inotify --no-default-features
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding inotify v0.8.3 to dependencies

$ cat Cargo.toml
...
[dependencies]
inotify = { version = "0.8.3", default-features = false }
```

Note that Cargo uses [semver caret ranges](https://docs.npmjs.com/misc/semver#caret-ranges-123-025-004) implicitly, which is typically what you want and most Rust crates support that convention.

We can begin by watching our container log directory for `WRITE` events and print them out:

```rust
// main.rs
use std::io;

use inotify::{Inotify, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

fn main() -> io::Result<()> {
    let mut inotify = Inotify::init()?;

    inotify.add_watch(CONTAINER_LOG_DIRECTORY, WatchMask::MODIFY)?;

    let mut buffer = [0; 1024];
    loop {
        let events = inotify.read_events_blocking(&mut buffer)?;

        for event in events {
            eprintln!("{:?}", event);
        }
    }
}
```

The usage of `inotify` is based on the [example](https://docs.rs/inotify/0.8/inotify/#example) in the crate documentation.
There's nothing too surprising here – the most interesting thing is probably that `inotify` expects us to supply a buffer that it will use internally to read events from the inotify OS API.
We're filtering watch events to only [`MODIFY`](https://docs.rs/inotify/0.8/inotify/struct.WatchMask.html#associatedconstant.MODIFY) events for now.

Let's see if it does what we expect (print an event every second as the writer writes):

```
$ make reset monitoring
...
monitoring_1  | Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }
monitoring_1  | Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }
monitoring_1  | Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }
monitoring_1  | Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("writer.log") }

^C
$ make down
```

Indeed, we seem to be picking up one event per second.
The [`inotify::Event`](https://docs.rs/inotify/0.8/inotify/struct.Event.html) that we're logging includes:

- `wd` – the [`WatchDescriptor`](https://docs.rs/inotify/0.8/inotify/struct.WatchDescriptor.html) for the directory we're watching (returned on line 11 of our `main.rs`, but we're ignoring it).
- `mask` – an [`EventMask`](https://docs.rs/inotify/0.8/inotify/struct.EventMask.html) indicating the type of the received event.
- `cookie` – a `u32` that inotify uses to correlate related events, such as renames which appear as a pair of [`MOVED_FROM`](https://docs.rs/inotify/0.8/inotify/struct.EventMask.html#associatedconstant.MOVED_FROM) and [`MOVED_TO`](https://docs.rs/inotify/0.8/inotify/struct.EventMask.html#associatedconstant.MOVED_TO) events.
- `name` – an `Option<&OsStr>` containing the name of the file that event pertains to.
  This is an `Option` because the value is `None` if the event pertains to the watched file.
  In our case this is the `/var/log/containers` directory itself, which we would not expect to be modified.

We will want to modify our log collector to take action depending on the `mask` and `name` at least, and possibly also the `cookie` if we need to correlate rename events (it's not clear at this stage whether we could get away without that or not).

First, it's worth looking over the [inotify manual page](https://man7.org/linux/man-pages/man7/inotify.7.html) to get a sense of the available events.
Some particularly interesting ones:

- `IN_EXCL_UNLINK`, `IN_ONLYDIR`, `IN_MASK_CREATE`: These are not really events, but can be supplied when initialising inotify to change the behaviour:

  - When watching a directory, `IN_EXCL_UNLINK` causes inotify to ignore events that would otherwise be generated for children after they have been unlinked from the watched directory.
    It's not yet clear if this desireable or not, as it may cause us to lose logs if a removed log file is written to after it is unlinked.
  - `IN_ONLYDIR` will cause inotify to raise an error if the watch target is not a directory.
    This would be a useful sanity check for us.
  - `IN_MASK_CREATE` will cause inotify to fail when attempting to watch a path that is already being watched.
    This would help to validate that we only ever have a single watch for a given path.

- `IN_CREATE`: Indicates that a file or directory was created in the watched directory.
  This sounds very useful!

- `IN_MODIFY`: Indicates that a file was modified.
  This is the main event that we want to know about.

- `IN_MOVED_FROM`, `IN_MOVED_TO`: Generated once each when a file in a watched directory is renamed.
  `IN_MOVED_FROM` would contain the old name of the file, whilst `IN_MOVED_TO` would contain the new name.
  The manual has a whole section outlining challenges when trying to match these two events – the upshot being that there are no guarantees, though in most cases the events are likely to be consecutive.

- `IN_CLOSE_WRITE`: Indicates that a file that was opened for writing has been closed.
  In the context of log files, this could indicate that the log file is finished.

- `IN_DELETE`: Indicates that a file or directory was deleted from a watched directory.
  This would probably indicate a log file rotation, or potentially a container having stopped.

For starters, let's add handling for the `IN_MODIFY` event.
When we receive that event, what we would like to do is print any *new content* in the file.
In order to determine what of the files contents is 'new', we will need to have an open handle to the file already.
For now, we will open files lazily when we receive the first event for the file.
Whilst we could capture some extra content by opening all the files on startup, this doesn't feel like a complete solution to ensuring we capture all the logs (e.g. what would happen if the monitoring process is restarted), so we will explore that element of reliability later.

So, we want the following logic:

- Create an empty `HashMap<PathBuf, File>` to store open handles.
- When we receive a `IN_MODIFY` event:
  - Work out the path of the affected file.
  - If the path is in our `HashMap` already, copy the file content from the last week position onwards to `stdout`.
  - If the path is not in our `HashMap`, open it, seek to the end, and store it in the `HashMap`.

```rust
// main.rs
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

struct Event {
    event_type: EventType,
    path: PathBuf,
}

enum EventType {
    Modify,
}

struct Collector {
    path: PathBuf,
    stdout: Stdout,
    live_files: HashMap<PathBuf, File>,
    inotify: Inotify,
}

impl Collector {
    pub fn new(path: &Path) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;
        inotify.add_watch(path, WatchMask::MODIFY)?;

        Ok(Self {
            path: path.to_path_buf(),
            stdout: io::stdout(),
            live_files: HashMap::new(),
            inotify,
        })
    }

    pub fn handle_events(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let events = self.inotify.read_events_blocking(buffer)?;

        for event in events {
            if let Some(event) = self.check_event(event) {
                let handler = match event.event_type {
                    EventType::Modify => Self::handle_event_modify,
                };
                handler(self, event.path)?;
            }
        }

        Ok(())
    }

    fn check_event<'ev>(&self, event: inotify::Event<&'ev OsStr>) -> Option<Event> {
        let event_type = if event.mask.contains(EventMask::MODIFY) {
            Some(EventType::Modify)
        } else {
            None
        }?;

        let name = event.name?;
        let mut path = PathBuf::with_capacity(self.path.capacity() + name.len());
        path.push(&self.path);
        path.push(name);

        Some(Event { event_type, path })
    }

    fn handle_event_modify(&mut self, path: PathBuf) -> io::Result<()> {
        if let Some(file) = self.live_files.get_mut(&path) {
            io::copy(file, &mut self.stdout)?;
        } else {
            let mut file = File::open(&path)?;

            use std::io::Seek;
            file.seek(io::SeekFrom::End(0))?;

            self.live_files.insert(path, file);
        }

        Ok(())
    }
}

fn main() -> io::Result<()> {
    let mut collector = Collector::new(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
```

We've restructured our logic into a `Collector` struct, along with some abstractions that we expect will help us later.
In particular, the `EventType` enum will allow us to expand the capabilities of our `Collector`, and compiler will ensure we're constructing and handling that possibility.

Let's see if it blends:

```
$ make reset monitoring
...
monitoring_1  | Mon Oct 19 18:53:40 UTC 2020
monitoring_1  | Mon Oct 19 18:53:41 UTC 2020
monitoring_1  | Mon Oct 19 18:53:42 UTC 2020
monitoring_1  | Mon Oct 19 18:53:43 UTC 2020
^C

$ make down
```

Hurray 🎉
That appears to be working – our monitoring process is printing out the timestamps that our writer is writing.

## Until next time

We've got a lot more to do, and we will probably have to change things around quite a bit before we reach a satisfactory conclusion, but this is a good start and it's in a shape we can work with going forward.

Our next steps probably look like:

- Add more `EventType`s to handle log rotation.
- Store the logs, instead of printing them to `stdout`.
- Try it out in Kubernetes!

[Back to the README](../README.md#posts)
