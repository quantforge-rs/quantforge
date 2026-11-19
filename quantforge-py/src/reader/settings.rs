//! Which store a reader opens: the settings the command line resolves, from
//! the arguments a reader is given and, where one is left out, from the
//! variable the command line reads for the same setting.
//!
//! - **An argument wins over its variable, and a variable over the
//!   default,** setting by setting. `None` leaves an argument out.
//! - **A variable that is read counts, though it is empty,** as on the
//!   command line: an empty value is refused, never taken for the default.
//!   One whose argument is given is not read, where the command line
//!   refuses a malformed variable even beside its flag.
//! - **A count is the command line's:** digits and nothing around them in a
//!   variable, within the range its flag takes, the pool's up to the most
//!   connections a PostgreSQL server serves.
//! - **The rules are the engine's** (`quantforge::StorageSettings::resolve`):
//!   a backend is selected or is SQLite, never guessed, and no setting of
//!   one backend is left unused beside the other.
//! - **No refusal quotes a value.** Each names the argument or the variable
//!   the value came from, so a URL written where another setting belongs
//!   never reaches a traceback with its password. Nor does any exception
//!   raised on the way carry one: text Python cannot write as UTF-8 is
//!   refused in words of the reader's own, since Python's own error would
//!   hold the whole text.

use std::path::PathBuf;

use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyString};
use quantforge::storage::settings::MAX_POOL_SIZE;
use quantforge::{
    ConnectionString, StorageBackend, StorageConfigError, StorageRequest, StorageSettings,
    StoreOwner, Supplied,
};

use crate::convert::wrong_type;

/// The arguments a reader is opened with, each `None` where it was left
/// out.
pub(crate) struct Given<'a, 'py> {
    pub(crate) storage: Option<&'a Bound<'py, PyAny>>,
    pub(crate) db: Option<&'a Bound<'py, PyAny>>,
    pub(crate) postgres_url: Option<&'a Bound<'py, PyAny>>,
    pub(crate) postgres_pool_size: Option<&'a Bound<'py, PyAny>>,
    pub(crate) postgres_connect_timeout_secs: Option<&'a Bound<'py, PyAny>>,
}

/// A setting's argument, and the variable read where the argument is left
/// out: the command line's, which the two share.
struct Setting {
    argument: &'static str,
    variable: &'static str,
}

const STORAGE: Setting = Setting {
    argument: "storage",
    variable: "QF_STORAGE",
};
const DB: Setting = Setting {
    argument: "db",
    variable: "QF_DB",
};
const POSTGRES_URL: Setting = Setting {
    argument: "postgres_url",
    variable: "QF_POSTGRES_URL",
};
const POSTGRES_POOL_SIZE: Setting = Setting {
    argument: "postgres_pool_size",
    variable: "QF_POSTGRES_POOL_SIZE",
};
const POSTGRES_CONNECT_TIMEOUT_SECS: Setting = Setting {
    argument: "postgres_connect_timeout_secs",
    variable: "QF_POSTGRES_CONNECT_TIMEOUT_SECS",
};

/// A value as it was supplied: by an argument, as the object it is, or by
/// a variable, as its text.
enum Source<'a, 'py> {
    Argument(&'a Bound<'py, PyAny>),
    Variable(String),
}

impl Setting {
    /// The value supplied for the setting, or `None` where neither its
    /// argument nor its variable holds one. The variable is read from
    /// `os.environ`, the environment as Python has it, which a program may
    /// have changed since it started.
    fn supplied<'a, 'py>(
        &self,
        py: Python<'py>,
        argument: Option<&'a Bound<'py, PyAny>>,
    ) -> PyResult<Option<Source<'a, 'py>>> {
        if let Some(argument) = argument {
            return Ok(Some(Source::Argument(argument)));
        }
        let environment = py
            .import(intern!(py, "os"))?
            .getattr(intern!(py, "environ"))?;
        let found = environment.call_method1(intern!(py, "get"), (self.variable,))?;
        if found.is_none() {
            return Ok(None);
        }
        Ok(Some(Source::Variable(text(&found, self.variable)?)))
    }
}

