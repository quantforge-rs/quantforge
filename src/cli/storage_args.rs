//! The global storage flags: which backend keeps the data, and how to reach
//! it. `parse()` records where each value came from, and
//! [`StorageArgs::request`] hands the values the operator supplied,
//! labelled with their flag or variable, to the library's
//! `StorageSettings::resolve`, whose errors name those labels and never a
//! value.

use clap::parser::ValueSource;
use clap::{ArgMatches, Args};
use quantforge::storage::settings::{DEFAULT_CONNECT_TIMEOUT, DEFAULT_SQLITE_PATH};
use quantforge::{ConnectionString, StorageBackend, StorageRequest, Supplied};
use std::fmt;
use std::path::PathBuf;

/// Help section for the storage flags.
const STORAGE_HEADING: &str = "Storage";

/// Each storage flag's argument id, its flag, and its variable: the names
/// an error may use for where a value came from.
pub(super) const SETTINGS: [(&str, &str, &str); 5] = [
    ("db", "--db", "QF_DB"),
    ("storage", "--storage", "QF_STORAGE"),
    ("postgres_url", "--postgres-url", "QF_POSTGRES_URL"),
    (
        "postgres_pool_size",
        "--postgres-pool-size",
        "QF_POSTGRES_POOL_SIZE",
    ),
    (
        "postgres_connect_timeout_secs",
        "--postgres-connect-timeout-secs",
        "QF_POSTGRES_CONNECT_TIMEOUT_SECS",
    ),
];

#[derive(Args)]
pub(crate) struct StorageArgs {
    /// SQLite database path; refused with --storage postgres.
    #[arg(
        long,
        global = true,
        env = "QF_DB",
        default_value = DEFAULT_SQLITE_PATH,
        help_heading = STORAGE_HEADING
    )]
    pub(crate) db: PathBuf,

    /// Storage backend.
    #[arg(
        long,
        global = true,
        env = "QF_STORAGE",
        default_value = "sqlite",
        value_parser = super::backend_parser(),
        help_heading = STORAGE_HEADING
    )]
    pub(crate) storage: StorageBackend,

    /// PostgreSQL URL, postgres://USER:PASSWORD@HOST:PORT/DATABASE, optionally with sslmode
    /// and sslrootcert; prefer QF_POSTGRES_URL: command-line values can land in shell history
    /// and process listings
    #[arg(
        long,
        global = true,
        env = "QF_POSTGRES_URL",
        hide_env_values = true,
        help_heading = STORAGE_HEADING
    )]
    pub(crate) postgres_url: Option<String>,

    /// Most PostgreSQL connections this process opens; omitted, 2, or 8 for serve run.
    #[arg(
        long,
        global = true,
        env = "QF_POSTGRES_POOL_SIZE",
        value_parser = clap::value_parser!(u32).range(1..),
        help_heading = STORAGE_HEADING
    )]
    pub(crate) postgres_pool_size: Option<u32>,

    /// Seconds opening a PostgreSQL connection, TLS included, may take.
    #[arg(
        long,
        global = true,
        env = "QF_POSTGRES_CONNECT_TIMEOUT_SECS",
        default_value_t = DEFAULT_CONNECT_TIMEOUT.as_secs(),
        value_parser = clap::value_parser!(u64).range(1..),
        help_heading = STORAGE_HEADING
    )]
    pub(crate) postgres_connect_timeout_secs: u64,

    /// Set by `parse()`: the flag or variable each setting came from, or
    /// `None` for a default.
    #[arg(skip)]
    sources: Sources,
}

#[derive(Debug, Default)]
struct Sources {
    db: Option<String>,
    storage: Option<String>,
    postgres_url: Option<String>,
    postgres_pool_size: Option<String>,
    postgres_connect_timeout_secs: Option<String>,
}

impl StorageArgs {
    /// Records where each setting came from. Global propagation leaves the
    /// highest-precedence source on the root matches.
    pub(super) fn record_sources(&mut self, matches: &ArgMatches) {
        let label = |id: &str| {
            let (_, flag, env) = SETTINGS.iter().find(|(known, _, _)| *known == id)?;
            match matches.value_source(id)? {
                ValueSource::CommandLine => Some((*flag).to_string()),
                ValueSource::EnvVariable => Some((*env).to_string()),
                _ => None,
            }
        };
        self.sources = Sources {
            db: label("db"),
            storage: label("storage"),
            postgres_url: label("postgres_url"),
            postgres_pool_size: label("postgres_pool_size"),
            postgres_connect_timeout_secs: label("postgres_connect_timeout_secs"),
        };
    }

