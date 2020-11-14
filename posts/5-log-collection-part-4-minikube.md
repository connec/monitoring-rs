# Log collection (part 4 – minikube)

## Validating against Kubernetes

In [Log collection (part 1)](1-log-collection-part-1.md) we did some poking around in Kubernetes by executing horrendous `kubectl run` snippets such as:

```sh
kubectl run monitoring-rs \
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
```

Ooft.
Let's try and make this a bit more ergonomic by using [minikube](https://minikube.sigs.k8s.io/) to remove the need for a remote cluster and our `Makefile` to make the commands repeatable.

### Minikube

See minikube's [Get Started!](https://minikube.sigs.k8s.io/docs/start/) documentation for how to install minikube.
Once installed, we should be able to run `minikube` to start a cluster:

```
$ minikube start
...
🏄  Done! kubectl is now configured to use "minikube" by default

$ kubectl config get-contexts
CURRENT   NAME              CLUSTER           AUTHINFO                NAMESPACE
...
*         minikube          minikube          minikube
...

$ kubectl get pods
No resources found in default namespace.

$ kubectl run dockertest \
  --image alpine \
  --restart Never \
  --attach \
  --rm \
  --wait \
  echo 'Hello from Kubernetes!'
Hello from Kubernetes!
pod "dockertest" deleted
```

Very nice.
You should also take whatever steps are required by your docker registry to allow Kubernetes to pull images.

### `Makefile`

Let's use our `Makefile` to set up some repeatable commands that we can use to explore things in our established minikube:

```diff
--- a/Makefile
+++ b/Makefile
@@ -1,8 +1,13 @@
 # Makefile
-.PHONY: monitoring writer inspect rotate down reset
+.PHONY: build-monitoring monitoring writer inspect rotate down reset push

-monitoring:
- @docker-compose up --build --force-recreate monitoring
+DOCKER_IMAGE := registry.digitalocean.com/connec-co-uk/monitoring-rs:latest
+
+build-monitoring:
+ @docker-compose build monitoring
+
+monitoring: build-monitoring
+ @docker-compose up --force-recreate monitoring

 writer:
  @docker-compose up -d writer
@@ -17,3 +22,25 @@ down:
  @docker-compose down --timeout 0 --volumes

 reset: down writer
+
+push: build-monitoring
+ @docker push $(DOCKER_IMAGE)
+
+kuberun: push
+ @kubectl run monitoring-rs \
+     --image $(DOCKER_IMAGE) \
+     --env RUST_LOG=monitoring_rs \
+     --restart Never \
+     --dry-run=client \
+     --output json \
+   | jq '.spec.containers[0].volumeMounts |= [{ "name":"varlog", "mountPath":"/var/log", "readOnly":true }, { "name":"varlibdockercontainers", "mountPath":"/var/lib/docker/containers", "readOnly":true }]' \
+   | jq '.spec.volumes |= [{ "name":"varlog", "hostPath": { "path":"/var/log", "type":"Directory" }}, { "name":"varlibdockercontainers", "hostPath": { "path": "/var/lib/docker/containers", "type": "Directory" }}]' \
+   | kubectl run monitoring-rs \
+     --image $(DOCKER_IMAGE) \
+     --restart Never \
+     --overrides "$$(cat)"
+ @kubectl wait --for=condition=Ready pod/monitoring-rs
+ @kubectl logs -f monitoring-rs
+
+kubecleanup:
+ @kubectl delete pods monitoring-rs --ignore-not-found
```

Now we can run our collector on Kubernetes with a single command:

```
$ make kuberun
...
If you don't see a command prompt, try pressing enter.
```

And... nothing.
If we ctrl+c out of that and check `kubectl logs` we can see that the watch is being initialised:

```
$ kubectl logs monitoring-rs
[2020-11-13T21:58:58Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
```

We can also `exec` against the pod to see that there are indeed log files present:

