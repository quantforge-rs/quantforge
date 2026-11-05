//! Shared per-invocation construction: tracing, the store the storage
//! settings select, opened through the library's store factory, and the
//! selected exchange's connectors, built through its registry — once, from
//! the parsed [`Cli`], and handed to the handlers — plus the
//! interactive-session and confirmation helpers.

use super::{Cli, Command};
use anyhow::{Context, Result, anyhow};
use quantforge::exchange;
use quantforge::{
    AnyStore, ApiCredentials, ConnectorConfig, Durability, ExchangeConnectors, ExchangeId,
    ExecutionMode, RunIdentity, SignedConnectors, StorageConfigError, StorageSettings,
    StoreControl, StoreFactory, StoreIdentity, StoreOpenError, StoreOwner, VenueAccountId,
    describe_environment,
};
use std::io::{self, IsTerminal, Write};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

/// Appended to every missing-credentials error so it names both ways to
/// supply them.
const CREDENTIALS_REMEDY: &str =
    "pass --api-key and --api-secret, or set QF_API_KEY and QF_API_SECRET";

/// Logged when a credential came from the command line of an interactive
/// session. It never includes the value itself.
const SHELL_HISTORY_WARNING: &str = "an API credential was passed on the command line; a \
     literally typed value may be saved in shell history, and command-line values are visible \
     in process listings while quantforge runs; prefer QF_API_KEY/QF_API_SECRET";

/// The same warning for a PostgreSQL URL that carries a password.
const URL_SHELL_HISTORY_WARNING: &str = "a PostgreSQL URL with a password was passed on the \
     command line; a literally typed value may be saved in shell history, and command-line \
     values are visible in process listings while quantforge runs; prefer QF_POSTGRES_URL";

/// Everything a command handler needs, constructed once per invocation.
#[derive(Debug)]
pub(crate) struct AppContext {
    pub(crate) store: AnyStore,
    /// The selection `store` was opened from: which backend, and where.
    pub(crate) storage: StorageSettings,
    /// What the store knows about itself after `init` brought it to the
    /// current schema: its ID, environment and activation.
    pub(crate) identity: StoreIdentity,
    pub(crate) base_url: Url,
    pub(crate) exchange: ExchangeConnectors,
    credentials: Option<ApiCredentials>,
}

impl AppContext {
    /// Build the context in startup order: tracing first (so the credential
    /// and store warnings can be emitted), then the storage settings and the
    /// base URL, which are validated before anything is created, then the
    /// store, then the connectors.
    pub(crate) fn init(cli: &Cli) -> Result<Self> {
        init_tracing(&cli.log_level)?;
        if cli.credentials.on_command_line && is_interactive() {
            warn!("{SHELL_HISTORY_WARNING}");
        }
        if cli.storage.password_on_command_line() && is_interactive() {
            warn!("{URL_SHELL_HISTORY_WARNING}");
        }

        let owner = match cli.command {
            Command::Serve { .. } => StoreOwner::Server,
            _ => StoreOwner::Command,
        };
        // A command that sends orders opens a store whose every connection
        // is checked; any other can run on a weaker one, and warns below.
        let mut request = cli.storage.request();
        request.durability = Some(match &cli.command {
            Command::Trade { command } if command.is_live() => Durability::Enforced,
            _ => Durability::Reported,
        });
        let storage = StorageSettings::resolve(request, owner).map_err(storage_refusal)?;

        // Without a flag or variable, the exchange's own production
        // endpoint, as its registry entry names it.
        let base_url = match &cli.base_url {
            Some(raw) => Url::parse(raw).context("failed to parse --base-url")?,
            None => {
                let default = exchange::default_base_url(cli.exchange)
                    .context("the selected exchange has no adapter")?;
                Url::parse(default).context("failed to parse the exchange's default base URL")?
            }
        };

        info!(%storage, "using storage");
        // Only a SQLite file can be created by opening it; PostgreSQL warns
        // for itself when it finds an empty database.
        let created = match &storage {
            StorageSettings::Sqlite(sqlite) => (!sqlite.path.exists()).then_some(&sqlite.path),
            _ => None,
        };
        let store = StoreFactory::builtin()
            .open(&storage)
            .map_err(|error| match error {
                StoreOpenError::Storage(error) => anyhow!(error).context(format!(
                    "failed to initialize {} store at {}",
                    storage.backend(),
                    storage.location()
                )),
                StoreOpenError::Config(error) => storage_refusal(error),
                other => anyhow!(other),
            })?;
        let identity = store
            .store_identity()
            .context("failed to read the store's identity")?;
        let durability = store
            .durability()
            .context("failed to read the store's commit settings")?;
        if let Some(problem) = durability.problem() {
            warn!(
                %problem,
                "this store's commits may not survive a crash, and live trading refuses it"
            );
        }
        if let Some(path) = created {
            warn!(
                db_path = %path.display(),
                "database did not exist and was created empty; if you expected existing data \
                 (run state, candles), check the --db path or QF_DB"
            );
        }

        info!(
            exchange = exchange::name(cli.exchange),
            base_url = %display_url(&base_url),
            "using exchange base URL"
        );
        let credentials = ApiCredentials::resolve(
            cli.credentials.api_key.clone(),
            cli.credentials.api_secret.clone(),
        );
        let exchange = exchange::connect(
            cli.exchange,
            ConnectorConfig {
                base_url: base_url.clone(),
                credentials: credentials.clone(),
            },
        )
        .context("failed to build the exchange connectors")?;

        Ok(Self {
            store,
            storage,
            identity,
            base_url,
            exchange,
            credentials,
        })
    }

