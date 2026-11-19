"""Candles and a backtest's result as pandas DataFrames, and a frame of
candles as a batch: `candles_to_dataframe`, `dataframe_to_candles`,
`trades_to_dataframe` and `equity_to_dataframe`, which the package publishes
(`docs/python-pandas.md`).

A frame is put together and taken apart here, with pandas' own calls, and
holds nothing Python worked out:

- **A price, a volume and an amount are float64,** by the engine's own
  conversion and no other. A frame of candles is made of the arrays
  `CandleBatch.to_numpy` gives and read back through
  `CandleBatch.from_numpy`, and a result's columns are made in the
  extension the same way (`docs/python-numpy.md`).
- **A time is the engine's own milliseconds,** shown as a UTC time of
  millisecond resolution and never rounded on the way.

pandas and NumPy are the package's extras, and neither is imported before
one of these functions is called. This module is private: import the four
from `quantforge_py`.
"""

from . import _native
from ._native import CandleBatch, MarketId

# A frame of candles: the name of its index, and its columns in order.
_OPEN_TIME = "open_time"
_CLOSE_TIME = "close_time"
_FLOATS = ("open", "high", "low", "close", "volume")
_TRADES = "trades"

# The one integer of 64 bits that is no time to pandas: it is how NaT, "not a
# time", is kept.
_NO_TIME = -(2**63)

# The largest whole number of seconds whose milliseconds fit 64 bits.
_MOST_SECONDS = (2**63 - 1) // 1000

# How many of a finer unit make a millisecond.
_PER_MILLISECOND = {"us": 1_000, "ns": 1_000_000}

_NO_ZONE = "a time without a zone is never read as an instant; name its zone with tz_localize"


def _wrong_type(what, takes, value):
    """A value of a type `what` does not take, worded as the extension
    words one."""
    return TypeError(f"{what} must be {takes}, not {type(value).__name__}")


def _refuse_first(np, wrong, said):
    """Refuses the first row that `wrong` flags, with what `said` makes of
    its number."""
    rows = np.flatnonzero(wrong)
    if rows.size:
        raise ValueError(said(int(rows[0])))


def _times(np, pd, milliseconds, column, name=None):
    """A column of the engine's times as pandas' UTC times of the same
    milliseconds: `datetime64[ms, UTC]`, one for each, and none rounded.

    `milliseconds` is an int64 array of milliseconds since the Unix epoch,
    which `column` names in a refusal. Every integer of 64 bits is a time
    to the engine and all but one are to pandas: the lowest is how pandas
    keeps a time that is missing, and is refused, never read as one.
    """
    _refuse_first(
        np,
        milliseconds == _NO_TIME,
        lambda row: f"invalid {column}[{row}] {_NO_TIME}: pandas reads it as no time at all (NaT)",
    )
    # The integers themselves, read as milliseconds: nothing is converted,
    # so nothing depends on the unit a version of pandas would infer.
    return pd.DatetimeIndex(milliseconds.view("datetime64[ms]"), name=name).tz_localize("UTC")


def _frame(pd, columns, index=None):
    """The frame of `columns`, under their names and in their order, with
    `index` or numbered from 0.

    The names of its columns are text of Python's own, of the type
    `object`, on every pandas. What pandas makes of text by default changed
    with pandas 3, for the names of a frame's columns as for a column of
    text, and follows an option of pandas' besides: a frame's names follow
    neither. So the frame is made of its columns by their places, and is
    given their names once it is made: pandas is never left to say what
    the names are.
    """
    frame = pd.DataFrame(dict(enumerate(columns.values())), index=index)
    frame.columns = pd.Index(list(columns), dtype=object)
    return frame


