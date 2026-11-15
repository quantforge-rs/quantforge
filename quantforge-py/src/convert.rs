//! How a value crosses between Python and the engine, in one place, so that
//! every bound type takes and gives a value the same way.
//!
//! - **A decimal** is a `decimal.Decimal` on the Python side, or an `int`
//!   where one is taken, and crosses as exact text: nothing is rounded to
//!   fit, and a `float` is never read as one.
//! - **An integer** is an `int` within the range of the engine's type.
//! - **Text and a flag** are a `str` and a `bool`, and nothing that merely
//!   converts to one.
//!
//! A value of the wrong type is a `TypeError`; a value of the right type
//! that the engine refuses is a `ValueError`, in the engine's own words
//! wherever it has a rule for it ([`refused`]).

use std::fmt;

use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::pyclass::boolean_struct::True;
use pyo3::sync::PyOnceLock;
use pyo3::types::{PyBool, PyFloat, PyInt, PyString, PyTuple, PyType};
use pyo3::{PyClass, intern};
use rust_decimal::Decimal;

/// A value the engine refused, as a `ValueError` that carries the engine's
/// message unchanged.
pub(crate) fn refused(error: impl fmt::Display) -> PyErr {
    PyValueError::new_err(error.to_string())
}

/// The name of `value`'s type, as a message names it.
fn type_name(value: &Bound<'_, PyAny>) -> String {
    match value.get_type().name() {
        Ok(name) => name.to_string(),
        Err(_) => "object".to_string(),
    }
}

/// `what` was handed a value of a type it does not take.
pub(crate) fn wrong_type(what: impl fmt::Display, takes: &str, value: &Bound<'_, PyAny>) -> PyErr {
    PyTypeError::new_err(format!("{what} must be {takes}, not {}", type_name(value)))
}

/// One row of a column, named as Python indexes it: `open[3]`.
pub(crate) struct Row(pub(crate) &'static str, pub(crate) usize);

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.0, self.1)
    }
}

static DECIMAL: PyOnceLock<Py<PyType>> = PyOnceLock::new();

