"""What a succeeded job produced, as a client holds it: `JobResult` and
`ResultFile`, which the package publishes; a result read strictly from what
the protocol writes; and a table downloaded, held to the hash its job lists
for it, and read by the engine's own reader of the result format
(`docs/python-jobs.md`, `docs/result-spec.md`).

A job that has succeeded has three files: its metrics, its closed trades
and its equity curve. A client reads them in two steps:

1. **`GET /v1/backtests/{job_id}/result`:** the metrics, and each file's
   hash and size. The metrics are held to the hash of `metrics.json`: the
   client writes them as the result format writes that file, by the
   engine's own writer, and what it wrote has to be the file the job
   lists. So the metrics a result gives are the ones the file holds,
   though the file is never downloaded.
2. **`GET /v1/backtests/{job_id}/result/<file>`,** for a table, the first
   time something asks for it and not before: read to the size the job
   lists and no further, held to that size and to the hash, and then read
   by the engine's own reader, which refuses a file that is not byte for
   byte what the result format writes for its rows.

A table is read into the engine's own types, each amount an exact decimal
with the digits the server wrote, and a frame is made of it by the function
that makes one of a local run's result. So a job's frames are a local
run's, column for column, and its exact values are where a local run's
are, in a `BacktestResult` (`docs/python-pandas.md`).

This module is private: import its names from `quantforge_py`.
"""

from . import _native
from ._errors import RemoteError, _published
from ._jobs import (
    _count,
    _final,
    _given,
    _is_content_hash,
    _is_job_id,
    _not_writable,
    _Record,
)
from ._native import BacktestResult
from ._wire import _member, _object, _quoted, _Unreadable

# The result format this version of the package reads.
_FORMAT = 1

# A result's files, as the result format names them, in the order a job
# lists them: what each holds, the file it is fetched as, and its media
# type.
_ARROW = "application/vnd.apache.arrow.file"
_FILES = (
    ("metrics", "metrics.json", "application/json"),
    ("trades", "trades.arrow", _ARROW),
    ("equity", "equity.arrow", _ARROW),
)

# The amounts of a result's metrics, in the order the result format writes
# them. The number of closed trades follows them.
_AMOUNTS = ("initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct")
_TRADES = "trade_count"

# The longest result file that is downloaded, in bytes. A year of one-minute
# bars, the most a server runs one job on unless it is told otherwise, makes
# an equity curve of a fiftieth of this: 42 bytes a bar, where every amount
# has as many digits as the engine's decimal holds.
_MOST_FILE_BYTES = 1024 * 1024 * 1024


def _hash_of(file):
    """The name of bytes as stored content: `sha256:` and their SHA-256."""
    import hashlib

    return f"sha256:{hashlib.sha256(file).hexdigest()}"


@_final
@_published
class ResultFile(_Record):
    """One result file of a job, as its server lists it: what it holds, and
    the bytes it is, by their hash and their number.

    A `JobResult` and a `Provenance` give one for each file, and none is
    built. It is a value, and never changes: two are equal when every field
    is.

    - `name`: what the file holds: `"metrics"`, `"trades"` or `"equity"`.
    - `media_type`: the file's media type, such as `"application/json"`.
    - `hash`: the file's name as stored content: `sha256:` and the SHA-256
      of its bytes, in hex. The same result is the same bytes on every
      server and every store, so the hash names what the job produced.
    - `size_bytes`: how many bytes the file holds.
    """

    __slots__ = ("_name", "_media_type", "_hash", "_size_bytes")

    _BUILT = (
        "a ResultFile cannot be built: a JobResult and a Provenance give one for each file a "
        "job produced"
    )

    name = _given("_name", "What the file holds: metrics, trades or equity.")
    media_type = _given("_media_type", "The file's media type.")
    hash = _given("_hash", "The file's name as stored content: the SHA-256 of its bytes.")
    size_bytes = _given("_size_bytes", "How many bytes the file holds.")

    def __repr__(self):
        return f"<ResultFile {self._name} {self._hash} of {self._size_bytes} bytes>"


