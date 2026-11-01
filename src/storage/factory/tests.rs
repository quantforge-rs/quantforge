use super::*;
use crate::ports::StoreControl;
use crate::storage::settings::{StorageRequest, StoreOwner, Supplied};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn sqlite_settings(path: &Path) -> StorageSettings {
    let request = StorageRequest {
        sqlite_path: Some(Supplied::new(path.to_path_buf(), "--db")),
        ..StorageRequest::default()
    };
    StorageSettings::resolve(request, StoreOwner::Command).expect("sqlite settings")
}

/// Records what each call was handed, and opens an uninitialized SQLite
/// store at `path` whatever the backend, so that dispatch can be told
/// apart from what is opened.
fn recording(calls: &Arc<Mutex<Vec<String>>>, path: PathBuf) -> StoreProvider {
    let calls = Arc::clone(calls);
    Arc::new(move |settings: &StorageSettings| {
        calls.lock().expect("calls").push(settings.to_string());
        Ok(AnyStore::Sqlite(SqliteStore::new(&path)))
    })
}

fn calls(recorded: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    recorded.lock().expect("calls").clone()
}

#[test]
fn the_builtin_factory_opens_sqlite_creating_its_directory_and_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("market.sqlite");
    let store = StoreFactory::builtin()
        .open(&sqlite_settings(&path))
        .expect("a store");
    assert_eq!(store.backend(), StorageBackend::Sqlite);
    assert!(path.is_file());
    let identity = store.store_identity().expect("identity");
    assert_eq!(
        identity.schema_version,
        crate::storage::sqlite::SCHEMA_VERSION
    );
    assert_eq!(
        StoreFactory::builtin().registered().collect::<Vec<_>>(),
        [StorageBackend::Sqlite]
    );
}

#[test]
fn each_selection_reaches_its_own_provider_and_no_other() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite_calls = Arc::new(Mutex::new(Vec::new()));
    let postgres_calls = Arc::new(Mutex::new(Vec::new()));
    let factory = StoreFactory::empty()
        .register(
            StorageBackend::Sqlite,
            recording(&sqlite_calls, dir.path().join("a.sqlite")),
        )
        .register(
            StorageBackend::Postgres,
            recording(&postgres_calls, dir.path().join("b.sqlite")),
        );

    let sqlite = sqlite_settings(&dir.path().join("market.sqlite"));
    factory.open(&sqlite).expect("sqlite");
    assert_eq!(calls(&sqlite_calls), [sqlite.to_string()]);
    assert!(calls(&postgres_calls).is_empty());

    #[cfg(feature = "postgres")]
    {
        let request = StorageRequest {
            backend: Some(Supplied::new(StorageBackend::Postgres, "--storage")),
            postgres_url: Some(Supplied::new(
                crate::storage::settings::ConnectionString::new(
                    "postgres://quant:pw-sentinel-417@db.example.com/market",
                ),
                "QF_POSTGRES_URL",
            )),
            ..StorageRequest::default()
        };
        let postgres =
            StorageSettings::resolve(request, StoreOwner::Server).expect("postgres settings");
        factory.open(&postgres).expect("postgres");
        // The provider was handed the checked settings, nothing else.
        assert_eq!(
            calls(&postgres_calls),
            ["postgres postgres://db.example.com:5432/market"]
        );
        let StorageSettings::Postgres(handed) = &postgres else {
            panic!("not postgres");
        };
        assert_eq!(handed.pool_size, 8);
        assert_eq!(calls(&sqlite_calls).len(), 1, "sqlite was asked again");
    }
}

#[test]
fn an_empty_factory_refuses_every_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let refused = StoreFactory::empty()
        .open(&sqlite_settings(&dir.path().join("market.sqlite")))
        .expect_err("no provider");
    assert_eq!(refused.to_string(), "sqlite storage has no adapter yet");
    assert_eq!(std::fs::read_dir(dir.path()).expect("dir").count(), 0);
}

#[cfg(feature = "postgres")]
#[test]
fn postgres_without_a_provider_is_refused_and_sqlite_is_never_asked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite_calls = Arc::new(Mutex::new(Vec::new()));
    let only_sqlite = StoreFactory::empty().register(
        StorageBackend::Sqlite,
        recording(&sqlite_calls, dir.path().join("fallback.sqlite")),
    );
    let request = StorageRequest {
        backend: Some(Supplied::new(StorageBackend::Postgres, "--storage")),
        postgres_url: Some(Supplied::new(
            crate::storage::settings::ConnectionString::new("postgres://u@127.0.0.1:9/db"),
            "--postgres-url",
        )),
        ..StorageRequest::default()
    };
    let postgres =
        StorageSettings::resolve(request, StoreOwner::Command).expect("postgres settings");
    for factory in [&only_sqlite, &StoreFactory::builtin()] {
        let refused = factory.open(&postgres).expect_err("no postgres provider");
        assert!(
            matches!(
                refused,
                StoreOpenError::Config(StorageConfigError::NoAdapter {
                    backend: StorageBackend::Postgres
                })
            ),
            "{refused:?}"
        );
        assert_eq!(refused.to_string(), "postgres storage has no adapter yet");
    }
    assert!(calls(&sqlite_calls).is_empty(), "sqlite was asked instead");
    assert_eq!(
        std::fs::read_dir(dir.path()).expect("dir").count(),
        0,
        "a file was created"
    );
}

#[test]
fn a_later_registration_replaces_an_earlier_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let first = Arc::new(Mutex::new(Vec::new()));
    let second = Arc::new(Mutex::new(Vec::new()));
    let factory = StoreFactory::empty()
        .register(
            StorageBackend::Sqlite,
            recording(&first, dir.path().join("1")),
        )
        .register(
            StorageBackend::Sqlite,
            recording(&second, dir.path().join("2")),
        );
    factory
        .open(&sqlite_settings(&dir.path().join("market.sqlite")))
        .expect("opened");
    assert!(calls(&first).is_empty());
    assert_eq!(calls(&second).len(), 1);
    assert_eq!(factory.registered().count(), 1);
}

#[test]
fn a_providers_failure_is_a_storage_error() {
    let failing: StoreProvider =
        Arc::new(|_: &StorageSettings| Err(StorageError::Unavailable("down".to_string())));
    let refused = StoreFactory::empty()
        .register(StorageBackend::Sqlite, failing)
        .open(&sqlite_settings(Path::new("unused.sqlite")))
        .expect_err("failed");
    assert!(
        matches!(
            refused,
            StoreOpenError::Storage(StorageError::Unavailable(_))
        ),
        "{refused:?}"
    );
}

#[cfg(feature = "postgres")]
#[test]
fn the_sqlite_provider_refuses_settings_of_another_backend() {
    let request = StorageRequest {
        backend: Some(Supplied::new(StorageBackend::Postgres, "--storage")),
        postgres_url: Some(Supplied::new(
            crate::storage::settings::ConnectionString::new("postgres://u@localhost/db"),
            "--postgres-url",
        )),
        ..StorageRequest::default()
    };
    let postgres = StorageSettings::resolve(request, StoreOwner::Command).expect("settings");
    let refused = open_sqlite(&postgres).expect_err("wrong settings");
    assert!(
        matches!(refused, StorageError::InvalidArgument(_)),
        "{refused:?}"
    );
}