def _milliseconds(np, times, name):
    """The engine's milliseconds of pandas' times, exactly: an int64 array
    of milliseconds since the Unix epoch.

    `times` is a `pandas.DatetimeIndex` with a time zone, of any zone and
    any of pandas' units, which `name` names in a refusal. A time in
    another zone is the same instant, and is read as it. One that is
    missing, one that is not a whole number of milliseconds and one whose
    milliseconds do not fit 64 bits are refused, never rounded.
    """
    held = times.tz_convert(None).to_numpy()
    unit, _ = np.datetime_data(held.dtype)
    held = held.view("int64")
    # The first time that is not taken is the one refused, whatever it is
    # refused for: every rule is asked of all of them before any is named.
    # A missing time is the lowest integer. In seconds it lies beyond what
    # fits, and in a finer unit it is no whole number of milliseconds (two
    # to the 63rd has no factor of five), so each rule below flags it too:
    # it is named as missing, which is asked of a row before its rule.
    missing = held == _NO_TIME
    if unit == "ms":
        whole, wrong, rule = held, missing, None
    elif unit == "s":
        wrong = (held > _MOST_SECONDS) | (held < -_MOST_SECONDS)
        whole, rule = None, "its milliseconds must fit 64 bits"
    else:
        whole, rest = np.divmod(held, _PER_MILLISECOND[unit])
        wrong, rule = rest != 0, "must be a whole number of milliseconds"
    rows = np.flatnonzero(wrong)
    if rows.size:
        row = int(rows[0])
        if missing[row]:
            raise ValueError(f"invalid {name}[{row}] NaT: a candle's time is never missing")
        raise ValueError(f"invalid {name}[{row}] {np.datetime64(int(held[row]), unit)} UTC: {rule}")
    return held * 1000 if unit == "s" else whole


def _column(frame, name):
    """The one column of `frame` that is named `name`.

    It is found among the names that are text, and taken by its place: a
    frame may name two columns alike, and one under a name that is no text
    is no column of a candle.
    """
    places = [
        place
        for place, label in enumerate(frame.columns)
        if isinstance(label, str) and label == name
    ]
    if not places:
        raise ValueError(f"frame has no column named {name}")
    if len(places) > 1:
        raise ValueError(f"frame has {len(places)} columns named {name}")
    return frame.iloc[:, places[0]]


def candles_to_dataframe(candles, /):
    """The candles of a batch as a pandas DataFrame, for computing with:
    `candles_to_dataframe(candles)`.

    `candles` is a `CandleBatch`. The frame has one row for each candle, in
    the batch's order, and is new each time: changing it changes nothing in
    the batch.

    Index: `open_time`, a `DatetimeIndex` of `datetime64[ms, UTC]`: when
    each bar opened.

    Columns, in this order:

    - `close_time`: `datetime64[ms, UTC]`, when the bar closed.
    - `open`, `high`, `low`, `close`: `float64`, the bar's prices, in the
      quote asset for one of the base asset.
    - `volume`: `float64`, the volume traded, in the base asset.
    - `trades`: `UInt64`, the exchange's trade count, and `<NA>` where it
      reported none: never NaN, 0 or -1.

    A time is the engine's own milliseconds since the Unix epoch, exactly,
    shown as a UTC time. A price and a volume are the float nearest to
    their decimal, by the engine's own conversion, as
    `CandleBatch.to_numpy` gives them: floats are for computing with, and
    the exact decimals stay in the batch.

    The frame holds no market, and its `attrs` are empty: the market stays
    with the batch. Give it to `dataframe_to_candles` by `market=` to build
    the batch again.

    It needs pandas and NumPy, which `pip install "quantforge-py[pandas]"`
    installs with the package, and raises an `ImportError` that says so
    where either is missing. A `candles` that is no `CandleBatch` is a
    `TypeError`, and a time pandas cannot hold, the lowest integer of 64
    bits, a `ValueError`.
    """
    pd = _native._pandas()
    np = _native._numpy()
    if type(candles) is not CandleBatch:
        raise _wrong_type("candles", "a CandleBatch", candles)
    arrays = candles.to_numpy()
    index = _times(np, pd, arrays["open_time_ms"], "open_time_ms", _OPEN_TIME)
    columns = {_CLOSE_TIME: _times(np, pd, arrays["close_time_ms"], "close_time_ms")}
    for name in _FLOATS:
        columns[name] = arrays[name]
    # A masked count is a missing one: the flags of the mask are pandas' own
    # flags of what is missing, and no number stands for it.
    counts = arrays[_TRADES]
    columns[_TRADES] = pd.arrays.IntegerArray(np.ma.getdata(counts), np.ma.getmaskarray(counts))
    return _frame(pd, columns, index)