```
$ kubectl exec -it monitoring-rs -- ls /var/log/containers
coredns-f9fd979d6-2nb5l_kube-system_coredns-278e89d9af38b7466f1d91ca9d9cec5cd91c1214c036689ba7b766442caa0bab.log
etcd-minikube_kube-system_etcd-181a7ad1394128cb0586209583efb98901c594520b739a20a0a449473011f9a9.log
kube-apiserver-minikube_kube-system_kube-apiserver-0b1094f3e5ffc3d0e9736167d34de50b312c9fec6cf7a3398d14f7d7ccc6e195.log
kube-controller-manager-minikube_kube-system_kube-controller-manager-4b35df48f450c393db3f5a6918c1c5713c74a8661aacd51eb3e13433a2ebc698.log
kube-proxy-675td_kube-system_kube-proxy-1b187129f0bbee77c1c452548b2c69cdab5330ad743a5cc0d25dc59ba84c57f4.log
kube-scheduler-minikube_kube-system_kube-scheduler-2d776733a7712d12dd0877bcebe0c050c439b88ff0bf20c4a6ec5364a2cc4625.log
monitoring-rs_default_monitoring-rs-a60032170d01b104ab50ec41b8696d6e358ab4ab40a9abdda1ce324a4c1e767f.log
storage-provisioner_kube-system_storage-provisioner-7cd228a377865f577f73ed36df520e0c3a09ef5289fa5786da88cbbad7a6e148.log
```

And yet our collector isn't seeing any events.
We can try to `tail` one of the log files to confirm that logs are indeed being written (you will have to copy+paste a log file from the `ls` command above – `kube-controller-manager` is quite chatty):

```
$ kubectl exec monitoring-rs -- tail -f /var/log/containers/kube-controller-manager-minikube_kube-system_kube-controller-manager-4b35df48f450c393db3f5a6918c1c5713c74a8661aacd51eb3e13433a2ebc698.log
...
{"log":"I1113 22:07:01.452369       1 clientconn.go:948] ClientConn switching balancer to \"pick_first\"\n","stream":"stderr","time":"2020-11-13T22:07:01.4530253Z"}
...
```

If you wait a few seconds you should see new logs appear, and yet our collector is none the wiser:

```
$ kubectl logs monitoring-rs
[2020-11-13T21:58:58Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
```

Let's poke the directory directly and see if anything shows up:

```
$ kubectl exec monitoring-rs -- touch /var/log/containers/hello
touch: /var/log/containers/hello: Read-only file system
command terminated with exit code 1
```

Oh yeah, we mounted the log directories as `readOnly: true`.
Let's temporarily change that:

```diff
--- a/Makefile
+++ b/Makefile
@@ -33,7 +33,7 @@ kuberun: push
      --restart Never \
      --dry-run=client \
      --output json \
-   | jq '.spec.containers[0].volumeMounts |= [{ "name":"varlog", "mountPath":"/var/log", "readOnly":true }, { "name":"varlibdockercontainers", "mountPath":"/var/lib/docker/containers", "readOnly":true }]' \
+   | jq '.spec.containers[0].volumeMounts |= [{ "name":"varlog", "mountPath":"/var/log" }, { "name":"varlibdockercontainers", "mountPath":"/var/lib/docker/containers", "readOnly":true }]' \
    | jq '.spec.volumes |= [{ "name":"varlog", "hostPath": { "path":"/var/log", "type":"Directory" }}, { "name":"varlibdockercontainers", "hostPath": { "path": "/var/lib/docker/containers", "type": "Directory" }}]' \
    | kubectl run monitoring-rs \
      --image $(DOCKER_IMAGE) \
```

We can then stop and restart our monitoring pod:

```
$ make kubecleanup kuberun
pod "monitoring-rs" deleted
...
If you don't see a command prompt, try pressing enter.
```

Ctrl+c out of the logs and let's try poking again:

