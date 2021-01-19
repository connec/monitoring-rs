// api/mod.rs

//! Types and functions for initialising the `monitoring-rs` HTTP API.

use std::sync::Arc;

use async_std::sync::RwLock;

use crate::log_database::Database;

type State = Arc<RwLock<Database>>;

/// An instance of the `monitoring-rs` HTTP API.
///
/// This is aliased to save typing out the entire `State` type. In future it could be replaced by an
/// opaque `impl Trait` type.
pub type Server = tide::Server<State>;

/// Initialise an instance of the `monitoring-rs` HTTP API.
pub fn server(database: State) -> Server {
    let mut app = tide::Server::with_state(database);
    app.at("/logs/:key/*value").get(read_logs);
    app
}

async fn read_logs(req: tide::Request<State>) -> tide::Result {
    let key = req.param("key")?;
    let value = req.param("value")?;
    let database = req.state().read().await;

    Ok(match database.query(key, value)? {
        Some(logs) => tide::Response::builder(tide::StatusCode::Ok)
            .body(tide::Body::from_json(&logs)?)
            .build(),
        None => tide::Response::new(tide::StatusCode::NotFound),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_std::sync::RwLock;
    use tide_testing::TideTestingExt;

    use crate::test::{self, log_entry, temp_database};

    #[async_std::test]
    async fn read_logs_non_existent_key() -> test::Result {
        let (_tempdir, database) = temp_database()?;
        let api = super::server(Arc::new(RwLock::new(database)));

        let response = api.get("/logs/foo/bar").await?;

        assert_eq!(response.status(), 404);

        Ok(())
    }

    #[async_std::test]
    async fn read_logs_existing_key() -> test::Result {
        let (_tempdir, mut database) = temp_database()?;

        database.write(&log_entry("hello", &[("foo", "bar")]))?;
        database.write(&log_entry("world", &[("foo", "bar")]))?;

        let api = super::server(Arc::new(RwLock::new(database)));

        let mut response = api.get("/logs/foo/bar").await?;

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.body_json::<Vec<String>>().await?,
            vec!["hello".to_string(), "world".to_string()]
        );

        Ok(())
    }
}
