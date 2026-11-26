# The Python package

`quantforge-py` is QuantForge for Python: the package a strategy written in
Python runs through, on the same deterministic Rust engine as the
`quantforge` command line. This page says what the package is called, what
it holds, which Pythons it runs on, how it is built and tested, and what
adding it changed for the `quantforge` crate. The code is `quantforge-py/`.

**The package is not released.** It builds, installs and imports, says
what it is, and holds the engine's types: what a market is, candles, a
backtest's settings and result, and the strategy a class implements
([the Python types](python-types.md)). `run_backtest` runs such a class
over candles in the engine, which drives it through
[the strategy adapter](#the-strategy-adapter). An SMA cross written in
Python is held to the frozen results of the engine's own, digit for digit
([the goldens](golden-parity.md)). A batch of candles crosses to NumPy
arrays and back ([candles as NumPy arrays](python-numpy.md)), and candles
and a backtest's result are given as pandas DataFrames
([candles and results as pandas DataFrames](python-pandas.md)). The candles
themselves are read from the store the command line wrote, SQLite or
PostgreSQL, by `CandleReader` ([the candle reader](python-reader.md)). A
QuantForge server runs strategy bundles in workers that are a Python
interpreter with the package installed, through the same bindings
([the Python worker](python-worker.md)), and `Client` reaches such a
server from Python: it lists the server's backtest jobs, gets one and
cancels one ([the Python client](python-client.md)), and submits one, of a
strategy's files made into a bundle, or of a class by its source
([submitting a backtest from Python](python-bundles.md)).

## Contents

- [Names](#names)
- [What it holds](#what-it-holds)
- [Two versions](#two-versions)
- [Python versions](#python-versions)
- [Storage backends](#storage-backends)
- [Extras](#extras)
- [The strategy adapter](#the-strategy-adapter)
- [Layout](#layout)
- [Building it](#building-it)
- [Testing it](#testing-it)
- [The workspace](#the-workspace)
- [What is left](#what-is-left)

## Names

<!-- begin: names -->
| What | Name |
| --- | --- |
| The distribution, as `pip` installs it | `quantforge-py` |
| The package, as Python imports it | `quantforge_py` |
| The compiled extension module, which is private | `quantforge_py._native` |
| The Cargo package that builds it | `quantforge-py` |
<!-- end: names -->

```python
import quantforge_py as qf
```

`qf` is the alias the examples use.

**The package imports under the name it installs under.** `quantforge-py`
and `quantforge_py` are one name to `pip`, so what a user installs and what
they import cannot be taken for two things.

**The import name is not `quantforge`.** That name on PyPI is another
project, an option-pricing library whose wheel installs a top-level
`quantforge` package. Under that import name, `pip install quantforge`
would bring a library that is not this one, and with both installed each
would overwrite the other's files.

**The extension module is private.** `quantforge_py/__init__.py` publishes
what the extension holds, and modules written in Python join it in the same
package. Code imports from `quantforge_py`, never from
`quantforge_py._native`, whose layout may change.

The crate's library is `quantforge_py` as well, which keeps its build
output apart from the `quantforge` library's in the shared target
directory.

## What it holds

What the package says about itself, the engine's types, the strategy a
class implements, the function that runs one, the two conversions between a
decimal and a float, the reader of the candles a store holds, the four
functions that give or take a pandas DataFrame, the client of a QuantForge
server with the job and the status it gives, what a strategy bundle is
refused with before it is sent, and what a request to a server fails with:

<!-- begin: public names -->
| Name | What it is |
| --- | --- |
| `__version__` | The package's own version, such as `0.1.0` |
| `ENGINE_VERSION` | The version of the engine the package was built with, such as `0.2.0`: the `quantforge` crate's |
| `STORAGE_BACKENDS` | The stores the build can open, by the names `--storage` takes: `("sqlite", "postgres")`, or `("sqlite",)` for the SQLite-only build |
| `Symbol` | A trading symbol, such as `BTCUSDT` |
| `Interval` | A candle interval, such as `1m` |
| `Category` | A product family: `spot`, `linear` or `inverse` |
| `ExchangeId` | A product line: one venue's market of one family |
| `MarketId` | The key market data is stored under: a product line, a symbol and an interval |
| `Candle` | One bar: its times, its prices, its volume and its trade count |
| `CandleBatch` | Candles held column by column |
| `BacktestConfig` | How a backtest starts and what it charges |
| `ClosedTrade` | One closed round trip of a candle strategy on Spot |
| `EquityPoint` | Equity at one bar's close |
| `BacktestResult` | What a backtest produced |
| `TargetPosition` | The position a strategy asks for |
| `StrategyContext` | The engine's state as a strategy's callback is handed it |
| `Strategy` | The strategy a class implements: three callbacks an engine drives |
| `run_backtest` | Runs a strategy over candles in the engine, and gives the result |
| `decimal_to_f64` | The float nearest to a decimal |
| `decimal_from_f64` | The decimal of a float's shortest text |
| `CandleReader` | The candles a store holds, read by market and time range: the store `quantforge data sync` wrote, opened to read and never written |
| `candles_to_dataframe` | A batch of candles as a pandas DataFrame |
| `dataframe_to_candles` | A pandas DataFrame of candles as a batch |
| `trades_to_dataframe` | The closed trades of a backtest's result as a pandas DataFrame |
| `equity_to_dataframe` | The equity curve of a backtest's result as a pandas DataFrame |
| `Client` | The client of a QuantForge server: its backtest jobs listed, got, cancelled and submitted |
| `Job` | One backtest job a server holds: its ID, the status the server last gave of it, and what can be asked of it |
| `JobStatus` | Where a job stood when the server was last asked: its state, its times, why it failed, and what it runs |
| `BundleError` | A strategy bundle that is refused before anything of it is sent, with every problem by the bundle spec's code: a `ValueError` |
| `RemoteError` | A request to a server that did not get its answer: every exception below is one |
| `AuthError` | The server refused the request's credentials |
| `ValidationError` | The server refused the request as malformed or as naming something invalid, in the server's words |
| `NotFound` | What the request names does not exist on the server |
| `Conflict` | The request contradicts the state of what it names |
| `ServerUnavailable` | The server could not be reached, or cannot answer now |
| `VersionMismatch` | The server and the package share no protocol version |
<!-- end: public names -->

```python
>>> import quantforge_py as qf
>>> qf.__version__
'0.1.0'
>>> qf.ENGINE_VERSION
'0.2.0'
>>> qf.STORAGE_BACKENDS
('sqlite', 'postgres')
```

These are `quantforge_py.__all__`, and the package holds no other public
name. Importing it opens no store and no connection. A `CandleReader` is
the one object that holds a store, and with PostgreSQL the one that holds a
database's credential: what a reader gives carries neither, and a backtest
opens no store: it runs on the candles it is handed. A `Client` is the one
object that reaches a server, when it is first used and not before, and
the one that holds a server's token, which it never shows.

[The Python types](python-types.md) is what each type takes and gives: how
a decimal crosses, every signature and field, what each refuses, how a
strategy is called, and how a backtest is run. A price, a quantity or an
amount is a `decimal.Decimal` on both sides, and never a float. It becomes
one only where it is asked to, by the engine's own conversion:
`decimal_to_f64` for one value, a batch's columns as NumPy arrays, which
[candles as NumPy arrays](python-numpy.md) is the contract of, and the
frames of candles and of a result, which
[candles and results as pandas DataFrames](python-pandas.md) is the
contract of. [The candle reader](python-reader.md) is the contract of
`CandleReader`: which store it opens, what it reads, and what it never does
to a store. [The Python client](python-client.md) is the contract of
`Client`, `Job`, `JobStatus` and the exceptions: which server a client
reaches and with which token, what it agrees on with the server, what a
job's status holds, and what is raised for each way a request fails.
[Submitting a backtest from Python](python-bundles.md) is the contract of
`client.backtest.submit` and `submit_class`, and of `BundleError`: what is
bundled, how its manifest is written, what is checked before anything is
sent, and what is never done, pickling among it.

## Two versions

The package and the engine each have a version, and neither follows the
other:

- **`__version__`** counts the package's own releases. It is the `version`
  in `quantforge-py/Cargo.toml`, which the wheel takes as its own.
- **`ENGINE_VERSION`** is the `quantforge` crate the wheel was built with:
  the `version` in the repository's `Cargo.toml`, which the crate publishes
  as `quantforge::ENGINE_VERSION`. A strategy bundle's `engine` requirement
  ([bundle-spec.md](bundle-spec.md#engine)) is held to this version, so it
  is the one to read before writing `engine = ">=0.3.0, <0.4.0"`.

Cargo and Python spell a pre-release differently: `0.1.0-rc.1` in
`Cargo.toml` is `0.1.0rc1` in a wheel's metadata. `__version__` is Cargo's
text, and the tests hold it to the installed metadata, so the first
pre-release of the package has to settle which spelling `__version__`
takes.

## Python versions

**One wheel per platform loads on CPython 3.11 and on every later
version.** The oldest version is:

<!-- begin: oldest python -->
3.11
<!-- end: oldest python -->

The extension is built against Python's stable ABI as it was in that
version (PyO3's `abi3-py311`), so a wheel is tagged `cp311-abi3` and is not
rebuilt for each Python. `requires-python` in `pyproject.toml` says the
same, so `pip` refuses an older one.

3.11 is the oldest CPython still maintained (3.10 reached its end of life
in October 2026), the oldest that pandas and pyarrow release for, and the
first whose stable ABI has the buffer protocol. NumPy's current series, 2.5,
needs Python 3.12; on 3.11 `pip` installs 2.4, the last that runs there.
pandas' current series, 3.1, runs on both.

Two kinds of interpreter are left out:

- **Free-threaded builds** (`python3.14t`) do not load a wheel built for
  `abi3`. The stable ABI they share with the default build, `abi3t`, starts
  at Python 3.15.
- **PyPy and GraalPy** do not take a `cp311-abi3` wheel.

## Storage backends

The crate has one feature, `postgres`, and it is the default one, so the
package has two builds:

<!-- begin: builds -->
| Build | Features | `STORAGE_BACKENDS` | Published | Built and tested in CI |
| --- | --- | --- | --- | --- |
| The package | The default: `postgres` | `("sqlite", "postgres")` | As `quantforge-py`, one wheel for each platform | For release on Linux, macOS and Windows, by the `wheel` legs; in the `dev` profile on Linux, by `python` |
| The SQLite-only package | None: `--no-default-features` | `("sqlite",)` | Never | In the `dev` profile on Linux, by `python` |
<!-- end: builds -->

The package is built with the `postgres` feature, as every published
artifact is ([parity](parity/README.md#artifacts)). The SQLite-only package
holds no PostgreSQL code and says so; it is built and tested, and never
published as `quantforge-py`.

**No other build is supported, and neither passes for the other.** Cargo
refuses a feature the crate does not have. A wheel is tested as the package
unless the tests are told it is the SQLite-only one
([testing it](#testing-it)), so a wheel built without PostgreSQL fails
where the package is tested, as each release's wheel is
([ci.md](ci.md#the-wheels)). Importing either opens no store and needs no
database: the `wheel` legs on macOS and Windows test the package with none.

`CandleReader` is what opens a store, of either backend, by the settings
the `quantforge` command line resolves: its flags as arguments, its
variables where an argument is left out, SQLite at `data/market.sqlite`
where nothing is set ([the candle reader](python-reader.md)). The
SQLite-only package refuses `postgres` as not in the build, and opens no
SQLite file in its place.

## Extras

**The package requires nothing but Python.** It has two extras, which `pip`
installs where the package is asked for with one:

<!-- begin: extras -->
| Extra | Requires | Installed by |
| --- | --- | --- |
| `numpy` | `numpy>=2.0,<3` | `pip install "quantforge-py[numpy]"` |
| `pandas` | `numpy>=2.0,<3`, `pandas>=2.2.2,<4` | `pip install "quantforge-py[pandas]"` |
<!-- end: extras -->

`CandleBatch.to_numpy` and `CandleBatch.from_numpy` are what need NumPy
([candles as NumPy arrays](python-numpy.md)). The four functions that give
or take a DataFrame are what need pandas, and NumPy with it: a frame is
made of a batch's arrays, so the `pandas` extra asks for the NumPy the
`numpy` extra asks for, in the same words
([candles and results as pandas DataFrames](python-pandas.md)).

- **The client needs neither, and nothing else.** It is written with the
  standard library alone, and imports what a request takes, `http.client`
  and `ssl` among it, when it first asks a server: importing the package
  imports nothing of the network
  ([the Python client](python-client.md#connections)). A bundle it submits
  is built and checked by the engine's own code, which the extension
  module carries.
- **Without either, everything else works.** What needs one raises an
  `ImportError` that says what to install, where Python finds none and
  where the one it finds is one the extra does not take: a NumPy older than
  2.0 or from 3 on ([NumPy itself](python-numpy.md#numpy-itself)), a pandas
  older than 2.2.2 or from 4 on
  ([pandas itself](python-pandas.md#pandas-itself)).
- **Each is imported when something that needs it is first called,** and
  never when the package is. Importing `quantforge_py`, building its types
  and running a backtest import neither, and a batch's arrays import no
  pandas.
- **The package brings neither into an environment of its own accord.** A
  strategy bundle pins the packages it imports, NumPy and pandas among them
  ([bundle-spec.md](bundle-spec.md#deps)), and a worker that runs it has
  those: a package that required one would have a say in which.
- **The extension is not built against either.** It finds the NumPy that
  is installed as it runs, through the `numpy` crate, so the one wheel of a
  platform serves every NumPy the tests run on, from 2.0.0 to the newest,
  as it serves every Python from 3.11 on. pandas is reached through its
  own Python calls, from the one module of the package that is written in
  Python. Building a wheel needs neither.
- **How an extra is found is one piece of the extension for both**
  (`src/extra.rs`): whether Python finds it, which version it states, and
  whether the extra takes that version, read as `pip` reads the extra's
  bounds.
- **The tests run on the oldest NumPy and the oldest pandas the extras
  take, on the newest, with NumPy alone, and with neither.** The oldest
  Python the package supports has no wheel of the newest NumPy, and the
  newest Python none of the oldest NumPy or the oldest pandas: NumPy 2.0.0
  and pandas 2.2.2 run on Python up to 3.12, and the current NumPy from
  3.12 on. So the two oldest are tested together on the oldest Python, the
  newest of each that `pip` finds on each Python, and on the oldest Python
  twice more: the package installed with its `numpy` extra alone, and
  without its extras ([testing it](#testing-it)).

## The strategy adapter

The engine drives a strategy through its `Strategy` trait.
`quantforge-py/src/adapter.rs` is that trait over an object of a class
derived from the package's `Strategy`: every host that runs a Python
strategy in an engine does it through this one adapter, and the adapter
adds nothing to a run.

- **A callback attaches its thread to Python** for as long as it runs and
  no longer, whatever thread the engine is on. A thread that is attached
  already, as the one that called into the package is, stays so. One that
  is not takes the interpreter for the callback and gives it back.
- **What a callback is handed is built anew:** a `StrategyContext`, and for
  a bar a `Candle`, each a copy of the engine's own value.
- **What a callback raises ends the run.** The engine is told in its
  `StrategyError`, a line that says where the strategy failed and then the
  exception as Python itself writes it, with its traceback. The exception
  is kept as it was raised, with that line as a note, for a host that is
  Python to raise again.

**Two kinds of host run an engine:**

| Host | The engine's thread | A callback that fails |
| --- | --- | --- |
| Python: `run_backtest`, and a server's worker, which is a Python interpreter | The thread that called, attached throughout | Raises its own exception, in the caller; a worker fails its job with it, its traceback in the job's log |
| One that embeds Python: the command line | A thread of the host's own, attached once around the run and detached while the engine computes | Is the engine's error, with the traceback in its text |

**A host that embeds Python attaches its engine thread once, around the
whole run,** and detaches inside it:
`Python::attach(|py| py.detach(|| engine.run(..)))`. Python keeps a
thread's own state, such as the decimal context a strategy set or a
`threading.local`, for as long as the thread stays attached. A thread
attached anew for every callback would lose that state between two bars,
and a strategy would compute differently there than in a run Python
hosts.

**A run Python hosts keeps the interpreter between callbacks.** The
engine's own work between two of them is a few decimal operations, and
other Python threads get their turn as they do during any Python code,
inside the callbacks. Giving the interpreter up after every callback would
let any busy thread take it, and keep it for a switch interval each time:
beside one such thread, a run of callbacks that do nothing took about
350 µs a bar that way, and 1.5 µs when it kept the interpreter.

**`run_backtest` is the run Python hosts.** `quantforge-py/src/run.rs`
reads its arguments, runs the engine on the calling thread through the
adapter, and raises what a callback raised, or what the engine refused in
the engine's words ([running a backtest](python-types.md#running-a-backtest)).

**A server's worker runs a bundle as `run_backtest` runs a strategy:** on
the worker's main thread, which stays attached, through the same adapter.
It is the interpreter the server was given, running the package's private
worker, `quantforge_py._worker`, which imports the bundle's strategy and
builds it in the runtime every host of a bundle shares
([the Python worker](python-worker.md)).

**The other kind of host is not built yet:** the command line runs a Python
strategy with the story that embeds Python ([what is left](#what-is-left)).
Until then the extension module holds one private entry in its place,
`quantforge_py._native._embedded_backtest`, which takes what `run_backtest`
takes and runs the engine as such a host does. The package's own tests
reach that kind of host through it, and hold both to the same cases.

## Layout

A package of two halves in one directory: the crate that compiles the
extension module, and the Python package the extension is built into.

<!-- begin: layout -->
```text
quantforge-py/
├── Cargo.toml              the crate: PyO3 with the stable ABI of 3.11, the `numpy` crate, the `postgres` feature, never published to crates.io
├── README.md               what PyPI shows for the package
├── pyproject.toml          the distribution: its name and metadata, NumPy and pandas as its extras, maturin as the build backend, where the package and the extension go
├── benches/
│   └── candle_arrays.py    how long a batch of candles takes to cross to NumPy arrays and back: figures to read, which no test holds a run to
├── python/
│   └── quantforge_py/
│       ├── __init__.py     the package: publishes the extension module's names, and those of the modules written in Python beside it: the four functions of the frames, the client, its job and its status, and the exceptions
│       ├── _answer.py      one answer of a server as the client reads it from its connection: held to what HTTP writes, its body to the longest that is taken and what frames it to a limit of its own, with the standard library alone
│       ├── _bundles.py     BundleError, and a strategy bundle as a client makes one before it submits a job: the files a caller names, its parameters and packages checked, the manifest written for them, and the bundle built and checked by the engine's own code
│       ├── _client.py      Client: the client of a QuantForge server, which is where the server is, the token it sends, and the server's backtest jobs
│       ├── _errors.py      RemoteError and the six exceptions derived from it: what a request to a server fails with, each raised by the code of the protocol's error envelope
│       ├── _frames.py      candles and a backtest's result as pandas DataFrames, and a frame of candles as a batch: written in Python, with pandas' own calls
│       ├── _jobs.py        Job and JobStatus: a server's backtest jobs listed, got and submitted, the handle on one, and its status read strictly from what the protocol writes, into the package's own types
│       ├── _names.py       what the text of a class sent by its source reads and does not define: the same answer on every Python, a class's body read in the order it is written, and what to do about each name, with nothing run and nothing asked
│       ├── _runtime.py     the runtime a strategy bundle runs in, in Python: the environment a worker offers, a bundle's strategy imported from its files and built from its parameters, what its own code may import, its lines and its failures
│       ├── _source.py      a class as submit_class sends one: the text of its class statement, found in its module's file or its notebook's cell by the class's own functions, below the imports its caller declares
│       ├── _submit.py      a backtest job submitted: what is checked before a server is asked, then its runtime, the bars it stores, the bundle uploaded under the hash computed here, and the job submitted once
│       ├── _wire.py        a server as a client's requests reach it, with the standard library alone: its URL and the token checked, the protocol version agreed on, each request on a connection of its own, what is asked again, and an answer given as its route's or raised as the exception it is
│       └── _worker.py      the worker a QuantForge server runs Python bundles in, `python -m quantforge_py._worker`: what the server gave it closed, its frames moved off standard output, then the extension's worker
├── src/
│   ├── adapter.rs          the engine's Strategy over a Python object: a thread attached for each callback, a failure with its traceback
│   ├── arrays.rs           a batch's columns as NumPy arrays, and arrays as a batch: what crosses exactly, what as a float64, and what is refused
│   ├── arrays/
│   │   ├── column.rs       one array as a column: held to the column's type as it is taken and again as its cells are read, read where it lies or through a copy NumPy makes
│   │   ├── counts.rs       the trade counts: a plain array or a masked one made over a plain array, and its mask read as bytes
│   │   └── installed.rs    NumPy as it is found: the versions its extra takes, and what is said of a module that has NumPy's name and not its arrays
│   ├── backtest.rs         BacktestConfig, EquityPoint and BacktestResult: a backtest's settings and its result
│   ├── bench.rs            the private entry a host that embeds Python is tested through: one backtest, run as such a host runs one
│   ├── bundle.rs           what the client builds a bundle with, each private: a bundle collected, checked and packed, a manifest written, and a server's runtime read, by the engine's own code
│   ├── candles.rs          Candle and CandleBatch: one bar, and bars held column by column with the market they are of
│   ├── convert.rs          how a value crosses: an exact decimal, an integer, text, and a refusal in the engine's words
│   ├── extra.rs            an extra of the package as it is found: imported when something that needs it is first asked for, held to the versions the extra takes, and what is said where there is none
│   ├── floats.rs           decimal_to_f64 and decimal_from_f64: the one crossing between a decimal and a float
│   ├── frames.rs           what the pandas functions are built on, each private: pandas and NumPy as they are found, and a result's trades and equity curve column by column
│   ├── lib.rs              the extension module, quantforge_py._native
│   ├── market.rs           Symbol, Interval, Category, ExchangeId and MarketId: what a market is
│   ├── reader.rs           CandleReader: the engine's own reader of a store's candles, opened read-only, a call that waits without the interpreter, and a store's failure as the exception Python is raised for it
│   ├── reader/
│   │   ├── range.rs        the bounds of a range of candles: an int of milliseconds, a datetime with a time zone or RFC 3339 text, each as the engine's millisecond, for a reader's range and for the window of a job
│   │   └── settings.rs     which store a reader opens: the command line's settings, from the reader's arguments and the variables behind them, refused by where a value came from and never by the value
│   ├── run.rs              run_backtest: a run's arguments read, the market of the run settled, the engine run on the calling thread, and what it refused raised in its words
│   ├── runtime.rs          the runtime a strategy bundle runs in, the one path every host takes: the bundle's strategy as the engine drives it, and a strategy that failed worded with its traceback
│   ├── strategy.rs         Strategy, StrategyContext and TargetPosition: the class a strategy derives from, what its callbacks are handed, what they hand back
│   ├── trade.rs            ClosedTrade: one closed round trip, with the venue's order IDs as text
│   └── worker.rs           the worker a server runs Python bundles in: the server's own frames, limits and sandbox, the environment it offers, what its job writes taken into the job's log
└── tests/
    ├── conftest.py         what the tests share: the checkout's manifests, the installed distribution, how a page and a refusal are read, how a strategy is run on either host, how a result is compared with a fixture's, where a store lives on each backend, and the protocol's codes and states as the crate has them
    ├── jobs.py             what the job tests share: a `serve run` whose workers are this interpreter, on a store of either backend, bundles as their canonical tar, the job routes spoken to with the standard library alone, a result's Arrow files read back as text, and a job in each state for a client to list, get and cancel
    ├── standin.py          what the client's tests share: a server of the test's own, which answers what a test tells it to and records what it was asked, and one that stores an upload and queues a submission as a server does
    ├── stores.py           what the reader's tests share: stores the quantforge command line wrote from an exchange that answers on this machine, a SQLite file and a PostgreSQL database of its own, and the server spoken to with the standard library alone
    ├── test_backtest.py    a backtest's config, a closed trade, an equity point and a result
    ├── test_bundles.py     a bundle as the package builds one: the spec's shared cases through the package, a code hash against a builder on the standard library's tarfile, a directory as the spec collects it, the manifest written from the arguments, and every refusal of a strategy, an entry, a parameter and a package
    ├── test_bundles_page.py  the page of bundles and submission, held to the package, its examples run against a real server
    ├── test_candles.py     a candle, and a batch of candles by its candles and by its columns, with the market it names
    ├── test_client.py      the client against servers of the test's own: what it refuses before it asks, which token it sends and where it sends none, the version it agrees on, the exception each of the protocol's codes is raised as, a refused token, what is asked again, and every answer it refuses to read
    ├── test_client_jobs.py  the client against a real `serve run` on either backend: its jobs in every state listed and got as the server's own routes give them, a queued and a running one cancelled, and the server's refusals raised in its words
    ├── test_client_page.py  the page of the client, held to the package, its examples run against a real server
    ├── test_decimals.py    how a decimal crosses, what is refused, and the float policy's shared cases
    ├── test_distribution.py  the installed distribution: its name, its Pythons, its license, its files
    ├── test_end_to_end.py  a strategy that decides from its bars, run over fixture candles: the engine's decisions and result, and what each figure means
    ├── test_golden_parity.py  the golden parity test: an SMA cross written in Python makes the trades, the final equity and the drawdown of the Rust one's frozen run, digit for digit
    ├── test_hostile_bundles.py  bundles that mean harm, run by a server's workers on either backend: each tries one thing a job has no business doing, a fork bomb, a memory bomb, a socket, a file outside its scratch directory, its server's secrets, a frame in its worker's place, and is held to what the workers say they enforce
    ├── test_market.py      a symbol, an interval, a product family, a product line and a market
    ├── test_numpy.py       a batch's columns as NumPy arrays and arrays as a batch: each array's type and shape, what crosses exactly and what as a float, and every refusal
    ├── test_numpy_fixtures.py  the repository's fixtures through a batch that names its market and through NumPy: every product line kept apart, a store's decimals, the frozen day of candles and its backtest
    ├── test_numpy_optional.py  NumPy as an extra of the package: when it is imported, and what the two conversions say where Python finds none or one the extra does not take
    ├── test_numpy_page.py  the page of the NumPy arrays, held to the package
    ├── test_package.py     the imported package: its names, its classes, its versions, its storage backends
    ├── test_pandas.py      candles and a result as pandas DataFrames, and a frame as a batch: each frame's index and columns, every time exact, what crosses as a float, and every refusal
    ├── test_pandas_fixtures.py  the repository's Spot fixtures through frames: the frozen day of candles and its backtest, the frames of the frozen result, the shared strategy cases, a store's decimals, every product line
    ├── test_pandas_optional.py  pandas as an extra of the package: when it is imported, what the four functions say where Python finds none or one the extra does not take, and the NumPy asked for beside it
    ├── test_pandas_page.py  the page of the DataFrames, held to the package
    ├── test_reader.py      the candle reader before it reads: the class it is, which store its settings select, an argument over its variable over the default, every refusal by where a value came from, and a reader closed, shared between threads or carried into another process
    ├── test_reader_page.py  the page of the candle reader, held to the package
    ├── test_reader_stores.py  what the reader reads on each backend, from stores the command line wrote: every candle digit for digit and oldest first, a range in each way a time is written, the sentence for a range that held nothing, product lines kept apart, and a store it refuses and leaves as it was
    ├── test_repository.py  this page, the readme and the manifests, held to the package
    ├── test_run_backtest.py  run_backtest: what it takes, the order it checks that in, the market a batch names, what the engine refuses, and that a run repeats
    ├── test_runtime.py     the runtime a bundle runs in, in this process: its strategy built from its entry and its parameters, what its own code may import and what quantforge_py imports for it, every refusal and its traceback, its lines, and what a worker offers
    ├── test_source.py      a class as submit_class sends one: its statement found in a file and in a notebook's cell, the imports written above it, every name it reads and does not define refused with what to do, the same on every Python and held to what running the file gives, and every class that is not sent
    ├── test_strategy.py    a target, a context, and the class a strategy derives from
    ├── test_strategy_cases.py  the strategy boundary's shared cases: a Python strategy called and obeyed as the engine's own is
    ├── test_strategy_failures.py  a callback that raises or returns what it may not, a strategy's name, and what is no strategy
    ├── test_strategy_hosts.py  the thread a callback runs on, the interpreter it holds, and a thread's own state from bar to bar
    ├── test_submit.py      a backtest submitted, against servers of the test's own: what is asked and in which order, what is sent, the window, what stops before anything is asked or uploaded, an upload asked again and a submission never, and every answer that is refused
    ├── test_submit_jobs.py  a backtest submitted to a real `serve run` on either backend: a strategy's directory run to the goldens' trades, the client's code hash held to the server's, the command line's and a builder's on the standard library, a class run from its source, and bundles refused before any upload
    ├── test_types_page.py  the page of the types, held to the package
    └── test_worker_jobs.py  bundles run by a server's workers: the goldens as a bundle on either backend, digit for digit; a raising strategy's traceback in its job's log; a package not on offer refused at upload and submission, and an import refused as it runs; what a strategy writes; a worker per job, holding nothing of its server's store; and how long a job takes to reach its first bar
```
<!-- end: layout -->

`maturin develop` builds the extension into `python/quantforge_py/`, as
`_native.abi3.so` (`_native.pyd` on Windows). Git ignores it.

## Building it

It takes Rust, as the crate does, CPython 3.11 or later, and
[maturin](https://www.maturin.rs) 1.16 or later, which `pip` installs:

```bash
cd quantforge-py
python -m venv .venv
. .venv/bin/activate
pip install maturin pytest
maturin develop --extras numpy,pandas
python -c "import quantforge_py"
```

`maturin develop` compiles the extension in the `dev` profile and installs
the package into the virtualenv in place: the virtualenv imports
`quantforge-py/python/quantforge_py/`, so a change to a Python file needs
no rebuild, and a change to the Rust code needs `maturin develop` again.
`--extras numpy,pandas` installs NumPy and pandas with it, as the package's
extras ask for them, which the tests of the arrays and of the frames need.
Without the flag the package is installed alone, as `pip install
quantforge-py` installs it.

`maturin build` writes a wheel to the repository's `target/wheels/`
instead:

| Command | What it builds |
| --- | --- |
| `maturin build` | A wheel in the `dev` profile, as CI's `python` job tests it |
| `maturin build --release` | An optimized wheel, in the workspace's release profile, as a release carries it |
| `maturin build --no-default-features` | The SQLite-only package |

A wheel carries the package, the extension module, the repository's
`LICENSE`, and a CycloneDX bill of materials of the crate's dependencies,
which maturin writes.

**CI builds the wheel a release carries for each platform,** and tests it
on that platform ([ci.md](ci.md#the-wheels)): Linux on x86-64, in a
`manylinux_2_28` container, so that it loads on any Linux with glibc 2.28
or later; macOS on Apple silicon; and Windows on x86-64. Each is built with
`--release` and `--compatibility pypi`, which fails a wheel whose tag PyPI
would refuse, and each that passes is kept with the run as an artifact,
`wheel-<os>`.

**The extension is built by maturin, never by `cargo build`.** maturin
tells PyO3 that it builds an extension module, which leaves the Python
library unlinked; cargo alone links the crate like a program that embeds
Python. The builds follow `Cargo.lock` as it is (`locked` in
`pyproject.toml`) and fail on one that is out of date.

**No source distribution is published or tested yet.** `maturin sdist`
builds one, which holds the `quantforge` crate beside the package, and
`pip` builds a wheel from it that passes the tests. That wheel describes
itself with the crate's `README.md`, not the package's: a source
distribution has `pyproject.toml` at its root, where `README.md` is the
crate's. The release settles this before it carries one.

## Testing it

The tests are Python's, run with pytest against the package as it is
installed, from a wheel or by `maturin develop`:

```bash
cargo build
cd quantforge-py
pytest
```

`cargo build`, at the repository's root, builds the `quantforge` binary:
the tests of the candle reader read stores the command line itself wrote,
and look for it in `target/debug/`, or where `QUANTFORGE_PY_TEST_CLI` says.

They hold the installed package to the checkout they are in:

- **The package** (`test_package.py`): the names it publishes and no
  other, that each type is a class of the package, its version against the
  crate's, the engine's against the `quantforge` crate's, and the storage
  backends of the build.
- **The types** (`test_market.py`, `test_candles.py`, `test_backtest.py`,
  `test_decimals.py`): that each is built by the engine's rule, gives back
  what it was given digit for digit, and is refused in the engine's words;
  and the decimal and float64 policy's shared cases,
  `tests/fixtures/decimal_float_cases.csv`, which the crate's own tests
  check as well.
- **The strategy** (`test_strategy.py`, `test_strategy_cases.py`,
  `test_strategy_failures.py`, `test_strategy_hosts.py`): the class a
  strategy derives from, its context and its targets; then the adapter, in
  the engine and as each kind of host runs it. A strategy written in Python
  is held to the strategy boundary's shared cases,
  `tests/fixtures/strategy_cases.json`, which the crate's own tests write
  from a strategy written in Rust: every callback in order, with the state
  it was handed digit for digit, and the result. The rest is what a
  callback raises or returns, and the thread it runs on.
- **The run** (`test_run_backtest.py`, `test_end_to_end.py`):
  `run_backtest` itself, which is how every test above reaches the engine
  as Python hosts a run. What it takes and in what order it checks it, a
  market that is not Spot and a run of no candles refused in the engine's
  words, the result's fields held to the engine's own types one for one,
  and the same run giving the same result. Then a strategy that decides
  from its bars, run end to end over four hours of fixture candles: one of
  the shared cases is decided by a rule written in Rust, and the same rule
  written in Python has to decide the same on every bar and bring the same
  result, each figure of which is then worked out again from the trades.
- **The goldens** (`test_golden_parity.py`): the package's headline
  promise. `tests/fixtures/golden/` holds one frozen day of candles, what
  the engine's built-in Rust SMA cross made of them, and the same strategy
  written in Python ([golden-parity.md](golden-parity.md)). The Python one
  is run through `run_backtest`, and as a host that embeds Python runs
  one, and has to decide the same on all 1,440 bars and bring the same
  trades, final equity and drawdown, each compared as the text of its
  decimal. Then the fixture is shown to tell apart what it should:
  strategies that take equal averages for a signal, skip the warm-up,
  leave out the bar's own close, round their averages to the cent, or
  average floats in each of three ways, each trade differently on it, and
  so does the strategy itself in a decimal context too coarse for the sums
  it adds up. A fourth way of averaging floats makes the same trades, and
  is caught by its decisions, which the goldens record for every bar.
- **NumPy** (`test_numpy.py`, `test_numpy_fixtures.py`,
  `test_numpy_page.py`): a batch's columns as arrays, and arrays as a
  batch. Each array's type and shape; every time and every trade count
  exact, an unknown count masked; a price as the float Python reads from
  its text, and a float as the decimal of Python's `repr` of it, over
  thousands of values and the policy's shared cases; every refusal; arrays
  laid out in memory in every way NumPy lays one out; an array changed
  where it stands after it was taken, by other code and by another thread,
  refused for what it is and never read past its end, and a mask resized
  where it stands refused for its length; memory that is not there, a
  `MemoryError`. Once a first call has set everything up, the thread is
  let in where the call lets a thread in, as NumPy copies a column, and
  nowhere else. Three more scenarios run in an interpreter of their own,
  since the array bindings end the process where code of Python's changes
  an array that is borrowed: an array changed again as its refusal is
  worded, and a mask changed as the view of it is made, by a thread and,
  before Python 3.12, by what the collector runs. Then the repository's
  fixtures: every product line kept with its batch and apart from its
  namesakes, and the goldens' frozen day taken to NumPy and back, where it
  is the same batch by value and its backtest the frozen result by value.
  And every table and example of
  [python-numpy.md](python-numpy.md).
- **pandas** (`test_pandas.py`, `test_pandas_fixtures.py`,
  `test_pandas_page.py`): candles and a backtest's result as DataFrames,
  and a frame of candles as a batch. The index and the columns of each
  frame, by name and by type, the same on every pandas the extra takes and
  whatever pandas is told to make of text, the names of the columns among
  it, and a frame beside the one pandas makes of its cells, equal to it but
  for the type of those names; every time the engine's own millisecond,
  shown as a UTC time and checked against Python's own dates, the one
  integer pandas reads as no time refused, and whole milliseconds in a
  finer unit read as integers; a price and an amount as the float Python
  reads from its text, and a float as the decimal of Python's `repr` of
  it; a count that is unknown missing, and never a number; a frame read
  whatever the order of its columns, the zone and the unit of its times and
  the place of its rows, with any other column left unread; and every
  refusal, each type held in the columns' order before a value is read, a
  frame and an index of a derived type among them, and the first time that
  is refused named whatever it is refused for. Every call of the four
  functions, the page's examples' among them, is held to drawing no warning
  from pandas or NumPy, which is how either says that what was asked of it
  will change. That each docstring states its frame's index and columns.
  Then the repository's fixtures, none of which is changed: the goldens'
  frozen day taken to a frame and
  back, where it is the same batch by value and its backtest the frozen
  result by value; the frames of the frozen result held to Python's reading
  of the frozen file's text, cell for cell; the candles and the results of
  the shared strategy cases; a store's decimals; and every product line,
  whose frames are one frame and whose batches are six. And every table
  and example of [python-pandas.md](python-pandas.md).
- **pandas as an extra** (`test_pandas_optional.py`): that pandas is
  imported only when a frame is asked for; what each of the four functions
  says where Python finds none, with everything else at work beside them,
  a batch's arrays among it; a pandas taken or refused by its version as
  `pip` reads the extra's bounds, with `packaging` as the judge; a pandas
  that fails as it is imported, which raises its own error; and NumPy asked
  for after pandas and before any argument is looked at. Each asks a fresh
  interpreter that is kept from pandas or handed a module in its place, so
  these run where pandas is installed and where it is not.
- **NumPy as an extra** (`test_numpy_optional.py`): that NumPy is imported
  only when a conversion is asked for, and what the two conversions say
  where Python finds none, with everything else at work beside them; a
  NumPy taken or refused by its version exactly as `pip` reads the extra's
  bounds, older than 2.0 and from 3 on, over releases, pre-releases and
  development builds, with `packaging` as the judge, and text `pip` would
  not take read by its numbers; a NumPy that fails as it is imported,
  which raises its own
  error; and a module that only has NumPy's name, with a version or
  without one. Each asks a fresh interpreter that is kept from NumPy or
  handed a module in its place, so these run where NumPy is installed and
  where it is not.
- **The candle reader** (`test_reader.py`, `test_reader_stores.py`,
  `test_reader_page.py`): the reader against stores the `quantforge`
  binary wrote, by `data sync` from an exchange that answers on the test's
  own machine with the candles the test gives it. The class it is, and
  which store its settings select: the command line's default path, an
  argument over its variable and a variable over the default, and every
  refusal, which names the argument or the variable a value came from and
  never the value, a password least of all. Then what it reads, on SQLite
  and on PostgreSQL: every candle digit for digit as the exchange wrote it,
  oldest first, the goldens' frozen day among them, which takes the
  exchange more than one answer to give; a range that takes both of its
  ends, bounded by an integer, by a time in any zone and by text; an empty
  batch and one warning for a market or a range that holds nothing, with
  what the store holds instead; every product line of one symbol read
  alone; and the same batch from either backend. That it never writes: a
  store it read is the same byte
  for byte, a file that is not there stays so, a database without a store
  stays empty, and a store at another schema is refused and left there,
  until the command the refusal names has brought it forward; candles a
  killed writer left in a SQLite store's log are read and left there, a
  store in rollback mode stays in it, and a journal a writer left
  unfinished is refused, until the command the refusal names has rolled it
  back. Another process's lock on a SQLite store, waited for, and held past
  the wait, a `ConnectionError`; a reader alone, beside a command line that
  writes all the while, reading whole batches only. What is no file, a
  pipe or a device, refused before it is opened. The pool's size and the
  connect timeout by what they do, against a server that holds its tables
  locked or never answers, and the environment as Python has it. A reader
  closed, used by many threads at once, waiting on a lock without the
  interpreter, and carried into a forked process. And every table and
  example of [python-reader.md](python-reader.md), its PostgreSQL commands
  among them.
- **The worker** (`test_worker_jobs.py`, `test_runtime.py`): strategy
  bundles run by a `quantforge serve run` whose workers are the
  interpreter the tests run on, the binary the reader's tests run, on a
  store of either backend. The goldens' SMA cross as a bundle makes their
  trades, equity curve and summary digit for digit; a strategy that raises
  fails its job with its traceback in the job's log; a package the server
  does not offer is refused as a bundle is uploaded and as a job is
  submitted, and an import a bundle may not make as it runs; what a
  strategy writes is its job's log; every job has a worker of its own,
  whose environment, parameters, traceback and log hold nothing of the
  store's URL; and how long a job takes to reach its first bar is measured
  and printed. Then the runtime in the test's own process: every import
  rule and every refusal, word for word, and what a worker offers
  ([the Python worker](python-worker.md#testing-it)).
- **The client** (`test_client.py`, `test_client_jobs.py`,
  `test_client_page.py`): first against servers of the test's own, which
  answer what a test tells them to and record what they were asked. What
  is no server's URL and no token refused before anything is asked; the
  token as the argument, then the variable, then none, sent with every
  request, never shown, and never made for a server it may not cross the
  network to; the protocol version agreed on at the first use and once;
  every code the protocol has, read from the crate, raised as its exception
  with the server's message word for word; a server that checks its token,
  which no QuantForge server does yet, refusing a missing and a wrong one
  as an `AuthError`; a request sent again while the server is busy, five
  tries in all at most; a server that is not there, a connection that
  breaks, an answer cut short, a redirect and what is not HTTP, each named
  for what it is; an answer read by the client itself and held to what
  HTTP writes, to the longest body that is taken and to the most that is
  taken of what frames one; answers put together by a seed from what a
  server sends and what none does, each read or raised as an exception of
  the client's own; and every status the protocol does not write refused,
  member by member. Then against a real
  `quantforge serve run` on SQLite and on PostgreSQL, its workers the
  interpreter of the run, with a job in each
  state: one that succeeded, one that failed, one that runs until it is
  cancelled, and one queued behind it. The client lists and gets them as
  the server's own routes give them, cancels the queued one and the
  running one, is refused for one that has ended, for a job the server
  does not hold and for a limit it does not take, each in the server's
  words, and reaches the next server on the same store after the first
  stopped. And every table and example of
  [python-client.md](python-client.md), the examples against such a
  server.
- **Bundles and submission** (`test_bundles.py`, `test_source.py`,
  `test_submit.py`, `test_submit_jobs.py`, `test_bundles_page.py`): first
  the bundle, in the test's own process. The bundle spec's shared cases,
  `tests/fixtures/bundle_cases.json`, hold through the package, each case
  of files with its code hash or its problems, and each code hash is
  checked against a builder of the test's own, written on the standard
  library's `tarfile` as the spec shows one; a directory is collected as
  the spec collects one, and gives the same bundle whatever its files'
  times, modes and order; a manifest that TOML 1.1 allows and a server
  refuses is refused on every Python, though Python's own `tomllib` reads
  it from 3.15 on; a manifest is written in its one form, and every value a
  parameter is reads back as the value it was, a float between two shortest
  texts among them; every refusal of a strategy, an entry, a parameter and
  a package, word for word; a strategy's files are never imported or run,
  and no module of the package names a pickler. Then a class as
  `submit_class` sends one: its statement found in a file and, for a class
  whose module has none, in the cell that defined it, as a notebook's
  kernel keeps one; the file it writes run where nothing else is defined;
  every name its source reads and does not define refused with what to do,
  one its body reads before the class has set it and one the class sets
  and reads inside a function or a comprehension among them; that answer
  held to Python itself, over four thousand class bodies put together from
  a seed, each of which runs exactly when the reader finds no name to
  refuse, and on every way through a thousand more that may be left early,
  each run once for every setting of the points that decide what raises in
  it, what is taken and what matches, and held to it read by read: no read
  is refused that some way satisfies, and in a body with no loop, no
  `except*` and no `del` that may fail, a read is refused exactly where
  some way reaches it and none satisfies it; so is every small body of a
  loop around another statement that has blocks, none of them left to a
  seed; the seeded ones held to one hash each, the same on whichever Python
  reads them; nothing asked of what a class's attributes hold; and every
  class it does not send, one that takes a variable from the function
  around it among them. Then the requests, against servers of the
  test's own that record what they were asked: their order, what is sent,
  the window read within the bars the server stores, nothing asked for
  what is refused locally and nothing uploaded for a bundle the server's
  runtime cannot run, an upload asked again while the server is busy and a
  submission never, and a receipt or a job that is not of what was sent
  refused. Then against a real `quantforge serve run` on SQLite and on
  PostgreSQL: the goldens' strategy submitted as a directory makes their
  trades, equity curve and summary digit for digit; the code hash the
  client computes is the one the server holds the bundle under and the one
  the command line's `backtest --remote` computes for the same files and
  flags, which the server answers by saying it holds that bundle already;
  a strategy's worker says in its job's log what it was built from, which
  is what was given; a class submitted by its source makes the goldens'
  trades; and bundles that are too large, or need a package the server
  does not offer, leave the server's store without a bundle. And every
  table and example of [python-bundles.md](python-bundles.md), the examples
  against such a server.
- **The page of the types** (`test_types_page.py`): every table of
  [python-types.md](python-types.md), every signature, every refusal it
  shows, and its examples, which run as they are written.
- **The distribution** (`test_distribution.py`): that the package
  imported is the one installed, its name, one oldest Python stated alike
  by the crate's PyO3 feature, `requires-python`, the installed metadata
  and the wheel's tag, the repository's license, and that it brings the
  package and nothing beside it.
- **This page and the manifests** (`test_repository.py`): the tables and
  the tree above, the builds among them, the readme's example, and what
  [the workspace](#the-workspace) says; and CI as
  [ci.md](ci.md#the-wheels) says it builds and tests the package, the
  wheel of each platform among it.

A build's storage backends cannot be read from anywhere but the build
itself, so the tests are told when it is the SQLite-only one:

```bash
maturin develop --no-default-features
QUANTFORGE_PY_TEST_SQLITE_ONLY=1 pytest
```

Unset, they expect the package as it is published, with both stores. Any
other value stops the run.

They are told as well when they run on the oldest NumPy and the oldest
pandas the extras take, each of which `pyproject.toml` pins in a dependency
group of its own:

```bash
pip install "numpy==2.0.0" "pandas==2.2.2"
QUANTFORGE_PY_TEST_OLDEST_NUMPY=1 QUANTFORGE_PY_TEST_OLDEST_PANDAS=1 pytest
```

Told so, they hold the NumPy and the pandas of the run to those pins.
Unset, they take any NumPy and any pandas the extras do.

And they are told when the package is installed without its extras, where
there is no NumPy and no pandas to import:

```bash
pip uninstall --yes numpy pandas
QUANTFORGE_PY_TEST_WITHOUT_NUMPY=1 QUANTFORGE_PY_TEST_WITHOUT_PANDAS=1 pytest
```

Told so, the tests of the arrays and of the frames are skipped, and the
rest hold the package to working without either: the readme's examples of
the arrays and of the frames have to say what to install. Each extra has a
variable of its own, so a run with NumPy and without pandas, as
`pip install "quantforge-py[numpy]"` leaves an environment, is told the
second alone, and has the tests of the arrays; a run without NumPy is one
without pandas, which does not import without it, and is told both. A run
stops before its first test where it is not what it was said to be, one
with a NumPy or a pandas that was said to have none or one without that was
not, so that no run passes for want of the tests it skipped.

The reader's tests on PostgreSQL need a server: the one
`QUANTFORGE_TEST_POSTGRES_URL` names, the variable the crate's own tests
read, reached over plain TCP on this machine by a user allowed to create
databases. Each test has a database of its own there, which it drops.

```bash
eval "$(scripts/postgres-test-servers.sh start 18)"
cd quantforge-py
pytest
```

Without the variable those tests skip, and the same tests still run on
SQLite. Where `QUANTFORGE_TEST_POSTGRES_REQUIRED` is `1`, as in CI, a test
that would skip fails instead, so that no run passes there for want of a
server. The SQLite-only build opens no PostgreSQL store, and skips them
whatever it is told. The tests speak to the server with the standard
library alone, and need no driver.

Some tests wait on another thread, which a fault in how the interpreter is
held could leave waiting for good. A test that takes longer than a minute
ends the run, with the traceback of every thread (`faulthandler_timeout`
in `pyproject.toml`), which is why the tests need pytest 9.

The crate has no Rust tests of its own: a test binary would have to link
the Python library, and everything the extension does is reached from
Python. Clippy checks it, with and without the default features:

```bash
cargo clippy -p quantforge-py --all-targets -- -D warnings
cargo clippy -p quantforge-py --all-targets --no-default-features -- -D warnings
```

PyO3's build script asks a Python for its configuration, so these need
`python3` on the `PATH`, 3.11 or later, or `PYO3_PYTHON` naming one. The
crate forbids unsafe code as the `quantforge` crate does, `cargo fmt --all`
formats it, and its files are held to the same
[size budget](../CONTRIBUTING.md#file-size-budget).

`scripts/python-package-check.sh` runs all of it: the lints, the crate's
check with Rust 1.85, the `quantforge` binary, which it builds for the
reader's tests, the wheel with its extras on every Python it is given, the
wheel on the oldest NumPy and the oldest pandas, the wheel with NumPy
alone, the wheel without its extras, the SQLite-only wheel, and the
in-place install. It is CI's `python` job, which names the oldest Python
and the newest, and has a PostgreSQL server for the reader's tests:
[ci.md](ci.md#the-python-package).

`scripts/python-wheel-check.sh` tests a wheel built for release, as CI's
`wheel` legs test the wheel of each platform: the one wheel of the
directory it is given, installed with its extras into a fresh virtualenv of
every Python it is given, with the binary built for the reader's tests and
the whole suite run against it. It tells the tests nothing, so the wheel
has to be the package as it is published, with both stores. It runs on
Linux, macOS and Windows, there in Git Bash. From the repository's root:

```bash
maturin build --release --manifest-path quantforge-py/Cargo.toml --out dist
scripts/python-wheel-check.sh dist python3.11 python3.15
```

A directory that holds no wheel, or more than one, is refused, so that no
wheel another build left there is tested in the place of the one built.

## The workspace

The repository is a Cargo workspace of two packages: the `quantforge`
crate at its root, and `quantforge-py` as its one member. **Nothing in
what the crate builds, tests or publishes changed:**

- **A cargo command run at the root without `-p` acts on the `quantforge`
  crate alone**, as before: `cargo build`, `cargo test`, `cargo clippy`,
  `cargo package` and `cargo publish`. Two of the crate's checks read the
  member, and neither builds it: `cargo fmt --all` formats both packages,
  and the file-size budget covers both.
- **`quantforge-py` never goes to crates.io.** Its manifest says
  `publish = false`, so cargo refuses to publish it; its wheels go to PyPI.
- **The crate's package holds nothing of it.** Cargo leaves a nested
  package out of the one around it, the crate's `exclude` names the
  directory as well, and `scripts/release-check.sh` fails if the package
  list names a file of it.
- **One `Cargo.lock`** serves both. It gained PyO3, the `numpy` crate and
  what the two build on, and no package the crate uses changed version.
- **One release profile** serves both, the one in the root `Cargo.toml`.
- **Rust 1.85** is the oldest for both.

The spikes under `spikes/` are workspaces of their own, as before.

## What is left

| What | Story |
| --- | --- |
| The goldens reproduced from candles read out of each store, and by a server's job | QUA-458 |
| A Python strategy run by the command line | QUA-461 |
| A job waited for, its log followed, its result as DataFrames, and its provenance | QUA-468 |
| Type stubs and the `py.typed` marker | QUA-471 |
| The release to PyPI, and whether it carries a source distribution | QUA-476 |
