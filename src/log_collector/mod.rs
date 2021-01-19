// log_collector/mod.rs

//! The interface for log collection in `monitoring-rs`.

pub mod directory;
mod watcher;

use std::io;

use crate::LogEntry;

/// A log collector can be any type that can be used as an `Iterator` of [`LogEntry`]s.
///
/// This is currently just a marker trait, but this could change as new log collectors are added.
pub trait Collector: Iterator<Item = Result<LogEntry, io::Error>> {}
