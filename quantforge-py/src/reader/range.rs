//! The bounds of a range of candles: the first and the last open time a
//! reader is asked for, each as the engine's own millisecond.
//!
//! A bound is given in one of the three ways QuantForge writes a time:
//!
//! - **an `int`**: milliseconds since the Unix epoch, UTC, as every `_ms`
//!   field of the package is;
//! - **a `datetime.datetime` with a time zone**: an instant, in whatever
//!   zone it is written. One without a zone names no instant and is
//!   refused, and no zone is assumed for it;
//! - **a `str`**: an RFC 3339 time, read as the command line reads
//!   `--start` and `--end`, by the engine's own parser.
//!
//! A range is of candles' open times, both bounds included, and a candle
//! opens on a whole millisecond. A time between two milliseconds, a
//! `datetime` or text, bounds the range where it stands: as a start it is
//! the millisecond after it, and as an end the one before, so that no
//! candle outside the two instants is read and none inside is left out.

use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyString, PyType};

use crate::convert::{integer, wrong_type};

/// Which end of a range a bound is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Start,
    End,
}

impl Edge {
    /// The argument's name.
    const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

/// What a bound takes, as a refusal says it.
const TAKES: &str = "an int of milliseconds, a datetime.datetime with a time zone, a str or None";

/// The most a refusal quotes of a time written as text, in characters.
const SHOWN: usize = 80;

static DATETIME: PyOnceLock<Py<PyType>> = PyOnceLock::new();
static EPOCH: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
static MILLISECOND: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

/// Python's `datetime.datetime`.
fn datetime_type(py: Python<'_>) -> PyResult<&Bound<'_, PyType>> {
    DATETIME.import(py, "datetime", "datetime")
}

/// The Unix epoch as a `datetime` in UTC.
fn epoch(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    let epoch = EPOCH.get_or_try_init(py, || -> PyResult<Py<PyAny>> {
        let utc = py.import("datetime")?.getattr("timezone")?.getattr("utc")?;
        let epoch = datetime_type(py)?.call1((1970, 1, 1, 0, 0, 0, 0, utc))?;
        Ok(epoch.unbind())
    })?;
    Ok(epoch.bind(py))
}

/// One millisecond as a `datetime.timedelta`.
fn millisecond(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    let millisecond = MILLISECOND.get_or_try_init(py, || -> PyResult<Py<PyAny>> {
        let timedelta = py.import("datetime")?.getattr("timedelta")?;
        Ok(timedelta.call1((0, 0, 0, 1))?.unbind())
    })?;
    Ok(millisecond.bind(py))
}

/// The millisecond a bound names, or `None` where it is left out: the
/// range has no such end then.
pub(crate) fn bound(value: Option<&Bound<'_, PyAny>>, edge: Edge) -> PyResult<Option<i64>> {
    let name = edge.name();
    let Some(value) = value.filter(|value| !value.is_none()) else {
        return Ok(None);
    };
    let py = value.py();
    if let Ok(text) = value.cast::<PyString>() {
        let text = text.to_cow()?;
        return quantforge::parse_rfc3339_to_ms(&text)
            // The engine's millisecond is the one a time lies in. A start
            // past its beginning is the next one.
            .map(|ms| match edge {
                Edge::Start if past_its_millisecond(&text) => Some(ms.saturating_add(1)),
                _ => Some(ms),
            })
            .map_err(|error| {
                let shown = match text.char_indices().nth(SHOWN) {
                    Some((cut, _)) => format!("{}...", &text[..cut]),
                    None => text.to_string(),
                };
                PyValueError::new_err(format!(
                    "invalid {name} '{shown}': {error}; write an RFC 3339 time, such as \
                     2026-01-01T00:00:00Z"
                ))
            });
    }
    if value.is_instance(datetime_type(py)?)? {
        return instant(value, edge).map(Some);
    }
    // An integer as every other of the package is read: within the
    // engine's range, and never a flag, which is a number to Python.
    match integer::<i64>(value, name) {
        Ok(ms) => Ok(Some(ms)),
        Err(error) if error.is_instance_of::<PyTypeError>(py) => {
            Err(wrong_type(name, TAKES, value))
        }
        Err(error) => Err(error),
    }
}

/// Whether an RFC 3339 time, which the engine has read, holds anything past
/// its millisecond: a digit other than zero after the third of its
/// fraction of a second. The fraction is the one place such a time has a
/// point.
fn past_its_millisecond(text: &str) -> bool {
    text.split_once('.').is_some_and(|(_, fraction)| {
        fraction
            .chars()
            .take_while(char::is_ascii_digit)
            .skip(3)
            .any(|digit| digit != '0')
    })
}

/// The millisecond a `datetime` bounds a range at: the first one at or
/// after it as a start, and the last one at or before it as an end.
///
/// The arithmetic is the value's own, so a type derived from `datetime`
/// that holds more than microseconds, as pandas' `Timestamp` does, is read
/// with all it holds.
fn instant(value: &Bound<'_, PyAny>, edge: Edge) -> PyResult<i64> {
    let py = value.py();
    let name = edge.name();
    if value.call_method0(intern!(py, "utcoffset"))?.is_none() {
        return Err(PyTypeError::new_err(format!(
            "{name} must be a datetime.datetime with a time zone, not one without: a time \
             without a zone is never read as an instant; give it one, such as \
             datetime.timezone.utc"
        )));
    }
    let since = value.sub(epoch(py)?)?;
    let one = millisecond(py)?;
    let whole = match edge {
        // Floor division rounds down, so the millisecond after an instant
        // is the one before its negation, negated.
        Edge::Start => since.neg()?.floor_div(one)?.neg()?,
        Edge::End => since.floor_div(one)?,
    };
    match whole.extract::<i64>() {
        Ok(ms) => Ok(ms),
        // No `datetime` of Python's own is that far from the epoch: a type
        // derived from it may be.
        Err(error) if error.is_instance_of::<PyOverflowError>(py) => {
            Err(PyValueError::new_err(format!(
                "invalid {name}: must be between {} and {} milliseconds from the Unix epoch",
                i64::MIN,
                i64::MAX
            )))
        }
        Err(error) => Err(error),
    }
}
