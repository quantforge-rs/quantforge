//! NumPy as it is installed: imported when a conversion is first asked
//! for, and held to the versions the package's extra takes.
//!
//! NumPy is an extra of the package and no requirement of it
//! (`pyproject.toml`). So there may be none to import, or one that `pip`
//! was never asked to hold to the extra's bounds, or a module that only
//! has NumPy's name. Each is refused here, before anything is asked of the
//! array bindings, which panic where NumPy is not what they expect, and
//! which take every NumPy from 2.0 on for a NumPy 2. What is refused of
//! any extra, and in what words, is `extra.rs`; what is NumPy's alone is
//! here.

use pyo3::exceptions::{PyException, PyImportError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::PyCapsule;

use crate::extra::Extra;

/// NumPy, the extra `CandleBatch.to_numpy` and `from_numpy` need.
///
/// - **`oldest`** is the lower bound of the package's `numpy` extra, and
///   the oldest NumPy the tests run on.
/// - **`next`** is its upper bound. The array bindings know NumPy 2: the
///   module its arrays are kept in, the place of each function in the
///   table of its own, and how it lays a type out. They ask a NumPy only
///   whether it is 2.0 or later, so one that changed any of that would be
///   read as a NumPy 2 and not refused. No NumPy 3 exists to be tested:
///   the bound is raised once one does and the tests run on it.
pub(crate) const NUMPY: Extra = Extra {
    module: "numpy",
    library: "NumPy",
    needs: "to_numpy and from_numpy need",
    oldest: &[2, 0],
    next: &[3],
    install: r#"pip install "quantforge-py[numpy]""#,
};

/// NumPy itself, imported before anything is asked of the array bindings,
/// and held to the versions the conversions take.
///
/// - **Where Python finds no NumPy to import,** this is a
///   `ModuleNotFoundError` that says what to install, with Python's own
///   error as its cause.
/// - **A NumPy that fails as it is imported** raises its own error, which
///   says what is wrong with it.
/// - **A NumPy the conversions do not take,** one older than the extra's
///   lower bound or one from its upper bound on, is an `ImportError`.
///   Nothing else holds an environment to the extra's bounds, unless `pip`
///   was asked for the extra.
/// - **A module that has NumPy's name and not its arrays** is an
///   `ImportError` as well, whether or not it states a version.
///
/// The array bindings look NumPy up when they are first used, and panic
/// where it cannot be imported or is not NumPy. Nothing is asked of them
/// before this has passed.
pub(crate) fn imported(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let numpy = NUMPY.imported(py)?;
    match arrays(py) {
        Ok(()) => Ok(numpy),
        // An error of Python's own kind that is no failure of this lookup,
        // such as an interrupt, is raised as it is.
        Err(error) if !error.is_instance_of::<PyException>(py) => Err(error),
        Err(error) => {
            let said = format!(
                "{}, and what imports as numpy here does not hold NumPy's arrays: {}",
                NUMPY.needed(),
                numpy.repr()?
            );
            Err(NUMPY.unmet::<PyImportError>(py, said, Some(error)))
        }
    }
}

/// Looks for NumPy's arrays as the array bindings look for them, step for
/// step, and without their panic: the module they are kept in, and in it
/// the capsule that holds the table of NumPy's own functions.
///
/// A module that states NumPy's version and is not NumPy has none: a file
/// or a directory that happens to be named `numpy`, or what a test puts in
/// NumPy's place.
fn arrays(py: Python<'_>) -> PyResult<()> {
    let kept = numpy::get_array_module(py)?.getattr(intern!(py, "_ARRAY_API"))?;
    kept.cast_into::<PyCapsule>()?.pointer_checked(None)?;
    Ok(())
}
