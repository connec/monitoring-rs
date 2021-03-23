// loadgen/src/main.rs
use std::error::Error;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use sanakirja::{self, Commit, RootDb};
use structopt::StructOpt;

use loadgen::{Distribution, Generator};
use monitoring_rs::database::{Database, Event, Labels, Query};

#[derive(StructOpt)]
struct Args {
    #[structopt(long, parse(try_from_str = Self::parse_database))]
    database: DatabaseArg,

    #[structopt(long)]
    avg_events_per_second: u32,

    #[structopt(long, parse(try_from_str = Self::parse_distribution))]
    distribution: Distribution,

    #[structopt(long)]
    seconds: u64,

    #[structopt(long)]
    streams: u32,
}

impl Args {
    fn parse_database(input: &str) -> Result<DatabaseArg, String> {
        match input {
            "crate" => Ok(DatabaseArg::Crate),
            "sanakirja" => Ok(DatabaseArg::Sanakirja),
            _ => Err(format!("unrecognised database: {}", input)),
        }
    }

    fn parse_distribution(input: &str) -> Result<Distribution, String> {
        match input {
            "uniform" => Ok(Distribution::Uniform),
            "linear" => Ok(Distribution::Linear),
            _ => Err(format!("unrecognised distribution: {}", input)),
        }
    }
}

enum DatabaseArg {
    Crate,
    Sanakirja,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::from_args();

    let tempdir = tempfile::tempdir()?;

    let (event, count_entries) = match args.database {
        DatabaseArg::Crate => crate_interface(tempdir.path())?,
        DatabaseArg::Sanakirja => sanakirja_interface(tempdir.path())?,
    };

    let total_events = args.avg_events_per_second * args.streams;
    let gen = Generator::new(
        Duration::from_secs(args.seconds),
        args.streams,
        total_events,
        args.distribution,
        event,
    );

    smol::block_on(gen.run());

    assert_eq!(count_entries()?, total_events as usize);

    Ok(())
}

type DbInterface = (
    Box<dyn Fn()>,
    Box<dyn Fn() -> Result<usize, Box<dyn Error>>>,
);

fn crate_interface(tmp_path: &Path) -> Result<DbInterface, Box<dyn Error>> {
    let db = Rc::new(Database::open(tmp_path.join("data"))?);
    let event = {
        let db = Rc::clone(&db);
        move || db.push(&make_labels(&[("hello", "world")]), make_event(0, "wow"))
    };
    let count_entries = move || {
        let query = Query::Label {
            name: "hello".to_string(),
            value: "world".to_string(),
        };
        Ok(db.query(&query)?.len())
    };

    Ok((Box::new(event), Box::new(count_entries)))
}

fn sanakirja_interface(tmp_path: &Path) -> Result<DbInterface, Box<dyn Error>> {
    let env = Rc::new(sanakirja::Env::new(tmp_path.join("data"), 8192, 2)?);
    let mut txn = sanakirja::Env::mut_txn_begin(env.as_ref())?;
    let db: sanakirja::btree::Db<u64, sanakirja::btree::UDb<[u8], [u8]>> =
        sanakirja::btree::create_db_(&mut txn)?;
    txn.set_root(0, db.db);
    txn.commit()?;

    let event = {
        let env = Rc::clone(&env);
        move || {
            let mut txn = sanakirja::Env::mut_txn_begin(env.as_ref()).expect("begin transaction");
            let mut db: sanakirja::btree::UDb<[u8], [u8]> =
                txn.root_db(0).expect("missing database");
            let mut labels = Vec::new();
            serde_json::to_writer(&mut labels, &make_labels(&[("hello", "world")]))
                .expect("serialize labels");
            let mut event = Vec::new();
            serde_json::to_writer(&mut event, &make_event(0, "wow")).expect("serialize event");
            sanakirja::btree::put(&mut txn, &mut db, &labels[..], &event[..]).expect("btree put");
            txn.commit().expect("txn commit");
        }
    };
    let count_entries = move || {
        let txn = sanakirja::Env::txn_begin(env.as_ref())?;
        let db: sanakirja::btree::UDb<[u8], [u8]> = txn.root_db(0).expect("missing database");
        let iter = sanakirja::btree::iter(&txn, &db, None)?;
        Ok(iter.count())
    };

    Ok((Box::new(event), Box::new(count_entries)))
}

fn make_labels(labels: &[(&str, &str)]) -> Labels {
    labels
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn make_event(timestamp: u64, data: impl AsRef<[u8]>) -> Event {
    Event::new(timestamp, data.as_ref().into())
}