def dataframe_to_candles(frame, /, *, market=None):
    """The batch of a pandas DataFrame of candles:
    `dataframe_to_candles(frame, *, market=None)`.

    `frame` is a `pandas.DataFrame` as `candles_to_dataframe` gives one,
    with one row for each candle, and `market` the `MarketId` the candles
    are of, or `None`: the frame names none, and nothing is read from it or
    assumed for it.

    Index: `open_time`, a `DatetimeIndex` with a time zone: when each bar
    opened.

    Columns, found by name, in any order:

    - `close_time`: `datetime64` with a time zone, when the bar closed.
    - `open`, `high`, `low`, `close`, `volume`: `float64`.
    - `trades`: `UInt64`, with `<NA>` where the count is unknown, or
      `uint64`, where every count is known.

    Any other column is left unread, so a frame that gained columns of its
    reader's own still gives its candles back. Nothing is cast: an int64
    column is no column of prices, and a count kept as a float with NaN for
    the unknown is no count.

    A time is an instant. It may be in any time zone and in any of pandas'
    units, and is read as the engine's milliseconds since the Unix epoch,
    exactly: one without a time zone is refused, since no zone is assumed
    for it, and so is one that is missing, one that is not a whole number
    of milliseconds, and one in seconds whose milliseconds do not fit 64
    bits. A float is read as the decimal of its shortest text, as
    `CandleBatch.from_numpy` reads one, and refused by its row where it has
    none: NaN, an infinity, or a float beyond the engine's decimal. A
    batch that went to a frame comes back the same batch by value where
    floats hold its decimals, which they do up to 15 significant digits.

    The frame is read and not kept: changing it afterwards changes nothing
    in the batch.

    It needs pandas and NumPy, which `pip install "quantforge-py[pandas]"`
    installs with the package, and raises an `ImportError` that says so
    where either is missing. A frame, an index or a column of a type it
    does not take is a `TypeError`, times without a time zone among them;
    an index of another name, a column that is missing or there twice, a
    time that is missing, is no whole millisecond or does not fit, and a
    float that has no decimal are a `ValueError`.
    """
    pd = _native._pandas()
    np = _native._numpy()
    # A frame of a type derived from pandas' own may mean something its
    # cells do not say, and is read through that type's code:
    # `pandas.DataFrame(frame)` gives the plain frame of one.
    if type(frame) is not pd.DataFrame:
        raise _wrong_type("frame", "a pandas.DataFrame", frame)
    # Everything is held to its type, in the columns' order and the market
    # last, before a value is read.
    opened = frame.index
    if type(opened) is not pd.DatetimeIndex:
        raise TypeError(
            f"the index must be a pandas.DatetimeIndex named {_OPEN_TIME}, "
            f"not {type(opened).__name__}"
        )
    # A name that is no text is another name, and is not compared: pandas'
    # own missing value answers no comparison.
    if not (isinstance(opened.name, str) and opened.name == _OPEN_TIME):
        raise ValueError(f"the index must be named {_OPEN_TIME}, not {opened.name!r}")
    if opened.tz is None:
        raise TypeError(
            f"{_OPEN_TIME} must be an index of datetime64 with a time zone, "
            f"not of {opened.dtype}: {_NO_ZONE}"
        )
    closed = _column(frame, _CLOSE_TIME)
    if not isinstance(closed.dtype, pd.DatetimeTZDtype):
        said = (
            f"{_CLOSE_TIME} must be a column of datetime64 with a time zone, not of {closed.dtype}"
        )
        if isinstance(closed.dtype, np.dtype) and closed.dtype.kind == "M":
            said = f"{said}: {_NO_ZONE}"
        raise TypeError(said)
    floats = {}
    for name in _FLOATS:
        column = _column(frame, name)
        if not (isinstance(column.dtype, np.dtype) and column.dtype == np.float64):
            raise TypeError(f"{name} must be a column of float64, not of {column.dtype}")
        floats[name] = column
    counts = _column(frame, _TRADES)
    masked = isinstance(counts.dtype, pd.UInt64Dtype)
    if not masked and not (isinstance(counts.dtype, np.dtype) and counts.dtype == np.uint64):
        raise TypeError(f"{_TRADES} must be a column of UInt64 or of uint64, not of {counts.dtype}")
    if market is not None and type(market) is not MarketId:
        raise _wrong_type("market", "a MarketId or None", market)
    arrays = {
        "open_time_ms": _milliseconds(np, opened, _OPEN_TIME),
        "close_time_ms": _milliseconds(np, pd.DatetimeIndex(closed), _CLOSE_TIME),
    }
    for name, column in floats.items():
        arrays[name] = column.to_numpy()
    if masked:
        # A missing count is a masked one. What is put under its mask is
        # never read as a count.
        known = counts.to_numpy(dtype=np.uint64, na_value=0)
        arrays[_TRADES] = np.ma.MaskedArray(known, mask=counts.isna().to_numpy())
    else:
        arrays[_TRADES] = counts.to_numpy()
    return CandleBatch.from_numpy(**arrays, market=market)


