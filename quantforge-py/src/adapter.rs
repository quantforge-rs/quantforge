//! How an engine drives a strategy written in Python: [`PyStrategy`] is the
//! engine's `Strategy` over an object of a class derived from the package's
//! own (`strategy.rs`). It adds nothing to a run and keeps nothing from it.
//!
//! - **A callback attaches its thread to Python.** Each one holds the
//!   interpreter for as long as it runs and no longer, whatever thread the
//!   engine is on. A thread that is attached already, as the one that called
//!   into the package is, stays so; one that is not takes the interpreter
//!   for the callback and gives it back.
//! - **A host attaches its engine thread once, around the whole run,** and
//!   detaches inside it: `Python::attach(|py| py.detach(|| engine.run(..)))`.
//!   Python keeps a thread's own state, such as the decimal context a
//!   strategy set or a `threading.local`, for as long as the thread stays
//!   attached. A thread attached anew for every callback would lose it
//!   between two bars, and a strategy would compute differently there.
//! - **The interpreter is the host's.** The adapter starts none: Python is
//!   running before a run starts, and until it is over.
//! - **What a callback is handed is built anew:** a `StrategyContext`, and
//!   for a bar a `Candle`, each a copy of the engine's own value.
//! - **What a callback raises ends the run.** The engine is told in a
//!   `StrategyError`: a line that says which callback failed and at which
//!   bar, then the exception as Python itself writes it, with its
//!   traceback. The exception is kept as it was raised, with that line as a
//!   note, for a host that is Python to raise again
//!   ([`PyStrategy::failure`]).
//! - **A callback that returns what it may not has failed too,** with a
//!   `TypeError` of the package's own: nothing is read into a value that is
//!   neither a target nor `None`.

use pyo3::exceptions::PyTypeError;
use pyo3::intern;
use pyo3::prelude::*;
use pyo3::types::PyType;
use quantforge::{StrategyError, TimestampMs};

use crate::candles::Candle;
use crate::convert::{text, type_name, wrong_type};
use crate::strategy::{Strategy, StrategyContext, TargetPosition};

/// A strategy written in Python, as an engine drives it.
#[derive(Debug)]
pub(crate) struct PyStrategy {
    strategy: Py<PyAny>,
    /// Read once: the engine borrows a name, and asks for it at any time.
    name: String,
    /// The exception that ended the run, until a host takes it.
    failure: Option<PyErr>,
}

/// The callback a failure happened in.
#[derive(Clone, Copy, Debug)]
enum Callback {
    Start,
    /// With the open time of its bar.
    Bar(TimestampMs),
    Finish,
}

impl PyStrategy {
    /// The engine's strategy over `strategy`, an object of a class derived
    /// from the package's `Strategy`. It is named by its `name`, a `str`,
    /// where it has one, and by its class otherwise.
    pub(crate) fn new(strategy: &Bound<'_, PyAny>) -> PyResult<Self> {
        let py = strategy.py();
        if !strategy.is_instance_of::<Strategy>() {
            // The slip of handing over a class in place of an object of it.
            if let Ok(class) = strategy.cast::<PyType>() {
                let base = py.get_type::<Strategy>();
                if class.is_subclass(&base)? && !class.is(&base) {
                    return Err(PyTypeError::new_err(format!(
                        "strategy must be a Strategy, not the class {}: build one from it",
                        class.name()?
                    )));
                }
            }
            return Err(wrong_type("strategy", "a Strategy", strategy));
        }
        let name = match strategy.getattr_opt(intern!(py, "name"))? {
            Some(name) => text(&name, "a strategy's name")?,
            None => strategy.get_type().name()?.to_string(),
        };
        Ok(Self {
            strategy: strategy.clone().unbind(),
            name,
            failure: None,
        })
    }

    /// The exception that ended the run, where a callback failed: the one
    /// it raised, as it was raised, for a host that is Python to raise
    /// again. It is handed over once.
    pub(crate) fn failure(&mut self) -> Option<PyErr> {
        self.failure.take()
    }

    fn started(&self, py: Python<'_>, ctx: &quantforge::StrategyContext) -> PyResult<()> {
        let returned = self
            .strategy
            .bind(py)
            .call_method1(intern!(py, "on_start"), (StrategyContext(ctx.clone()),))?;
        nothing(&returned, "on_start")
    }