    /// What the operator supplied; a setting left at its default is not.
    pub(crate) fn request(&self) -> StorageRequest {
        let sources = &self.sources;
        let mut request = StorageRequest::default();
        request.backend = sources
            .storage
            .clone()
            .map(|label| Supplied::new(self.storage, label));
        request.sqlite_path = sources
            .db
            .clone()
            .map(|label| Supplied::new(self.db.clone(), label));
        request.postgres_url = self
            .postgres_url
            .clone()
            .zip(sources.postgres_url.clone())
            .map(|(url, label)| Supplied::new(ConnectionString::new(url), label));
        request.postgres_pool_size = self
            .postgres_pool_size
            .zip(sources.postgres_pool_size.clone())
            .map(|(size, label)| Supplied::new(size, label));
        request.postgres_connect_timeout_secs = sources
            .postgres_connect_timeout_secs
            .clone()
            .map(|label| Supplied::new(self.postgres_connect_timeout_secs, label));
        request
    }

    /// A password-bearing URL arrived on the command line, where shell
    /// history and process listings can keep it.
    pub(crate) fn password_on_command_line(&self) -> bool {
        self.sources.postgres_url.as_deref() == Some("--postgres-url")
            && self
                .postgres_url
                .as_ref()
                .is_some_and(|url| ConnectionString::new(url.clone()).has_password())
    }
}

/// Shows only whether a URL is set, so a `{:?}` of the parsed `Cli` never
/// prints a password.
impl fmt::Debug for StorageArgs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageArgs")
            .field("db", &self.db)
            .field("storage", &self.storage)
            .field(
                "postgres_url",
                &self.postgres_url.as_ref().map(|_| "<redacted>"),
            )
            .field("postgres_pool_size", &self.postgres_pool_size)
            .field(
                "postgres_connect_timeout_secs",
                &self.postgres_connect_timeout_secs,
            )
            .field("sources", &self.sources)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, command};
    use clap::{FromArgMatches, Parser};
    use std::collections::BTreeSet;

    #[test]
    fn storage_args_debug_never_prints_the_postgres_url() {
        let cli = Cli::try_parse_from([
            "quantforge",
            "--postgres-url",
            "postgres://quant:pw-sentinel-417@db.example.com/market",
            "store",
            "status",
        ])
        .expect("cli");
        let debug = format!("{cli:?}");
        assert!(
            debug.contains("<redacted>"),
            "Debug output lacks the redaction marker"
        );
        assert!(
            !debug.contains("pw-sentinel-417"),
            "Debug output contains the password"
        );
        assert!(
            !debug.contains("db.example.com"),
            "Debug output contains the URL"
        );
    }

    /// The labels storage errors use are the flags and variables clap defines,
    /// and every storage flag has one.
    #[test]
    fn every_storage_setting_is_labelled_with_its_flag_and_variable() {
        let command = command();
        let storage_ids: BTreeSet<String> = command
            .get_arguments()
            .filter(|arg| arg.get_help_heading() == Some("Storage"))
            .map(|arg| arg.get_id().to_string())
            .collect();
        let labelled: BTreeSet<String> = SETTINGS
            .iter()
            .map(|(id, _, _)| (*id).to_string())
            .collect();
        assert_eq!(storage_ids, labelled);
        for (id, flag, env) in SETTINGS {
            let arg = command
                .get_arguments()
                .find(|arg| arg.get_id() == id)
                .expect("a storage argument");
            assert_eq!(
                arg.get_long().map(|long| format!("--{long}")).as_deref(),
                Some(flag)
            );
            assert_eq!(arg.get_env().and_then(|env| env.to_str()), Some(env));
        }
    }

    /// A flag is supplied under its own name; a default is not supplied.
    #[test]
    fn the_storage_request_holds_only_what_was_supplied_labelled_by_its_flag() {
        let argv = [
            "quantforge",
            "--storage",
            "postgres",
            "--postgres-pool-size",
            "4",
            "store",
            "status",
        ];
        let matches = command().try_get_matches_from(argv).expect("matches");
        let mut cli = Cli::from_arg_matches(&matches).expect("cli");
        cli.storage.record_sources(&matches);
        let request = cli.storage.request();
        let backend = request.backend.expect("the backend");
        assert_eq!(
            (backend.value, backend.label.as_str()),
            (StorageBackend::Postgres, "--storage")
        );
        let pool = request.postgres_pool_size.expect("the pool size");
        assert_eq!(
            (pool.value, pool.label.as_str()),
            (4, "--postgres-pool-size")
        );
        assert!(request.sqlite_path.is_none());
        assert!(request.postgres_url.is_none());
        assert!(request.postgres_connect_timeout_secs.is_none());
    }
}
