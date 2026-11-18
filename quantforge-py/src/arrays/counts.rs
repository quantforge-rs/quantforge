//! The trade counts as `CandleBatch.from_numpy` takes them: a plain array,
//! where every count is known, or a masked array made over one, masked
//! where a count is not.

use numpy::{PyArrayMethods, PyUntypedArray};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::PyType;

use super::column::{Column, plain, room_for};
use super::masked_array;
use crate::convert::{type_name, wrong_type};

/// The trade counts as they are taken: a count for every row, and for a
/// masked array whose mask is set a flag for every row, which is set where
/// the count is unknown.
pub(super) struct Counts<'py> {
    pub(super) counts: Column<'py, u64>,
    unknown: Option<Column<'py, bool>>,
}

pub(super) const TRADES: &str = "trades";
const COUNTS: &str = "a numpy.ndarray or a numpy.ma.MaskedArray of uint64";
const MASK: &str = "the mask of trades";

/// `value` as the trade counts: a `numpy.ndarray` of uint64, where every
/// count is known, or a `numpy.ma.MaskedArray` of uint64, masked where one
/// is not. Each is that type itself, and nothing derived from it.
pub(super) fn counts<'py>(value: &Bound<'py, PyAny>) -> PyResult<Counts<'py>> {
    let py = value.py();
    if !value.is_exact_instance(masked_array(py)?) {
        return match value.cast_exact::<PyUntypedArray>() {
            Ok(array) => Ok(Counts {
                counts: Column::of(array, TRADES, COUNTS)?,
                unknown: None,
            }),
            Err(_) => Err(wrong_type(TRADES, COUNTS, value)),
        };
    }
    // A masked array made over an array of a derived type is of that type
    // underneath, and is refused as an array of that type is. It is refused
    // before anything is asked of it: numpy.ma hands its cells out through
    // that type's own code.
    let over = value.getattr(intern!(py, "baseclass"))?;
    if !over.is(py.get_type::<PyUntypedArray>()) {
        let named = match over.cast::<PyType>() {
            Ok(class) => class.name()?.to_string(),
            Err(_) => type_name(&over),
        };
        return Err(PyTypeError::new_err(format!(
            "{TRADES} must be {COUNTS}, not a MaskedArray over {named}"
        )));
    }
    // Its counts as the plain array they are over, held to their type
    // before its mask is asked for.
    let ma = py.import(intern!(py, "numpy.ma"))?;
    let data = ma.getattr(intern!(py, "getdata"))?.call1((value,))?;
    let Ok(data) = data.cast_exact::<PyUntypedArray>() else {
        return Err(wrong_type(TRADES, COUNTS, &data));
    };
    let counts = Column::of(data, TRADES, COUNTS)?;
    // A mask that was never set masks nothing, and there is none to read.
    let mask = ma.getattr(intern!(py, "getmask"))?.call1((value,))?;
    if mask.is(&ma.getattr(intern!(py, "nomask"))?) {
        return Ok(Counts {
            counts,
            unknown: None,
        });
    }
    // The mask is taken as the array of flags it is, and nothing is made
    // of it yet: its flags are read as bytes when the counts are.
    let unknown = plain::<bool>(&mask, MASK)?;
    if unknown.len() != counts.len() {
        return Err(rows_apart(unknown.len(), counts.len()));
    }
    Ok(Counts {
        counts,
        unknown: Some(unknown),
    })
}

/// The refusal of a mask that has not a flag for every count.
fn rows_apart(flags: usize, counts: usize) -> PyErr {
    PyValueError::new_err(format!(
        "{MASK} has {flags} rows, and {TRADES} has {counts}"
    ))
}

/// The trade counts, each known unless its row is masked.
pub(super) fn known(trades: &Counts<'_>) -> PyResult<Vec<Option<u64>>> {
    // Both arrays are made ready before either is borrowed: making one
    // ready may run NumPy's copy, and with it other code of Python's.
    let counts = trades.counts.ready()?;
    let flags = trades.unknown.as_ref().map(Column::ready).transpose()?;
    // The flags' bytes come last of all that Python's code has a part in:
    // they are read through a view that is made now.
    let bytes = flags.as_ref().map(|flags| flags.bytes()).transpose()?;
    // Nothing of Python's runs once the counts are borrowed. The view is
    // borrowed after them, so that no borrow is held should the counts be
    // refused: a refusal is worded in Python.
    let counts = counts.borrowed()?;
    let unknown = bytes
        .as_ref()
        .map(|bytes| bytes.try_readonly())
        .transpose()?;
    let counts = counts.as_slice()?;
    let mut known = room_for(counts.len(), TRADES)?;
    match &unknown {
        Some(unknown) => {
            let unknown = unknown.as_slice()?;
            // The mask may have changed its length since it was taken.
            if unknown.len() != counts.len() {
                return Err(rows_apart(unknown.len(), counts.len()));
            }
            known.extend(
                counts
                    .iter()
                    .zip(unknown)
                    .map(|(count, unknown)| (*unknown == 0).then_some(*count)),
            );
        }
        None => known.extend(counts.iter().copied().map(Some)),
    }
    Ok(known)
}
