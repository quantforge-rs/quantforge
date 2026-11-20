//! The candle reader: [`CandleReader`], the candles a store holds, read by
//! market and time range into a `CandleBatch`.
//!
//! It is the engine's own reader (`quantforge::CandleReader`), opened by
//! the engine's store factory from the settings the command line resolves
//! (`settings.rs`), so Python and the command line agree on which store is
//! selected. It opens the store read-only: it creates no database and no
//! schema, migrates nothing and writes nothing, on either backend. Beside a
//! SQLite database in WAL mode it may leave the `-wal` and `-shm` files any
//! reader of one leaves.
//!
//! - **The Python object owns its store** (ADR 0001): its file handles or
//!   its pool of connections, with their credentials. `close`, the end of a
//!   `with` block, or the object's own end closes it.
//! - **A call waits without the interpreter.** Opening and loading release
//!   it while the store is asked, so other Python threads run meanwhile,
//!   and no thread of the store's ever touches Python.
//! - **A reader belongs to the process that opened it.** A forked child has
//!   none of a pool's threads, so it opens a reader of its own.
//! - **A SQLite store is read through the engine's copy of SQLite,** which
//!   the extension carries. Python's `sqlite3` links another, and two copies
//!   in one process drop each other's locks on a file they both hold
//!   (`docs/python-reader.md`, "One SQLite in a process"), which no code
//!   here can prevent: the page says not to.

mod range;
mod settings;