/// The text `value` holds, a `str`, as UTF-8. One that holds a lone
/// surrogate, as a variable whose bytes are no UTF-8 does in `os.environ`,
/// is refused without the value: Python's own error would carry all of it.
fn text(value: &Bound<'_, PyAny>, label: &str) -> PyResult<String> {
    match value.cast::<PyString>() {
        Ok(text) => utf8(text, label),
        Err(_) => Err(wrong_type(label, "a str", value)),
    }
}

fn utf8(text: &Bound<'_, PyString>, label: &str) -> PyResult<String> {
    match text.to_cow() {
        Ok(text) => Ok(text.into_owned()),
        Err(_) => Err(PyValueError::new_err(format!(
            "{label} is no text that can be written as UTF-8: it holds a lone surrogate"
        ))),
    }
}

/// The settings of the store a reader opens: what the arguments and the
/// variables supplied, resolved by the engine's rules for a reader. Nothing
/// is opened.
pub(crate) fn resolved(py: Python<'_>, given: Given<'_, '_>) -> PyResult<StorageSettings> {
    let mut request = StorageRequest::default();
    request.backend = match STORAGE.supplied(py, given.storage)? {
        Some(Source::Argument(value)) => {
            let label = STORAGE.argument;
            Some(Supplied::new(backend(&text(value, label)?, label)?, label))
        }
        Some(Source::Variable(value)) => {
            let label = STORAGE.variable;
            Some(Supplied::new(backend(&value, label)?, label))
        }
        None => None,
    };
    request.sqlite_path = match DB.supplied(py, given.db)? {
        Some(Source::Argument(value)) => {
            let label = DB.argument;
            Some(Supplied::new(file(&path_text(value)?, label)?, label))
        }
        Some(Source::Variable(value)) => {
            let label = DB.variable;
            Some(Supplied::new(file(&value, label)?, label))
        }
        None => None,
    };
    request.postgres_url = match POSTGRES_URL.supplied(py, given.postgres_url)? {
        Some(Source::Argument(value)) => {
            let label = POSTGRES_URL.argument;
            Some(Supplied::new(
                ConnectionString::new(text(value, label)?),
                label,
            ))
        }
        Some(Source::Variable(value)) => Some(Supplied::new(
            ConnectionString::new(value),
            POSTGRES_URL.variable,
        )),
        None => None,
    };
    // A pool has room made for all its connections as it is built, and a
    // server serves no more than these.
    request.postgres_pool_size = count(
        py,
        &POSTGRES_POOL_SIZE,
        given.postgres_pool_size,
        u64::from(MAX_POOL_SIZE),
        ", the most connections a PostgreSQL server serves",
    )?
    .map(|supplied| {
        // Within the range just checked, which a `u32` holds.
        let size = u32::try_from(supplied.value).unwrap_or(MAX_POOL_SIZE);
        Supplied::new(size, supplied.label)
    });
    request.postgres_connect_timeout_secs = count(
        py,
        &POSTGRES_CONNECT_TIMEOUT_SECS,
        given.postgres_connect_timeout_secs,
        u64::MAX,
        "",
    )?;
    // No durability is asked for: a reader commits nothing, and its stores
    // judge none.
    StorageSettings::resolve(request, StoreOwner::Reader).map_err(refused)
}

/// The backend `name` names, `sqlite` or `postgres`.
fn backend(name: &str, label: &str) -> PyResult<StorageBackend> {
    name.parse().map_err(|_| {
        let names: Vec<&str> = StorageBackend::ALL
            .iter()
            .map(|backend| backend.as_str())
            .collect();
        PyValueError::new_err(format!("{label} must be {}", names.join(" or ")))
    })
}

/// The text of a path an argument holds: a `str`, or what an `os.PathLike`
/// gives for one.
fn path_text(value: &Bound<'_, PyAny>) -> PyResult<String> {
    let py = value.py();
    let takes = "a str or an os.PathLike of one";
    if let Ok(text) = value.cast::<PyString>() {
        return utf8(text, DB.argument);
    }
    // Looked at before it is called: a value whose type has no `__fspath__`
    // is of the wrong type, and what one that has it raises is its own.
    let own = value.get_type().getattr_opt(intern!(py, "__fspath__"));
    if !matches!(own, Ok(Some(found)) if !found.is_none()) {
        return Err(wrong_type(DB.argument, takes, value));
    }
    let path = py
        .import(intern!(py, "os"))?
        .call_method1(intern!(py, "fspath"), (value,))?;
    match path.cast::<PyString>() {
        Ok(text) => utf8(text, DB.argument),
        // A path of bytes names a file in an encoding nothing here knows.
        Err(_) => Err(wrong_type(DB.argument, takes, value)),
    }
}

