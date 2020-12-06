// main.rs
#[macro_use]
extern crate log;

mod log_collector;

use std::io;

use log_collector::Collector;

const CONTAINER_LOG_DIRECTORY: &str = "/var/log/containers";

fn main() -> io::Result<()> {
    env_logger::init();

    let mut collector = Collector::initialize(CONTAINER_LOG_DIRECTORY.as_ref())?;

    let mut buffer = [0; 1024];
    loop {
        collector.handle_events(&mut buffer)?;
    }
}