    /// A store bound to an environment is used only with connectors that
    /// reach it: mainnet means a production endpoint and nothing else does.
    /// Called before any command that talks to the exchange, so a testnet
    /// store never records production orders and the other way round.
    pub(crate) fn verify_environment(&self) -> Result<()> {
        let Some(environment) = self.identity.environment else {
            return Ok(());
        };
        let production = self.exchange.market_data.is_production();
        if production != environment.is_production() {
            let endpoint = if production { "production" } else { "test" };
            return Err(anyhow!(
                "store {} is bound to {environment}, but {} is a {endpoint} endpoint; refusing \
                 before any request (pass the {environment} base URL, or use {})",
                self.identity.store_id,
                display_url(&self.base_url),
                self.another_store()
            ));
        }
        Ok(())
    }

    /// What an operator selects instead of this store, for a remedy.
    pub(crate) fn another_store(&self) -> &'static str {
        match self.storage {
            StorageSettings::Sqlite(_) => "another --db",
            _ => "another PostgreSQL database",
        }
    }

    /// The identity a run started by this invocation records: the store's
    /// environment and, for a live run, the fingerprint of the API key in
    /// use. A dry run has no account.
    pub(crate) fn run_identity(&self, mode: ExecutionMode) -> RunIdentity {
        let account = match mode {
            ExecutionMode::Live => self
                .credentials
                .as_ref()
                .map(|credentials| VenueAccountId::KeyFingerprint(credentials.key_fingerprint())),
            ExecutionMode::DryRun => None,
        };
        RunIdentity::recorded(self.identity.environment, account)
    }

    /// `mainnet`, `testnet`, `demo`, or `undeclared`.
    pub(crate) fn environment_text(&self) -> String {
        describe_environment(self.identity.environment)
    }

    /// The exchange the connectors talk to: the exchange of every market a
    /// command keys candles and runs by.
    pub(crate) fn exchange_id(&self) -> ExchangeId {
        self.exchange.market_data.exchange_id()
    }

    /// The signed connectors, or an error that opens with `subject` (the
    /// command's tested wording) and names both ways to supply credentials.
    pub(crate) fn require_signed(&self, subject: &'static str) -> Result<SignedConnectors<'_>> {
        self.exchange
            .signed()
            .ok_or_else(|| anyhow!("{subject}: {CREDENTIALS_REMEDY}"))
    }
}

/// A refused storage setting, with the flags and variables that fix it
/// where the library's message cannot name them.
fn storage_refusal(error: StorageConfigError) -> anyhow::Error {
    match error {
        StorageConfigError::MissingPostgresUrl => anyhow!(
            "{error}; set QF_POSTGRES_URL, or pass --postgres-url (the variable keeps a password \
             out of shell history)"
        ),
        StorageConfigError::PostgresSettingWithoutBackend { .. } => {
            anyhow!("{error}: pass --storage postgres or --storage sqlite, or set QF_STORAGE")
        }
        other => anyhow!(other),
    }
}

fn init_tracing(level: &str) -> Result<()> {
    // The filter comes only from --log-level/QF_LOG: `try_new` never reads
    // RUST_LOG, unlike `EnvFilter::from_default_env`. The fmt layer still
    // honors NO_COLOR, an output convention rather than a setting.
    let env_filter = EnvFilter::try_new(level)
        .with_context(|| format!("invalid log level/filter expression: {level}"))?;

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
    Ok(())
}

/// Both stdin and stdout are terminals: an operator at a shell, as opposed
/// to a script, supervisor, or test harness driving piped stdio.
pub(crate) fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Prints `prompt`, reads one stdin line, and applies the strict `yes`
/// check. The caller decides whether the session is interactive and what
/// preview text precedes the prompt.
pub(crate) fn prompt_confirmation(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .context("failed to flush confirmation prompt")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read live-trading confirmation")?;
    Ok(confirmation_is_yes(&input))
}

pub(crate) fn display_url(url: &Url) -> Url {
    let mut redacted = url.clone();
    let _ = redacted.set_username("");
    let _ = redacted.set_password(None);
    redacted
}

fn confirmation_is_yes(input: &str) -> bool {
    input.trim().eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_url_strips_userinfo() {
        let url = Url::parse("https://user:secret@api.binance.com/").expect("url");
        assert_eq!(display_url(&url).as_str(), "https://api.binance.com/");
    }

    #[test]
    fn live_confirmation_accepts_only_yes() {
        for input in ["yes", "YES", " yes \n", "Yes"] {
            assert!(confirmation_is_yes(input), "for input {input:?}");
        }
        for input in ["", "y", "no", "live", "yes please"] {
            assert!(!confirmation_is_yes(input), "for input {input:?}");
        }
    }
}