/// Python's `decimal.Decimal`.
fn decimal_type(py: Python<'_>) -> PyResult<&Bound<'_, PyType>> {
    DECIMAL.import(py, "decimal", "Decimal")
}

/// What the engine's decimal holds, as a refusal states it.
const DECIMAL_RULE: &str = "must fit a 96-bit decimal with at most 28 decimal places";

/// The most a refusal quotes of the value it refuses, in characters.
const SHOWN: usize = 80;

/// `what` was handed `value`, of the right type, which breaks `rule`. The
/// value is quoted as Python writes it, cut short where it is very long.
fn invalid(what: impl fmt::Display, value: &Bound<'_, PyAny>, rule: impl fmt::Display) -> PyErr {
    // A value Python cannot write, such as an int of more digits than it
    // converts to text, is refused without being quoted.
    let message = match value.str() {
        Ok(text) => {
            let text = text.to_string();
            match text.char_indices().nth(SHOWN) {
                Some((cut, _)) => format!("invalid {what} {}...: {rule}", &text[..cut]),
                None => format!("invalid {what} {text}: {rule}"),
            }
        }
        Err(_) => format!("invalid {what}: {rule}"),
    };
    PyValueError::new_err(message)
}

/// The largest and the smallest power of ten whose first digit the engine's
/// decimal can hold: a value's most significant digit lies between them.
const ADJUSTED: std::ops::RangeInclusive<i64> = -28..=28;

/// The decimal `value` holds, exactly: a `decimal.Decimal` or an `int`.
///
/// The decimal's digits and its decimal places cross as they are, so `1.10`
/// keeps both places; an exponent above zero is written out, since the
/// engine's decimal has none. A negative zero becomes zero, as everywhere in
/// the engine. What does not fit is refused, never rounded, and so is a
/// `float`: it crosses only through `decimal_from_f64`.
///
/// A value of a type derived from `Decimal` or from `int` crosses by the
/// number it holds, whatever its own methods say of it.
pub(crate) fn decimal(value: &Bound<'_, PyAny>, what: impl fmt::Display) -> PyResult<Decimal> {
    let py = value.py();
    let decimal = decimal_type(py)?;
    if value.is_instance(decimal)? {
        // A type derived from `Decimal` may write itself as it likes, and
        // one that formats itself to two places would cross rounded. The
        // value is read from a plain `Decimal`, which `Decimal(value)`
        // copies from it digit for digit.
        let plain = if value.is_exact_instance(decimal) {
            value.clone()
        } else {
            decimal.call1((value,))?
        };
        // Where the value's first digit stands decides most refusals before
        // any text is written: `1E+999999999` would take a gigabyte of it. A
        // zero is zero whatever its exponent, unless that asks for places.
        let adjusted: i64 = plain.call_method0(intern!(py, "adjusted"))?.extract()?;
        if adjusted < *ADJUSTED.start()
            || (adjusted > *ADJUSTED.end()
                && !plain.call_method0(intern!(py, "is_zero"))?.is_truthy()?)
        {
            return Err(invalid(what, &plain, DECIMAL_RULE));
        }
        // Fixed-point text with every digit the value has: no context
        // rounds it, and no exponent is left in it. The engine's own parser
        // reads it, which refuses what it would have to round, and reads a
        // negative zero as zero.
        let text = plain.call_method1(intern!(py, "__format__"), (intern!(py, "f"),))?;
        let text = text.cast::<PyString>()?.to_cow()?;
        return Decimal::from_str_exact(&text).or_else(|_| {
            let finite = plain.call_method0(intern!(py, "is_finite"))?.is_truthy()?;
            let rule = if finite {
                DECIMAL_RULE
            } else {
                "must be a finite number"
            };
            Err(invalid(what, &plain, rule))
        });
    }
    if value.is_instance_of::<PyFloat>() {
        return Err(PyTypeError::new_err(format!(
            "{what} must be a decimal.Decimal or an int, not float: a float is never read as a \
             decimal; convert it with decimal_from_f64"
        )));
    }
    if value.is_instance_of::<PyBool>() {
        return Err(wrong_type(what, "a decimal.Decimal or an int", value));
    }
    let Some(whole) = whole_number(value)? else {
        return Err(wrong_type(what, "a decimal.Decimal or an int", value));
    };
    match whole.extract::<i128>() {
        Ok(number) => Decimal::try_from_i128_with_scale(number, 0)
            .map_err(|_| invalid(what, &whole, DECIMAL_RULE)),
        Err(error) if error.is_instance_of::<PyOverflowError>(py) => {
            Err(invalid(what, &whole, DECIMAL_RULE))
        }
        Err(error) => Err(error),
    }
}

/// The `int` that `value` is, or says it is through `__index__`, as the
/// integers of every other field are read; `None` for anything else. A
/// number that would have to be cut to a whole one says no such thing.
///
/// What is returned is a plain `int`. A type derived from `int` may shift
/// and write itself as it likes, and the 128 bits a decimal's whole number
/// takes are read by shifting: `int.__index__` copies the number it holds.
fn whole_number<'py>(value: &Bound<'py, PyAny>) -> PyResult<Option<Bound<'py, PyInt>>> {
    let py = value.py();
    let whole = match value.cast::<PyInt>() {
        Ok(whole) => whole.clone(),
        Err(_) => {
            let Ok(index) = value.getattr(intern!(py, "__index__")) else {
                return Ok(None);
            };
            let Ok(whole) = index.call0()?.cast_into::<PyInt>() else {
                return Ok(None);
            };
            whole
        }
    };
    if whole.is_exact_instance_of::<PyInt>() {
        return Ok(Some(whole));
    }
    let plain = py
        .get_type::<PyInt>()
        .call_method1(intern!(py, "__index__"), (whole,))?;
    Ok(plain.cast_into::<PyInt>().ok())
}

/// `value` as a `decimal.Decimal`, from its exact text: every digit and
/// every decimal place the engine holds, trailing zeros included.
pub(crate) fn decimal_object(py: Python<'_>, value: Decimal) -> PyResult<Bound<'_, PyAny>> {
    decimal_type(py)?.call1((value.to_string(),))
}

/// A column of decimals as a tuple of `decimal.Decimal`.
pub(crate) fn decimal_tuple<'py>(
    py: Python<'py>,
    values: &[Decimal],
) -> PyResult<Bound<'py, PyTuple>> {
    let objects = values
        .iter()
        .map(|value| decimal_object(py, *value))
        .collect::<PyResult<Vec<_>>>()?;
    PyTuple::new(py, objects)
}

/// An integer type of the engine's, with the range a refusal states.
pub(crate) trait Whole:
    for<'a, 'py> FromPyObject<'a, 'py, Error = PyErr> + fmt::Display
{
    const MIN: Self;
    const MAX: Self;
}

impl Whole for i64 {
    const MIN: Self = i64::MIN;
    const MAX: Self = i64::MAX;
}

impl Whole for u64 {
    const MIN: Self = u64::MIN;
    const MAX: Self = u64::MAX;
}

impl Whole for usize {
    const MIN: Self = usize::MIN;
    const MAX: Self = usize::MAX;
}

