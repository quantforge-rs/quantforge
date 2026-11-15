//! The one crossing between a decimal and a float64, in both directions:
//! the engine's own two conversions, which follow
//! `docs/decimal-float-policy.md`. Nothing else in the package turns one
//! into the other.

use pyo3::prelude::*;
use pyo3::types::PyFloat;

use crate::convert::{decimal, decimal_object, refused, wrong_type};

/// The float nearest to a decimal: `decimal_to_f64(value)`.
///
/// `value` is a `decimal.Decimal` or an `int` the engine's decimal holds.
/// A value exactly halfway between two floats goes to the one whose last
/// bit is even, and zero of either sign gives `0.0`. Every such decimal
/// converts, and no result is NaN or infinite.
#[pyfunction]
#[pyo3(signature = (value, /))]
pub(crate) fn decimal_to_f64(value: &Bound<'_, PyAny>) -> PyResult<f64> {
    // A float is refused here in fewer words than elsewhere: it is already
    // what this function gives.
    if value.is_instance_of::<PyFloat>() {
        return Err(wrong_type("value", "a decimal.Decimal or an int", value));
    }
    Ok(quantforge::decimal_to_f64(decimal(value, "value")?))
}

/// The decimal of a float's shortest text: `decimal_from_f64(value)`.
///
/// `value` is a `float`, and the result the `decimal.Decimal` that
/// `Decimal(repr(value))` gives, with a negative zero made zero. NaN and
/// the infinities are refused with a `ValueError`, and so is a float whose
/// shortest text lies beyond the decimal range or needs more than 28
/// decimal places: it is never rounded to fit.
#[pyfunction]
#[pyo3(signature = (value, /))]
pub(crate) fn decimal_from_f64<'py>(value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let float = match value.cast::<PyFloat>() {
        Ok(float) => float.value(),
        Err(_) => return Err(wrong_type("value", "a float", value)),
    };
    let exact = quantforge::decimal_from_f64(float).map_err(refused)?;
    decimal_object(value.py(), exact)
}
