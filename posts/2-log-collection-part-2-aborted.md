# Log collection (part 2 – **aborted**)

**Note:** I am writing these posts as I attempt to build the solution.
The hope is that this would be useful for myself (and possibly others), as documention of my approach and progress.
An inevitable consequence of this is that some of the posts will be a bit of a dead-end, and this is such a post!
If you want to experience all the bumps in the road, it might be worth following or skimming this post.
Otherwise just skip ahead to [Log collection part 2 (redux)](2-log-collection-part-2-redux.md).

In the last post we set up a trivial Rust binary to print the contents of `/var/log`.
We also built a Docker container and ran it on Kubernetes.

This time, let's try and stream container logs into our log collector's own `stdout`.
The plan is to use the [`notify`](https://docs.rs/notify/) crate, rather than implementing our own file watching on top of inotify etc. as this is notoriously fiddly.

## Add a dependency

Let's bring in our first dependency.
You can directly add `notify = "4"` to your `Cargo.toml`, or you could use [`cargo-edit`](https://github.com/killercup/cargo-edit):

```sh
$ cargo add notify
    Updating 'https://github.com/rust-lang/crates.io-index' index
      Adding notify v4.0.15 to dependencies
```

## Watch the tail

Let's split our objective for the next iteration into two separate parts that we will combine:

- Efficiently 'tail' a file.
- Efficiently watch for log rotations.

### Efficiently 'tail' a file

What do we mean by "tail a file"?
We mean to open the file at (or near) its end and print new content as it comes in.
Let's set up a simple test for how this could be done using Rust.
We will first clear out our `main.rs` (don't worry – git remembers!):

```rust
fn main() {
    println!("Hello, world!");
}
```

Let's update it to open a `test.log` file in the current directory, and copy it to `stdout`:

```rust
use std::fs::File;
use std::io;

fn main() -> io::Result<()> {
    let mut log = File::open("test.log")?;

    io::copy(&mut log, &mut io::stdout())?;

    Ok(())
}
```

We can now write some content to `test.log`, invoke `cargo run` and see the contents printed out:

```sh
$ cat <<EOF > test.log
hello
world
what is up
EOF
$ cargo run
hello
world
what is up
```

Notice that our program exits once it has copied all the contents from the file.
Let's fix this in an especially naïve way:

```rust
use std::fs::File;
use std::io;

fn main() -> io::Result<()> {
    let mut log = File::open("test.log")?;

    loop {
        io::copy(&mut log, &mut io::stdout())?;
    }
}
```

Rust [`File`](https://doc.rust-lang.org/stable/std/fs/struct.File.html) objects are wrappers around a system file handle, and the position in the file is retained between operations.
As such, we can just try to copy repeatedly – if we're already at EOF nothing special happens, we just go around the loop again.

Let's see what happens now:

```sh
$ cargo run
hello
world
what is up
...
```

And our program appears to hang.
Let's try adding some lines to our log from another shell tab:

```sh
echo wow >> test.log
```

Back in our `cargo run` tab we now see:

```sh
$ cargo run
hello
world
what is up
wow
...
```

And our program continues to wait for more input.
Great!
Or is it?
Let's have a quick look at `top` (or `taskmgr` if you're on Windows):

```sh
$ top -o cpu
PID    COMMAND       %CPU
56314  monitoring-rs 99.6
```

Ooft – that's a lot of CPU for doing nothing.
Let's try and fix that, also naïvely, by using the `notify` crate to tell us when something has happened to the file once we hit EOF:

```rust
use std::error::Error;
use std::fs::File;
use std::io;
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let mut log = File::open("test.log")?;
    let mut stdout = io::stdout();

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch("test.log", RecursiveMode::NonRecursive)?;

    loop {
        if io::copy(&mut log, &mut stdout)? == 0 {
            rx.recv()?;
        }
    }
}
```

We're setting up a `RecommendedWatcher` instance based on the `notify` crate's [documentation](https://docs.rs/notify/4.0.15/notify/#raw-api).
We're using the "raw" API and ignoring the event's content because we don't care about it – we just want it to wake us up and cause us to revisit the file.

Let's see if it blended:

```sh
$ cargo run
hello
world
what is up
wow
...

# in another tab
$ top -o cpu | grep monitoring-rs
56521  monitoring-rs    0.0
$ echo wow again! >> test.log

# back in cargo run
...
wow again!
```

We've gone from \~100% CPU down to \~0 (I had to `grep` to find the process), not bad!

#### Timeliness

I've been trying to write these posts as I work through this, in a sort of stream-of-consciousness way.
Later on through the post, however, I notice an issue with the `notify` crate on MacOS, which it makes sense to fix here.
If you're concerned about the canon, perhaps imagine a scene where future you appears in a vision to warn you of this specific future peril, which can be demonstrated thus:

```sh
$ cargo run
...

# in another tab
$ tail -f test.log

# in *another* tab
$ cat >> test.log
hello?
oh dear
```

Note that as you type the lines `hello?` and `oh dear` into stdin for `cat`, the lines show up in the tab running `tail -f`, but *not* in our `cargo run` tab.
It turns out this is a [known issue](https://github.com/notify-rs/notify/issues/240) in the notify crate, and is just how the OSX fsevent API works.

If you're running linux, however, this will just work, since inotify is used and that behaves in the way that we desire.
We can verify this in Docker:

```sh
# re-establish DOCKER_REGISTRY if necessary
$ DOCKER_REGISTRY=<your registry>

$ docker build -t $DOCKER_REGISTRY/monitoring-rs .
...
Successfully tagged $DOCKER_REGISTRY/monitoring-rs:latest

$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'touch test.log && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'exec cat >> test.log'
hello?
woohoo!
```

Et voila!
You should now see the lines `hello?` and `woohoo` show up in the `docker run` tab.

### Tail all the files!

Of course, we don't want to tail only a single file – we want to tail every file in the `/var/log/containers/` directory on our cluster nodes. Let's introduce a new assumption first: that it's enough for our log collector to tail the logs from a single directory. This is true for the `/var/log/containers/` directory, and that's enough for us to start with.

Let's rebuild our `main.rs` with the following rough spec:

- Read a `CONTAINER_LOG_DIRECTORY` on startup, defaulting to `/var/log/containers`.
- Start a watcher on `CONTAINER_LOG_DIRECTORY`.
  This will emit events when files in `CONTAINER_LOG_DIRECTORY` change, and the events will include the path of the file that changed.
- Open all the files in the `CONTAINER_LOG_DIRECTORY` and seek to the end.
- When a file changes, copy the file's contents to `stdout` and go back to waiting.

Let's see what that looks like:

```rust
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let container_log_directory = fs::canonicalize(
        env::var("CONTAINER_LOG_DIRECTORY").unwrap_or_else(|_| "/var/log/containers".to_string()),
    )?;

    let mut files = HashMap::new();
    for entry in fs::read_dir(&container_log_directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let path = fs::canonicalize(entry.path())?;
        let mut file = File::open(&path)?;
        file.seek(SeekFrom::End(0))?;
        files.insert(path, file);
    }

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch(container_log_directory, RecursiveMode::NonRecursive)?;

    let mut stdout = io::stdout();

    for (_, mut file) in files.iter_mut() {
        io::copy(&mut file, &mut stdout)?;
    }

    for event in rx {
        if let Some(path) = event.path {
            if let Some(file) = files.get_mut(&path) {
                io::copy(file, &mut stdout)?;
            }
        }
    }

    Ok(())
}
```

There's a fair bit going on here, but it's mostly combining things we've used before:

- [`fs::read_dir`](https://doc.rust-lang.org/stable/std/fs/fn.read_dir.html) is used to iterate the contents of the `container_log_directory`.
- Each entry that's a file is opened, seeked to the end, and then stored in a [`HashMap`](https://doc.rust-lang.org/stable/std/collections/struct.HashMap.html), with the canonical path as its key.
- We start a watcher on the container log directory.
  We continue to use `RecursiveMode::NonRecursive` because we expect the container log directory to be flat.
- Having performed our setup steps, we make a first pass through the files and copy anything that was written since we opened them.
- Finally, we iterate through received events, match them to a specific file, and copy the contents of the file (from our current seek position) to `stdout`.

Let's run another experiment in Docker to validate it:

```sh
$ docker build -t $DOCKER_REGISTRY/monitoring-rs .
...
Successfully tagged $DOCKER_REGISTRY/monitoring-rs:latest

$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'mkdir /var/log/containers && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'while true ; do echo "hello from test 1" ; sleep 1 ; done | cat >> /var/log/containers/test1.log'
...

# in *another* tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'while true ; do echo "hello from test 2" ; sleep 2 ; done | cat >> /var/log/containers/test2.log'
...
```

...but nothing is showing again!
Let's open another instance of our monitor and see what happens:

```sh
# in **another** tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id ./monitoring-rs
hello from test 1
hello from test 2
hello from test 1
...
```

Aha!
The problem is that our monitor can't handle new files yet, thanks to this condition:

```rust
if let Some(file) = files.get_mut(&path) {
    ...
}
```

E.g., if we didn't see the file on start-up, we will ignore any events for it.
We can confirm this is the case once again with a little test:

```sh
$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'mkdir /var/log/containers \
    && touch /var/log/containers/test1.log /var/log/containers/test2.log \
    && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'while true ; do echo "hello from test 1" ; sleep 1 ; done | cat >> /var/log/containers/test1.log'

# in *another* tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'while true ; do echo "hello from test 2" ; sleep 2 ; done | cat >> /var/log/containers/test2.log'
```

This time, we should see the logs show up in our original tab.
Let's kill two birds with one stone and implement log rotation handling, which should also solve our new file detection issue.

## Rotators gonna rotate

What happens when a log file rotates?
Good question!
Observably, what happens is that a new file is created with the contents of the current file, and the existing file is truncated.
Let's set up an experiment!

First, let's update our `main.rs` to simply log detected events:

```rust
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let container_log_directory = fs::canonicalize(
        env::var("CONTAINER_LOG_DIRECTORY").unwrap_or_else(|_| "/var/log/containers".to_string()),
    )?;

    let mut files = HashMap::new();
    for entry in fs::read_dir(&container_log_directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let path = fs::canonicalize(entry.path())?;
        let mut file = File::open(&path)?;
        file.seek(SeekFrom::End(0))?;
        files.insert(path, file);
    }

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch(container_log_directory, RecursiveMode::NonRecursive)?;

    let mut stdout = io::stdout();

    for (_, mut file) in files.iter_mut() {
        io::copy(&mut file, &mut stdout)?;
    }

    for event in rx {
        eprintln!("event: {:?}", event);
    }

    Ok(())
}
```

Now, we can run our updated Docker container and see what happens when we rotate a log file.

```sh
$ docker build -t $DOCKER_REGISTRY/monitoring-rs .
$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'mkdir /var/log/containers \
    && touch /var/log/containers/test.log \
    && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c \
    'while true ; do echo test ; sleep 1 ; done | cat >> /var/log/containers/test.log'
```

We can see events appearing in our application.
Now, let's rotate the log:

```sh
# in *another* tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'apk add --no-cache logrotate \
    && echo "/var/log/containers/test.log {}" > test.config \
    && logrotate --force --verbose --log /var/log/containers/test.log test.config'
...
Handling 1 logs

rotating pattern: /var/log/containers/test.log  forced from command line (no old logs will be kept)
empty log files are rotated, old logs are removed
considering log /var/log/containers/test.log
Creating new state
  Now: 2020-10-11 19:58
  Last rotated at 2020-10-11 19:00
  log needs rotating
rotating log /var/log/containers/test.log, log->rotateCount is 0
dateext suffix '-20201011'
glob pattern '-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
renaming /var/log/containers/test.log.1 to /var/log/containers/test.log.2 (rotatecount 1, logstart 1, i 1),
old log /var/log/containers/test.log.1 does not exist
renaming /var/log/containers/test.log.0 to /var/log/containers/test.log.1 (rotatecount 1, logstart 1, i 0),
old log /var/log/containers/test.log.0 does not exist
log /var/log/containers/test.log.2 doesn't exist -- won't try to dispose of it
renaming /var/log/containers/test.log to /var/log/containers/test.log.1
disposeName will be /var/log/containers/test.log.1
removing old log /var/log/containers/test.log.1
```

So it looks like it renames `/var/log/containers/test.log` to `/var/log/containers/test.log.1`, then tries to remove it?
What does our program say:

```
event: RawEvent { path: Some("/var/log/containers/test.log"), op: Ok(WRITE), cookie: None }
event: RawEvent { path: Some("/var/log/containers/test.log"), op: Ok(RENAME), cookie: Some(189698) }
event: RawEvent { path: Some("/var/log/containers/test.log.1"), op: Ok(RENAME), cookie: Some(189698) }
event: RawEvent { path: Some("/var/log/containers/test.log.1"), op: Ok(WRITE), cookie: None }
event: RawEvent { path: Some("/var/log/containers/test.log.1"), op: Ok(REMOVE), cookie: None }
event: RawEvent { path: Some("/var/log/containers/test.log.1"), op: Ok(CLOSE_WRITE), cookie: None }
event: RawEvent { path: Some("/var/log/containers/test.log.1"), op: Ok(WRITE), cookie: None }
```

Indeed, from these events it looks like that's exactly what happens (the `cookie` value associates the two sides of the `RENAME` operation).
Interestingly, the process writing to the log file continues to write to the renamed location (`/var/log/containers/test.log.1`), and our monitor continues to see the updates, even after the file has been removed.
This is because the processes writing and reading the log file already have open handles to the file, which is associated to the 'inode' of the file at the time it was opened.
Renaming the file merely changes the path of the inode, and so the processes continue to read and write the same file on disk – but the path `/var/log/containers/test.log` no longer refers to that inode (and indeed no longer exists).

Very interesting, I'm sure, but does this help us implement proper handling for log rotation?
A quick look at the [`logrotate` docs](https://linux.die.net/man/8/logrotate) shows that log rotation can happen in multiple different ways, including:

- `copy`: This simply takes a snapshot of the log file, but leaves the existing one in place.
  This is not interesting for us, ideally we would even ignore the new files completely since they won't be written to.
- `copytruncate`: This takes a snapshot of the log file, then truncates it.
  This would be problematic in our current implementations since our reading is always based on the last seek position, and truncating would mean that would be beyond the end of the file and our reads would be empty until the file 'catches' up again.
- Scripts can be set to execute arbitrary shell commands when `logrotate` decides something needs to be rotated.

Furthermore, `logrotate` is not the only tool that exists to perform log rotation, so we probably shouldn't rely on any specific semantics if we can avoid it.
Our test is also not successfully representative – the log writer continued to write to the old log file, which we would certainly hope the Docker daemon would not do.

OK... how about to start we simply try to open and add to our `HashMap` new paths that we find:

```rust
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let container_log_directory = fs::canonicalize(
        env::var("CONTAINER_LOG_DIRECTORY").unwrap_or_else(|_| "/var/log/containers".to_string()),
    )?;

    let mut files = HashMap::new();
    for entry in fs::read_dir(&container_log_directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }

        let path = fs::canonicalize(entry.path())?;
        open_file(&mut files, path)?;
    }

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch(container_log_directory, RecursiveMode::NonRecursive)?;

    let mut stdout = io::stdout();

    for event in rx {
        if let Some(path) = event.path {
            if let Some(file) = files.get_mut(&path) {
                io::copy(file, &mut stdout)?;
            } else {
                let file = open_file(&mut files, path)?;
                io::copy(file, &mut stdout)?;
            }
        }
    }

    Ok(())
}

fn open_file(files: &mut HashMap<PathBuf, File>, path: PathBuf) -> io::Result<&mut File> {
    let mut file = File::open(&path)?;
    file.seek(SeekFrom::End(0))?;

    // We expect the key not to be set – https://github.com/rust-lang/rust/issues/65225 would
    // resolve this mismatch.
    let file = files.entry(path).or_insert(file);

    Ok(file)
}
```

The new `open_file` function encapsulates the logic of opening a file, seeking to the end, and storing it in a `HashMap`.
We use it on our initial search of the `container_log_directory` and again whenever we encounter an event for a path we're not aware of.

Let's run this in Docker and see what happens:

```sh
$ docker build -t $DOCKER_REGISTRY/monitoring-rs .
$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'mkdir /var/log/containers \
    && touch /var/log/containers/test.log \
    && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c \
    'while true ; do echo test ; sleep 1 ; done | cat >> /var/log/containers/test.log'

# in *another* tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'apk add --no-cache logrotate \
    && echo "/var/log/containers/test.log {}" > test.config \
    && logrotate --force --verbose --log /var/log/containers/test.log test.config'
...
Handling 1 logs

rotating pattern: /var/log/containers/test.log  forced from command line (no old logs will be kept)
empty log files are rotated, old logs are removed
considering log /var/log/containers/test.log
Creating new state
  Now: 2020-10-11 19:58
  Last rotated at 2020-10-11 19:00
  log needs rotating
rotating log /var/log/containers/test.log, log->rotateCount is 0
dateext suffix '-20201011'
glob pattern '-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
renaming /var/log/containers/test.log.1 to /var/log/containers/test.log.2 (rotatecount 1, logstart 1, i 1),
old log /var/log/containers/test.log.1 does not exist
renaming /var/log/containers/test.log.0 to /var/log/containers/test.log.1 (rotatecount 1, logstart 1, i 0),
old log /var/log/containers/test.log.0 does not exist
log /var/log/containers/test.log.2 doesn't exist -- won't try to dispose of it
renaming /var/log/containers/test.log to /var/log/containers/test.log.1
disposeName will be /var/log/containers/test.log.1
removing old log /var/log/containers/test.log.1

# back in our original tab
test
...
file, size 64 entries

Handling 1 logs

rotating pattern: /var/log/containers/test.log  forced from command line (no old logs will be kept)
empty log files are rotated, old logs are removed
considering log /var/log/containers/test.log
Creating new state
  Now: 2020-10-19 11:51
  Last rotated at 2020-10-19 11:00
  log needs rotating
rotating log /var/log/containers/test.log, log->rotateCount is 0
dateext suffix '-20201019'
glob pattern '-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
renaming /var/log/containers/test.log.1 to /var/log/containers/test.log.2 (rotatecount 1, logstart 1, i 1),
old log /var/log/containers/test.log.1 does not exist
renaming /var/log/containers/test.log.0 to /var/log/containers/test.log.1 (rotatecount 1, logstart 1, i 0),
old log /var/log/containers/test.log.0 does not exist
log /var/log/containers/test.log.2 doesn't exist -- won't try to dispose of it
renaming /var/log/containers/test.log to /var/log/containers/test.log.1
disposeName will be /var/log/containers/test.log.1
removing old log /var/log/containers/test.log.1
Error: Os { code: 2, kind: NotFound, message: "No such file or directory" }
```

Huh.
So, it would seem that `logrotate` logs its operations into the file that it rotates, and our log watcher is picking that up.
Sadly, our watcher is crashing!
But why?
The `logrotate` log gives us a hint:

> removing old log /var/log/containers/test.log.1

We are not configuring a number of rotations to maintain, so `logrotate` is cleaning up the old log immediately after it copies it.
As a trivial fix to this we can ignore `NotFound` errors:

```rust
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::channel;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

fn main() -> Result<(), Box<dyn Error>> {
    let container_log_directory = fs::canonicalize(
        env::var("CONTAINER_LOG_DIRECTORY").unwrap_or_else(|_| "/var/log/containers".to_string()),
    )?;

    let mut files = HashMap::new();
    for entry in fs::read_dir(&container_log_directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            continue;
        }

        let path = fs::canonicalize(entry.path())?;
        open_file(&mut files, path)?;
    }

    let (tx, rx) = channel();
    let mut watcher: RecommendedWatcher = Watcher::new_raw(tx)?;
    watcher.watch(container_log_directory, RecursiveMode::NonRecursive)?;

    let mut stdout = io::stdout();

    for event in rx {
        // let's `dbg` the events as we handle them
        if let Some(path) = event.path {
            let file = if let Some(file) = files.get_mut(&path) {
                file
            } else if let Some(file) = open_file(&mut files, path)? {
                file
            } else {
                continue;
            };
            io::copy(file, &mut stdout)?;
        }
    }

    Ok(())
}

fn open_file(files: &mut HashMap<PathBuf, File>, path: PathBuf) -> io::Result<Option<&mut File>> {
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    file.seek(SeekFrom::End(0))?;

    // We expect the key not to be set – https://github.com/rust-lang/rust/issues/65225 would
    // resolve this mismatch.
    let file = files.entry(path).or_insert(file);

    Ok(Some(file))
}
```

And a last pass through Docker 🤞

```sh
$ docker build -t $DOCKER_REGISTRY/monitoring-rs .
$ docker run $DOCKER_REGISTRY/monitoring-rs:latest sh -c 'mkdir /var/log/containers \
    && touch /var/log/containers/test.log \
    && exec ./monitoring-rs'

# in another tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c \
    'while true ; do echo test ; sleep 1 ; done | cat >> /var/log/containers/test.log'

# in *another* tab
$ DOCKER_REGISTRY=<your registry>
$ container_id="$(docker ps --filter "ancestor=$DOCKER_REGISTRY/monitoring-rs:latest" -q)"
$ docker exec -it $container_id sh -c 'apk add --no-cache logrotate \
    && echo "/var/log/containers/test.log {}" > test.config \
    && logrotate --force --verbose --log /var/log/containers/test.log test.config'
...
Handling 1 logs

rotating pattern: /var/log/containers/test.log  forced from command line (no old logs will be kept)
empty log files are rotated, old logs are removed
considering log /var/log/containers/test.log
Creating new state
  Now: 2020-10-11 19:58
  Last rotated at 2020-10-11 19:00
  log needs rotating
rotating log /var/log/containers/test.log, log->rotateCount is 0
dateext suffix '-20201011'
glob pattern '-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]'
renaming /var/log/containers/test.log.1 to /var/log/containers/test.log.2 (rotatecount 1, logstart 1, i 1),
old log /var/log/containers/test.log.1 does not exist
renaming /var/log/containers/test.log.0 to /var/log/containers/test.log.1 (rotatecount 1, logstart 1, i 0),
old log /var/log/containers/test.log.0 does not exist
log /var/log/containers/test.log.2 doesn't exist -- won't try to dispose of it
renaming /var/log/containers/test.log to /var/log/containers/test.log.1
disposeName will be /var/log/containers/test.log.1
removing old log /var/log/containers/test.log.1

# back in our original tab
...
test
...
removing old log /var/log/containers/test.log.1
```

And... it's still running, but nothing is showing.
This is because our writer is now writing to an inode with the path `/var/log/containers/test.log.1`, which `logrotate` has already unlinked:

```sh
# in the logrotate tab
$ docker exec -it $container_id ls /var/log/containers
```

There is nothing in that directory now.
If we restart the writer, we will resume picking up logs, though:

```sh
# in the writer tab
^C
$ docker exec -it $container_id sh -c \
    'while true ; do echo test ; sleep 1 ; done | cat >> /var/log/containers/test.log'
```

And... nothing is showing up in our original tab :(
Why?
Because there is already a `/var/log/containers/test.log` entry in the `HashMap`, and we're using
`entry(...).or_insert(...)`!
More generally, our `HashMap` is not kept in sync with file renames, so we might have to care about event types after all, in order to know that an existing key needs to be evicted or updated.

This is quite a series of unfortunate events, and we've been going for a while.
However, we are closer to being able to articulate the logic we need for our log watcher.
It seems like what we want is to maintain a `HashMap` of 'live' log files, which we can scrape from whenever we notice a write.
We need to be able to use the incoming events both to trigger us to scrape a file, as well as to trigger updates to the `HashMap` – opening and adding files that become active, whilst pruning files that become inactive.

So, it's not a complete loss.
However, it feels like continuing to push through with the current approach could be quite painful.
In particular, having to test things in Docker makes for a pretty awkward experience and slow feedback loop with the way we have it set up.
As such, it seems better to take this as 'lessons learned' and start again, with a bit of investment in our development environment and a clearer idea of what we're aiming for.

[Back to the README](../README.md#posts)
