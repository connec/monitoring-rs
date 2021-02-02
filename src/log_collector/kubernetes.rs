// src/log_collector/kubernetes.rs
//! A log collector that collects logs from containers on a Kubernetes node.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use k8s_openapi::api::core::v1::Pod;
use kube::api::Meta;

use crate::log_collector::directory;
use crate::log_collector::watcher::Watcher;
use crate::LogEntry;

const DEFAULT_ROOT_PATH: &str = "/var/log/containers";

/// Configuration for [`initialize`].
pub struct Config {
    /// The root path from which to collect logs.
    ///
    /// This will default to the default Kubernetes log directory (`/var/log/containers`) if empty.
    pub root_path: Option<PathBuf>,
}

/// Initialize a [`Collector`](super::Collector) that collects logs from containers on a Kubernetes
/// node.
///
/// This wraps a [`directory`](super::directory) collector and post-processes
/// collected [`LogEntry`](crate::LogEntry)s to add metadata from the Kubernetes API.
///
/// See [`directory::initialize]`](super::directory::initialize) for more information about the file
/// watching behaviour.
///
/// # Errors
///
/// Propagates any `io::Error`s that occur during initialization.
pub fn initialize(config: Config) -> io::Result<impl super::Collector> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()?;

    // TODO: `unwrap` is not ideal, but we can't easily recover from bad/missing Kubernetes config,
    // and it wouldn't be much better to propagate the failure through `io::Error`.
    let kube_client = runtime.block_on(kube::Client::try_default()).unwrap();

    let watcher = super::watcher::watcher()?;
    Ok(Collector {
        runtime,
        kube_client,
        kube_resource: kube::Resource::all::<Pod>(),
        directory: directory::Collector::initialize(
            directory::Config {
                root_path: config
                    .root_path
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_ROOT_PATH)),
            },
            watcher,
        )?,
    })
}

/// A log collector that collects logs from containers on a Kubernetes node.
///
/// Under-the-hood this wraps a [`directory`](super::directory) collector and post-
/// processes collected [`LogEntry`](crate::LogEntry)s to add metadata from the Kubernetes API.
struct Collector<W: Watcher> {
    runtime: tokio::runtime::Runtime,
    kube_client: kube::Client,
    kube_resource: kube::Resource,
    directory: directory::Collector<W>,
}

impl<W: Watcher> Collector<W> {
    fn parse_path(path: &str) -> [&str; 4] {
        use std::convert::TryInto;

        // TODO: `unwrap` is not ideal, since we could feasibly have log files without a file stem.
        let stem = Path::new(path).file_stem().unwrap();

        // `unwrap` is OK since we converted from `str` above.
        let stem = stem.to_str().unwrap();

        // TODO: `unwrap` is not ideal, since log file names may not have exactly 3 underscores.
        stem.split('_').collect::<Vec<_>>().try_into().unwrap()
    }

    fn query_pod_metadata(&mut self, namespace: &str, pod_name: &str) -> BTreeMap<String, String> {
        self.kube_resource.namespace = Some(namespace.to_string());

        // TODO: `unwrap` may be OK here, since the only errors that can occur are from constructing
        // the HTTP request. This could only happen if `Resource::get` built an invalid URL. In our
        // case, that could only happen if the data in `k8s_openapi` or `namespace` is corrupt. We
        // couldn't reaasonably handle corruption in `k8s_openapi`, but we should check in future
        // what would happen for files containing dodgy (i.e. URL-unsafe) namespaces.
        let request = self.kube_resource.get(pod_name).unwrap();

        // TODO: `unwrap` is not ideal here, since missing pods or transient failures to communicate
        // with the Kubernetes API probably shouldn't crash the monitor. There's not really anything
        // better we can do with the current APIs, however (e.g. propagating in `io::Error` wouldn't
        // be better).
        let pod = self
            .runtime
            .block_on(self.kube_client.request::<Pod>(request))
            .unwrap();

        let meta = pod.meta();

        meta.labels.as_ref().cloned().unwrap_or_default()
    }
}

impl<W: Watcher> super::Collector for Collector<W> {}

impl<W: Watcher> Iterator for Collector<W> {
    type Item = io::Result<LogEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        let entry = self.directory.next()?;
        Some(entry.map(|mut entry| {
            // `unwrap` is OK since we know `directory` always sets `path`.
            let path = entry.metadata.remove("path").unwrap();
            let [pod_name, namespace, container_name, container_id] = Self::parse_path(&path);
            entry
                .metadata
                .insert("pod_name".to_string(), pod_name.to_string());
            entry
                .metadata
                .insert("namespace".to_string(), namespace.to_string());
            entry
                .metadata
                .insert("container_name".to_string(), container_name.to_string());
            entry
                .metadata
                .insert("container_id".to_string(), container_id.to_string());

            for (key, value) in self.query_pod_metadata(namespace, pod_name) {
                entry.metadata.insert(key, value);
            }

            entry
        }))
    }
}