def _file(value, what):
    """One result file as the protocol lists it, read strictly."""
    record = _object(value, what)
    name = _member(record, "name", str, what)
    named = _member(record, "hash", str, what)
    if not _is_content_hash(named):
        raise _Unreadable(f"the `hash` of {what} is no content hash")
    return ResultFile._of(
        name=name,
        media_type=_member(record, "media_type", str, what),
        hash=named,
        size_bytes=_count(record, "size_bytes", what),
    )


def _files(listed, what):
    """The three files of the result format among the ones a job lists, in
    the format's order, each once and of the media type the format gives
    it. A file a later revision of the protocol lists beside them is none
    of this format's, and is left out."""
    found = {}
    for each in listed:
        file = _file(each, f"a file of {what}")
        if file.name in found:
            raise _Unreadable(f"{what} lists its {_quoted(file.name)} file twice")
        found[file.name] = file
    for name, _, media_type in _FILES:
        if name not in found:
            raise _Unreadable(f"{what} lists no {name} file")
        if found[name].media_type != media_type:
            raise _Unreadable(
                f"{what} lists its {name} file as {_quoted(found[name].media_type)}, and the "
                f"result format writes it as {media_type}"
            )
    return tuple(found[name] for name, _, _ in _FILES)


def _listing(job_id):
    """Reads the answer of a job's result route: the metrics, and the three
    files, of the job `job_id` and of no other, in the format this version
    reads, the metrics held to the hash of the file that holds them."""

    def read(value):
        import decimal

        what = "a job's result"
        record = _object(value, what)
        named = _member(record, "job_id", str, what)
        if not _is_job_id(named):
            raise _Unreadable(f"the `job_id` of {what} is no job ID")
        if named != job_id:
            raise _Unreadable(f"it is the result of job {named}, not of job {job_id}")
        what = f"job {job_id}'s result"
        number = _member(record, "format", int, what)
        if number != _FORMAT:
            raise _Unreadable(
                f"it is a result of format {number}, and this version of the package reads "
                f"format {_FORMAT}"
            )
        metrics = _member(record, "metrics", dict, what)
        what = f"job {job_id}'s metrics"
        amounts = [_member(metrics, name, str, what) for name in _AMOUNTS]
        trades = _count(metrics, _TRADES, what)
        try:
            # As the result format writes them, by the engine's own writer,
            # which reads each amount by the engine's own rule first.
            written = _native._result_metrics(*amounts, trades)
        except ValueError as refused:
            raise _Unreadable(f"{what} hold an amount the engine does not ({refused})") from None
        what = f"job {job_id}'s result"
        files = _files(_member(record, "results", list, what), what)
        listed = files[0]
        if (len(written), _hash_of(written)) != (listed.size_bytes, listed.hash):
            raise _Unreadable(
                f"job {job_id}'s metrics are not the ones its metrics.json holds: written as "
                f"that file they are {len(written)} bytes hashing to {_hash_of(written)}, and the "
                f"file is listed as {listed.size_bytes} bytes hashing to {listed.hash}"
            )
        return (*map(decimal.Decimal, amounts), trades), files

    return read


def _table(job_id, file, read, trades=None):
    """Reads a table's file as it was downloaded: held to the size and the
    hash the job lists for it, then read by `read`, the engine's own reader
    of it. `trades` is how many rows the job's metrics say a table of
    trades has."""

    def rows(raw):
        held = _hash_of(raw)
        if (len(raw), held) != (file.size_bytes, file.hash):
            raise _Unreadable(
                f"it is {len(raw)} bytes hashing to {held}, and job {job_id}'s result lists its "
                f"{file.name} file as {file.size_bytes} bytes hashing to {file.hash}"
            )
        try:
            table = read(raw)
        except ValueError as refused:
            raise _Unreadable(f"it is not what the result format writes ({refused})") from None
        if trades is not None and len(table) != trades:
            raise _Unreadable(
                f"it holds {len(table)} trades, and job {job_id}'s metrics count {trades}"
            )
        return table

    return rows


