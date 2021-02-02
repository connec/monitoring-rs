// main.rs
#[macro_use]
extern crate clap;

use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_std::prelude::FutureExt;
use async_std::sync::RwLock;
use async_std::task;
use structopt::StructOpt;

use monitoring_rs::log_collector::Collector;
use monitoring_rs::log_database::{self, Database};
use monitoring_rs::{api, log_collector};

/// Minimal Kubernetes monitoring pipeline.
#[derive(StructOpt)]
struct Args {
    /// The log collector to use.
    #[structopt(long, default_value, env, possible_values = &CollectorArg::variants())]
    log_collector: CollectorArg,

    /// The root path to watch.
    #[structopt(long, env, required_if("log-collector", "Directory"))]
    root_path: Option<PathBuf>,
}

arg_enum! {
    enum CollectorArg {
        Directory,
        Kubernetes,
    }
}

impl Default for CollectorArg {
    fn default() -> Self {
        Self::Kubernetes
    }
}

#[async_std::main]
async fn main() -> io::Result<()> {
    env_logger::init();

    let args = Args::from_args();

    let collector = init_collector(args)?;

    let database = init_database()?;

    let api_handle = api::server(Arc::clone(&database)).listen("0.0.0.0:8000");

    let collector_handle = task::spawn(blocking::unblock(move || {
        run_collector(collector, database)
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

fn init_collector(args: Args) -> io::Result<Box<dyn Collector + Send>> {
    match args.log_collector {
        CollectorArg::Directory => {
            use log_collector::directory::{self, Config};
            Ok(Box::new(directory::initialize(Config {
                // We can `unwrap` because we expect presence to be validated by structopt.
                root_path: args.root_path.unwrap(),
            })?))
        }
        CollectorArg::Kubernetes => {
            use log_collector::kubernetes::{self, Config};
            Ok(Box::new(kubernetes::initialize(Config {
                root_path: args.root_path,
            })?))
        }
    }
}

fn run_collector(collector: Box<dyn Collector>, database: Arc<RwLock<Database>>) -> io::Result<()> {
    for entry in collector {
        let entry = entry?;
        let mut database = task::block_on(database.write());
        database.write(&entry)?;
    }
    Ok(())
}
