// lib.rs

//! The elements that drive the `monitoring-rs` binary.

#![warn(
    explicit_outlives_requirements,
    macro_use_extern_crate,
    meta_variable_misuse,
    missing_crate_level_docs,
    missing_docs,
    private_doc_tests,
    single_use_lifetimes,
    trivial_casts,
    trivial_numeric_casts,
    unreachable_pub,
    unused_extern_crates,
    unused_lifetimes,
    variant_size_differences,
    clippy::cargo,
    clippy::pedantic
)]

pub mod api;
pub mod database;
pub mod log_collector;
pub mod log_database;

#[cfg(test)]
pub mod test;

use std::collections::HashMap;

/// A log entry that can be processed by the various parts of this library.
#[derive(Debug, PartialEq)]
pub struct LogEntry {
    /// A line of text in the log.
    pub line: String,

    /// Metadata associated with this log line.
    pub metadata: HashMap<String, String>,
}