def trades_to_dataframe(result, /):
    """The closed trades of a backtest's result as a pandas DataFrame, for
    computing with: `trades_to_dataframe(result)`.

    `result` is a `BacktestResult`. The frame has one row for each of its
    trades, in the result's order, and every field of a `ClosedTrade` as a
    column, under the field's name but for the two times.

    Index: a `RangeIndex` from 0, with no name: row `i` is
    `result.trades[i]`.

    Columns, in this order:

    - `symbol`: `object`, the symbol traded, as text.
    - `entry_time`, `exit_time`: `datetime64[ms, UTC]`, when the entry and
      the exit filled.
    - `entry_price`, `exit_price`: `float64`, the prices they filled at, in
      the quote asset for one of the base asset.
    - `qty`: `float64`, the quantity traded, in the base asset.
    - `gross_quote_pnl`: `float64`, the trade's PnL in the quote asset. It
      is Spot's PnL field, with the meanings it has in the engine: in a
      backtest the change in cash over the trade, with both fees already
      taken off. It is not a futures PnL.
    - `entry_order_id`, `exit_order_id`: `object`, the venue's order IDs as
      text, and `None` where no order reached a venue, as in a backtest.

    A time is the engine's own milliseconds since the Unix epoch, exactly,
    shown as a UTC time. An amount is the float nearest to its decimal, by
    the engine's own conversion: floats are for computing with, and the
    exact decimals stay in the result.

    It needs pandas and NumPy, which `pip install "quantforge-py[pandas]"`
    installs with the package, and raises an `ImportError` that says so
    where either is missing. A `result` that is no `BacktestResult` is a
    `TypeError`.
    """
    pd = _native._pandas()
    columns = _native._trade_columns(result)
    np = _native._numpy()
    # Text is kept as Python's own, `str` or `None`, on every pandas: what a
    # column of text is by default changed with pandas 3.
    return _frame(
        pd,
        {
            "symbol": pd.Series(columns["symbol"], dtype=object),
            "entry_time": _times(np, pd, columns["entry_time_ms"], "entry_time_ms"),
            "exit_time": _times(np, pd, columns["exit_time_ms"], "exit_time_ms"),
            "entry_price": columns["entry_price"],
            "exit_price": columns["exit_price"],
            "qty": columns["qty"],
            "gross_quote_pnl": columns["gross_quote_pnl"],
            "entry_order_id": pd.Series(columns["entry_order_id"], dtype=object),
            "exit_order_id": pd.Series(columns["exit_order_id"], dtype=object),
        },
    )


def equity_to_dataframe(result, /):
    """The equity curve of a backtest's result as a pandas DataFrame, for
    computing with: `equity_to_dataframe(result)`.

    `result` is a `BacktestResult`. The frame has one row for each point of
    its curve, which is one for every bar of the run, in order.

    Index: `open_time`, a `DatetimeIndex` of `datetime64[ms, UTC]`: the
    open time of the bar, which names it, as the index of a frame of
    candles does.

    Columns:

    - `equity`: `float64`, cash plus the position valued at the bar's
      close, after the bar's fill, in the quote currency.

    A time is the engine's own milliseconds since the Unix epoch, exactly,
    shown as a UTC time. The equity is the float nearest to its decimal, by
    the engine's own conversion: floats are for computing with, and the
    exact decimals stay in the result.

    It needs pandas and NumPy, which `pip install "quantforge-py[pandas]"`
    installs with the package, and raises an `ImportError` that says so
    where either is missing. A `result` that is no `BacktestResult` is a
    `TypeError`.
    """
    pd = _native._pandas()
    columns = _native._equity_columns(result)
    np = _native._numpy()
    index = _times(np, pd, columns["open_time_ms"], "open_time_ms", _OPEN_TIME)
    return _frame(pd, {"equity": columns["equity"]}, index)