```
$ kubectl exec monitoring-rs -- touch /var/log/containers/hello
$ kubectl logs monitoring-rs
[2020-11-13T22:15:15Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
```

Still nothing.
We should have at least seen the `trace` for received events.
As a final check, let's write to the file:

```
$ kubectl exec monitoring-rs -- sh -c 'echo "hello?" >> /var/log/containers/hello'
$ kubectl logs monitoring-rs
[2020-11-13T22:15:15Z DEBUG monitoring_rs] Initialising watch on path "/var/log/containers"
[2020-11-13T22:21:05Z TRACE monitoring_rs] Received event: Event { wd: WatchDescriptor { id: 1, fd: (Weak) }, mask: MODIFY, cookie: 0, name: Some("hello") }
[2020-11-13T22:21:05Z DEBUG monitoring_rs] Create /var/log/containers/hello
```

Wow, finally!
So, `inotify` works in our Kubernetes environment – that's relieving!
However, writes to the actual log files do not emit events.
Let's take a closer look at the log files:

```
$ kubectl exec monitoring-rs -- ls -l /var/log/containers
lrwxrwxrwx  1 root  root  100 Nov 13 21:49 coredns-f9fd979d6-2nb5l_kube-system_coredns-6a8fd4ba73c8bcbe96eebab8f0151aa98b7276714f236560e2db4e05647ed8bc.log -> /var/log/pods/kube-system_coredns-f9fd979d6-2nb5l_812814b8-435b-41ab-90b1-fd21e9360013/coredns/1.log
lrwxrwxrwx  1 root  root   83 Nov 13 21:49 etcd-minikube_kube-system_etcd-181a7ad1394128cb0586209583efb98901c594520b739a20a0a449473011f9a9.log -> /var/log/pods/kube-system_etcd-minikube_d186e6390814d4dd7e770f47c08e98a2/etcd/1.log
-rw-r--r--  1 root  root    7 Nov 13 22:21 hello
lrwxrwxrwx  1 root  root  103 Nov 13 21:49 kube-apiserver-minikube_kube-system_kube-apiserver-160183f734f526c5a9499d89b11cded62e4615530e87f3b97ebe6360e9394a74.log -> /var/log/pods/kube-system_kube-apiserver-minikube_f7c3d51df5e2ce4e433b64661ac4503c/kube-apiserver/2.log
lrwxrwxrwx  1 root  root  121 Nov 13 21:49 kube-controller-manager-minikube_kube-system_kube-controller-manager-8f5897d1a5580c2627a500e81f2f8d24fa1543c149fbd7cd6be3b98718326ef1.log -> /var/log/pods/kube-system_kube-controller-manager-minikube_dcc127c185c80a61d90d8e659e768641/kube-controller-manager/1.log
lrwxrwxrwx  1 root  root   96 Nov 13 21:49 kube-proxy-675td_kube-system_kube-proxy-d22bff9fa4fd4a1402efc55dd4b1a2b6a154ccc509ba26b9294083711fe217a9.log -> /var/log/pods/kube-system_kube-proxy-675td_ae1b3faf-50e8-41a3-9ea8-b7e692a57871/kube-proxy/1.log
lrwxrwxrwx  1 root  root  103 Nov 13 21:49 kube-scheduler-minikube_kube-system_kube-scheduler-2d776733a7712d12dd0877bcebe0c050c439b88ff0bf20c4a6ec5364a2cc4625.log -> /var/log/pods/kube-system_kube-scheduler-minikube_ff7d12f9e4f14e202a85a7c5534a3129/kube-scheduler/1.log
lrwxrwxrwx  1 root  root   92 Nov 13 22:15 monitoring-rs_default_monitoring-rs-8af7ed918ba1b75e433604d0fa41c0f69775526f856b3cad6866e746b115a231.log -> /var/log/pods/default_monitoring-rs_2dbf1e6c-400e-4374-97d2-694861d29fb0/monitoring-rs/0.log
lrwxrwxrwx  1 root  root  108 Nov 13 21:50 storage-provisioner_kube-system_storage-provisioner-c2f35ccb1eb03b51fffc693a5c548a31801547886bbde8d1f549c97c39ffc4b1.log -> /var/log/pods/kube-system_storage-provisioner_8a868071-86d1-4f88-822f-074ddfdd879a/storage-provisioner/4.log
```

