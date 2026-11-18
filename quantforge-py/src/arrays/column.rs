//! One array as a column of a batch: held to the column's type as it is
//! taken, made ready to be read, and borrowed, held to its type once more,
//! only for as long as its cells are read.
//!
//! **Nothing of Python's runs while an array is borrowed.** The array
//! bindings keep a record of every borrow, under what the array was when it
//! was borrowed, and end the process where they are asked to give back a
//! borrow of an array whose cells have another size or number by then. NumPy
//! lets an array be changed so where it stands, by whatever code of
//! Python's runs meanwhile: another thread's, or what the collector runs.
//! So a borrow is taken last and given back first: what makes an array
//! ready to be read runs before it, and a refusal is worded after it.

use numpy::{
    PyArray1, PyArrayDescr, PyArrayDescrMethods, PyArrayMethods, PyReadonlyArray1,
    PyReadwriteArray1, PyUntypedArray, PyUntypedArrayMethods,
};
use pyo3::exceptions::{PyMemoryError, PyTypeError, PyValueError};
use pyo3::intern;
use pyo3::prelude::*;

use super::{AsItLies, Cell};
use crate::convert::wrong_type;

/// One column held to its type and not read yet: an array of one dimension
/// whose cells are of `T`, under the name a refusal gives it, with what a
/// refusal says the column takes.
pub(super) struct Column<'py, T: Cell> {
    pub(super) name: &'static str,
    takes: String,
    pub(super) array: Bound<'py, PyArray1<T>>,
}

/// What an array is found to be where it is no column of `T`: of another
/// type, or of another number of dimensions.
enum Amiss<'py> {
    Type(Bound<'py, PyArrayDescr>),
    Dimensions(usize),
}

/// What is amiss with an array as a column of `T`, if anything: its cells
/// are of `T` in this machine's byte order, and it has one dimension.
///
/// It asks NumPy alone, and nothing of Python's runs in it: so it may be
/// asked of an array that is borrowed.
fn amiss<'py, T: Cell>(array: &Bound<'py, PyUntypedArray>) -> Option<Amiss<'py>> {
    let dtype = array.dtype();
    if !dtype.is_equiv_to(&numpy::dtype::<T>(array.py())) {
        return Some(Amiss::Type(dtype));
    }
    if array.ndim() != 1 {
        return Some(Amiss::Dimensions(array.ndim()));
    }
    None
}

impl Amiss<'_> {
    /// The refusal of the column `name`, which `takes` says what it takes.
    ///
    /// NumPy words a type in Python: so this is never asked while the array
    /// is borrowed.
    fn refusal(self, name: &str, takes: &str) -> PyErr {
        match self {
            Self::Type(dtype) => match dtype.str() {
                Ok(found) => {
                    PyTypeError::new_err(format!("{name} must be {takes}, not of {found}"))
                }
                Err(error) => error,
            },
            Self::Dimensions(found) => {
                PyValueError::new_err(format!("{name} must have one dimension, not {found}"))
            }
        }
    }
}

/// Holds an array that is not borrowed to a column of `T`, and words the
/// refusal where it is none.
fn held<T: Cell>(array: &Bound<'_, PyUntypedArray>, name: &str, takes: &str) -> PyResult<()> {
    match amiss::<T>(array) {
        Some(amiss) => Err(amiss.refusal(name, takes)),
        None => Ok(()),
    }
}

impl<'py, T: Cell> Column<'py, T> {
    /// An array as a column of `T`.
    pub(super) fn of(
        array: &Bound<'py, PyUntypedArray>,
        name: &'static str,
        takes: &str,
    ) -> PyResult<Self> {
        held::<T>(array, name, takes)?;
        Ok(Self {
            name,
            takes: takes.to_string(),
            array: array.cast::<PyArray1<T>>()?.clone(),
        })
    }

    pub(super) fn len(&self) -> usize {
        self.array.len()
    }

    /// Makes the column ready to be read: the array itself where its cells
    /// lie one after another on their alignment, and otherwise a copy of it
    /// that NumPy makes, which knows how to walk an array whose cells lie
    /// apart, backwards or off their alignment.
    ///
    /// Whatever of Python's code reading the column takes runs here, while
    /// nothing of the column is borrowed.
    pub(super) fn ready(&self) -> PyResult<Ready<'_, 'py, T>> {
        let cells = if self.array.is_c_contiguous() && self.array.is_aligned() {
            self.array.clone()
        } else {
            let copied = self.array.call_method0(intern!(self.array.py(), "copy"))?;
            match copied.cast_exact::<PyUntypedArray>() {
                Ok(copied) => {
                    held::<T>(copied, self.name, &self.takes)?;
                    copied.cast::<PyArray1<T>>()?.clone()
                }
                Err(_) => return Err(wrong_type(self.name, &self.takes, &copied)),
            }
        };
        Ok(Ready {
            column: self,
            cells,
        })
    }
}

/// A column ready to be read: the array whose cells are read, which is not
/// borrowed yet.
pub(super) struct Ready<'c, 'py, T: Cell> {
    column: &'c Column<'py, T>,
    cells: Bound<'py, PyArray1<T>>,
}

