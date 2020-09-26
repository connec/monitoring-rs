# Log collection (part 1)

That's enough discovery for now.
Let's implement something – starting with the clearest area of our requirements: collecting Kubernetes container logs.

## Prerequisites

### Obtain a Kubernetes cluster

We need a Kubernetes cluster!
I will be using one from DigitalOcean, set up based on [my own instructions](https://github.com/connec/do-k8s#create-a-cluster) (note that only the "Create a cluster" section matters).

**Aside:** the do-k8s project is also where the idea of creating a minimal monitoring application came from.
The de-facto logging and metrics components consume a huge amount of cluster resources for a small cluster for hobby development, which is not ideal.

From now on, we'll assume that `kubectl` is configured.
Let's create a namespace to work in:

```sh
$ kubectl create ns monitoring-rs
namespace/monitoring-rs created
```

If that worked, then you should be good to go.

### Set up a Docker registry

We will be pushing Docker images that we build along the way, and we also need to make sure our Kubernetes cluster can pull from the repository.
I will use a [DigitalOcean container registry](https://www.digitalocean.com/products/container-registry/) to see how that works out, but you can use whatever you want (baby) so long as you can push images with `docker push` and pull them onto your cluster.

To verify `docker push`, let's create a minimal Rust `Dockerfile` that we can update later:

```Dockerfile
# Dockerfile
FROM rust:1.46.0-alpine

RUN mkdir /build

WORKDIR /build
RUN cargo init --name dockertest .


FROM alpine

RUN apk add --no-cache tini
ENTRYPOINT ["/sbin/tini", "--"]

WORKDIR /root
COPY --from=0 /build/target/release/dockertest .

CMD ["./dockertest"]
```

Some notes:

- We use Alpine linux and a multi-stage build to ensure the final image is as small as possible.
- We use [`tini`](https://github.com/krallin/tini) to ensure that signals are handled properly, without having to implement proper signal handling in our binary.

We should create a `.dockerignore` as well to minimise what we send to the Docker daemon to process:

```.gitignore
*
!src/*
!Cargo.*
```

Before we build and try to push, let's put our Docker registry in an environment variable for reuse:

```sh
DOCKER_REGISTRY=<your registry>
```

OK, let's go:

```sh
$ docker build . -t $DOCKER_REGISTRY/dockertest:latest
...
Successfully tagged $DOCKER_REGISTRY/dockertest:latest

$ docker run $DOCKER_REGISTRY/dockertest:latest
Hello, world!

$ docker push $DOCKER_REGISTRY/dockertest:latest
...
latest: digest: ... size: 947
```

If you get an error at the `docker push` stage, check the documentation for your registry provider on how to authenticate.

Now let's try and pull from Kubernetes.
We can create a trivial `Pod` workload using `kubectl run`:

```sh
$ kubectl run dockertest \
  --namespace monitoring-rs \
  --image $DOCKER_REGISTRY/dockertest:latest \
  --restart Never \
  --attach \
  --rm \
  --wait
```

If the command appears to hang, debug with `kubectl get pods --namespace monitoring-rs` – if the status is `ErrImagePull` you probably need to fix your authentication.
Check the documentation for your registry provider on how to authenticate with Kubernetes.
If you're using `serviceaccount`-based `imagePullSecrets`, double check you put them on the appropriate service account in the `monitoring-rs` namespace (that caught me out...).

### Start a rust project

We can use Cargo to initialise a Rust project.

```sh
cargo new monitoring-rs
```

Obtaining Cargo is out of scope, but the easiest way is to use [rustup](https://rustup.rs/).

## Baby steps

Let's start by writing a trivial binary to periodically list the contents of `/var/log` and see what happens when we run it in our cluster.

The standard library has everything we need at this point, so let's just crack that out in `src/main.rs`:

```rust
// src/main.rs
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct DirTree {
    file_name: OsString,
    children: Option<Vec<DirTree>>,
}

impl DirTree {
    fn read_recursive<P: AsRef<Path>>(path: P) -> io::Result<Vec<Self>> {
        fs::read_dir(path)?
            .map(|entry| {
                let entry = entry?;
                let path = entry.path();

                let mut metadata = entry.metadata()?;
                if !metadata.is_file() && !metadata.is_dir() {
                    // Entry is a symlink
                    metadata = fs::metadata(&path)?;
                }

                let children = if metadata.is_file() {
                    None
                } else {
                    Some(Self::read_recursive(&path)?)
                };

                Ok(DirTree {
                    file_name: entry.file_name(),
                    children,
                })
            })
            .collect()
    }
}

impl fmt::Display for DirTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn write_tree(f: &mut fmt::Formatter<'_>, node: &DirTree, depth: usize) -> fmt::Result {
            let indentation = "  ".repeat(depth);

            if depth > 0 {
                writeln!(f)?;
            }

            write!(f, "{}{}", indentation, node.file_name.to_string_lossy())?;

            if let Some(ref children) = node.children {
                write!(f, "/")?;
                for child in children {
                    write_tree(f, child, depth + 1)?;
                }
            }

            Ok(())
        };

        write_tree(f, self, 0)
    }
}

fn main() {
    loop {
        match DirTree::read_recursive("/var/log") {
            Ok(entries) => println!(
                "{}",
                DirTree {
                    file_name: "/var/log".into(),
                    children: Some(entries),
                }
            ),
            Err(error) => {
                eprintln!("Warning: Failed to read /var/log due to: {:?}", error);
                eprintln!("Warning: Will try again in 30s");
            }
        }
        thread::sleep(Duration::from_secs(30));
    }
}
```

If you're on a Linux or Mac system you should be able to see the results:

```sh
$ cargo run
/var/log/
  fsck_apfs_error.log
  daily.out
  appfirewall.log
  mDNSResponder/
  displaypolicy/
    displaypolicyd.0:2:0.log
    displaypolicyd.log
  powermanagement/
    ...
  ...
```

### Docker build

Let's update our Dockerfile to build and run our `monitoring-rs` binary:

```diff
 # Dockerfile
 FROM rust:1.46.0-alpine

 RUN mkdir /build
+ADD . /build/

 WORKDIR /build
-RUN cargo init --name dockertest .
+RUN cargo build --release


 FROM alpine

 RUN apk add --no-cache tini
 ENTRYPOINT ["/sbin/tini", "--"]

 WORKDIR /root
-COPY --from=0 /build/target/release/dockertest .
+COPY --from=0 /build/target/release/monitoring-rs .

-CMD ["./dockertest"]
+CMD ["./monitoring-rs"]
```

Check it works with:

```sh
$ docker build . -t $DOCKER_REGISTRY/monitoring-rs
...
Successfully tagged $DOCKER_REGISTRY/monitoring-rs:latest
```

We could then run the built container (ctrl+C to exit):

```sh
$ docker run $DOCKER_REGISTRY/monitoring-rs:latest
/var/log
/var/log
...
^C
```

Very nice.

### Docker push

Let's push it and see what it does on Kubernetes:

```sh
$ docker push $DOCKER_REGISTRY/monitoring-rs:latest
...
latest: digest: ... size: 947

$ kubectl run monitoring-rs \
  --namespace monitoring-rs \
  --image $DOCKER_REGISTRY/monitoring-rs:latest \
  --restart Never \
  --attach \
  --rm \
  --wait
/var/log
/var/log
...
^C
```

Hrm, but where are the logs?
Surely we should at least see a log file for our running container, regardless of what else is on the cluster?
This is due to the default storage isolation of containers – right now `/var/log` in our container on Kubernetes is the `/var/log` from the container itself, which is an empty directory.

To get access to the host directory we can mount it into our monitoring-rs container.
We're stretching `kubectl run` now, but this can be achieved with some shenanigans:

```sh
$ kubectl run monitoring-rs \
    --image $DOCKER_REGISTRY/monitoring-rs:latest \
    --restart Never \
    --dry-run=client \
    --output json \
  | jq '.spec.containers[0].volumeMounts |= [{ "name":"varlog", "mountPath":"/var/log", "readOnly":true }, { "name":"varlibdockercontainers", "mountPath":"/var/lib/docker/containers", "readOnly":true }]' \
  | jq '.spec.volumes |= [{ "name":"varlog", "hostPath": { "path":"/var/log", "type":"Directory" }}, { "name":"varlibdockercontainers", "hostPath": { "path": "/var/lib/docker/containers", "type": "Directory" }}]' \
  | kubectl run monitoring-rs \
    --namespace monitoring-rs \
    --image $DOCKER_REGISTRY/monitoring-rs:latest \
    --restart Never \
    --overrides "$(cat)" \
    --attach
/var/log/
  ...
  containers/
    ...
  pods/
    ...
  ...
^C
```

**Note:** it can take some time for `kubectl run` to attach to the running container, meaning logs from the first iteration don't always show.
You can just wait 30 seconds to see the next iteration, or if you're impatient you can retrieve the logs you missed using `kubectl logs -n monitoring-rs monitoring-rs`.
Also, when exiting with ctrl+C `kubectl run` won't delete the pod, so you should clean it up manually with `kubectl delete pods -n monitoring-rs monitoring-rs`.

The `jq` invocations in the above command are augmenting our `Pod` with volumes for `/var/log` and `/var/lib/docker/containers`, which are mounted from the host.
The mount for `/var/lib/docker/containers` is needed because the log files in `/var/log/pods` are all symlinks into that directory.
The log files in `/var/log/pods` are themselves symlinked from the `/var/log/containers` directory, meaning the log files in `/var/log/containers` and `/var/log/pods` are the same but under different structures:

- The log files in `/var/log/containers` have the following form:

  ```
  <pod name>_<namespace>_<container name>-<container ID>.log
  ```

- The log files in `/var/log/pods` have the following form:

  ```
  <namespace>_<pod name>_<pod uid>/
    <container name>/
      <n - 1>.log
      <n>.log
  ```

  `n` starts at zero, and is incremented whenever the container restarts.
  Kubernetes only retains the logs of one previous container (`n - 1`).

Now that we've confirmed where the log files are, the next thing to do is start scraping their contents.
