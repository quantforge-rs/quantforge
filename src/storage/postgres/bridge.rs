//! The bridge: a pool of tokio-postgres connections that only the bridge's
//! own runtime ever touches, and the synchronous entry point that hands
//! work to it.
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
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres_rustls::MakeRustlsConnect;

use super::runner::{RunError, Runner};
use super::tls;
use crate::ports::StorageError;
use crate::storage::settings::{PostgresSettings, TlsMode};

/// The bridge's worker threads: they only drive connections' I/O, so two
/// serve every pool size ADR 0001 uses.
const WORKERS: usize = 2;

/// How long a caller keeps waiting past a call's deadline before it
/// decides the bridge itself is wedged.
const WATCHDOG_MARGIN: Duration = Duration::from_secs(5);

/// Whether a call reads or writes, which decides what a lost answer means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Read,
    Write,
}

/// The work of one call: a future over a pooled connection, which it may
/// borrow mutably to open a transaction.
pub(crate) type Work<'c, T> =
    Pin<Box<dyn Future<Output = Result<T, tokio_postgres::Error>> + Send + 'c>>;

/// A pool on a runtime of its own; see the module docs.
pub(crate) struct Bridge {
    // Declared before the runner: the pool closes before the runtime stops.
    pool: Pool,
    tls: MakeRustlsConnect,
    deadline: Duration,
    /// Visible to the tests, which check where work runs.
    pub(super) runner: Runner,
}

impl Bridge {
    /// Builds the pool on a runtime of its own. Nothing connects yet.
    pub(crate) fn new(settings: &PostgresSettings) -> Result<Self, StorageError> {
        let connection = &settings.connection;
        let tls = tls::connector(connection.tls().ca_file.as_deref())?;
        let mut config = tokio_postgres::Config::new();
        config
            .host(connection.host())
            .port(connection.port())
            .user(connection.user())
            .dbname(connection.dbname())
            .application_name(&settings.application_name)
            .connect_timeout(settings.connect_timeout)
            .keepalives(true)
            .keepalives_idle(settings.keepalive_idle)
            .ssl_mode(match connection.tls().mode {
                TlsMode::Disable => SslMode::Disable,
                TlsMode::Prefer => SslMode::Prefer,
                TlsMode::Require => SslMode::Require,
            });
        if let Some(password) = connection.password() {
            config.password(password);
        }

        // Set with SET, not the `options` startup parameter, which PgBouncer
        // refuses.
        let statement_timeout_ms = settings.statement_timeout.as_millis();
        let post_create = Hook::async_fn(move |client: &mut ClientWrapper, _| {
            Box::pin(async move {
                client
                    .batch_execute(&format!("SET statement_timeout = {statement_timeout_ms}"))
                    .await
                    .map_err(HookError::Backend)
            })
        });
        let manager = Manager::from_config(
            config,
            tls.clone(),
            ManagerConfig {
                recycling_method: RecyclingMethod::Verified,
            },
        );
        let pool_size = usize::try_from(settings.pool_size)
            .map_err(|_| StorageError::InvalidArgument("pool size out of range".to_string()))?;
        let pool = Pool::builder(manager)
            .max_size(pool_size)
            .wait_timeout(Some(settings.wait_timeout))
            .create_timeout(Some(settings.connect_timeout))
            .recycle_timeout(Some(settings.connect_timeout))
            .runtime(Runtime::Tokio1)
            .post_create(post_create)
            .build()
            .map_err(|error| {
                StorageError::Unavailable(format!("cannot build the pool: {error}"))
            })?;
        let runner = Runner::new("qf-postgres", WORKERS).map_err(|error| {
            StorageError::Unavailable(format!("cannot start the storage bridge: {error}"))
        })?;
        Ok(Self {
            pool,
            tls,
            deadline: settings.call_deadline,
            runner,
        })
    }

    /// Runs `work` on a pooled connection, on the bridge, within the
    /// deadline, and blocks the caller until it ends. Getting the connection
    /// spends the same deadline, and running out of it there is unavailable:
    /// nothing was sent. Past the deadline the query is cancelled on the
    /// server and the connection discarded, so no later call queues behind
    /// it. A transaction the work leaves open is rolled back.
    pub(crate) fn execute<T, F>(&self, effect: Effect, work: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut ClientWrapper) -> Work<'c, T> + Send + 'static,
    {
        let pool = self.pool.clone();
        let tls = self.tls.clone();
        let deadline = self.deadline;
        let job = async move {
            let until = tokio::time::Instant::now() + deadline;
            let mut client = match tokio::time::timeout_at(until, pool.get()).await {
                Ok(client) => client.map_err(|error| match error {
                    PoolError::Timeout(TimeoutType::Wait) => {
                        StorageError::Unavailable("no free connection in time".to_string())
                    }
                    PoolError::Backend(error) => {
                        StorageError::Unavailable(format!("cannot get a connection: {error}"))
                    }
                    other => StorageError::Unavailable(other.to_string()),
                })?,
                Err(_) => {
                    return Err(StorageError::Unavailable(format!(
                        "no connection within {} ms",
                        deadline.as_millis()
                    )));
                }
            };
            let token = client.cancel_token();
            match tokio::time::timeout_at(until, work(&mut client)).await {
                Ok(result) => result.map_err(|error| from_driver(effect, error)),
                Err(_) => {
                    // Best effort, and bounded by the connect timeout.
                    let _ = token.cancel_query(tls).await;
                    drop(Object::take(client));
                    Err(lost(
                        effect,
                        format!("no answer within {} ms", deadline.as_millis()),
                    ))
                }
            }
        };
        match self.runner.run(job, deadline + WATCHDOG_MARGIN) {
            Ok(result) => result,
            Err(error @ (RunError::Watchdog(_) | RunError::ShutDown)) => {
                Err(lost(effect, error.to_string()))
            }
            Err(error) => Err(StorageError::other(error)),
        }
    }
}

/// A call that lost its answer: nothing changed for a read, anything may
/// have for a write.
fn lost(effect: Effect, what: String) -> StorageError {
    match effect {
        Effect::Read => StorageError::Unavailable(what),
        Effect::Write => StorageError::OutcomeUnknown(what),
    }
}

/// Classifies a driver error. One the server reported, with its SQLSTATE,
/// is the statement's fault, except a statement timeout; any other means
/// the connection failed.
fn from_driver(effect: Effect, error: tokio_postgres::Error) -> StorageError {
    match error.code() {
        Some(code) if *code == SqlState::QUERY_CANCELED => {
            lost(effect, format!("the statement timed out: {error}"))
        }
        Some(code)
            if code.code().starts_with("08")
                || *code == SqlState::ADMIN_SHUTDOWN
                || *code == SqlState::CRASH_SHUTDOWN =>
        {
            lost(effect, format!("the server ended the call: {error}"))
        }
        Some(_) => StorageError::other(error),
        None => lost(effect, format!("the connection failed: {error}")),
    }
}

impl fmt::Debug for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bridge")
            .field("pool", &self.pool.status())
            .field("deadline", &self.deadline)
            .finish_non_exhaustive()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Closing wakes every waiter with `Closed` and drops idle
        // connections; the runner then stops in the background.
        self.pool.close();
    }
}
