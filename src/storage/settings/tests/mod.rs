//! How a backend is resolved from what a host supplied: every rule of
//! `StorageSettings::resolve`, and the shared limits. [`connection`] holds
//! the URL's own rules and the redaction.

mod connection;

use super::*;

/// A password that must never be shown, and a URL carrying it.
const SENTINEL: &str = "pw-sentinel-417";

fn url(text: &str) -> ConnectionString {
    ConnectionString::new(text)
}

fn parse(text: &str) -> Result<PostgresConnection, UrlProblem> {
    PostgresConnection::parse(&url(text))
}

fn supplied<T>(value: T, label: &str) -> Option<Supplied<T>> {
    Some(Supplied::new(value, label))
}

fn postgres_request(text: &str) -> StorageRequest {
    StorageRequest {
        backend: supplied(StorageBackend::Postgres, "--storage"),
        postgres_url: supplied(url(text), "QF_POSTGRES_URL"),
        ..StorageRequest::default()
    }
}

#[test]
fn backends_spell_their_names_both_ways_and_refuse_any_other() {
    for backend in StorageBackend::ALL {
        assert_eq!(
            backend.as_str().parse::<StorageBackend>().ok(),
            Some(backend)
        );
        assert_eq!(backend.to_string(), backend.as_str());
    }
    let refused = "mysql".parse::<StorageBackend>().expect_err("unknown");
    assert_eq!(
        refused.to_string(),
        "unknown storage backend \"mysql\"; use sqlite or postgres"
    );
    assert!(StorageBackend::Sqlite.is_built());
    assert_eq!(
        StorageBackend::Postgres.is_built(),
        cfg!(feature = "postgres")
    );
}

#[test]
fn nothing_supplied_means_sqlite_at_the_default_path() {
    let settings =
        StorageSettings::resolve(StorageRequest::default(), StoreOwner::Command).expect("sqlite");
    let StorageSettings::Sqlite(sqlite) = &settings else {
        panic!("not sqlite: {settings:?}");
    };
    assert_eq!(sqlite.path, PathBuf::from(DEFAULT_SQLITE_PATH));
    assert_eq!(sqlite.limits, StorageLimits::default());
    assert_eq!(settings.to_string(), "sqlite data/market.sqlite");
}

#[test]
fn a_supplied_path_is_used_with_the_backend_omitted_or_sqlite() {
    for backend in [None, supplied(StorageBackend::Sqlite, "QF_STORAGE")] {
        let request = StorageRequest {
            backend,
            sqlite_path: supplied(PathBuf::from("elsewhere.sqlite"), "--db"),
            ..StorageRequest::default()
        };
        let settings = StorageSettings::resolve(request, StoreOwner::Command).expect("sqlite");
        assert_eq!(settings.to_string(), "sqlite elsewhere.sqlite");
    }
}

#[test]
fn an_empty_path_is_refused_naming_its_label_with_the_backend_omitted_or_sqlite() {
    for backend in [None, supplied(StorageBackend::Sqlite, "--storage")] {
        let request = StorageRequest {
            backend,
            sqlite_path: supplied(PathBuf::new(), "QF_DB"),
            ..StorageRequest::default()
        };
        let refused = StorageSettings::resolve(request, StoreOwner::Command).expect_err("empty");
        assert!(
            matches!(&refused, StorageConfigError::EmptySqlitePath { label } if label == "QF_DB"),
            "{refused:?}"
        );
        assert_eq!(
            refused.to_string(),
            "QF_DB is empty; name the SQLite database file, or leave it unset"
        );
    }
}