use std::error::Error;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use pyo3::exceptions::{PyConnectionError, PyFileNotFoundError, PyRuntimeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use quantforge::{
    StorageBackend, StorageError, StorageErrorKind, StorageSettings, StoreFactory, StoreOpenError,
};

use crate::candles::CandleBatch;
use crate::market::MarketId;
use range::{Edge, bound};
use settings::Given;

/// The logger a reader says on why a range held no candle.
const LOGGER: &str = "quantforge_py";

/// The candles a store holds, read by market and time range. A reader
/// opens the store `quantforge data sync` wrote, and never writes to it.
///
/// `CandleReader(*, storage=None, db=None, postgres_url=None,
/// postgres_pool_size=None, postgres_connect_timeout_secs=None)` opens the
/// store the settings select, as the `quantforge` command line selects one:
/// SQLite at `data/market.sqlite` unless told otherwise. Each argument is
/// the command line's flag of the same name, and one left out is read from
/// that flag's variable: `QF_STORAGE`, `QF_DB`, `QF_POSTGRES_URL`,
/// `QF_POSTGRES_POOL_SIZE` and `QF_POSTGRES_CONNECT_TIMEOUT_SECS`. An
/// argument wins over its variable, and a variable over the default.
/// `storage` is `"sqlite"` or `"postgres"`; `db` is the SQLite file, a
/// `str` or an `os.PathLike`; `postgres_url` is
/// `postgres://USER:PASSWORD@HOST:PORT/DATABASE`.
///
/// `load(market, *, start=None, end=None)` gives the candles of a
/// `MarketId` that opened from `start` to `end`, both included, oldest
/// first, as a `CandleBatch` that names the market. A market the store
/// holds nothing of, and a range that holds no candle, give an empty batch
/// and no exception: a warning on the `quantforge_py` logger says which of
/// the two it was.
///
/// The store is opened read-only. The reader creates no database and no
/// schema, migrates nothing and writes nothing: a SQLite file that does not
/// exist is a `FileNotFoundError`, and a store at another schema than this
/// build reads is a `ValueError`, which names the command that brings an
/// older one forward, and says that a newer one takes a newer release.
/// Beside a SQLite database in WAL mode the reader may leave the `-wal` and
/// `-shm` files any reader of one leaves. Settings that are refused are a
/// `ValueError` or a `TypeError`
/// before anything is opened, and a store that cannot be reached is a
/// `ConnectionError`. No message holds a password.
///
/// `backend` and `location` say which store it is, the second without a
/// user or a password. `close()` closes it, as leaving a `with` block does,
/// and `closed` says whether it is. A reader is used by any number of
/// threads at once, in the process that opened it.
///
/// The package carries its own copy of SQLite. In a process that reads a
/// SQLite store with a reader, do not open the same file with Python's
/// `sqlite3` module as well: two copies of SQLite in one process drop each
/// other's locks on a file, and while another process writes it either can
/// read candles that are not in it.
#[pyclass(frozen, module = "quantforge_py")]
pub(crate) struct CandleReader {
    /// The engine's reader, until this one is closed.
    store: Mutex<Option<Arc<quantforge::CandleReader>>>,
    backend: &'static str,
    location: String,
    /// The process that opened it.
    opened_by: u32,
}

#[pymethods]
impl CandleReader {
    #[new]
    #[pyo3(signature = (*, storage=None, db=None, postgres_url=None, postgres_pool_size=None, postgres_connect_timeout_secs=None))]
    fn new(
        py: Python<'_>,
        storage: Option<&Bound<'_, PyAny>>,
        db: Option<&Bound<'_, PyAny>>,
        postgres_url: Option<&Bound<'_, PyAny>>,
        postgres_pool_size: Option<&Bound<'_, PyAny>>,
        postgres_connect_timeout_secs: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let settings = settings::resolved(
            py,
            Given {
                storage,
                db,
                postgres_url,
                postgres_pool_size,
                postgres_connect_timeout_secs,
            },
        )?;
        // Connecting can take as long as its timeout: other threads run
        // meanwhile.
        let opened = py.detach(|| StoreFactory::builtin().open_reader(&settings));
        let store = opened.map_err(|error| not_opened(error, &settings))?;
        Ok(Self {
            backend: store.backend().as_str(),
            location: store.location().to_string(),
            store: Mutex::new(Some(Arc::new(store))),
            opened_by: std::process::id(),
        })
    }

    /// The candles of `market`, a `MarketId`, that opened from `start` to
    /// `end`, both included, oldest first: a `CandleBatch` that names the
    /// market, every decimal with the digits the store holds.
    ///
    /// A bound is an `int` of milliseconds since the Unix epoch, a
    /// `datetime.datetime` with a time zone, or an RFC 3339 `str` such as
    /// `2026-01-01T00:00:00Z`; left out or `None`, it bounds nothing, and
    /// with neither every candle of the market is read.
    ///
    /// A market the store holds nothing of, and a range that holds no
    /// candle, give an empty batch, and a warning on the `quantforge_py`
    /// logger that says what the store holds instead. A store that cannot
    /// be reached is a `ConnectionError`.
    #[pyo3(signature = (market, *, start=None, end=None))]
    fn load(
        &self,
        py: Python<'_>,
        market: &Bound<'_, PyAny>,
        start: Option<&Bound<'_, PyAny>>,
        end: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<CandleBatch> {
        let market = MarketId::taken(market, "a MarketId")?;
        let start = bound(start, Edge::Start)?;
        let end = bound(end, Edge::End)?;
        // Two bounds that both lie inside one millisecond: the start moved
        // to the next and the end to its own, which ends the range before
        // it starts, and that is not what was asked.
        let inside = match (start, end) {
            (Some(start), Some(end))
                if start.rounded && end.rounded && end.ms.checked_add(1) == Some(start.ms) =>
            {
                Some(end.ms)
            }
            _ => None,
        };
        let (start, end) = (start.map(|at| at.ms), end.map(|at| at.ms));
        let store = self.opened()?;
        let read = py.detach(|| -> Result<_, StorageError> {
            let candles = store.load(&market, start, end)?;
            let nothing = match (candles.is_empty(), inside) {
                (false, _) => None,
                (true, Some(millisecond)) => Some(store.nothing_inside(&market, millisecond)),
                (true, None) => Some(store.nothing_found(&market, start, end)),
            };
            Ok((quantforge::CandleBatch::from_candles(&candles), nothing))
        });
        let (candles, nothing) = read.map_err(raised)?;
        if let Some(why) = nothing {
            py.import(intern!(py, "logging"))?
                .call_method1(intern!(py, "getLogger"), (LOGGER,))?
                .call_method1(intern!(py, "warning"), (why,))?;
        }
        Ok(CandleBatch {
            candles,
            market: Some(market),
        })
    }

    /// Closes the store: its files, or its connections. A reader that is
    /// closed loads nothing more, and closing one again does nothing.
    fn close(&self) {
        let closed = self.held().take();
        // Dropped once the lock is free: the last holder closes the store.
        drop(closed);
    }

    /// The backend the store is on: `sqlite` or `postgres`.
    #[getter]
    fn backend(&self) -> &'static str {
        self.backend
    }

    /// Where the store is: a file's path, or `postgres://HOST:PORT/DATABASE`,
    /// without a user or a password.
    #[getter]
    fn location(&self) -> &str {
        &self.location
    }

    /// Whether the reader is closed.
    #[getter]
    fn closed(&self) -> bool {
        self.held().is_none()
    }

    fn __enter__(slf: Bound<'_, Self>) -> PyResult<Bound<'_, Self>> {
        slf.get().opened()?;
        Ok(slf)
    }

    #[pyo3(signature = (*_raised))]
    fn __exit__(&self, _raised: &Bound<'_, PyAny>) {
        self.close();
    }

    fn __repr__(&self) -> String {
        let closed = if self.closed() { ", closed" } else { "" };
        format!("<CandleReader {} {}{closed}>", self.backend, self.location)
    }
}

impl CandleReader {
    /// What the reader holds of its store. A thread that failed while it
    /// held the lock left the store as it was.
    fn held(&self) -> std::sync::MutexGuard<'_, Option<Arc<quantforge::CandleReader>>> {
        self.store.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The engine's reader, where this one is open and in the process that
    /// opened it.
    fn opened(&self) -> PyResult<Arc<quantforge::CandleReader>> {
        let here = std::process::id();
        if here != self.opened_by {
            return Err(PyRuntimeError::new_err(format!(
                "this reader was opened by process {}, and this is process {here}: a reader \
                 belongs to the process that opened it; open one here",
                self.opened_by
            )));
        }
        self.held()
            .clone()
            .ok_or_else(|| PyValueError::new_err("the reader is closed"))
    }
}

/// An error and what caused it, on one line: a storage error with no
/// category says only that it is one, and its cause says what happened.
fn with_causes(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(found) = cause {
        text.push_str(&format!(": {found}"));
        cause = found.source();
    }
    text
}

/// A store's failure as the exception Python is raised for it, by what a
/// caller does about it, the same on every backend.
fn raised(error: StorageError) -> PyErr {
    match error.kind() {
        // The store is not one a reader reads, or holds what it cannot
        // read: nothing a retry changes.
        StorageErrorKind::InvalidArgument | StorageErrorKind::SchemaUnsupported => match error {
            StorageError::InvalidArgument(refusal) => PyValueError::new_err(refusal),
            other => PyValueError::new_err(other.to_string()),
        },
        // The store was not reached in time, and nothing was read.
        StorageErrorKind::Unavailable | StorageErrorKind::Contention => {
            PyConnectionError::new_err(error.to_string())
        }
        _ => PyRuntimeError::new_err(with_causes(&error)),
    }
}

/// Why a store could not be opened to read, as the exception Python is
/// raised for it. A failure that names no store is told with the one the
/// settings selected: a SQLite file by its path, unless the path holds an
/// `@` or an `=`, as a URL or a connection string given in its place does.
fn not_opened(error: StoreOpenError, settings: &StorageSettings) -> PyErr {
    let selected = || {
        let location = settings.location();
        let shown = if settings.backend() == StorageBackend::Sqlite && may_be_a_secret(&location) {
            "the path given".to_string()
        } else {
            location
        };
        format!("cannot open the {} store at {shown}", settings.backend())
    };
    match error {
        StoreOpenError::NoSuchFile { path, resolved } => no_such_file(&path, resolved.as_deref()),
        StoreOpenError::Config(refused) => PyValueError::new_err(refused.to_string()),
        StoreOpenError::Storage(failure) => match failure.kind() {
            StorageErrorKind::InvalidArgument | StorageErrorKind::SchemaUnsupported => {
                raised(failure)
            }
            StorageErrorKind::Unavailable | StorageErrorKind::Contention => {
                PyConnectionError::new_err(format!("{}: {failure}", selected()))
            }
            _ => PyRuntimeError::new_err(format!("{}: {}", selected(), with_causes(&failure))),
        },
        other => PyRuntimeError::new_err(format!("{}: {}", selected(), with_causes(&other))),
    }
}

/// A SQLite file that is not there: the reader creates none. A path that
/// does not start at the root was settled to `resolved` as the reader
/// opened, where the process ran then, and the message says where it was
/// looked for.
///
/// The path is shown unless it holds an `@` or an `=`, and so is the full
/// path, which holds it. A path names a file and holds no secret, but a
/// value given where the path belongs may be a URL with a user and a
/// password before an `@`, or a connection string with one after an `=`,
/// and is then no file at all.
fn no_such_file(path: &Path, resolved: Option<&Path>) -> PyErr {
    let text = path.to_string_lossy();
    let shown = if may_be_a_secret(&text) {
        "the path given, which is not shown: it holds an @ or an =, as a URL or a connection \
         string with a password does"
            .to_string()
    } else {
        match resolved {
            match resolved {
                Some(resolved) if !may_be_a_secret(&resolved.to_string_lossy()) => {
                    format!("{text} (looked for at {})", resolved.display())
                }
                _ => text.into_owned(),
            }
            None => text.into_owned(),
        }
    };
    PyFileNotFoundError::new_err(format!(
        "there is no SQLite database at {shown}: a reader creates none, and `quantforge data \
         sync` does; db, or QF_DB, names another file"
    ))
}

/// Whether text a path was given as may be a secret: a URL with a password
/// holds an `@`, and a connection string an `=`.
fn may_be_a_secret(text: &str) -> bool {
    text.contains(['@', '='])
}