    fn decided(
        &self,
        py: Python<'_>,
        ctx: &quantforge::StrategyContext,
        bar: &quantforge::Candle,
    ) -> PyResult<Option<quantforge::TargetPosition>> {
        let returned = self.strategy.bind(py).call_method1(
            intern!(py, "on_bar"),
            (StrategyContext(ctx.clone()), Candle(bar.clone())),
        )?;
        if returned.is_none() {
            return Ok(None);
        }
        match returned.cast::<TargetPosition>() {
            Ok(target) => Ok(Some(target.get().0)),
            Err(_) => Err(PyTypeError::new_err(format!(
                "on_bar must return a TargetPosition or None, not {}",
                type_name(&returned)
            ))),
        }
    }

    fn finished(&self, py: Python<'_>, ctx: &quantforge::StrategyContext) -> PyResult<()> {
        let returned = self
            .strategy
            .bind(py)
            .call_method1(intern!(py, "on_finish"), (StrategyContext(ctx.clone()),))?;
        nothing(&returned, "on_finish")
    }

    /// Keeps the exception `callback` failed with, and words the failure
    /// for the engine.
    fn failed(&mut self, py: Python<'_>, callback: Callback, error: PyErr) -> StrategyError {
        let name = &self.name;
        let said = match callback {
            Callback::Start => format!("the strategy {name:?} failed in on_start"),
            Callback::Bar(open_time_ms) => format!(
                "the strategy {name:?} failed in on_bar, at the bar that opened at {open_time_ms}"
            ),
            Callback::Finish => format!("the strategy {name:?} failed in on_finish"),
        };
        // Written before the note is added, which would repeat the line.
        let written = written(py, &error);
        // The exception is the strategy's own, and stays so: a class of
        // exception that takes no note is raised again without one.
        let _ = error.add_note(py, said.as_str());
        self.failure = Some(error);
        StrategyError::msg(format!("{said}\n{written}"))
    }
}

/// Holds what `on_start` or `on_finish` returned to `None`: a decision
/// returned from either would otherwise be dropped without a word.
fn nothing(returned: &Bound<'_, PyAny>, callback: &str) -> PyResult<()> {
    if returned.is_none() {
        return Ok(());
    }
    Err(PyTypeError::new_err(format!(
        "{callback} must return None, not {}",
        type_name(returned)
    )))
}

/// `error` as Python itself writes an exception: its traceback, the
/// exceptions it came from, its own line and its notes.
fn written(py: Python<'_>, error: &PyErr) -> String {
    let formatted = || -> PyResult<String> {
        // The traceback is handed over beside the exception: before Python
        // 3.12 an exception no `except` has caught does not hold its own.
        let lines = py.import(intern!(py, "traceback"))?.call_method1(
            intern!(py, "format_exception"),
            (error.get_type(py), error.value(py), error.traceback(py)),
        )?;
        let mut text = String::new();
        for line in lines.try_iter()? {
            text.push_str(&line?.extract::<String>()?);
        }
        Ok(text)
    };
    match formatted() {
        Ok(text) => text.trim_end().to_string(),
        // Its last line alone, where Python could not write the rest.
        Err(_) => error.to_string(),
    }
}

impl quantforge::Strategy for PyStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn on_start(&mut self, ctx: &quantforge::StrategyContext) -> Result<(), StrategyError> {
        Python::attach(|py| {
            self.started(py, ctx)
                .map_err(|error| self.failed(py, Callback::Start, error))
        })
    }

    fn on_bar(
        &mut self,
        ctx: &quantforge::StrategyContext,
        bar: &quantforge::Candle,
    ) -> Result<Option<quantforge::TargetPosition>, StrategyError> {
        Python::attach(|py| {
            self.decided(py, ctx, bar)
                .map_err(|error| self.failed(py, Callback::Bar(bar.open_time_ms), error))
        })
    }

    fn on_finish(&mut self, ctx: &quantforge::StrategyContext) -> Result<(), StrategyError> {
        Python::attach(|py| {
            self.finished(py, ctx)
                .map_err(|error| self.failed(py, Callback::Finish, error))
        })
    }
}
