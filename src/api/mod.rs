// api/mod.rs
use std::sync::Arc;

use async_std::sync::RwLock;

use crate::log_database::Database;

type State = Arc<RwLock<Database>>;

pub type Server = tide::Server<State>;

pub fn server(database: State) -> Server {
    let mut app = tide::Server::with_state(database);
    app.at("/logs/*key").get(read_logs);
    app
}

async fn read_logs(req: tide::Request<State>) -> tide::Result {
    let key = req.param("key")?;
    let database = req.state().read().await;

    Ok(match database.read(key)? {
        Some(logs) => tide::Response::builder(tide::StatusCode::Ok)
            .body(tide::Body::from_json(&logs)?)
            .build(),
        None => tide::Response::new(tide::StatusCode::NotFound),
    })
}