#[test]
fn a_postgres_setting_beside_an_omitted_backend_is_refused_naming_its_label() {
    let cases = [
        (
            StorageRequest {
                postgres_url: supplied(url("postgres://u@127.0.0.1/db"), "QF_POSTGRES_URL"),
                postgres_pool_size: supplied(3, "--postgres-pool-size"),
                ..StorageRequest::default()
            },
            "QF_POSTGRES_URL",
        ),
        (
            StorageRequest {
                postgres_pool_size: supplied(3, "QF_POSTGRES_POOL_SIZE"),
                ..StorageRequest::default()
            },
            "QF_POSTGRES_POOL_SIZE",
        ),
        (
            StorageRequest {
                postgres_connect_timeout_secs: supplied(9, "--postgres-connect-timeout-secs"),
                ..StorageRequest::default()
            },
            "--postgres-connect-timeout-secs",
        ),
    ];
    for (request, label) in cases {
        let refused = StorageSettings::resolve(request, StoreOwner::Command).expect_err(label);
        assert!(
            matches!(&refused, StorageConfigError::PostgresSettingWithoutBackend { label: named } if named == label),
            "{refused:?}"
        );
        assert!(
            refused
                .to_string()
                .starts_with(&format!("{label} is a PostgreSQL setting"))
        );
    }
}

#[test]
fn an_explicit_sqlite_backend_leaves_the_postgres_settings_unused() {
    let request = StorageRequest {
        backend: supplied(StorageBackend::Sqlite, "--storage"),
        postgres_url: supplied(url("not a url"), "QF_POSTGRES_URL"),
        postgres_pool_size: supplied(0, "QF_POSTGRES_POOL_SIZE"),
        ..StorageRequest::default()
    };
    let settings = StorageSettings::resolve(request, StoreOwner::Command).expect("sqlite");
    assert_eq!(settings.backend(), StorageBackend::Sqlite);
}

#[test]
fn a_store_enforces_the_contract_unless_a_host_says_otherwise() {
    let limits = StorageLimits::default();
    assert_eq!(limits.durability, Durability::Enforced);
    assert_eq!(limits.wait_budget, POSTGRES_WAIT_TIMEOUT);

    let resolved = |durability| {
        let request = StorageRequest {
            durability,
            ..StorageRequest::default()
        };
        match StorageSettings::resolve(request, StoreOwner::Command).expect("settings") {
            StorageSettings::Sqlite(sqlite) => sqlite.limits.durability,
            other => panic!("not sqlite: {other}"),
        }
    };
    assert_eq!(resolved(None), Durability::Enforced);
    assert_eq!(resolved(Some(Durability::Reported)), Durability::Reported);
}

#[test]
fn a_route_leaves_a_second_of_the_three_a_webhook_delivery_has_unless_a_host_sets_it() {
    assert_eq!(DEFAULT_ROUTE_BUDGET, Duration::from_secs(2));
    let resolved = |route_budget| {
        let request = StorageRequest {
            route_budget,
            ..StorageRequest::default()
        };
        StorageSettings::resolve(request, StoreOwner::Command)
            .expect("settings")
            .limits()
            .route_budget
    };
    assert_eq!(resolved(None), DEFAULT_ROUTE_BUDGET);
    assert_eq!(
        resolved(Some(Duration::from_millis(1500))),
        Duration::from_millis(1500)
    );
}

#[test]
fn a_lock_wait_ends_before_the_call_does() {
    for remaining in [
        Duration::from_millis(40),
        Duration::from_secs(2),
        Duration::MAX,
    ] {
        assert!(lock_wait(remaining) < remaining);
    }
    assert_eq!(
        lock_wait(Duration::from_secs(2)),
        Duration::from_millis(1500)
    );
}

#[cfg(not(feature = "postgres"))]
#[test]
fn postgres_is_refused_in_a_build_without_it_before_its_settings_are_read() {
    let refused = StorageSettings::resolve(postgres_request("not a url"), StoreOwner::Command)
        .expect_err("not built");
    assert!(
        matches!(refused, StorageConfigError::NotBuilt { .. }),
        "{refused:?}"
    );
}

#[cfg(feature = "postgres")]
mod postgres {
    use super::*;

    fn resolve(request: StorageRequest) -> Result<PostgresSettings, StorageConfigError> {
        match StorageSettings::resolve(request, StoreOwner::Command)? {
            StorageSettings::Postgres(settings) => Ok(*settings),
            other => panic!("not postgres: {other:?}"),
        }
    }

