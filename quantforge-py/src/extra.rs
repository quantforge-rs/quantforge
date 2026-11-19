//! An extra of the package as it is found: a library the package does not
//! require, imported when something that needs it is first asked for, and
//! held to the versions its extra takes.
//!
//! NumPy and pandas are the package's two extras, and no requirement of it
//! (`pyproject.toml`). So there may be none to import, or one that `pip`
//! was never asked to hold to the extra's bounds, or a module that only has
//! the library's name. Each is refused here, in the same words for both,
//! before anything is asked of the library.

use std::cmp::Ordering;

use pyo3::PyTypeInfo;
use pyo3::exceptions::{PyAttributeError, PyImportError, PyModuleNotFoundError};
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyString};

/// One extra of the package: the library it brings, what of the package
/// needs it, and the versions of it the extra takes.
pub(crate) struct Extra {
    /// The module, as Python imports it: `numpy`.
    pub(crate) module: &'static str,
    /// The library, as a refusal names it: `NumPy`.
    pub(crate) library: &'static str,
    /// What of the package needs it, as every refusal opens: `to_numpy and
    /// from_numpy need`.
    pub(crate) needs: &'static str,
    /// The oldest version taken: the lower bound of the extra, and the
    /// oldest the tests run on.
    pub(crate) oldest: &'static [u64],
    /// The first version not taken: the upper bound of the extra.
    pub(crate) next: &'static [u64],
    /// What installs a version that is taken: the package with the extra.
    pub(crate) install: &'static str,
}

/// The numbers of a version as it is written: `2.0`.
fn written(numbers: &[u64]) -> String {
    let numbers: Vec<String> = numbers.iter().map(u64::to_string).collect();
    numbers.join(".")
}

impl Extra {
    /// What every refusal opens with: what needs the library, and the
    /// versions of it that are taken.
    pub(crate) fn needed(&self) -> String {
        format!(
            "{} {} {} or later and before {}",
            self.needs,
            self.library,
            written(self.oldest),
            written(self.next)
        )
    }

    /// The library itself, imported and held to the versions its extra
    /// takes, before anything is asked of it.
    ///
    /// - **Where Python finds none to import,** this is a
    ///   `ModuleNotFoundError` that says what to install, with Python's own
    ///   error as its cause.
    /// - **One that fails as it is imported** raises its own error, which
    ///   says what is wrong with it.
    /// - **One the extra does not take,** older than its lower bound or
    ///   from its upper bound on, is an `ImportError`. Nothing else holds
    ///   an environment to the extra's bounds, unless `pip` was asked for
    ///   the extra.
    /// - **A module that has the library's name and states no version** is
    ///   an `ImportError` as well.
    pub(crate) fn imported<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyModule>> {
        let module = match py.import(self.module) {
            Ok(module) => module,
            Err(error) if self.finds_none(py, &error) => {
                let said = format!(
                    "{}, and Python finds none to import: {}",
                    self.needed(),
                    self.install
                );
                return Err(self.unmet::<PyModuleNotFoundError>(py, said, Some(error)));
            }
            Err(error) => return Err(error),
        };
        let (stated, why) = match module.getattr(intern!(py, "__version__")) {
            Ok(stated) => (stated.cast_into::<PyString>().ok(), None),
            Err(error) if error.is_instance_of::<PyAttributeError>(py) => (None, Some(error)),
            Err(error) => return Err(error),
        };
        let Some(stated) = stated else {
            let said = format!(
                "{}, and what imports as {} here states no version: {}",
                self.needed(),
                self.module,
                module.repr()?
            );
            return Err(self.unmet::<PyImportError>(py, said, why));
        };
        let version = stated.to_cow()?;
        if !self.takes(&version) {
            let said = format!(
                "{}, and this is {} {version}: {}",
                self.needed(),
                self.library,
                self.install
            );
            return Err(self.unmet::<PyImportError>(py, said, None));
        }
        Ok(module)
    }

    /// Whether `error` says that there is no such module to import:
    /// Python's `ModuleNotFoundError` for that name itself, and not for a
    /// module the library looked for as it was imported and did not find.
    fn finds_none(&self, py: Python<'_>, error: &PyErr) -> bool {
        if !error.is_instance_of::<PyModuleNotFoundError>(py) {
            return false;
        }
        let Ok(name) = error.value(py).getattr(intern!(py, "name")) else {
            return false;
        };
        name.cast::<PyString>()
            .is_ok_and(|name| name.to_cow().is_ok_and(|name| name == self.module))
    }

    /// An `ImportError` of the kind `E` that names the library's module as
    /// the one it is about, as Python's own do, with `cause` as what it was
    /// raised from.
    pub(crate) fn unmet<E: PyTypeInfo>(
        &self,
        py: Python<'_>,
        said: String,
        cause: Option<PyErr>,
    ) -> PyErr {
        let named = PyDict::new(py);
        let raised = named
            .set_item(intern!(py, "name"), self.module)
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

    /// Whether `version`, as the library states one, is a version the
    /// extra takes: the oldest or a later one, and one before the next it
    /// does not.
    ///
    /// A version the library has stated is read as `pip` reads the extra's
    /// bounds: a pre-release of the oldest version itself comes before it,
    /// and a pre-release of the next is not before the next. Other text is
    /// read by the numbers it begins with, and is no version where it
    /// begins with none: what follows the numbers is looked at only for
    /// what marks a pre-release.
    fn takes(&self, version: &str) -> bool {
        let after = version.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.');
        let numbered = &version[..version.len() - after.len()];
        // A dot may part the numbers from what follows them: `2.0.0.dev0`.
        let release = match numbered.strip_suffix('.') {
            Some(release) if !after.is_empty() => release,
            _ => numbered,
        };
        let (Some(oldest), Some(next)) =
            (compared(release, self.oldest), compared(release, self.next))
        else {
            return false;
        };
        // Whatever follows the numbers of the next version, it is not
        // before it: `pip` takes no pre-release of a version for one below
        // it.
        if next != Ordering::Less {
            return false;
        }
        match oldest {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => !before_its_release(after),
        }
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
