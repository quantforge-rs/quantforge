//! The bridge: a pool of tokio-postgres connections that only the bridge's
//! own runtime ever touches, and the synchronous entry point that hands
//! work to it.
//!
//! deadpool-postgres starts each connection's driver with `tokio::spawn` on
//! whichever runtime asks the pool for it, so the pool is only ever used
//! inside bridge tasks: every driver then lives on the bridge, never on a
//! caller's runtime that may sit blocked waiting for an answer.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use deadpool_postgres::{
    ClientWrapper, Hook, HookError, Manager, ManagerConfig, Object, Pool, PoolError,
    RecyclingMethod, Runtime, TimeoutType,
};
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres_rustls::MakeRustlsConnect;

use super::durability::{self, FLUSHED_COMMITS, STILL_DURABLE, limits_within, session_limits};
use super::runner::{RunError, Runner};
use super::tls;
use crate::ports::StorageError;
use crate::storage::settings::{Durability, PostgresSettings, TlsMode};

/// The bridge's worker threads: they only drive connections' I/O, so two
/// serve every pool size ADR 0001 uses.
const WORKERS: usize = 2;

/// The longest a caller keeps waiting past a call's deadline before it
/// decides the bridge itself is wedged; a quarter of a shorter deadline.
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

/// A handle on a pool on a runtime of its own; see the module docs. Every
/// handle shares them, and may bound its own calls more tightly.
#[derive(Clone)]
pub(crate) struct Bridge {
    /// Visible to the tests, which check where work runs.
    pub(super) shared: Arc<Shared>,
    /// All that a call through this handle may take, when it is bounded.
    budget: Option<Duration>,
}

/// What every handle shares.
pub(super) struct Shared {
    // Declared before the runner: the pool closes before the runtime stops.
    pool: Pool,
    tls: MakeRustlsConnect,
    deadline: Duration,
    lock_wait: Duration,
    statement_timeout: Duration,
    /// The session's limits, as a bounded call leaves them.
    limits: String,
    pub(super) runner: Runner,
}

impl Bridge {
    /// Builds the pool on a runtime of its own. Nothing connects yet.
    pub(crate) fn new(settings: &PostgresSettings) -> Result<Self, StorageError> {
        let connection = &settings.connection;
        let tls = tls::connector(connection.tls())?;
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

        let limits = session_limits(settings.limits.wait_budget, settings.statement_timeout);
        let setup = format!("{limits} {FLUSHED_COMMITS}");
        let enforced = settings.limits.durability == Durability::Enforced;
        let post_create = Hook::async_fn(move |client: &mut ClientWrapper, _| {
            let setup = setup.clone();
            Box::pin(async move {
                client
                    .batch_execute(&setup)
                    .await
                    .map_err(HookError::Backend)?;
                if enforced {
                    let report = durability::effective(client)
                        .await
                        .map_err(HookError::Backend)?;
                    if let Some(problem) = report.problem() {
                        return Err(HookError::message(problem));
                    }
                }
                Ok(())
            })
        });
        let manager = Manager::from_config(
            config,
            tls.clone(),
            ManagerConfig {
                recycling_method: if enforced {
                    RecyclingMethod::Custom(STILL_DURABLE.to_string())
                } else {
                    RecyclingMethod::Verified
                },
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
            shared: Arc::new(Shared {
                pool,
                tls,
                deadline: settings.call_deadline,
                lock_wait: settings.limits.wait_budget,
                statement_timeout: settings.statement_timeout,
                limits,
                runner,
            }),
            budget: None,
        })
    }