    #[test]
    fn a_complete_url_resolves_with_adr_0001s_values() {
        let settings = resolve(postgres_request(&format!(
            "postgresql://quant%40forge:{SENTINEL}@db.example.com:6432/market%20data"
        )))
        .expect("settings");
        let connection = &settings.connection;
        assert_eq!(connection.host(), "db.example.com");
        assert_eq!(connection.port(), 6432);
        assert_eq!(connection.user(), "quant@forge");
        assert_eq!(connection.password(), Some(SENTINEL));
        assert_eq!(connection.dbname(), "market data");
        assert_eq!(connection.tls().mode, TlsMode::Require);
        assert_eq!(settings.pool_size, 2);
        assert_eq!(settings.connect_timeout, Duration::from_secs(5));
        assert_eq!(settings.wait_timeout, Duration::from_secs(5));
        assert_eq!(settings.statement_timeout, Duration::from_secs(30));
        assert_eq!(settings.call_deadline, Duration::from_secs(35));
        assert_eq!(settings.keepalive_idle, Duration::from_secs(30));
        assert_eq!(settings.application_name, "quantforge");
        assert_eq!(settings.limits, StorageLimits::default());
    }

    #[test]
    fn the_pool_size_defaults_by_owner_and_a_supplied_one_wins() {
        let owned = |owner| match StorageSettings::resolve(
            postgres_request("postgres://u@localhost/db"),
            owner,
        ) {
            Ok(StorageSettings::Postgres(settings)) => settings.pool_size,
            other => panic!("{other:?}"),
        };
        assert_eq!(owned(StoreOwner::Command), 2);
        assert_eq!(owned(StoreOwner::Server), 8);

        let mut request = postgres_request("postgres://u@localhost/db");
        request.postgres_pool_size = supplied(5, "--postgres-pool-size");
        request.postgres_connect_timeout_secs = supplied(12, "QF_POSTGRES_CONNECT_TIMEOUT_SECS");
        let settings = resolve(request).expect("settings");
        assert_eq!(settings.pool_size, 5);
        assert_eq!(settings.connect_timeout, Duration::from_secs(12));
    }

    #[test]
    fn a_zero_pool_size_or_timeout_is_refused_naming_its_label() {
        let mut request = postgres_request("postgres://u@localhost/db");
        request.postgres_pool_size = supplied(0, "QF_POSTGRES_POOL_SIZE");
        assert_eq!(
            resolve(request).expect_err("zero").to_string(),
            "QF_POSTGRES_POOL_SIZE must be at least 1"
        );
        let mut request = postgres_request("postgres://u@localhost/db");
        request.postgres_connect_timeout_secs = supplied(0, "--postgres-connect-timeout-secs");
        assert_eq!(
            resolve(request).expect_err("zero").to_string(),
            "--postgres-connect-timeout-secs must be at least 1"
        );
    }

    #[test]
    fn postgres_without_a_url_is_refused() {
        let request = StorageRequest {
            backend: supplied(StorageBackend::Postgres, "QF_STORAGE"),
            ..StorageRequest::default()
        };
        assert!(matches!(
            resolve(request),
            Err(StorageConfigError::MissingPostgresUrl)
        ));
    }

    #[test]
    fn a_sqlite_path_beside_postgres_is_refused_naming_its_label() {
        let mut request = postgres_request("postgres://u@localhost/db");
        request.sqlite_path = supplied(PathBuf::from("market.sqlite"), "QF_DB");
        let refused = resolve(request).expect_err("both");
        assert!(
            matches!(&refused, StorageConfigError::SqlitePathWithPostgres { label } if label == "QF_DB"),
            "{refused:?}"
        );
    }

    #[test]
    fn an_invalid_url_is_refused_naming_its_label_and_never_its_text() {
        let refused = resolve(postgres_request(&format!(
            "mysql://u:{SENTINEL}@db.example.com/db"
        )))
        .expect_err("scheme");
        let shown = format!("{refused} {refused:?}");
        assert!(shown.starts_with("QF_POSTGRES_URL is not a usable PostgreSQL URL"));
        assert!(!shown.contains(SENTINEL), "the password leaked");
        assert!(!shown.contains("db.example.com"), "the URL leaked");
    }
}
