// log_collector/mod.rs
pub mod directory;
mod watcher;

use std::io;

use crate::LogEntry;

pub trait Collector: Iterator<Item = Result<LogEntry, io::Error>> {}
