//! Market identity: a symbol, an interval, a product family, a product
//! line, and the market that a line, a symbol and an interval name. Each
//! class holds the engine's own value, is built by the engine's own rule,
//! and is refused in the engine's own words.

use pyo3::PyClass;
use pyo3::prelude::*;
use pyo3::types::{PyString, PyTuple};

use crate::convert::{refused, repr_of, text, wrong_type};

/// A trading symbol, such as `BTCUSDT`.
///
/// `Symbol(value)` trims the text and writes it in capitals, as the engine
/// does everywhere, and refuses text that holds nothing with `ValueError:
/// invalid symbol: empty`. `str()` gives the symbol's text.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Symbol(pub(crate) quantforge::Symbol);

#[pymethods]
impl Symbol {
    #[new]
    #[pyo3(signature = (value, /))]
    fn new(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        quantforge::Symbol::new(text(value, "a symbol")?)
            .map(Self)
            .map_err(refused)
    }

    fn __str__(&self) -> &str {
        self.0.as_str()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        repr_of(
            "Symbol",
            &[("", PyString::new(py, self.0.as_str()).into_any())],
        )
    }
}

impl Symbol {
    /// The symbol an argument names: a `Symbol`, or the text of one.
    pub(crate) fn taken(value: &Bound<'_, PyAny>, what: &str) -> PyResult<quantforge::Symbol> {
        if let Ok(symbol) = value.cast::<Self>() {
            return Ok(symbol.get().0.clone());
        }
        match value.cast::<PyString>() {
            Ok(text) => quantforge::Symbol::new(text.to_cow()?).map_err(refused),
            Err(_) => Err(wrong_type(what, "a Symbol or a str", value)),
        }
    }
}

/// A candle interval, such as `1m` or `8h`.
///
/// `Interval(value)` reads the interval's text, with the space around it
/// trimmed, and refuses any other with `ValueError: invalid interval: 7m`.
/// Each interval is also a constant of the class, named as the engine names
/// it, such as `Interval.M1` and `Interval.H8`; `Interval.ALL` holds them
/// all, shortest first. `str()` gives the interval's text.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Interval(pub(crate) quantforge::Interval);

#[pymethods]
impl Interval {
    #[new]
    #[pyo3(signature = (value, /))]
    fn new(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        text(value, "an interval")?
            .parse()
            .map(Self)
            .map_err(refused)
    }

    /// The length of one candle, in milliseconds.
    #[getter]
    fn step_ms(&self) -> i64 {
        self.0.step_ms()
    }

    fn __str__(&self) -> &'static str {
        self.0.as_str()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        repr_of(
            "Interval",
            &[("", PyString::new(py, self.0.as_str()).into_any())],
        )
    }
}

impl Interval {
    /// The interval an argument names: an `Interval`, or the text of one.
    pub(crate) fn taken(value: &Bound<'_, PyAny>, what: &str) -> PyResult<quantforge::Interval> {
        if let Ok(interval) = value.cast::<Self>() {
            return Ok(interval.get().0);
        }
        match value.cast::<PyString>() {
            Ok(text) => text.to_cow()?.parse().map_err(refused),
            Err(_) => Err(wrong_type(what, "an Interval or a str", value)),
        }
    }
}

/// A product family: `spot`, `linear` or `inverse`.
///
/// `Category(value)` reads that name exactly, and refuses any other with a
/// `ValueError` that lists the three. Each family is also a constant of the
/// class, its name in capitals; `Category.ALL` holds them, Spot first.
/// `str()` gives the family's name.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Category(pub(crate) quantforge::Category);

#[pymethods]
impl Category {
    #[new]
    #[pyo3(signature = (value, /))]
    fn new(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        text(value, "a category")?
            .parse()
            .map(Self)
            .map_err(refused)
    }

    /// Whether the family trades futures contracts rather than inventory.
    #[getter]
    fn is_futures(&self) -> bool {
        self.0.is_futures()
    }

    fn __str__(&self) -> &'static str {
        self.0.as_str()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        repr_of(
            "Category",
            &[("", PyString::new(py, self.0.as_str()).into_any())],
        )
    }
}

/// A product line: one venue's Spot, linear or inverse market, by the name
/// its candles are stored under, such as `binance_spot`.
///
/// `ExchangeId(value)` reads that name exactly, and refuses any other with a
/// `ValueError` that lists the lines there are. A venue's name alone is not
/// a product line: nothing takes it for the venue's Spot line. Each line is
/// also a constant of the class, its name in capitals; `ExchangeId.ALL`
/// holds them all. `str()` gives the line's name.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ExchangeId(pub(crate) quantforge::ExchangeId);