def _result(wire, job_id):
    """Asks a server for the result of the job `job_id`, and gives it."""
    metrics, files = wire.ask("GET", f"backtests/{job_id}/result", _listing(job_id))
    result = object.__new__(JobResult)
    for name, value in zip(JobResult.__slots__, (wire, job_id, metrics, files, {})):
        object.__setattr__(result, name, value)
    return result


@_final
@_published
class JobResult:
    """What a succeeded job produced: its metrics, and its closed trades
    and its equity curve, as a client holds them.

    `Job.result()` gives one, and none is built. Its metrics came with it.
    Each of its two tables is downloaded the first time something asks for
    it, checked against the hash its job lists, read by the engine's own
    reader of the result format, and kept: `trades_df`, `equity_df` and
    `to_backtest_result` ask the server for what they need and do not hold
    yet, and reading an attribute never does.

    - `job_id`: the job's ID.
    - `metrics`: the job's summary, a dict of the fields `metrics.json`
      holds, in its order: `initial_cash`, `final_equity`,
      `total_return_pct` and `max_drawdown_pct`, each an exact
      `decimal.Decimal` with the digits the server wrote, and
      `trade_count`, an `int`. The dict is new each time it is read.
    - `files`: the job's three result files, a tuple of `ResultFile`:
      `metrics`, `trades` and `equity`, each with the hash and the size of
      its bytes.
    - `trades_df()` and `equity_df()`: the closed trades and the equity
      curve as pandas DataFrames, column for column the frames
      `trades_to_dataframe` and `equity_to_dataframe` give of a local
      run's result.
    - `to_backtest_result()`: the result as a `BacktestResult`, the type a
      local run gives, with every amount exact.

    A result is not a value: it equals itself alone, and is not copied or
    pickled.
    """

    __slots__ = ("_wire", "_job_id", "_metrics", "_files", "_tables")

    def __init__(self, *args, **kwargs):
        raise TypeError(
            "a JobResult cannot be built: Job.result() gives the result of a job that has succeeded"
        )

    def __setattr__(self, name, value):
        _not_writable(self, name)

    def __delattr__(self, name):
        _not_writable(self, name)

    @property
    def job_id(self):
        """The job's ID."""
        return self._job_id

    @property
    def metrics(self):
        """The job's summary: a dict of the fields `metrics.json` holds, in
        its order, each amount an exact Decimal and `trade_count` an int."""
        return dict(zip((*_AMOUNTS, _TRADES), self._metrics))

    @property
    def files(self):
        """The job's three result files, a tuple of ResultFile: metrics,
        trades and equity."""
        return self._files

    def _rows(self, name):
        """The rows of one of the two tables, downloaded and checked the
        first time they are asked for, and kept."""
        rows = self._tables.get(name)
        if rows is not None:
            return rows
        (file,) = (file for file in self._files if file.name == name)
        (file_name,) = (file_name for held, file_name, _ in _FILES if held == name)
        if file.size_bytes > _MOST_FILE_BYTES:
            raise RemoteError(
                f"job {self._job_id}'s result lists its {name} file as {file.size_bytes} bytes, "
                f"and the client downloads a result file of at most {_MOST_FILE_BYTES}; read it "
                "with a tool that reads Arrow files from the server's own route"
            )
        if name == "trades":
            read = _table(self._job_id, file, _native._result_trades, self._metrics[-1])
        else:
            read = _table(self._job_id, file, _native._result_equity)
        rows = self._wire.fetch(
            f"backtests/{self._job_id}/result/{file_name}",
            read,
            media_type=file.media_type,
            most=file.size_bytes,
            held_to=(
                f"the {file.size_bytes} bytes job {self._job_id}'s result lists for its {name} file"
            ),
        )
        # Two threads that ask at the same time each download it, and
        # either's serves.
        self._tables[name] = rows
        return rows

    def _backtest(self, trades=(), equity_curve=()):
        """The engine's own result of these metrics and these rows."""
        initial_cash, final_equity, total_return_pct, max_drawdown_pct, count = self._metrics
        return BacktestResult(
            initial_cash=initial_cash,
            final_equity=final_equity,
            total_return_pct=total_return_pct,
            trade_count=count,
            max_drawdown_pct=max_drawdown_pct,
            trades=trades,
            equity_curve=equity_curve,
        )

    def trades_df(self):
        """The job's closed trades as a pandas DataFrame, for computing
        with: the frame `trades_to_dataframe` gives of a local run's
        result, column for column.

        The frame has one row for each trade, in the order the trades
        closed, and is new each time.

        Index: a `RangeIndex` from 0, with no name.

        Columns, in this order:

        - `symbol`: `object`, the symbol traded, as text.
        - `entry_time`, `exit_time`: `datetime64[ms, UTC]`, when the entry
          and the exit filled.
        - `entry_price`, `exit_price`: `float64`, the prices they filled
          at, in the quote asset for one of the base asset.
        - `qty`: `float64`, the quantity traded, in the base asset.
        - `gross_quote_pnl`: `float64`, the change in cash over the trade,
          in the quote asset, with both fees already taken off.
        - `entry_order_id`, `exit_order_id`: `object`, and `None` in every
          row: a backtest sends no order to a venue.

        A time is the engine's own milliseconds since the Unix epoch,
        exactly, shown as a UTC time. An amount is the float nearest to its
        decimal, by the engine's own conversion: floats are for computing
        with, and the exact decimals are in `to_backtest_result()`.

        The first call downloads the job's `trades.arrow`, checks it
        against the hash the job lists, and keeps its rows; a request that
        fails is raised as a `RemoteError`, or the exception derived from
        it that the failure is. It needs pandas and NumPy, which `pip
        install "quantforge-py[pandas]"` installs with the package, and
        raises an `ImportError` that says so where either is missing,
        before anything is downloaded.
        """
        from ._frames import trades_to_dataframe

        _native._pandas()
        _native._numpy()
        return trades_to_dataframe(self._backtest(trades=self._rows("trades")))

    def equity_df(self):
        """The job's equity curve as a pandas DataFrame, for computing
        with: the frame `equity_to_dataframe` gives of a local run's
        result, column for column.

        The frame has one row for each bar the job ran on, in order, and is
        new each time.

        Index: `open_time`, a `DatetimeIndex` of `datetime64[ms, UTC]`: the
        open time of the bar, which names it.

        Columns:

        - `equity`: `float64`, cash plus the position valued at the bar's
          close, after the bar's fill, in the quote currency.

        A time is the engine's own milliseconds since the Unix epoch,
        exactly, shown as a UTC time. The equity is the float nearest to
        its decimal, by the engine's own conversion: floats are for
        computing with, and the exact decimals are in
        `to_backtest_result()`.

        The first call downloads the job's `equity.arrow`, checks it
        against the hash the job lists, and keeps its rows; a request that
        fails is raised as a `RemoteError`, or the exception derived from
        it that the failure is. It needs pandas and NumPy, which `pip
        install "quantforge-py[pandas]"` installs with the package, and
        raises an `ImportError` that says so where either is missing,
        before anything is downloaded.
        """
        from ._frames import equity_to_dataframe

        _native._pandas()
        _native._numpy()
        return equity_to_dataframe(self._backtest(equity_curve=self._rows("equity")))

    def to_backtest_result(self):
        """The job's result as a `BacktestResult`, the type `run_backtest`
        gives of a local run: the metrics as its fields, every closed trade
        as a `ClosedTrade` and every point of the equity curve as an
        `EquityPoint`, each amount an exact `decimal.Decimal` with the
        digits the server wrote. A trade's two order IDs are `None`, as a
        local run's are.

        The result is new each time, and equal to the one a local run of
        the same strategy over the same candles gives. It downloads
        whichever of the job's two tables the result does not hold yet; a
        request that fails is raised as a `RemoteError`, or the exception
        derived from it that the failure is.
        """
        return self._backtest(self._rows("trades"), self._rows("equity"))

    def __repr__(self):
        trades = self._metrics[-1]
        return f"<JobResult of job {self._job_id}: {trades} trade{'' if trades == 1 else 's'}>"
