//! The bridge: a pool of tokio-postgres connections that only the bridge's
//! own runtime ever touches, and the synchronous entry point that hands work
//! to it.
//!
//! deadpool-postgres starts each connection's driver with `tokio::spawn` on
//! whichever runtime asks the pool for it, so the pool is only ever used
//! inside bridge tasks: every driver then lives on the bridge, never on a
//! caller's runtime that may sit blocked waiting for an answer.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use deadpool_postgres::{
    ClientWrapper, Hook, HookError, Manager, ManagerConfig, Object, Pool, PoolError,
    RecyclingMethod, Runtime, TimeoutType,
};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::runner::{RunError, Runner};
use crate::tls::{self, TlsError};

/// What a bridge connects to and how far it may go.
#[derive(Clone, Debug)]
pub struct BridgeConfig {
    /// A libpq-style `postgres://` URL; see [`tls::prepare`].
    pub url: String,
    /// Shown in `pg_stat_activity`, so that operators can tell these
    /// connections apart.
    pub application_name: String,
    /// The most connections the pool opens.
    pub max_size: usize,
    /// How long a call waits for a free connection before it is refused.
    pub wait_timeout: Duration,
    /// How long opening a connection may take, TLS included.
    pub connect_timeout: Duration,
    /// The server-side limit on every statement, set on each new connection.
    pub statement_timeout: Duration,
    /// How long one call may take, end to end; past it the query is
    /// cancelled on the server and its connection discarded.
    pub deadline: Duration,
}

impl BridgeConfig {
    /// The CLI's defaults: two connections, a few seconds' patience.
    pub fn new(url: impl Into<String>, application_name: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            application_name: application_name.into(),
            max_size: 2,
            wait_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            statement_timeout: Duration::from_secs(30),
            deadline: Duration::from_secs(35),
        }
    }
}

/// Whether a call reads or writes, which decides what a lost answer means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Read,
    Write,
}

/// How a call failed, in the terms a caller acts on.
#[derive(Debug)]
pub enum PgError {
    /// Nothing was done: no connection, no TLS, no free connection in time,
    /// or a read that ran out of time. Safe to retry; a server answers 503.
    Unavailable(String),
    /// A write lost its answer: it may or may not have committed. Reconcile
    /// before retrying.
    OutcomeUnknown(String),
    /// The database refused the statement; the SQLSTATE is in the source.
    Database(tokio_postgres::Error),
    /// The bridge itself could not serve the call.
    Bridge(RunError),
    /// The connection string or its TLS settings were refused.
    Tls(TlsError),
}

impl PgError {
    /// The kind's name, as an error report would print it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "unavailable",
            Self::OutcomeUnknown(_) => "outcome_unknown",
            Self::Database(_) => "database",
            Self::Bridge(_) => "bridge",
            Self::Tls(_) => "invalid_config",
        }
    }

    fn lost(effect: Effect, what: impl Into<String>) -> Self {
        match effect {
            Effect::Read => Self::Unavailable(what.into()),
            Effect::Write => Self::OutcomeUnknown(what.into()),
        }
    }

    /// Classifies a driver error: one the server reported, with its
    /// SQLSTATE, is the statement's fault, except a statement timeout; any
    /// other means the connection failed.
    fn from_driver(effect: Effect, error: tokio_postgres::Error) -> Self {
        match error.code() {
            Some(code) if *code == tokio_postgres::error::SqlState::QUERY_CANCELED => {
                Self::lost(effect, format!("the statement timed out: {error}"))
            }
            Some(_) => Self::Database(error),
            None => Self::lost(effect, format!("the connection failed: {error}")),
        }
    }
}

impl fmt::Display for PgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(what) => write!(f, "storage unavailable: {what}"),
            Self::OutcomeUnknown(what) => write!(f, "storage write outcome unknown: {what}"),
            Self::Database(error) => write!(f, "storage refused the statement: {error}"),
            Self::Bridge(error) => write!(f, "storage bridge: {error}"),
            Self::Tls(error) => write!(f, "storage configuration: {error}"),
        }
    }
}

impl std::error::Error for PgError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::Bridge(error) => Some(error),
            Self::Tls(error) => Some(error),
            Self::Unavailable(_) | Self::OutcomeUnknown(_) => None,
        }
    }
}

/// The work of one call: a future over a pooled connection.
pub type Work<'c, T> = Pin<Box<dyn Future<Output = Result<T, tokio_postgres::Error>> + Send + 'c>>;

