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

#[cfg(test)]
mod tests {
    use async_std::sync::RwLock;
    use std::sync::Arc;

    use tide_testing::TideTestingExt;

    use crate::log_database::test::open_temp_database;

    #[async_std::test]
    async fn read_logs_non_existent_key() {
        let (database, _tempdir) = open_temp_database();
        let api = super::server(Arc::new(RwLock::new(database)));

        let response = api.get("/logs//foo").await.unwrap();

        assert_eq!(response.status(), 404);
    }

    #[async_std::test]
    async fn read_logs_existing_key() {
        let (mut database, _tempdir) = open_temp_database();
        database.write("/foo", "hello").unwrap();
        database.write("/foo", "world").unwrap();

        let api = super::server(Arc::new(RwLock::new(database)));

        let mut response = api.get("/logs//foo").await.unwrap();

        assert_eq!(response.status(), 200);
        assert_eq!(
            response.body_json::<Vec<String>>().await.unwrap(),
            vec!["hello".to_string(), "world".to_string()]
        );
    }
}
