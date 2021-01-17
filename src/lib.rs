// lib.rs
#[macro_use]
extern crate log;

pub mod api;
pub mod log_collector;
pub mod log_database;

use std::collections::HashMap;

#[derive(Debug, PartialEq)]
pub struct LogEntry {
    pub line: String,
    pub metadata: HashMap<String, String>,
}