/// A pool on a runtime of its own; see the module docs.
pub struct PgBridge {
    // Declared before the runner: the pool closes before the runtime stops.
    pool: Pool,
    tls: MakeRustlsConnect,
    deadline: Duration,
    runner: Runner,
}

impl PgBridge {
    /// Builds the pool on a 2-worker runtime of its own and checks, from the
    /// bridge, that the database answers, so that a wrong URL fails now.
    pub fn connect(config: &BridgeConfig) -> Result<Self, PgError> {
        let prepared = tls::prepare(&config.url).map_err(PgError::Tls)?;
        let tls = tls::connector(prepared.ca_file.as_ref()).map_err(PgError::Tls)?;
        let mut pg = prepared.config;
        pg.application_name(&config.application_name)
            .connect_timeout(config.connect_timeout)
            .keepalives(true)
            .keepalives_idle(Duration::from_secs(30));

        // Set with SET, not the `options` startup parameter, which PgBouncer
        // refuses.
        let statement_timeout_ms = config.statement_timeout.as_millis();
        let post_create = Hook::async_fn(move |client: &mut ClientWrapper, _| {
            Box::pin(async move {
                client
                    .batch_execute(&format!("SET statement_timeout = {statement_timeout_ms}"))
                    .await
                    .map_err(HookError::Backend)
            })
        });
        let manager = Manager::from_config(
            pg,
            tls.clone(),
            ManagerConfig {
                recycling_method: RecyclingMethod::Verified,
            },
        );
        let pool = Pool::builder(manager)
            .max_size(config.max_size)
            .wait_timeout(Some(config.wait_timeout))
            .create_timeout(Some(config.connect_timeout))
            .recycle_timeout(Some(config.connect_timeout))
            .runtime(Runtime::Tokio1)
            .post_create(post_create)
            .build()
            .map_err(|error| PgError::Unavailable(format!("cannot build the pool: {error}")))?;
        let runner = Runner::new("qf-postgres", 2)
            .map_err(|error| PgError::Unavailable(format!("cannot start the bridge: {error}")))?;

        let bridge = Self {
            pool,
            tls,
            deadline: config.deadline,
            runner,
        };
        bridge.execute(Effect::Read, |client| {
            Box::pin(async move { client.batch_execute("SELECT 1").await })
        })?;
        Ok(bridge)
    }

    /// Runs `work` on a pooled connection, on the bridge, within the
    /// deadline, and blocks the caller until it ends. Past the deadline the
    /// query is cancelled on the server and the connection discarded, so no
    /// later call queues behind it.
    pub fn execute<T, F>(&self, effect: Effect, work: F) -> Result<T, PgError>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c ClientWrapper) -> Work<'c, T> + Send + 'static,
    {
        let pool = self.pool.clone();
        let tls = self.tls.clone();
        let deadline = self.deadline;
        let job = async move {
            let client = pool.get().await.map_err(|error| match error {
                PoolError::Timeout(TimeoutType::Wait) => {
                    PgError::Unavailable("no free connection in time".to_string())
                }
                PoolError::Backend(error) => {
                    PgError::Unavailable(format!("cannot get a connection: {error}"))
                }
                other => PgError::Unavailable(other.to_string()),
            })?;
            let token = client.cancel_token();
            match tokio::time::timeout(deadline, work(&client)).await {
                Ok(result) => result.map_err(|error| PgError::from_driver(effect, error)),
                Err(_) => {
                    // Best effort, and bounded by the connect timeout.
                    let _ = token.cancel_query(tls).await;
                    drop(Object::take(client));
                    Err(PgError::lost(
                        effect,
                        format!("no answer within {} ms", deadline.as_millis()),
                    ))
                }
            }
        };
        // The watchdog only bounds the wait if the bridge itself wedges.
        let watchdog = deadline + Duration::from_secs(5);
        self.runner.run(job, watchdog).map_err(PgError::Bridge)?
    }

    /// The bridge's runtime, for tests that check where work runs.
    pub fn runner(&self) -> &Runner {
        &self.runner
    }
}

impl fmt::Debug for PgBridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgBridge")
            .field("pool", &self.pool.status())
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl Drop for PgBridge {
    fn drop(&mut self) {
        // Closing wakes every waiter with `Closed` and drops idle
        // connections; the runner then stops in the background.
        self.pool.close();
    }
}
