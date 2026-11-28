# quantforge-py

[QuantForge](https://github.com/quantforge-rs/quantforge) for Python:
strategies written in Python, run on the same deterministic Rust engine as
the `quantforge` command line.

**Not released yet.** The package is being built in the open. So far it
says what it is, holds the engine's types, each built by the engine's own
rule, has the class a strategy derives from, runs a backtest, reads the
candles the command line stored, and reaches the backtest jobs of a
QuantForge server, to which it submits a strategy's files as a job, and
follows that job to its result:

```python
from decimal import Decimal

import quantforge_py as qf

qf.__version__       # the package's own version
qf.ENGINE_VERSION    # the engine it was built with
qf.STORAGE_BACKENDS  # ("sqlite", "postgres")

market = qf.MarketId("binance_spot", "btcusdt", "1m")
market.symbol        # Symbol('BTCUSDT')
market.category      # Category('spot')

candle = qf.Candle(
    open_time_ms=1_700_000_000_000,
    close_time_ms=1_700_000_059_999,
    open=Decimal("81940.01000000"),
    high=Decimal("81990.5"),
    low=Decimal("81900"),
    close=Decimal("81955.25"),
    volume=Decimal("0.17235000"),
)
candle.open          # Decimal('81940.01000000'): exact, never a float

qf.BacktestConfig(initial_cash=Decimal("10000"), fee_bps=10, close_out_at_end=True)
```

A price is a `decimal.Decimal` on both sides, and a value the engine would
refuse is refused as it is built, in the engine's words:
`qf.Interval("7m")` raises `ValueError: invalid interval: 7m`.

A strategy is a class derived from `qf.Strategy` that defines `on_bar`,
which the engine calls once for every closed candle:

```python
class EnterThenExit(qf.Strategy):
    """Goes long on the first bar and flat on the third."""

    def __init__(self):
        self.bars = 0

    def on_bar(self, ctx, bar):
        self.bars += 1
        if self.bars == 1:
            return qf.TargetPosition.LONG_ALL_IN
        if self.bars == 3:
            return qf.TargetPosition.FLAT
        return None  # leave the position as it is
```

`ctx` is the engine's state at the bar: the market, the time, the cash and
the position. `run_backtest` runs a strategy over candles in the engine,
and gives the result:

```python
def minute(index, open, close):
    return qf.Candle(
        open_time_ms=index * 60_000,
        close_time_ms=index * 60_000 + 59_999,
        open=open,
        high=max(open, close),
        low=min(open, close),
        close=close,
        volume=1,
    )


candles = [minute(0, 100, 100), minute(1, 100, 110), minute(2, 110, 120), minute(3, 125, 125)]
config = qf.BacktestConfig(initial_cash=1000, fee_bps=0, close_out_at_end=True)

result = qf.run_backtest(candles, EnterThenExit(), config, market=market)
result.final_equity           # Decimal('1250')
result.total_return_pct       # Decimal('25.00')
result.trades[0].entry_price  # Decimal('100'): the open of the bar after the decision
result.trades[0].exit_price   # Decimal('125')
```

A decision made at a bar's close is filled at the next bar's open, a price
the strategy had not seen when it decided. The same candles and config give
the same result on every run, for a strategy that starts each run in the
same state and decides from its bars and that state alone, with no clock
and no randomness. A strategy keeps its state from one run to the next, so
each run is given one built anew, the same way. `market` says whose candles
they are, and is a Spot market: a candle strategy does not run on a futures
series. An exception a strategy raises ends the run as itself, with its
traceback.

The result is exact, and held to be: an SMA cross written in Python makes
the trades, the final equity and the drawdown of the engine's own Rust one
over a frozen day of candles, digit for digit, on every pull request
([the goldens](https://github.com/quantforge-rs/quantforge/blob/main/docs/golden-parity.md)).

The candles of a real run come from the store the `quantforge` command
line keeps. `quantforge data sync` stores them, in SQLite at
`data/market.sqlite` unless it is told otherwise, or in PostgreSQL, and a
`CandleReader` reads them back, by market and time range:

```python
with qf.CandleReader() as reader:  # the store the command line's settings select
    stored = reader.load(market, start="2026-01-01T00:00:00Z")

stored.market == market  # True: a batch names the market it was read for
stored.close[0]          # the exchange's own digits, such as Decimal('81940.01000000')
rerun = qf.run_backtest(stored, EnterThenExit(), config)  # on the market the batch names
```

A reader takes the command line's settings: `storage`, `db` and
`postgres_url` as arguments, and `QF_STORAGE`, `QF_DB` and
`QF_POSTGRES_URL` where an argument is left out, so Python reads the store
the command line wrote without being told twice. It opens the store
read-only, and creates, migrates and writes nothing. A market or a range
that holds no candle gives an empty batch, and a warning on the
`quantforge_py` logger that says what the store holds instead. The package
carries its own copy of SQLite: in a process that reads a SQLite store
this way, do not open the same file with Python's `sqlite3` module as well
([the candle reader](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-reader.md)).

For computing with, a batch of candles gives its columns as NumPy arrays,
and takes arrays back. NumPy is an extra of the package, which
`pip install "quantforge-py[numpy]"` installs with it:

```python
batch = qf.CandleBatch.from_candles(candles, market=market)
arrays = batch.to_numpy()   # a dict of eight arrays, one for each column
arrays["close"]             # float64: array([100., 110., 120., 125.])
arrays["open_time_ms"]      # int64: exact
arrays["trades"]            # uint64, masked where the exchange reported no count

again = qf.CandleBatch.from_numpy(**arrays, market=batch.market)
again == batch              # True: these prices are numbers a float holds
qf.run_backtest(again, EnterThenExit(), config) == result  # on the market the batch names
```

A price becomes a float there by the engine's own conversion and no other:
the float nearest to its decimal, and a float is read back as the decimal
of its shortest text, or refused. A time and a trade count cross exactly.
The arrays hold no market: it stays with the batch, and a batch is never
run on a market other than the one it names
([candles as NumPy arrays](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-numpy.md)).

For analysis, candles and a result are pandas DataFrames. pandas is the
package's other extra, and `pip install "quantforge-py[pandas]"` installs it
with the package, and NumPy with it:

```python
frame = qf.candles_to_dataframe(batch)  # one row for each candle, indexed by open_time, in UTC
frame["close"].tolist()                 # [100.0, 110.0, 120.0, 125.0]: float64, as the arrays are
frame["close"].rolling(2).mean()        # pandas from here on
qf.dataframe_to_candles(frame, market=batch.market) == batch  # True

trades = qf.trades_to_dataframe(result)  # one row for each closed trade
equity = qf.equity_to_dataframe(result)  # one row for each bar, indexed as the candles are
trades["gross_quote_pnl"].tolist()       # [250.0]
equity["equity"].tolist()                # [1000.0, 1100.0, 1200.0, 1250.0]
```

Every time in a frame is the engine's own millisecond, shown as a UTC time,
and a trade count the exchange did not report is missing, never a number.
The frames are for computing with: their amounts are floats, and the exact
decimals stay in the batch and in the result
([candles and results as pandas DataFrames](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-pandas.md)).

A QuantForge server, `quantforge serve run`, runs backtests as jobs, and a
`Client` reaches one from Python, with the standard library alone:

```python
client = qf.Client("http://127.0.0.1:9600")  # asks nothing yet; a token is read from QF_REMOTE_TOKEN
jobs = client.backtest.list()                # the server's jobs, the last submitted first
if jobs:                                     # none, on a server no job was submitted to
    job = client.backtest.get(jobs[0].id)    # one of them, by its ID
    job.status.state                         # 'queued', 'running', 'succeeded', 'failed' or 'cancelled'
    job.status.market                        # what it runs on, in the package's own types
    if not job.status.ended:
        job.cancel()                         # asks the server, and keeps what it answers
```

A client agrees on a protocol version with its server when it is first
used. A job is a handle: reading it asks nothing, and `refresh` and `cancel`
ask the server and keep its answer as the job's status. A request the
server refuses raises what the server said, word for word, as the exception
its code names: `qf.ValidationError`, `qf.NotFound`, `qf.Conflict`,
`qf.AuthError`, `qf.ServerUnavailable` or `qf.VersionMismatch`, each of
them a `qf.RemoteError`. A token goes only over `https`, or to a server on
this machine, and is never shown
([the Python client](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-client.md)).

A backtest is submitted from Python too. `submit` makes a strategy's files
into a bundle, checks it with the code a server checks an upload with,
uploads it and submits a job of it:

```python
job = client.backtest.submit(
    "my_strategy",                    # the directory of a strategy's files, or one .py file
    entry="sma_cross.py:SmaCross",    # its file and its class, as a bundle's manifest names them
    params={"fast": 20, "slow": 50},  # what the class is built from: SmaCross(fast=20, slow=50)
    market=market,                    # on the candles the server stores of the market,
    start="2026-01-01T00:00:00Z",     # from here on, to the last it stores
    config=config,
)
job.status.bundle                     # 'sha256:...': the bundle's code hash, computed here
job.refresh().state                   # asks the server where the job stands
```

Nothing is pickled: a strategy travels as its source files, and its
parameters as plain values in the bundle's manifest. A bundle that breaks
the [bundle spec](https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md),
with a file too large or of another kind in it, or that needs a package the
server does not offer, raises `qf.BundleError` before anything is uploaded,
with every problem by the spec's code. For a class written in a notebook,
`submit_class` sends the class's own source text, and takes
`source_only=True` as its caller's word that nothing else of the notebook
goes with it
([submitting a backtest from Python](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-bundles.md)).

The job is then followed from Python: its log as it is written, its end,
what it produced and what it ran on:

```python
for line in job.stream_logs():           # the job's log, line by line, as the job writes it
    print(line.seq, line.level, line.message)
job.wait().state                         # 'succeeded': asks until the job has ended
remote = job.result()                    # what it produced: its metrics, and no table yet
remote.metrics["trade_count"]            # 15
remote.metrics["final_equity"]           # a Decimal: exact, with the digits the server wrote
remote.to_backtest_result()              # a BacktestResult, as run_backtest gives one
provenance = job.provenance()            # what it ran on, as the server recorded it
provenance.code_hash                     # the bundle's code hash: job.status.bundle
provenance.identity                      # the same for every job given the same inputs
trades = remote.trades_df()              # the frame trades_to_dataframe gives of a local result
curve = remote.equity_df()               # and the one equity_to_dataframe gives
```

A log is followed over a stream the server offers, and a page at a time
where none can be had, so a log followed late is the whole log and one
followed from the start is every line as it comes. A result's two tables
are downloaded when they are first asked for, each held to the hash its job
lists, and are read by the engine's own reader. Its frames are the frames
of a local result, column for column, with floats for amounts, and
`to_backtest_result()` is the result a local run of the same strategy over
the same candles gives, every amount exact. A job that has not succeeded
has no result, a `qf.Conflict` in the server's words, and one that has not
ended within `wait`'s `timeout` is Python's own `TimeoutError`
([a job from Python](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-jobs.md)).

The package installs as `quantforge-py` and imports as `quantforge_py`.
`quantforge` on PyPI is another project.

One wheel per platform, built against Python's stable ABI, loads on CPython
3.11 and every later version. It requires nothing but Python: the arrays
need NumPy 2, from 2.0 on, and the DataFrames pandas, from 2.2.2 on, and
without one what needs it says what to install while everything else works.

[The Python package](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-package.md)
says how it is named, built and tested, and
[the Python types](https://github.com/quantforge-rs/quantforge/blob/main/docs/python-types.md)
what each type takes and gives.
