# The Python package

`quantforge-py` is QuantForge for Python: the package a strategy written in
Python will run through, on the same deterministic Rust engine as the
`quantforge` command line. This page says what the package is called, what
it holds, which Pythons it runs on, how it is built and tested, and what
adding it changed for the `quantforge` crate. The code is `quantforge-py/`.

**The package is not released, and has no function that runs a backtest
yet.** It builds, installs and imports, says what it is, and holds the
engine's types: what a market is, candles, a backtest's settings and
result, and the strategy a class implements
([the Python types](python-types.md)). The engine drives such a class
through [the strategy adapter](#the-strategy-adapter). The backtest entry
point and the candle readers are bound by the stories that follow.

## Contents

- [Names](#names)
- [What it holds](#what-it-holds)
- [Two versions](#two-versions)
- [Python versions](#python-versions)
- [Storage backends](#storage-backends)
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
class implements, and the two conversions between a decimal and a float:

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
| `decimal_to_f64` | The float nearest to a decimal |
| `decimal_from_f64` | The decimal of a float's shortest text |
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
name. Importing it opens no store and no connection, and none of its types
holds a credential.

[The Python types](python-types.md) is what each type takes and gives: how
a decimal crosses, every signature and field, what each refuses, and how a
strategy is called. A price, a quantity or an amount is a
`decimal.Decimal` on both sides, and never a float.

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

Two kinds of interpreter are left out:

- **Free-threaded builds** (`python3.14t`) do not load a wheel built for
  `abi3`. The stable ABI they share with the default build, `abi3t`, starts
  at Python 3.15.
- **PyPy and GraalPy** do not take a `cp311-abi3` wheel.

## Storage backends

The package is built with the crate's `postgres` feature, as every
published artifact is ([parity](parity/README.md#artifacts)), so
`STORAGE_BACKENDS` is `("sqlite", "postgres")`.

`--no-default-features` builds the SQLite-only package, which holds no
PostgreSQL code and says so: `STORAGE_BACKENDS` is `("sqlite",)`. It is
built and tested, and never published as `quantforge-py`.

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
| Python: the function the package will run a backtest with | The thread that called, attached throughout | Raises its own exception, in the caller |
| One that embeds Python: a server's worker, the command line | A thread of the host's own, attached once around the run and detached while the engine computes | Is the engine's error, with the traceback in its text |

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

**No function of the package runs a backtest yet.** The extension module
holds one private entry, `quantforge_py._native._backtest`, which runs a
strategy in the engine as either kind of host does. The package's own
tests reach the adapter through it, and the story that binds `run_backtest`
builds on the same adapter.

## Layout

A package of two halves in one directory: the crate that compiles the
extension module, and the Python package the extension is built into.

<!-- begin: layout -->
```text
quantforge-py/
├── Cargo.toml              the crate: PyO3 with the stable ABI of 3.11, the `postgres` feature, never published to crates.io
├── README.md               what PyPI shows for the package
├── pyproject.toml          the distribution: its name and metadata, maturin as the build backend, where the package and the extension go
├── python/
│   └── quantforge_py/
│       └── __init__.py     the package: publishes the extension module's names
├── src/
│   ├── adapter.rs          the engine's Strategy over a Python object: a thread attached for each callback, a failure with its traceback
│   ├── backtest.rs         BacktestConfig, EquityPoint and BacktestResult: a backtest's settings and its result
│   ├── bench.rs            the private entry the adapter is tested through: one backtest, as each kind of host runs one
│   ├── candles.rs          Candle and CandleBatch: one bar, and bars held column by column
│   ├── convert.rs          how a value crosses: an exact decimal, an integer, text, and a refusal in the engine's words
│   ├── floats.rs           decimal_to_f64 and decimal_from_f64: the one crossing between a decimal and a float
│   ├── lib.rs              the extension module, quantforge_py._native
│   ├── market.rs           Symbol, Interval, Category, ExchangeId and MarketId: what a market is
│   ├── strategy.rs         Strategy, StrategyContext and TargetPosition: the class a strategy derives from, what its callbacks are handed, what they hand back
│   └── trade.rs            ClosedTrade: one closed round trip, with the venue's order IDs as text
└── tests/
    ├── conftest.py         what the tests share: the checkout's manifests, the installed distribution, how a page and a refusal are read
    ├── test_backtest.py    a backtest's config, a closed trade, an equity point and a result
    ├── test_candles.py     a candle, and a batch of candles by its candles and by its columns
    ├── test_decimals.py    how a decimal crosses, what is refused, and the float policy's shared cases
    ├── test_distribution.py  the installed distribution: its name, its Pythons, its license, its files
    ├── test_market.py      a symbol, an interval, a product family, a product line and a market
    ├── test_package.py     the imported package: its names, its classes, its versions, its storage backends
    ├── test_repository.py  this page, the readme and the manifests, held to the package
    ├── test_strategy.py    a target, a context, and the class a strategy derives from
    ├── test_strategy_cases.py  the strategy boundary's shared cases: a Python strategy called and obeyed as the engine's own is
    ├── test_strategy_failures.py  a callback that raises or returns what it may not, a strategy's name, and what is no strategy
    ├── test_strategy_hosts.py  the thread a callback runs on, the interpreter it holds, and a thread's own state from bar to bar
    └── test_types_page.py  the page of the types, held to the package
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
maturin develop
python -c "import quantforge_py"
```

`maturin develop` compiles the extension in the `dev` profile and installs
the package into the virtualenv in place: the virtualenv imports
`quantforge-py/python/quantforge_py/`, so a change to a Python file needs no
rebuild, and a change to the Rust code needs `maturin develop` again.

`maturin build` writes a wheel to the repository's `target/wheels/`
instead:

| Command | What it builds |
| --- | --- |
| `maturin build` | A wheel in the `dev` profile, as CI tests it |
| `maturin build --release` | An optimized wheel, in the workspace's release profile |
| `maturin build --no-default-features` | The SQLite-only package |

A wheel carries the package, the extension module, the repository's
`LICENSE`, and a CycloneDX bill of materials of the crate's dependencies,
which maturin writes.

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
cd quantforge-py
pytest
```

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
- **The page of the types** (`test_types_page.py`): every table of
  [python-types.md](python-types.md), every signature, every refusal it
  shows, and its examples, which run as they are written.
- **The distribution** (`test_distribution.py`): that the package
  imported is the one installed, its name, one oldest Python stated alike
  by the crate's PyO3 feature, `requires-python`, the installed metadata
  and the wheel's tag, the repository's license, and that it brings the
  package and nothing beside it.
- **This page and the manifests** (`test_repository.py`): the tables and
  the tree above, the readme's example, and what
  [the workspace](#the-workspace) says.

A build's storage backends cannot be read from anywhere but the build
itself, so the tests are told when it is the SQLite-only one:

```bash
maturin develop --no-default-features
QUANTFORGE_PY_TEST_SQLITE_ONLY=1 pytest
```

Unset, they expect the package as it is published, with both stores. Any
other value stops the run.

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
check with Rust 1.85, the wheel on every Python it is given, the
SQLite-only wheel, and the in-place install. It is CI's `python` job, which
names the oldest Python and the newest: [ci.md](ci.md#the-python-package).

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
- **One `Cargo.lock`** serves both. It gained PyO3 and what PyO3 builds
  on, and no package the crate uses changed version.
- **One release profile** serves both, the one in the root `Cargo.toml`.
- **Rust 1.85** is the oldest for both.

The spikes under `spikes/` are workspaces of their own, as before.

## What is left

| What | Story |
| --- | --- |
| `run_backtest`, the function that runs a strategy | QUA-470 |
| NumPy and pandas interchange, and the candle reader | QUA-475, QUA-473, QUA-472 |
| Wheels built and tested on Linux, macOS and Windows | QUA-474 |
| Type stubs and the `py.typed` marker | QUA-471 |
| The release to PyPI, and whether it carries a source distribution | QUA-476 |