impl<'py, T: AsItLies> Ready<'_, 'py, T> {
    /// Borrows the array, held to its type once more: its cells are then
    /// read from the borrow as one run of memory.
    ///
    /// NumPy lets an array's type and shape be changed where it stands, and
    /// Python's code has run since the array was first held to them: the
    /// cells of an array that is no longer of `T` would be read past their
    /// end, or as numbers they are not.
    ///
    /// Whoever holds the borrow runs nothing of Python's until it is given
    /// back. Nothing here does either. The array is borrowed first: the
    /// first borrow of a process lets other threads run before it is
    /// recorded, while the array bindings set up their record. Then it is
    /// asked what it is, without a word of NumPy's. Where it is no longer a
    /// column of `T`, the borrow is given back as it was taken, and only
    /// then is the refusal worded.
    pub(super) fn borrowed(&self) -> PyResult<PyReadonlyArray1<'py, T>> {
        let borrowed = self.cells.try_readonly()?;
        match amiss::<T>(self.cells.as_untyped()) {
            None => Ok(borrowed),
            Some(amiss) => {
                drop(borrowed);
                Err(amiss.refusal(self.column.name, &self.column.takes))
            }
        }
    }
}

impl<'py> Ready<'_, 'py, bool> {
    /// The flags as the bytes they are, one for each: a flag is set where
    /// its byte is not zero. Whoever reads them borrows what this gives,
    /// last of all.
    ///
    /// They are read through a view of the array as bytes, which is made
    /// here, as they are read, and not when the array is taken. A view
    /// keeps a pointer of its own, and is left pointing at memory the
    /// array has given back where the array is resized where it stands
    /// afterwards: NumPy lets one be, by whoever asks it not to look for
    /// what else uses the array.
    ///
    /// - **The flags themselves are never borrowed for longer than an
    ///   instant.** Making the view runs code of Python's, and the flags
    ///   are the caller's own array, which that code may change.
    /// - **Making the view is the last of Python's code that runs.** The
    ///   flags are held to their type after it, and nothing ran in between:
    ///   so the view is of an array of flags, of one dimension, and as long
    ///   as it.
    /// - **The view alone is borrowed to be read.** No other code can
    ///   reach it, so nothing changes it while it is.
    pub(super) fn bytes(&self) -> PyResult<Bound<'py, PyArray1<u8>>> {
        let flags = self.cells.as_untyped();
        let py = flags.py();
        // The array bindings set up their record of borrows when one is
        // first asked for, and let other threads run while they do. A
        // borrow that is given back at once puts that behind us: nothing
        // may run between the making of the view and the last read of it.
        drop(self.cells.try_readonly()?);
        let view = flags.call_method1(intern!(py, "view"), (numpy::dtype::<u8>(py),))?;
        held::<bool>(flags, self.column.name, &self.column.takes)?;
        let bytes = view
            .cast_exact::<PyUntypedArray>()
            .ok()
            .and_then(|view| view.cast::<PyArray1<u8>>().ok());
        match bytes {
            Some(bytes) => Ok(bytes.clone()),
            None => Err(wrong_type(self.column.name, &self.column.takes, &view)),
        }
    }
}

/// How a refusal names the array `numpy.zeros` was asked for.
const MADE: &str = "what numpy.zeros gave";

/// What `numpy.zeros` gave for an array of `rows` cells of `T`, held to
/// that: `numpy.ndarray` itself, of that type, of one dimension and of
/// that length.
pub(super) fn allocated<'py, T: Cell>(
    made: &Bound<'py, PyAny>,
    rows: usize,
) -> PyResult<Bound<'py, PyArray1<T>>> {
    let made = plain::<T>(made, MADE)?;
    if made.len() != rows {
        return Err(PyValueError::new_err(format!(
            "{MADE} has {} rows, and {rows} were asked for",
            made.len()
        )));
    }
    Ok(made.array)
}

/// Borrows, to be written, the array that `allocated` held to what was
/// asked of `numpy.zeros`, and holds it to that once more now that it is
/// borrowed, without a word, as a column that is read is.
///
/// The first borrow of a process lets other threads run before it is
/// recorded, and what stands where `numpy.zeros` was may have kept the
/// array it gave: the cells are written as an array of this type and
/// length.
pub(super) fn written<'py, T: Cell>(
    array: &Bound<'py, PyArray1<T>>,
    rows: usize,
) -> PyResult<PyReadwriteArray1<'py, T>> {
    let borrowed = array.try_readwrite()?;
    if amiss::<T>(array.as_untyped()).is_none() && array.len() == rows {
        return Ok(borrowed);
    }
    drop(borrowed);
    // Worded now that nothing is borrowed, for what the array has become.
    allocated::<T>(array.as_any(), rows)?;
    Err(PyValueError::new_err(format!(
        "{MADE} was changed as it was written"
    )))
}

/// An empty vector with room for `rows` values of the column `name`, or
/// Python's `MemoryError` where there is no memory for them: a vector left
/// to grow on its own stops the whole process instead.
pub(super) fn room_for<V>(rows: usize, name: &str) -> PyResult<Vec<V>> {
    let mut room = Vec::new();
    match room.try_reserve_exact(rows) {
        Ok(()) => Ok(room),
        Err(_) => Err(PyMemoryError::new_err(format!(
            "no memory for {rows} rows of {name}"
        ))),
    }
}

/// `value` as a column of `T`: a `numpy.ndarray` of that type and of one
/// dimension.
///
/// - **It is `numpy.ndarray` itself.** A type derived from it may mean
///   something else by the same memory, a mask or a unit, which reading its
///   cells would drop without a word: `numpy.asarray` gives the array of
///   one, and whoever calls it has decided so.
/// - **Its cells are of `T` in this machine's byte order.** Nothing is
///   cast: an int64 is not read as a float, nor a float32 as a float64.
pub(super) fn plain<'py, T: Cell>(
    value: &Bound<'py, PyAny>,
    name: &'static str,
) -> PyResult<Column<'py, T>> {
    let takes = format!("a numpy.ndarray of {}", T::NAME);
    match value.cast_exact::<PyUntypedArray>() {
        Ok(array) => Column::of(array, name, &takes),
        Err(_) => Err(wrong_type(name, &takes, value)),
    }
}