/// The integer `value` holds: an `int` within `T`'s range, and neither a
/// `bool` nor a number that would have to be cut to a whole one.
pub(crate) fn integer<T: Whole>(value: &Bound<'_, PyAny>, what: impl fmt::Display) -> PyResult<T> {
    let py = value.py();
    if value.is_instance_of::<PyBool>() {
        return Err(wrong_type(what, "an int", value));
    }
    match value.extract::<T>() {
        Ok(whole) => Ok(whole),
        Err(error) if error.is_instance_of::<PyOverflowError>(py) => {
            // Quoted as the number it is, not as its type writes itself.
            let plain = whole_number(value)?;
            Err(invalid(
                what,
                plain.as_deref().unwrap_or(value),
                format_args!("must be between {} and {}", T::MIN, T::MAX),
            ))
        }
        Err(error) if error.is_instance_of::<PyTypeError>(py) => {
            Err(wrong_type(what, "an int", value))
        }
        Err(error) => Err(error),
    }
}

/// The integer `value` holds, or `None` for Python's `None`: a value the
/// source did not report stays unknown, and is never read as zero.
pub(crate) fn optional_integer<T: Whole>(
    value: &Bound<'_, PyAny>,
    what: impl fmt::Display,
) -> PyResult<Option<T>> {
    if value.is_none() {
        return Ok(None);
    }
    match integer(value, &what) {
        Ok(whole) => Ok(Some(whole)),
        Err(error) if error.is_instance_of::<PyTypeError>(value.py()) => {
            Err(wrong_type(what, "an int or None", value))
        }
        Err(error) => Err(error),
    }
}

/// The text `value` holds: a `str`.
pub(crate) fn text(value: &Bound<'_, PyAny>, what: impl fmt::Display) -> PyResult<String> {
    match value.cast::<PyString>() {
        Ok(text) => Ok(text.to_cow()?.into_owned()),
        Err(_) => Err(wrong_type(what, "a str", value)),
    }
}

/// The flag `value` holds: a `bool`, and no number that stands for one.
pub(crate) fn flag(value: &Bound<'_, PyAny>, what: impl fmt::Display) -> PyResult<bool> {
    match value.cast::<PyBool>() {
        Ok(flag) => Ok(flag.is_true()),
        Err(_) => Err(wrong_type(what, "a bool", value)),
    }
}

/// Every item of `values` as the bound class `T`'s own value, in order:
/// `values` is anything Python iterates, `what` names it in a refusal, and
/// `takes` names the class with its article, such as `a Candle`.
pub(crate) fn each<T, V>(
    values: &Bound<'_, PyAny>,
    what: &'static str,
    takes: &str,
    value_of: impl Fn(&T) -> V,
) -> PyResult<Vec<V>>
where
    T: PyClass<Frozen = True> + Sync,
{
    let class = takes.split_once(' ').map_or(takes, |(_, class)| class);
    let items = values
        .try_iter()
        .map_err(|_| wrong_type(what, &format!("an iterable of {class}"), values))?;
    let mut taken = Vec::new();
    for (row, item) in items.enumerate() {
        let item = item?;
        match item.cast::<T>() {
            Ok(object) => taken.push(value_of(object.get())),
            Err(_) => return Err(wrong_type(Row(what, row), takes, &item)),
        }
    }
    Ok(taken)
}

/// Every item of the column `values`, read by `read` under its row's name.
pub(crate) fn column<V>(
    values: &Bound<'_, PyAny>,
    name: &'static str,
    read: impl Fn(&Bound<'_, PyAny>, Row) -> PyResult<V>,
) -> PyResult<Vec<V>> {
    // A str iterates too, one character at a time, and is never a column.
    if values.is_instance_of::<PyString>() {
        return Err(wrong_type(name, "an iterable of values", values));
    }
    let items = values
        .try_iter()
        .map_err(|_| wrong_type(name, "an iterable of values", values))?;
    let mut taken = Vec::new();
    for (row, item) in items.enumerate() {
        taken.push(read(&item?, Row(name, row))?);
    }
    Ok(taken)
}

/// `Name(first=..., second=...)`: a `repr` Python evaluates back to an equal
/// object, each field as Python itself writes it. A field without a name is
/// written as a positional argument.
pub(crate) fn repr_of(name: &str, fields: &[(&str, Bound<'_, PyAny>)]) -> PyResult<String> {
    repr_with(name, fields, &[])
}

/// As [`repr_of`], with `counted` after the fields: sequences too long to
/// write out, each shown as the text given, such as `<3 trades>`.
pub(crate) fn repr_with(
    name: &str,
    fields: &[(&str, Bound<'_, PyAny>)],
    counted: &[(&str, String)],
) -> PyResult<String> {
    let mut parts = Vec::with_capacity(fields.len() + counted.len());
    for (field, value) in fields {
        let value = value.repr()?;
        let value = value.to_cow()?;
        parts.push(match field {
            &"" => value.into_owned(),
            field => format!("{field}={value}"),
        });
    }
    for (field, shown) in counted {
        parts.push(format!("{field}={shown}"));
    }
    Ok(format!("{name}({})", parts.join(", ")))
}