So all the log files are links to files in `/var/log/pods`.
We actually glossed over this in [Log collection (part 1)](1-log-collection-part-1.md):

> The mount for `/var/lib/docker/containers` is needed because the log files in `/var/log/pods` are all symlinks into that directory.
> The log files in `/var/log/pods` are themselves symlinked from the `/var/log/containers` directory, meaning the log files in `/var/log/containers` and `/var/log/pods` are the same but under different structures [...]

We can confirm this using `realpath`:

```
$ kubectl exec monitoring-rs -- realpath /var/log/containers/kube-controller-manager-minikube_kube-system_kube-controller-manager-4b35df48f450c393db3f5a6918c1c5713c74a8661aacd51eb3e13433a2ebc698.log
/var/lib/docker/containers/4b35df48f450c393db3f5a6918c1c5713c74a8661aacd51eb3e13433a2ebc698/4b35df48f450c393db3f5a6918c1c5713c74a8661aacd51eb3e13433a2ebc698-json.log
```

In order to obtain events for these symlinked files, we will have to modify our collector's use of inotify to:

1. Create a watch for create events (rather than modify events) on the directory.
1. List the files in the directory, and create a watch for modify events on the *real path* of each one.
1. Handle create events in the directory by looking up the real path of the created file and adding a watch for it.
1. Handle modify events as now.

Let's rework our implementation:

