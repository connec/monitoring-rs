// main.rs
#[cfg_attr(target_os = "linux", macro_use)]
extern crate log;

mod api;
mod log_database;

#[cfg(target_os = "linux")]
mod log_collector;

use std::env;
use std::fs;
use std::io;
use std::sync::Arc;

use async_std::prelude::FutureExt;
use async_std::sync::RwLock;
use async_std::task;

#[cfg(target_os = "linux")]
use log_collector::Collector;
use log_database::Database;

#[cfg(target_os = "linux")]
const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[async_std::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let database = init_database()?;

    let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

    let collector_handle = task::spawn(blocking::unblock(move || init_collector(database)));

    api_handle.try_join(collector_handle).await?;

    Ok(())
}

fn init_database() -> io::Result<Arc<RwLock<Database>>> {
    let mut data_directory = env::current_dir()?;
    data_directory.push(".data");
    fs::create_dir_all(&data_directory)?;

    let config = log_database::Config { data_directory };
    let database = Database::open(config)?;
    Ok(Arc::new(RwLock::new(database)))
}

#[cfg(target_os = "linux")]
fn init_collector(database: Arc<RwLock<Database>>) -> io::Result<()> {
    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;
    let mut buffer = [0; 1024];
    loop {
        let entries = collector.collect_entries(&mut buffer)?;
        let mut database = task::block_on(database.write());
        for entry in entries {
            let key = entry.path.to_string_lossy();
            database.write(&key, &entry.line)?;
        }
    }
}

#[cfg(not(test))]
#[cfg(not(target_os = "linux"))]
fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
    compile_error!("log_collector is only available on Linux due to dependency on `inotify`");
    unreachable!()
}

#[cfg(test)]
#[cfg(not(target_os = "linux"))]
fn init_collector(_database: Arc<RwLock<Database>>) -> io::Result<()> {
    panic!("log_collector is only available on Linux due to dependency on `inotify`")
}