    /// The same pool, each call through the handle bounded by `budget`.
    pub(crate) fn within(&self, budget: Duration) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            budget: Some(budget),
        }
    }

    /// Runs `work` on a pooled connection, on the bridge, within the
    /// deadline, and blocks the caller until it ends. Getting the connection
    /// spends the same deadline, and running out of it there is unavailable:
    /// nothing was sent. Past the deadline the query is cancelled on the
    /// server and the connection discarded, so no later call queues behind
    /// it. A transaction the work leaves open is rolled back.
    ///
    /// A bounded call first gives the session what remains of its budget, a
    /// lock wait ending before a statement and a statement before the call,
    /// and puts the session's own limits back after.
    pub(crate) fn execute<T, F>(&self, effect: Effect, work: F) -> Result<T, StorageError>
    where
        T: Send + 'static,
        F: for<'c> FnOnce(&'c mut ClientWrapper) -> Work<'c, T> + Send + 'static,
    {
        let shared = Arc::clone(&self.shared);
        let pool = shared.pool.clone();
        let tls = shared.tls.clone();
        let deadline = self
            .budget
            .map_or(shared.deadline, |budget| budget.min(shared.deadline));
        let bounded = self.budget.is_some();
        let (lock_wait, statement_timeout) = (shared.lock_wait, shared.statement_timeout);
        let limits = shared.limits.clone();
        let job = async move {
            let until = tokio::time::Instant::now() + deadline;
            let mut client = match tokio::time::timeout_at(until, pool.get()).await {
                Ok(client) => client.map_err(|error| match error {
                    PoolError::Timeout(TimeoutType::Wait) => {
                        StorageError::Unavailable("no free connection in time".to_string())
                    }
                    PoolError::Backend(error) => not_connected(&error),
                    PoolError::Timeout(_) => StorageError::Unavailable(
                        "cannot get a connection: the server did not answer in time".to_string(),
                    ),
                    PoolError::PostCreateHook(HookError::Message(problem)) => {
                        StorageError::DurabilityUnmet(problem.into_owned())
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
            if bounded {
                let left = until.saturating_duration_since(tokio::time::Instant::now());
                let tightened = limits_within(left, lock_wait, statement_timeout);
                let set = tokio::time::timeout_at(until, client.batch_execute(&tightened)).await;
                if !matches!(set, Ok(Ok(()))) {
                    drop(Object::take(client));
                    return Err(StorageError::Unavailable(
                        "cannot limit the session in time".to_string(),
                    ));
                }
            }
            match tokio::time::timeout_at(until, work(&mut client)).await {
                Ok(result) => {
                    if bounded {
                        let reset = client.batch_execute(&limits);
                        if !matches!(tokio::time::timeout_at(until, reset).await, Ok(Ok(()))) {
                            drop(Object::take(client));
                        }
                    }
                    result.map_err(|error| from_driver(effect, error))
                }
                Err(_) => {
                    // Best effort, and never on the caller's time: a server
                    // that stopped answering takes the connect timeout here.
                    tokio::spawn(async move {
                        let _ = token.cancel_query(tls).await;
                    });
                    drop(Object::take(client));
                    Err(lost(
                        effect,
                        format!("no answer within {} ms", deadline.as_millis()),
                    ))
                }
            }
        };
        let margin = WATCHDOG_MARGIN.min(deadline / 4);
        match self.shared.runner.run(job, deadline + margin) {
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

/// Classifies a driver error (`docs/storage-durability.md`). One the server
/// reported, with its SQLSTATE, is the statement's fault, unless the
/// SQLSTATE says it lost to another writer or ran out of time or room; any
/// other means the connection failed. The categories that name no server
/// text keep what it said out of the caller's message.
pub(super) fn from_driver(effect: Effect, error: tokio_postgres::Error) -> StorageError {
    match error.code() {
        Some(code)
            if *code == SqlState::T_R_SERIALIZATION_FAILURE
                || *code == SqlState::T_R_DEADLOCK_DETECTED =>
        {
            StorageError::Conflict(format!(
                "the server rolled the transaction back (SQLSTATE {})",
                code.code()
            ))
        }
        Some(code) if *code == SqlState::LOCK_NOT_AVAILABLE => {
            StorageError::Contention("another writer held a lock past the wait".to_string())
        }
        Some(code) if code.code().starts_with("53") => StorageError::Unavailable(format!(
            "the server is out of resources (SQLSTATE {})",
            code.code()
        )),
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
        None => lost(
            effect,
            format!("the connection failed: {}", with_causes(&error)),
        ),
    }
}

/// Why a new connection failed. What the server said can name the user, so
/// a refusal is told in words of ours, by its SQLSTATE.
fn not_connected(error: &tokio_postgres::Error) -> StorageError {
    let why = match error.code() {
        Some(code) if code.code().starts_with("28") => format!(
            "the server refused the URL's user or password (SQLSTATE {})",
            code.code()
        ),
        Some(code) if *code == SqlState::INVALID_CATALOG_NAME => format!(
            "the database the URL names does not exist (SQLSTATE {})",
            code.code()
        ),
        Some(code) => format!("the server refused it (SQLSTATE {})", code.code()),
        None => with_causes(error),
    };
    StorageError::Unavailable(format!("cannot get a connection: {why}"))
}

/// A driver error and what caused it, on one line: its own text stops at
/// "error connecting to server".
fn with_causes(error: &tokio_postgres::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(found) = cause {
        text.push_str(&format!(": {found}"));
        cause = found.source();
    }
    text
}

impl fmt::Debug for Bridge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bridge")
            .field("pool", &self.shared.pool.status())
            .field("deadline", &self.shared.deadline)
            .field("budget", &self.budget)
            .finish_non_exhaustive()
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        // Closing wakes every waiter with `Closed` and drops idle
        // connections; the runner then stops in the background.
        self.pool.close();
    }
}