#[pymethods]
impl ExchangeId {
    #[new]
    #[pyo3(signature = (value, /))]
    fn new(value: &Bound<'_, PyAny>) -> PyResult<Self> {
        text(value, "an exchange")?
            .parse()
            .map(Self)
            .map_err(refused)
    }

    /// The line's product family.
    #[getter]
    fn category(&self) -> Category {
        Category(self.0.category())
    }

    fn __str__(&self) -> &'static str {
        self.0.as_str()
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        repr_of(
            "ExchangeId",
            &[("", PyString::new(py, self.0.as_str()).into_any())],
        )
    }
}

impl ExchangeId {
    /// The line an argument names: an `ExchangeId`, or the text of one.
    fn taken(value: &Bound<'_, PyAny>, what: &str) -> PyResult<quantforge::ExchangeId> {
        if let Ok(exchange) = value.cast::<Self>() {
            return Ok(exchange.get().0);
        }
        match value.cast::<PyString>() {
            Ok(text) => text.to_cow()?.parse().map_err(refused),
            Err(_) => Err(wrong_type(what, "an ExchangeId or a str", value)),
        }
    }
}

/// The key market data is stored and served under: a product line, a
/// symbol and an interval.
///
/// `MarketId(exchange, symbol, interval)` takes each part as its own type or
/// as the text of one, read by that type's rule, so a bad part is refused
/// with that type's `ValueError`. The market's product family is its
/// product line's, and is never assumed.
#[pyclass(frozen, eq, hash, module = "quantforge_py")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MarketId(pub(crate) quantforge::MarketId);

#[pymethods]
impl MarketId {
    #[new]
    fn new(
        exchange: &Bound<'_, PyAny>,
        symbol: &Bound<'_, PyAny>,
        interval: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        Ok(Self(quantforge::MarketId::new(
            ExchangeId::taken(exchange, "exchange")?,
            Symbol::taken(symbol, "symbol")?,
            Interval::taken(interval, "interval")?,
        )))
    }

    /// The product line the market trades on.
    #[getter]
    fn exchange(&self) -> ExchangeId {
        ExchangeId(self.0.exchange)
    }

    /// The market's symbol.
    #[getter]
    fn symbol(&self) -> Symbol {
        Symbol(self.0.symbol.clone())
    }

    /// The interval of the market's candles.
    #[getter]
    fn interval(&self) -> Interval {
        Interval(self.0.interval)
    }

    /// The product family the market belongs to: its product line's.
    #[getter]
    fn category(&self) -> Category {
        Category(self.0.category())
    }

    fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
        let py = slf.py();
        let market = slf.get();
        repr_of(
            "MarketId",
            &[
                ("", Bound::new(py, market.exchange())?.into_any()),
                ("", Bound::new(py, market.symbol())?.into_any()),
                ("", Bound::new(py, market.interval())?.into_any()),
            ],
        )
    }
}

/// Gives a class one constant for each of `values`, named by `name`, and
/// `ALL`, a tuple of them in the order given.
pub(crate) fn name_each<T: PyClass>(
    py: Python<'_>,
    values: impl IntoIterator<Item = (String, T)>,
) -> PyResult<()>
where
    PyClassInitializer<T>: From<T>,
{
    let class = py.get_type::<T>();
    let mut all = Vec::new();
    for (name, value) in values {
        let constant = Bound::new(py, value)?;
        class.setattr(name.as_str(), &constant)?;
        all.push(constant);
    }
    class.setattr("ALL", PyTuple::new(py, all)?)
}

/// Names every interval, product family and product line the engine has as
/// a constant of its class. They are listed from the engine, never here, so
/// the classes cannot fall behind it.
pub(crate) fn name_constants(py: Python<'_>) -> PyResult<()> {
    // An interval by its variant's name, `M1` or `H8`: the engine's tests
    // hold that to what `Debug` prints.
    name_each(
        py,
        quantforge::Interval::ALL.map(|interval| (format!("{interval:?}"), Interval(interval))),
    )?;
    let capitals = |name: &str| name.to_ascii_uppercase();
    name_each(
        py,
        quantforge::Category::ALL.map(|category| (capitals(category.as_str()), Category(category))),
    )?;
    name_each(
        py,
        quantforge::ExchangeId::all()
            .map(|exchange| (capitals(exchange.as_str()), ExchangeId(exchange))),
    )
}
