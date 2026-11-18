//! NumPy as it is installed: imported when a conversion is first asked
//! for, and held to the versions the package's extra takes.
//!
//! NumPy is the package's one extra and no requirement of it
//! (`pyproject.toml`). So there may be none to import, or one that `pip`
//! was never asked to hold to the extra's bounds, or a module that only
//! has NumPy's name. Each is refused here, before anything is asked of the
//! array bindings, which panic where NumPy is not what they expect, and
//! which take every NumPy from 2.0 on for a NumPy 2.

use std::cmp::Ordering;

use pyo3::PyTypeInfo;
use pyo3::exceptions::{PyAttributeError, PyException, PyImportError, PyModuleNotFoundError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyCapsule, PyDict, PyString};

/// The oldest NumPy the conversions take: the lower bound of the package's
/// `numpy` extra, and the oldest the tests run on.
const OLDEST: [u64; 2] = [2, 0];

/// The first NumPy the conversions do not take: the upper bound of the
/// extra.
///
/// The array bindings know NumPy 2: the module its arrays are kept in, the
/// place of each function in the table of its own, and how it lays a type
/// out. They ask a NumPy only whether it is 2.0 or later, so one that
/// changed any of that would be read as a NumPy 2 and not refused. No
/// NumPy 3 exists to be tested: the bound is raised once one does and the
/// tests run on it.
const NEXT: [u64; 1] = [3];

/// What installs a NumPy the conversions take: the package with its extra.
const INSTALL: &str = r#"pip install "quantforge-py[numpy]""#;

/// What every refusal here opens with.
fn needed() -> String {
    format!(
        "to_numpy and from_numpy need NumPy {}.{} or later and before {}",
        OLDEST[0], OLDEST[1], NEXT[0]
    )
}

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
pub(super) fn imported(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let numpy = match py.import(intern!(py, "numpy")) {
        Ok(numpy) => numpy,
        Err(error) if finds_no_numpy(py, &error) => {
            let said = format!("{}, and Python finds none to import: {INSTALL}", needed());
            return Err(unmet::<PyModuleNotFoundError>(py, said, Some(error)));
        }
        Err(error) => return Err(error),
    };
    let (stated, why) = match numpy.getattr(intern!(py, "__version__")) {
        Ok(stated) => (stated.cast_into::<PyString>().ok(), None),
        Err(error) if error.is_instance_of::<PyAttributeError>(py) => (None, Some(error)),
        Err(error) => return Err(error),
    };
    let Some(stated) = stated else {
        let said = format!(
            "{}, and what imports as numpy here states no version: {}",
            needed(),
            numpy.repr()?
        );
        return Err(unmet::<PyImportError>(py, said, why));
    };
    let version = stated.to_cow()?;
    if !taken(&version) {
        let said = format!("{}, and this is NumPy {version}: {INSTALL}", needed());
        return Err(unmet::<PyImportError>(py, said, None));
    }
    match arrays(py) {
        Ok(()) => Ok(numpy),
        // An error of Python's own kind that is no failure of this lookup,
        // such as an interrupt, is raised as it is.
        Err(error) if !error.is_instance_of::<PyException>(py) => Err(error),
        Err(error) => {
            let said = format!(
                "{}, and what imports as numpy here does not hold NumPy's arrays: {}",
                needed(),
                numpy.repr()?
            );
            Err(unmet::<PyImportError>(py, said, Some(error)))
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

/// Whether `error` says that there is no `numpy` to import: Python's
/// `ModuleNotFoundError` for that name itself, and not for a module that
/// NumPy looked for as it was imported and did not find.
fn finds_no_numpy(py: Python<'_>, error: &PyErr) -> bool {
    if !error.is_instance_of::<PyModuleNotFoundError>(py) {
        return false;
    }
    let Ok(name) = error.value(py).getattr(intern!(py, "name")) else {
        return false;
    };
    name.cast::<PyString>()
        .is_ok_and(|name| name.to_cow().is_ok_and(|name| name == "numpy"))
}

/// An `ImportError` of the kind `E` that names NumPy as the module it is
/// about, as Python's own do, with `cause` as what it was raised from.
fn unmet<E: PyTypeInfo>(py: Python<'_>, said: String, cause: Option<PyErr>) -> PyErr {
    let named = PyDict::new(py);
    let raised = named
        .set_item(intern!(py, "name"), intern!(py, "numpy"))
        .and_then(|()| py.get_type::<E>().call((said,), Some(&named)));
    match raised {
        Ok(raised) => {
            let unmet = PyErr::from_value(raised);
            if cause.is_some() {
                unmet.set_cause(py, cause);
            }
            unmet
        }
        Err(error) => error,
    }
}

/// Whether `version`, as NumPy states one, is a version the conversions
/// take: the oldest or a later one, and one before the next they do not.
///
/// A version NumPy has stated is read as `pip` reads the extra's bounds: a
/// pre-release of the oldest version itself comes before it, and a
/// pre-release of the next is not before the next. Other text is read by
/// the numbers it begins with, and is no version where it begins with
/// none: what follows the numbers is looked at only for what marks a
/// pre-release.
fn taken(version: &str) -> bool {
    let after = version.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
    let numbered = &version[..version.len() - after.len()];
    // A dot may part the numbers from what follows them: `2.0.0.dev0`.
    let release = match numbered.strip_suffix('.') {
        Some(release) if !after.is_empty() => release,
        _ => numbered,
    };
    let (Some(oldest), Some(next)) = (compared(release, &OLDEST), compared(release, &NEXT)) else {
        return false;
    };
    // Whatever follows the numbers of the next version, it is not before
    // it: `pip` takes no pre-release of a version for one below it.
    if next != Ordering::Less {
        return false;
    }
    match oldest {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => !before_its_release(after),
    }
}

/// How the numbers of `release` compare with those of `bound`, the first
/// that differ deciding it, or `None` where `release` leaves a number
/// empty and so is no version.
fn compared(release: &str, bound: &[u64]) -> Option<Ordering> {
    let mut numbers = release.split('.');
    let mut bound = bound.iter();
    let mut compared = Ordering::Equal;
    loop {
        // A number that either leaves out is zero: `2` is `2.0`, and
        // `2.0.1` is later than `2.0`.
        let (number, held) = match (numbers.next(), bound.next()) {
            (None, None) => return Some(compared),
            (Some(""), _) => return None,
            // Digits alone are left, so a number that is not read is one
            // beyond every number of the bound.
            (Some(number), held) => (
                number.parse::<u64>().unwrap_or(u64::MAX),
                held.copied().unwrap_or(0),
            ),
            (None, Some(held)) => (0, *held),
        };
        if compared == Ordering::Equal {
            compared = number.cmp(&held);
        }
    }
}

/// Whether what follows a version's numbers marks a pre-release of it, an
/// alpha, a beta, a release candidate or a development build: `rc1` of
/// `2.0.0rc1`, `.dev0` of `2.0.0.dev0`, and not `.post1`.
///
/// The mark is a word of its own, with a number after it or nothing: a
/// build's own name that only begins like one, as `-custom` begins with a
/// `c`, marks none, and neither does a build's label after a `+`.
fn before_its_release(after: &str) -> bool {
    let marker = after
        .trim_start_matches(['.', '-', '_'])
        .to_ascii_lowercase();
    [
        "alpha", "beta", "preview", "pre", "rc", "dev", "a", "b", "c",
    ]
    .iter()
    .any(|mark| {
        marker
            .strip_prefix(mark)
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_alphabetic()))
    })
}
