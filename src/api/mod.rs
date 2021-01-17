// api/mod.rs
use std::sync::Arc;

use async_std::sync::RwLock;

use crate::log_database::Database;

type State = Arc<RwLock<Database>>;

pub type Server = tide::Server<State>;

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
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_std::sync::RwLock;
    use tide_testing::TideTestingExt;

    use crate::log_database::test::open_temp_database;
    use crate::LogEntry;

    #[async_std::test]
    async fn read_logs_non_existent_key() {
        let (database, _tempdir) = open_temp_database();
        let api = super::server(Arc::new(RwLock::new(database)));

        let response = api.get("/logs/foo/bar").await.unwrap();

        assert_eq!(response.status(), 404);
    }

    #[async_std::test]
    async fn read_logs_existing_key() {
        let (mut database, _tempdir) = open_temp_database();
        let metadata: HashMap<_, _> = vec![("foo".to_string(), "bar".to_string())]
            .into_iter()
            .collect();
        database
            .write(&LogEntry {
                line: "hello".into(),
                metadata: metadata.clone(),
            })
            .unwrap();
        database
            .write(&LogEntry {
                line: "world".into(),
                metadata,
            })
            .unwrap();

        let api = super::server(Arc::new(RwLock::new(database)));

        let mut response = api.get("/logs/foo/bar").await.unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.body_json::<Vec<String>>().await.unwrap(),
            vec!["hello".to_string(), "world".to_string()]
        );
    }
}
