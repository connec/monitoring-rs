// main.rs
#[macro_use]
extern crate log;

mod api;
mod log_collector;
mod log_database;

use std::env;
use std::fs;
use std::io;
use std::sync::Arc;
use std::thread;

use async_std::prelude::FutureExt;
use async_std::sync::RwLock;
use async_std::task::block_on;

use log_collector::Collector;
use log_database::Database;

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[async_std::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let mut data_directory = env::current_dir()?;
    data_directory.push(".data");
    fs::create_dir_all(&data_directory)?;

    let database = Arc::new(RwLock::new(Database::open(log_database::Config {
        data_directory,
    })?));

    let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

    let collector_thread = thread::spawn::<_, io::Result<()>>(move || {
        let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
        let mut buffer = [0; 1024];
        loop {
            let entries = collector.collect_entries(&mut buffer)?;
            let mut database = block_on(database.write());
            for entry in entries {
                let key = entry.path.to_string_lossy();
                database.write(&key, &entry.line)?;
            }
        }
    });
    let collector_handle = blocking::unblock(|| collector_thread.join().unwrap());

    api_handle.try_join(collector_handle).await?;

    Ok(())
}