```rust
// main.rs
#[macro_use]
extern crate log;

use std::collections::hash_map::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Seek, Stdout};
use std::path::{Path, PathBuf};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[derive(Debug)]
enum Event<'collector> {
    Create {
        path: PathBuf,
    },
    Append {
        stdout: &'collector mut Stdout,
        live_file: &'collector mut LiveFile,
    },
    Truncate {
        stdout: &'collector mut Stdout,
        live_file: &'collector mut LiveFile,
    },
}

impl Event<'_> {
    fn name(&self) -> &str {
        match self {
            Event::Create { .. } => "Create",
            Event::Append { .. } => "Append",
            Event::Truncate { .. } => "Truncate",
        }
    }

    fn path(&self) -> &Path {
        match self {
            Event::Create { path } => path,
            Event::Append { live_file, .. } => &live_file.path,
            Event::Truncate { live_file, .. } => &live_file.path,
        }
    }
}

impl std::fmt::Display for Event<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.name(), self.path().display())
    }
}

#[derive(Debug)]
struct LiveFile {
    path: PathBuf,
    file: File,
}

struct Collector {
    root_path: PathBuf,
    root_wd: WatchDescriptor,
    stdout: Stdout,
    live_files: HashMap<WatchDescriptor, LiveFile>,
    inotify: Inotify,
}

impl Collector {
    pub fn initialize(root_path: &Path) -> io::Result<Self> {
        let mut inotify = Inotify::init()?;

        debug!("Initialising watch on root path {:?}", root_path);
        let root_wd = inotify.add_watch(root_path, WatchMask::CREATE)?;

        let mut collector = Self {
            root_path: root_path.to_path_buf(),
            root_wd,
            stdout: io::stdout(),
            live_files: HashMap::new(),
            inotify,
        };

        for entry in fs::read_dir(root_path)? {
            let entry = entry?;
            let path = entry.path();

            debug!("{}", Event::Create { path: path.clone() });
            collector.handle_event_create(path)?;
        }

        Ok(collector)
    }

    pub fn handle_events(&mut self, buffer: &mut [u8]) -> io::Result<()> {
        let inotify_events = self.inotify.read_events_blocking(buffer)?;

        for inotify_event in inotify_events {
            trace!("Received inotify event: {:?}", inotify_event);

            if let Some(event) = self.check_event(inotify_event)? {
                debug!("{}", event);

                match event {
                    Event::Create { path } => self.handle_event_create(path),
                    Event::Append { stdout, live_file } => {
                        Self::handle_event_append(stdout, &mut live_file.file)
                    }
                    Event::Truncate { stdout, live_file } => {
                        Self::handle_event_truncate(stdout, &mut live_file.file)
                    }
                }?;
            }
        }

        Ok(())
    }

    fn check_event<'ev>(
        &mut self,
        inotify_event: inotify::Event<&'ev OsStr>,
    ) -> io::Result<Option<Event>> {
        if inotify_event.wd == self.root_wd {
            if !inotify_event.mask.contains(EventMask::CREATE) {
                warn!(
                    "Received unexpected event for root fd: {:?}",
                    inotify_event.mask
                );
                return Ok(None);
            }

            let name = match inotify_event.name {
                None => {
                    warn!("Received CREATE event for root fd without a name");
                    return Ok(None);
                }
                Some(name) => name,
            };

            let mut path = PathBuf::with_capacity(self.root_path.capacity() + name.len());
            path.push(&self.root_path);
            path.push(name);

            return Ok(Some(Event::Create { path }));
        }

        let stdout = &mut self.stdout;
        let live_file = match self.live_files.get_mut(&inotify_event.wd) {
            None => {
                warn!(
                    "Received event for unregistered watch descriptor: {:?} {:?}",
                    inotify_event.mask, inotify_event.wd
                );
                return Ok(None);
            }
            Some(live_file) => live_file,
        };

        let metadata = live_file.file.metadata()?;
        let seekpos = live_file.file.seek(io::SeekFrom::Current(0))?;

        if seekpos <= metadata.len() {
            Ok(Some(Event::Append { stdout, live_file }))
        } else {
            Ok(Some(Event::Truncate { stdout, live_file }))
        }
    }

    fn handle_event_create(&mut self, path: PathBuf) -> io::Result<()> {
        let realpath = fs::canonicalize(&path)?;

        let wd = self.inotify.add_watch(&realpath, WatchMask::MODIFY)?;
        let mut file = File::open(realpath)?;
        file.seek(io::SeekFrom::End(0))?;

        self.live_files.insert(wd, LiveFile { path, file });

        Ok(())
    }

    fn handle_event_append(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
        io::copy(&mut file, stdout)?;

        Ok(())
    }

    fn handle_event_truncate(stdout: &mut io::Stdout, mut file: &mut File) -> io::Result<()> {
        file.seek(io::SeekFrom::Start(0))?;
        io::copy(&mut file, stdout)?;

        Ok(())
    }
}

fn main() -> io::Result<()> {
    env_logger::init();

    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
```

This is a substantial refactor, so the whole updated file is shown.
The high-level approach remains similar, however:

1. Initialize a collector, which now watches the root path for `CREATE` events and the directory's contents for `MODIFY` events.
1. Poll for `inotify::Event`s in a loop.
1. On receipt of an `inotify::Event`, try to convert it into an `Event` that's meaningful to our collector, and act accordingly.

Let's build and run our new collector in Kubernetes and see what happens:

```
$ make kubecleanup kuberun
pod "monitoring-rs" deleted
<explosion>
^C
```

Wow, so we're getting logs alright.
In fact, we've accidentally created an infinite log generator, since the collector will collect and re-print its own messages (which it will then collect and re-print... and so on).
We should probably stop it before it fills the node disk:

```
$ make kubecleanup
pod "monitoring-rs" deleted
```

Of course, forwarding logs to `stdout` is just a stand-in for our actual log collection behaviour.
In the spirit of keeping things minimal, we could persist logs in the same process... but that will be an exercise for another day!

[Back to the README](../README.md#posts)
