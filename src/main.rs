// main.rs
use std::env;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use async_std::prelude::FutureExt;
use async_std::sync::RwLock;
use async_std::task;

use monitoring_rs::log_database::{self, Database};
use monitoring_rs::{api, log_collector};

const VAR_CONTAINER_LOG_DIRECTORY: &str = "CONTAINER_LOG_DIRECTORY";
const DEFAULT_CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

#[async_std::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let container_log_directory = env::var(VAR_CONTAINER_LOG_DIRECTORY)
        .or_else(|error| match error {
            env::VarError::NotPresent => Ok(DEFAULT_CONTAINER_LOG_DIRECTORY.to_string()),
            error => Err(error),
        })
        .map_err(|error| io::Error::new(io::ErrorKind::Other, error))?;

    let database = init_database()?;

    let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

    let collector_handle = task::spawn(blocking::unblock(move || {
        init_collector(container_log_directory.as_ref(), database)
    }));

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

fn init_collector(
    container_log_directory: &Path,
    database: Arc<RwLock<Database>>,
) -> io::Result<()> {
    let collector = log_collector::directory::initialize(container_log_directory)?;
    for entry in collector {
        let entry = entry?;
        let mut database = task::block_on(database.write());
        database.write(&entry)?;
    }
    Ok(())
}