/// The SQLite file `path` names. A PostgreSQL URL written where the file
/// belongs is refused here, unquoted: as a path it would be shown. It is
/// told by its scheme, whatever its case, the quotes or the space around
/// it, a driver's name after a `+` as SQLAlchemy writes one, and a `/`
/// that `pathlib` took out of its `//`.
fn file(path: &str, label: &str) -> PyResult<PathBuf> {
    if path.contains('\0') {
        return Err(PyValueError::new_err(format!(
            "{label} holds a NUL character, which no file's path holds"
        )));
    }
    if names_postgres(path) {
        return Err(PyValueError::new_err(format!(
            "{label} takes a SQLite file's path, and holds a PostgreSQL URL: give the URL as {} \
             or in {}, with {}=\"postgres\" or {}=postgres",
            POSTGRES_URL.argument, POSTGRES_URL.variable, STORAGE.argument, STORAGE.variable
        )));
    }
    Ok(PathBuf::from(path))
}

/// Whether `text` is written as a PostgreSQL URL: a scheme that starts with
/// `postgres`, of letters, digits and `+`, and a `:` after it.
fn names_postgres(text: &str) -> bool {
    let text = text.trim_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    scheme.to_ascii_lowercase().starts_with("postgres")
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+')
}

/// A count a setting holds, from 1 to `most`, with where it came from:
/// the command line's own range for the setting. `why` says why `most` is
/// the most, where it is no type's limit.
fn count(
    py: Python<'_>,
    setting: &Setting,
    argument: Option<&Bound<'_, PyAny>>,
    most: u64,
    why: &str,
) -> PyResult<Option<Supplied<u64>>> {
    let out_of_range = |label: &str| {
        PyValueError::new_err(format!(
            "{label} must be a whole number from 1 to {most}{why}"
        ))
    };
    let in_range = |count: u64, label: &'static str| match count {
        1.. if count <= most => Ok(Some(Supplied::new(count, label))),
        _ => Err(out_of_range(label)),
    };
    match setting.supplied(py, argument)? {
        None => Ok(None),
        Some(Source::Argument(value)) => {
            let label = setting.argument;
            if value.is_instance_of::<PyBool>() {
                return Err(wrong_type(label, "an int", value));
            }
            match value.extract::<u64>() {
                Ok(count) => in_range(count, label),
                Err(error) if error.is_instance_of::<PyOverflowError>(py) => {
                    Err(out_of_range(label))
                }
                Err(error) if error.is_instance_of::<PyTypeError>(py) => {
                    Err(wrong_type(label, "an int", value))
                }
                Err(error) => Err(error),
            }
        }
        // Read as the command line reads it: digits, and nothing around
        // them.
        Some(Source::Variable(value)) => match value.parse::<u64>() {
            Ok(count) => in_range(count, setting.variable),
            Err(_) => Err(out_of_range(setting.variable)),
        },
    }
}

/// Settings the engine refused, as a `ValueError` in the engine's words,
/// with the arguments and the variables that fix it where the engine's
/// message cannot name them.
fn refused(error: StorageConfigError) -> PyErr {
    let remedy = match &error {
        StorageConfigError::MissingPostgresUrl => format!(
            "; give it as {}, or in {}, which keeps a password out of the code",
            POSTGRES_URL.argument, POSTGRES_URL.variable
        ),
        StorageConfigError::PostgresSettingWithoutBackend { .. } => format!(
            ": pass {storage}=\"postgres\" or {storage}=\"sqlite\", or set {}",
            STORAGE.variable,
            storage = STORAGE.argument
        ),
        StorageConfigError::NotBuilt { .. } => {
            "; quantforge_py.STORAGE_BACKENDS names the stores this build opens".to_string()
        }
        _ => String::new(),
    };
    PyValueError::new_err(format!("{error}{remedy}"))
}
