//! A database of its own for a test that works on a real server.

use super::database::unique;
use super::*;
use tokio_postgres::SimpleQueryMessage;

/// A database of its own on the test server, dropped with it.
pub(super) struct Scratch {
    server: PostgresStore,
    name: String,
    url: String,
}

impl Scratch {
    pub(super) fn create(server_url: &str) -> Self {
        let server =
            PostgresStore::connect(&settings(server_url, &unique("server"))).expect("the server");
        let name = unique("migrate");
        rows(&server, &format!("CREATE DATABASE {name}"));
        let mut url = url::Url::parse(server_url).expect("a URL");
        url.set_path(&format!("/{name}"));
        Self {
            server,
            name,
            url: url.to_string(),
        }
    }

    /// A store on the scratch database, its schema as the test left it.
    pub(super) fn store(&self) -> PostgresStore {
        PostgresStore::connect(&settings(&self.url, &unique("store"))).expect("a store")
    }

    /// A store that gives up on any lock long before a slow step ends.
    pub(super) fn impatient_store(&self) -> PostgresStore {
        let mut settings = settings(&self.url, &unique("store"));
        settings.limits.wait_budget = Duration::from_millis(20);
        PostgresStore::connect(&settings).expect("a store")
    }

    /// What new connections to the scratch database start with.
    pub(super) fn set_for_new_connections(&self, setting: &str, value: &str) {
        rows(
            &self.server,
            &format!("ALTER DATABASE {} SET {setting} = {value}", self.name),
        );
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let drop = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name);
        let _ = self.server.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move { client.batch_execute(&drop).await })
        });
    }
}

/// Every row of `sql`, each value as the server's text, NULL as `None`.
pub(super) fn rows(store: &PostgresStore, sql: &str) -> Vec<Vec<Option<String>>> {
    let owned = sql.to_string();
    store
        .bridge
        .execute(Effect::Write, move |client| {
            Box::pin(async move {
                let messages = client.simple_query(&owned).await?;
                Ok(messages
                    .into_iter()
                    .filter_map(|message| match message {
                        SimpleQueryMessage::Row(row) => Some(
                            (0..row.len())
                                .map(|index| row.get(index).map(str::to_string))
                                .collect(),
                        ),
                        _ => None,
                    })
                    .collect::<Vec<Vec<Option<String>>>>())
            })
        })
        .unwrap_or_else(|error| panic!("{sql}: {error:?}"))
}
