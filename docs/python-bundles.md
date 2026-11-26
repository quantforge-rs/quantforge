# Submitting a backtest from Python

`client.backtest.submit` makes a strategy's files into a
[strategy bundle](bundle-spec.md), checks it, uploads it to a QuantForge
server and submits a backtest of it; `client.backtest.submit_class` does
the same for a class written in a notebook, from its source. This page is
the contract of both: what is bundled, how its manifest is written, what is
checked before anything is sent, what a server is asked and in which order,
what is raised, and what is never done. **Nothing is pickled, and nothing
of a strategy is run where it is submitted from.**

It applies two pages and restates neither. What a bundle is, file by file
and byte by byte, is the [bundle spec](bundle-spec.md). Which server a
client reaches, with which token, and what a request fails with is
[the Python client](python-client.md), which gives the `Job` these two
calls answer.

```python
>>> import time
>>> import quantforge_py as qf
>>> client = qf.Client("http://127.0.0.1:9600")
>>> market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
>>> config = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)
```

`qf` is what the examples use. They ask a server that runs on this machine,
at the address `serve run` listens on unless it is told otherwise, whose
workers run Python bundles, and which stores a day of one-minute candles of
`BTCUSDT` on Binance Spot, from noon on the first day of 2026, UTC. And
they submit `my_strategy`, a directory beside them that holds one file,
`sma_cross.py`: the SMA cross of [the goldens](golden-parity.md).

## Contents

- [The two calls](#the-two-calls)
- [A strategy's files](#a-strategys-files)
- [The manifest that is written](#the-manifest-that-is-written)
- [Parameters](#parameters)
- [Packages](#packages)
- [What a job runs on](#what-a-job-runs-on)
- [What is checked, and when](#what-is-checked-and-when)
- [The code hash](#the-code-hash)
- [A class from a notebook](#a-class-from-a-notebook)
- [Nothing is pickled](#nothing-is-pickled)
- [What is raised](#what-is-raised)
- [Refusals](#refusals)
- [What is left](#what-is-left)

## The two calls

As Python's `inspect.signature` gives them:

<!-- begin: signatures -->
```text
Client.backtest.submit(strategy, *, entry=None, params=None, deps=None, market, start=None, end=None, config)
Client.backtest.submit_class(strategy, *, imports='import quantforge_py as qf', params=None, deps=None, market, start=None, end=None, config, source_only=False)
```
<!-- end: signatures -->

<!-- begin: arguments -->
| Argument | Of | What it is |
| --- | --- | --- |
| `strategy` | both | For `submit`, a strategy's `.py` file or the directory of its files, a `str` or an `os.PathLike`; for `submit_class`, a class derived from `Strategy` |
| `entry` | `submit` | The strategy's file in the bundle and its class, such as `"strategy.py:MyStrategy"`, as a manifest writes it; left out where the directory holds a `quantforge.toml` |
| `imports` | `submit_class` | The import statements the class's source needs, written above it in the bundle's file |
| `params` | both | The strategy's parameters, a `dict` of values JSON holds: the class is built from them, as keyword arguments |
| `deps` | both | The packages the strategy imports: a `dict` that pins each at a version, or a `list` or a `tuple` of names, each pinned at the version the server offers |
| `market` | both | The market to run on, a `MarketId` |
| `start` | both | The first bound of the window: the job runs on the bars the server stores from it on, or from the first it stores |
| `end` | both | The last bound of the window, to which the job runs, or to the last bar the server stores |
| `config` | both | How fills are simulated, a `BacktestConfig` |
| `source_only` | `submit_class` | `True` says that the caller knows that the class's source text is all that is sent of it |
<!-- end: arguments -->

```python
>>> job = client.backtest.submit(
...     "my_strategy",
...     entry="sma_cross.py:SmaCross",
...     params={"fast": 20, "slow": 50},
...     market=market,
...     config=config,
... )
>>> job.status.ended, job.status.market == market, job.status.config == config
(False, True, True)
>>> job.status.start_open_ms, job.status.end_open_ms
(1767268800000, 1767355140000)
>>> job.status.bundle[:7], len(job.status.bundle)
('sha256:', 71)
```

- **Either call gives the job the server queued,** as a `Job`, with the
  status the server answered: what it runs, the bundle's code hash among
  it, and the window it runs on. It does not wait for the job
  ([what is left](#what-is-left)): the job is asked where it stands, as any
  job is.

```python
>>> while not job.refresh().ended:
...     time.sleep(0.1)
...
>>> job.status.state
'succeeded'
```

- **`market` and `config` are required,** and named. A backtest's settings
  have no default anywhere in the package: what a job charges and what it
  starts with are what its caller wrote.
- **Every argument is checked, and the bundle built and checked, before
  the server is asked anything.** What is wrong with a call is known
  without a server, and what is wrong with a bundle before it is uploaded
  ([what is checked, and when](#what-is-checked-and-when)).

## A strategy's files

**`strategy` names what to bundle: a `.py` file, or a directory.**

- **A file is bundled under its own name,** at the bundle's root, beside
  the manifest written for it. Nothing that lies beside the file is read.
  It is a `.py` file, the one its entry names: a file of another kind is
  refused, and a strategy of more than one file is submitted as its
  directory.
- **A directory is collected as the [bundle spec](bundle-spec.md#collecting-a-directory)
  collects one:** every regular file below it, but for entries whose name
  starts with `.`, such as `.git` and `.venv`, and directories named
  `__pycache__`. A link is refused, and never followed.
- **Nothing else is left out.** A file a bundle cannot hold, a `notes.md`
  or a `model.pkl`, refuses the bundle, and is named: the same directory
  makes the same bundle, or none.
- **A directory's own `quantforge.toml` is its manifest,** sent as it is,
  comments and all. `entry`, `params` and `deps` are refused beside it:
  what a bundle runs is said in one place.
- **A file, and a directory without a manifest, get one written** from
  `entry`, `params` and `deps`, in the bundle and not on disk: nothing is
  written where the strategy's files are.
- **A file's bytes go as they are.** No line ending and no encoding is
  changed, and no file is read as Python: a file is its bytes here.

A bundle stays within the spec's limits, which a server holds an upload to
as well:

<!-- begin: limits -->
| Limit | The most |
| --- | --- |
| `max_file_bytes` | 1048576 bytes in one file (1 MiB) |
| `max_total_bytes` | 8388608 bytes in all the files together (8 MiB) |
| `max_files` | 256 files |
<!-- end: limits -->

## The manifest that is written

A manifest written from the arguments of the first example, for a server
that runs Python 3.12, with `deps={"numpy": "2.4.6"}` beside them:

<!-- begin: written manifest -->
```toml
format = 1
entry = "sma_cross.py:SmaCross"
engine = "==0.2.0"
python = "3.12"

[params]
fast = 20
slow = 50

[deps]
numpy = "2.4.6"
```
<!-- end: written manifest -->

- **`entry` is the argument, as given:** the strategy's file in the
  bundle, a colon and its class. It is written as a manifest writes it,
  `strategy.py:MyStrategy`, and as the command line's `--entry` takes it:
  a module's name, `strategy:MyStrategy`, is refused, with the entry to
  write instead.
- **`engine` is the engine this package was built with, exactly:**
  `==` and `ENGINE_VERSION`. A server runs the engine a local
  `run_backtest` runs, or refuses the bundle and says so.
- **`python` is the server's,** which it says before the manifest is
  written: a bundle runs only where the server's Python is the manifest's.
- **`[params]` and `[deps]` are the two arguments, each sorted by name,**
  and left out where there is nothing to write.
- **The same arguments write the same manifest,** byte for byte, in
  whatever order a `dict` was filled: so the same strategy makes the same
  bundle, with the same code hash.
- **It is the manifest the command line writes.** `quantforge backtest
  --remote` writes one from `--entry` and `--params` with the same writer,
  the engine's own, so the same files and the same parameters make the same
  bundle from either. A float is written as the engine writes one, which is
  not always the text Python prints for it, and is always the same float.

## Parameters

**`params` is the strategy's parameters: the class is built from them, as
keyword arguments,** `SmaCross(fast=20, slow=50)` for
`params={"fast": 20, "slow": 50}`. A parameter is a value JSON holds, of
exactly one of these types:

<!-- begin: params -->
| A parameter | Reaches the strategy as |
| --- | --- |
| `str` | The same text |
| `bool` | The same flag |
| `int` | The same whole number, within ±(2^53 − 1), which every JSON reader reads exactly |
| `float` | The same float, where it is finite |
| `list` | A `list` of the same values, in the same order |
| `dict` | A `dict` of the same values under the same keys, each key a `str` that does not start with `$`, in the order of the keys' names |
<!-- end: params -->

- **What the strategy is handed is what was given,** value for value and
  type for type: `2` stays an `int`, `2.0` a `float`, and `True` a `bool`.
  The client reads the manifest it wrote back, with the reader a server
  reads it with, and sends nothing where the two differ.
- **A value of any other type is refused, one that merely converts to
  one of these among them:** a `tuple`, a `Decimal`, a NumPy number, `None`,
  a `date`, and a type derived from `int` or from `str` are each a
  `TypeError` that names the parameter, such as `params.window.size`. A
  strategy built locally from a `tuple` would be built from a `list` on a
  server; so neither is.
- **An amount that must stay exact goes in a `str`,** of which the
  strategy makes a `Decimal` itself. A float is a float on both sides.
- **The values are copied as they are checked.** Changing the `dict`
  afterwards changes nothing of what is sent.
- **Values lie at most 32 levels deep in a manifest,** whose own table is
  the first level and `params` the second.

## Packages

**`deps` declares the packages the strategy imports.** A bundle's own code
imports the standard library, its own modules, `quantforge_py` and the
packages its manifest declares, and nothing else
([the Python worker](python-worker.md#what-a-bundle-may-import)).

- **A `dict` pins each package at a version,** as the manifest's `[deps]`
  table does: `deps={"numpy": "2.4.6"}`.
- **Names alone are pinned at the version the server offers:**
  `deps=["numpy", "pandas"]`. Where a server offers more than one version
  of a package, the caller says which, with a `dict`.
- **A name is a distribution's, in its normalized form,** `scikit-learn`
  and not `sklearn`, and a version is a pin, `2.4.6` and no range
  ([bundle-spec.md](bundle-spec.md#deps)). Any other spelling is refused,
  with the one to write.
- **A package the server does not offer, and a version it does not,
  refuse the bundle before it is uploaded,** in the words of the check a
  server makes of an upload:

```python
>>> try:
...     client.backtest.submit(
...         "my_strategy",
...         entry="sma_cross.py:SmaCross",
...         deps=["scipy"],
...         market=market,
...         config=config,
...     )
... except qf.BundleError as refused:
...     [code for code, _ in refused.problems]
...
['dependency_not_allowed']
```

## What a job runs on

**`market` is the market, a `MarketId`,** named in full: a venue's name
alone names no market. The server is asked by the protocol's own names, the
exchange and the family apart.

**`start` and `end` bound the window by the time each bar opened, both
included,** and mean what the command line's `--start` and `--end` mean:
the bars the server stores between them.

- **A bound is written in the three ways a candle reader's range takes
  one:** an `int` of milliseconds since the Unix epoch, a
  `datetime.datetime` with a time zone, or an RFC 3339 `str`
  ([the candle reader](python-reader.md#a-range-of-time)). A time between
  two milliseconds bounds the window where it stands.
- **A bound that is left out reaches the first or the last bar the server
  stores,** and one that lies beyond them reaches no further. The job's
  status says the window it was given: `start_open_ms` and `end_open_ms`.
- **A window the server stores no bar of is refused before anything is
  uploaded,** a `ValidationError`, and so is a market it stores nothing
  of. One that falls in a gap between two stored bars reaches the server,
  whose job then fails as a run over no candles does.

```python
>>> again = client.backtest.submit(
...     "my_strategy/sma_cross.py",
...     entry="sma_cross.py:SmaCross",
...     params={"slow": 50, "fast": 20},
...     market=market,
...     start="2026-01-01T18:00:00Z",
...     end="2026-01-01T23:59:00Z",
...     config=config,
... )
>>> again.status.start_open_ms, again.status.end_open_ms
(1767290400000, 1767311940000)
>>> again.status.bundle == job.status.bundle
True
>>> client.backtest.submit(
...     "my_strategy",
...     entry="sma_cross.py:SmaCross",
...     market=market,
...     start="2027-01-01T00:00:00Z",
...     config=config,
... )
Traceback (most recent call last):
    ...
quantforge_py.ValidationError: the server at http://127.0.0.1:9600/ stores binance spot BTCUSDT 1m from 2026-01-01T12:00:00Z to 2026-01-02T11:59:00Z, outside the window from 2027-01-01T00:00:00Z to 2026-01-02T11:59:00Z
```

**`config` is how fills are simulated, a `BacktestConfig`:** the cash a job
starts with, the fee on every fill, and whether a position still open
after the last bar is closed out. Each amount is sent as the exact text of
its decimal, with every digit it was given.

## What is checked, and when

<!-- begin: steps -->
| Step | Asks the server | What happens |
| --- | --- | --- |
| 1 | — | Every argument is checked. The strategy's files are read, and the bundle is built and checked against the bundle spec; a manifest that is written is written for no server in particular yet |
| 2 | `GET /version` | The protocol version is agreed on, at a client's first use and once |
| 3 | `GET /v1/runtime` | What the server's workers run bundles on. A manifest that is written is written for its Python, a package named alone is pinned at its version, and the bundle is built again and admitted against it |
| 4 | `GET /v1/markets` | The bars the server stores of the market, which the window is read within |
| 5 | `PUT /v1/bundles/{code_hash}` | The bundle's canonical tar is uploaded, under the code hash computed here |
| 6 | `POST /v1/backtests` | The job is submitted, once, and its status answered |
<!-- end: steps -->

- **The checks are the server's own.** The bundle is collected, checked,
  packed and hashed by the engine's code, which the package carries: the
  code a server checks an upload with. What passes here passes a server of
  the same release, and what is refused here is refused there in the same
  words. A manifest is read as
  a server reads one, as TOML 1.0, whatever TOML the Python that runs the
  client reads: Python's own `tomllib` reads TOML 1.1 from Python 3.15 on,
  which a server refuses.
- **A bundle that breaks the spec asks the server nothing.** One that is
  too large, holds a file of another kind, or names an entry it does not
  hold is a `BundleError` at step 1.
- **A bundle the server cannot run is uploaded nowhere.** A package it does
  not offer, another version of one, another Python than a manifest names
  and an engine the manifest refuses are each a `BundleError` at step 3,
  which names the server.
- **A server that runs no bundles is sent none.** One started without
  `--job-python` says so at step 3, a `ServerUnavailable`.
- **What is uploaded is what was checked last.** A directory is read again
  at step 3, as it is then, and the bundle checked whole again: a file that
  changed between the two steps is in the bundle as it changed, or refuses
  it.
- **An upload may be sent again, and a submission never.** A server stores
  a bundle once, under the hash of its bytes, so the upload is asked again
  while the server answers `503` with `Retry-After`, as a read is
  ([what is asked again](python-client.md#what-is-asked-again)). A
  submission is sent once: one the server took but could not answer for
  would run twice.
- **A submission that fails as a `ServerUnavailable` may have been taken
  all the same.** List the server's jobs, `client.backtest.list()`, before
  submitting it again.
- **An answer is held to what was sent.** A receipt for another bundle
  than the one uploaded, and a job that does not run what was submitted,
  are no answers: each is a `RemoteError` that says what came instead.

## The code hash

**A bundle is named by its code hash: the SHA-256 of its canonical tar,**
which the same files give on every machine
([bundle-spec.md](bundle-spec.md#the-canonical-tar)). A job's
`status.bundle` is that name.

- **The client computes it, and the server checks it.** A bundle is
  uploaded under the hash computed here. The server takes the SHA-256 of
  the bytes it received, and refuses an upload that hashes to another
  name, a `ValidationError` in its own words; then it reads the bytes back
  strictly, so that only a canonical tar is stored.
- **The server's receipt names the bundle again,** and its size. A receipt
  for another bundle, or of another size, is refused, and nothing is
  submitted.
- **The same files make the same bundle, whoever builds it:** this
  package, the command line's `backtest --remote`, or a builder written on
  Python's `tarfile` as the spec shows one. The tests hold all three to
  one name, and to the server's.

## A class from a notebook

**`submit_class` submits a class by its source text,** for a strategy
written in a notebook's cell, which is no file to submit.

```python
>>> class Momentum(qf.Strategy):
...     """Long while the last close is above the close `lookback` bars before it."""
...
...     def __init__(self, lookback: int):
...         self.lookback, self.closes = lookback, []
...
...     def on_bar(self, ctx, bar):
...         self.closes.append(bar.close)
...         if len(self.closes) <= self.lookback:
...             return None
...         rising = self.closes[-1] > self.closes[-1 - self.lookback]
...         return qf.TargetPosition.LONG_ALL_IN if rising else qf.TargetPosition.FLAT
...
>>> job = client.backtest.submit_class(
...     Momentum,
...     params={"lookback": 5},
...     market=market,
...     config=config,
...     source_only=True,
... )
>>> while not job.refresh().ended:
...     time.sleep(0.1)
...
>>> job.status.state
'succeeded'
```

The bundle it makes holds one Python file, `strategy.py`, beside a manifest
written as `submit` writes one, whose entry is `strategy.py:Momentum`:

<!-- begin: class file -->
```python
import quantforge_py as qf


class Momentum(qf.Strategy):
    """Long while the last close is above the close `lookback` bars before it."""

    def __init__(self, lookback: int):
        self.lookback, self.closes = lookback, []

    def on_bar(self, ctx, bar):
        self.closes.append(bar.close)
        if len(self.closes) <= self.lookback:
            return None
        rising = self.closes[-1] > self.closes[-1 - self.lookback]
        return qf.TargetPosition.LONG_ALL_IN if rising else qf.TargetPosition.FLAT
```
<!-- end: class file -->

**The file is `imports`, and the class's own `class` statement below
them.** That is all of it, and what it leaves behind is the caller's to
know before the call is made:

- **Only the class's source text is sent.** The server runs that text, read
  again by its own Python. It never sees the class as it is in memory: an
  attribute set on the class after it was defined, or a method replaced,
  is not in the text.
- **No import of the notebook is sent.** What the class's source needs is
  declared again, in `imports`: import statements, written above the class.
  Left out, they are `import quantforge_py as qf`, and a class that needs
  another import names every one it needs, that one among them:
  `imports="import quantforge_py as qf\nimport numpy as np"`. A package
  named there is declared in `deps` as well, or its import is refused as
  the job runs.
- **No global is sent.** A constant, a helper function or another class of
  the notebook that the class uses stays behind. It is defined inside the
  class, or the class is refused.
- **No variable of a function around the class is sent.** A class defined
  inside a function, which may use that function's variables, is refused,
  and so is one defined inside another class: a class is written at the top
  level of its module or of its cell.
- **The text is the class's as Python keeps it.** A class of a file is
  read from that file as it is now, which is not the class in memory if the
  file changed since it was imported. A class of a notebook is read from
  the cell that defined it.
- **A class is found by its own functions:** where one of them was
  compiled says which statement made the class, in a file as in a cell. A
  class that defines none is found by its name in its module's file, and
  before Python 3.13 a class says no more of where it was written than
  that: where the file defines the name more than once, such a class is
  refused.
- **Nothing of the class is run or asked to find its source.** What its
  attributes hold is told by its type: an object whose class answers for
  its attributes with code of its own is left alone.
- **A class Python keeps no source of is refused:** one made by calling
  `type`, and one typed at a prompt that keeps none. So is anything that is
  no class: a function, a `lambda`, an object of a strategy.

**`source_only=True` says that the caller knows this.** Without it the
call is refused before anything is read:

```python
>>> client.backtest.submit_class(Momentum, params={"lookback": 5}, market=market, config=config)
Traceback (most recent call last):
    ...
ValueError: submit_class sends the source text of the class as the one file of a bundle, and nothing else of where the class was written: no import, no global and no variable around it travels unless imports declares it, and the server runs that text, not the class as it is in memory here. Pass source_only=True to say that this is what is meant; a strategy kept in files is submitted with submit()
```

**Every name the class's source reads has to be defined in the file:** by
`imports`, by the class itself, or as one of Python's builtins. Any other
name refuses the call before anything is sent, with what to do about it:

```python
>>> THRESHOLD = 3
>>> class Leaky(qf.Strategy):
...     def on_bar(self, ctx, bar):
...         return None if bar.close > THRESHOLD else None
...
>>> client.backtest.submit_class(Leaky, market=market, config=config, source_only=True)
Traceback (most recent call last):
    ...
ValueError: Leaky uses THRESHOLD, which its source does not define: submit_class sends the class's own source below imports, and nothing else of where it was written; define THRESHOLD inside the class
>>> def make(threshold):
...     class Captures(qf.Strategy):
...         def on_bar(self, ctx, bar):
...             return None if bar.close > threshold else None
...
...     return Captures
...
>>> client.backtest.submit_class(make(3), market=market, config=config, source_only=True)
Traceback (most recent call last):
    ...
ValueError: Captures is defined inside the function make, and uses its threshold: a class is sent as its own source, and what it takes from around it stays behind. Write it at the top level of a module or of a notebook's cell, with what it needs inside it
```

- **The check reads the text, and runs nothing.** It is made of the names
  Python's own compiler finds in the file. A name looked up some other
  way, through `globals()`, `eval` or `getattr` on a module, is not seen
  by it, and fails where the job runs, with its traceback in the job's log.
- **A name is the class's own where the class has set it.** A class's body
  looks a name up among those it has set so far, and then outside the
  class: one it reads before it sets it, as `LIMIT = LIMIT` does, is read
  outside. Inside a function, a lambda or a comprehension of its body,
  Python does not look in the class at all: a name the class sets is read
  outside it there. A comprehension's first iterable is no part of that:
  the class's body evaluates it, so with `LIMIT = 4` set before,
  `[n for n in range(LIMIT)]` reads the class's `LIMIT`, where
  `[LIMIT for n in range(4)]` reads no name of the class's. A read outside
  the class is held to what any such read is. Where `imports` or Python's
  builtins define the name, it is theirs that is read, and the class is
  taken; where nothing does, the class is refused.
- **A private name is read as Python looks it up.** Inside `Mine`,
  `__limit` is `_Mine__limit`: that is the name an import has to give, and
  the name a refusal says.
- **Only what is certain is refused.** Whether a loop runs, and which way a
  branch goes, the text does not say. A class is taken where some way
  through its body has set a name by the time it is read. Where another
  way leaves the name unset, as a loop that does not run, the job fails
  there, with its traceback in the job's log.
- **A block may be left before its end,** and the class's body goes on
  from there. An exception may leave a block wherever a statement of it
  evaluates something, for a handler, or past a `with` whose manager takes
  it; a `break` and a `continue` leave it for their loop, and no handler
  takes them. What the block had set by then may be set there, and what it
  had not is not: a handler that reads `limit` where its `try` holds
  nothing but `limit = load()` runs only where `load()` raised, with
  `limit` never set, and is refused. Every way out of a `try` passes
  through its `finally`, and every way out of a handler clears the name
  the handler was given: what these unset is unset wherever the body goes
  on.
- **Where nothing leads, nothing is read.** What follows a `raise`, a
  `break` or a `continue` in its block does not run, and neither does the
  handler of a `try` that nothing can leave, as one that holds only
  `pass`: a read there is not held against the class, and what is set
  there is set on no way. The check takes every statement that evaluates
  something, and every `del`, to be able to fail, whatever it evaluates:
  the handler of `limit = 1` is read as one that may run.
- **The answer is the same on whichever Python reads the class, on
  whichever system.** A bundle runs on its server's Python. What annotates
  a name is held to being defined where its statement stands, as Python up
  to 3.13 evaluates it, though a later Python would not ask until the
  annotation is read. And Python's builtins are those of Python 3.11,
  which every Python the package runs on has: a name that only a later
  Python has for a builtin, or only Windows, is declared by an import, or
  refused.
- **A name that an import gives is refused with the line to add,** such as
  `add 'import numpy as np' to imports`, read from where the class was
  written. Nothing is added for the caller: an import is declared, never
  carried over.
- **`imports` holds import statements and nothing else.** A statement of
  another kind, an import relative to a package, and one with a `*` are
  refused.

A strategy kept in files is submitted with `submit`, which sends the files
themselves.

## Nothing is pickled

**No object is serialized, anywhere.** A bundle is files and a manifest, by
the [bundle spec](bundle-spec.md): pickles break when versions drift, hide
what code they depend on, and run arbitrary code when loaded.

- **A strategy travels as source files,** the ones `submit` is given, or
  the one `submit_class` writes from a class's text.
- **Its parameters travel as plain values** in the manifest, each a value
  JSON holds, and reach the strategy as the same values.
- **Nothing of a strategy is imported or run where it is submitted from.**
  A file is read as bytes, and a class's source as text.
- **No module of the package names a pickler,** and building a bundle
  loads none: the tests hold the package to both.

## What is raised

<!-- begin: raises -->
| Exception | When |
| --- | --- |
| `TypeError` | An argument is of a type it does not take |
| `ValueError` | An argument's value is one the rules refuse: a file that is no `.py` file, an entry that names no file and class, a parameter that is a number no manifest holds or lies under a key it reserves, a window that ends before it starts, a class `submit_class` does not send, a call to it without `source_only=True` |
| `FileNotFoundError` | `strategy` names no file and no directory |
| `BundleError` | The bundle breaks the bundle spec, or the server cannot run it. It is a `ValueError` |
| `ServerUnavailable` | The server runs no Python bundles; or it could not be reached, or cannot answer now |
| `ValidationError` | The server stores no bar of the market in the window; or the server refused the upload or the submission, in its own words |
| `RemoteError` | Any other failure of a request, as for every request a client makes |
<!-- end: raises -->

**A `BundleError` holds every problem of a bundle, by the spec's code.**
`problems` is a tuple of pairs, in the order the spec checks: the
[code](bundle-spec.md#checking-a-bundle), such as `file_too_large`,
`entry_not_found` or `dependency_not_allowed`, and a message that says what
to change. The codes are the spec's contract, and a message is for the
person fixing the bundle. `str()` of the exception says every problem.

- **A bundle that breaks the spec is refused as such, wherever it would be
  sent:** `the bundle is refused: …`.
- **One a server cannot run names the server:** `the server at
  http://127.0.0.1:9600/ cannot run the bundle: …`, with the problems of
  the server's own admission check.
- **A `BundleError` is no `RemoteError`.** No request failed: what was
  given cannot be submitted as it is, which is what a `ValueError` says.
- **An argument is refused before the server is asked anything,** and a
  bundle before anything is uploaded. One refusal of an argument waits for
  the server: a package named without a version, where the server turns
  out to offer more than one version of it.
- **What the client refuses on a server's behalf has no `code` and no
  `status`:** a market or a window the server stores no bar of, and a
  server that runs no bundles, are told from what the server answered, and
  no refusal of its own came with them. Each is raised as the exception
  the server's own refusal of the same would be.

Every other failure of a request is raised as
[the Python client](python-client.md#what-is-raised) says.

## Refusals

Each row is what Python raises for the call, where `client`, `market` and
`config` are the examples', `Momentum` is the class above, and `Decimal` is
`decimal.Decimal`:

<!-- begin: refusals -->
| Call | Raises |
| --- | --- |
| `client.backtest.submit("my_strategy", market=market, config=config)` | `ValueError: entry must name the strategy, such as entry="strategy.py:MyStrategy": my_strategy holds no quantforge.toml that names it` |
| `client.backtest.submit("my_strategy", entry="sma_cross:SmaCross", market=market, config=config)` | `ValueError: entry 'sma_cross:SmaCross' names a module: name the strategy's file and its class, as a bundle's manifest does, such as 'sma_cross.py:SmaCross'` |
| `client.backtest.submit("my_strategy", entry="SmaCross", market=market, config=config)` | `ValueError: entry 'SmaCross' names no .py file and class: write the strategy's file, a colon and its class, such as 'strategy.py:MyStrategy'` |
| `client.backtest.submit("my_strategy", entry="other.py:SmaCross", market=market, config=config)` | `BundleError: the bundle is refused: the entry names "other.py", which the bundle does not hold; its Python files are sma_cross.py` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:Sma-Cross", market=market, config=config)` | `BundleError: the bundle is refused: invalid entry "sma_cross.py:Sma-Cross": the class "Sma-Cross" is not a Python identifier: letters, digits, and _, not starting with a digit` |
| `client.backtest.submit("nowhere", entry="strategy.py:MyStrategy", market=market, config=config)` | `FileNotFoundError: there is no file or directory at nowhere: strategy names a strategy's .py file, or the directory its files are in` |
| `client.backtest.submit(7, entry="strategy.py:MyStrategy", market=market, config=config)` | `TypeError: strategy must be a str or an os.PathLike, not int` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params=[20, 50], market=market, config=config)` | `TypeError: params must be a dict or None, not list` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params={"cash": Decimal("10.5")}, market=market, config=config)` | `TypeError: params.cash must be a str, a bool, an int, a float, a list or a dict, not Decimal: a strategy's parameters reach it as JSON, and an amount that must stay exact goes in a str` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params={"levels": [1, (2, 3)]}, market=market, config=config)` | `TypeError: params.levels[1] must be a str, a bool, an int, a float, a list or a dict, not tuple: a strategy's parameters reach it as JSON, and an amount that must stay exact goes in a str` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params={"big": 2**53}, market=market, config=config)` | `ValueError: params.big is an integer beyond 2^53 - 1, which not every JSON reader holds exactly; write it as a str` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params={"ratio": float("nan")}, market=market, config=config)` | `ValueError: params.ratio is not a finite number` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", params={"$ref": 1}, market=market, config=config)` | `ValueError: params."$ref" starts with $, which is reserved` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", deps="numpy", market=market, config=config)` | `TypeError: deps must be a dict of each package's version, a list or a tuple of package names, or None, not str` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", deps={"numpy": 2.4}, market=market, config=config)` | `TypeError: deps['numpy'] must be a str, the version to pin, not float` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", deps=["NumPy"], market=market, config=config)` | `BundleError: the bundle is refused: deps.NumPy: invalid package name "NumPy": names are compared in their normalized form, so write it as "numpy"` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", deps={"numpy": ">=2.0"}, market=market, config=config)` | `BundleError: the bundle is refused: deps.numpy: invalid package version ">=2.0": expected a canonical version such as "2.1.3", "1.0rc1", or "2.0.post1"` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", market="BTCUSDT", config=config)` | `TypeError: market must be a MarketId, not str` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", market=market, config=None)` | `TypeError: config must be a BacktestConfig, not NoneType` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", market=market, start=1.5, config=config)` | `TypeError: start must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not float` |
| `client.backtest.submit("my_strategy", entry="sma_cross.py:SmaCross", market=market, start="2026-01-02T00:00:00Z", end="2026-01-01T00:00:00Z", config=config)` | `ValueError: the window ends before it starts: start is 2026-01-02T00:00:00Z and end 2026-01-01T00:00:00Z, each the whole millisecond it bounds the window at` |
| `client.backtest.submit_class(Momentum, market=market, config=config, source_only=1)` | `TypeError: source_only must be a bool, not int` |
| `client.backtest.submit_class(lambda ctx, bar: None, market=market, config=config, source_only=True)` | `TypeError: strategy must be a class derived from quantforge_py.Strategy, not function` |
| `client.backtest.submit_class(Momentum(5), market=market, config=config, source_only=True)` | `TypeError: strategy must be a class derived from quantforge_py.Strategy, not an object of one: pass Momentum itself, and what it is built from as params` |
| `client.backtest.submit_class(dict, market=market, config=config, source_only=True)` | `TypeError: dict is not a strategy: submit_class sends a class derived from quantforge_py.Strategy` |
| `client.backtest.submit_class(type("Made", (qf.Strategy,), {}), market=market, config=config, source_only=True)` | `ValueError: the source of Made cannot be read: Python keeps none of it, as of a class made by calling type() or typed at a prompt that keeps no source. submit_class sends a class written in a file or in a notebook's cell; a strategy kept in files is submitted with submit()` |
| `client.backtest.submit_class(Momentum, imports="THRESHOLD = 3", market=market, config=config, source_only=True)` | `ValueError: line 1 of imports is no import statement that names what it imports: imports holds import statements alone, none of them relative and none with a *` |
| `client.backtest.submit_class(Momentum, imports="import numpy as np", market=market, config=config, source_only=True)` | `ValueError: Momentum uses qf, which its source does not define: submit_class sends the class's own source below imports, and nothing else of where it was written; add 'import quantforge_py as qf' to imports` |
<!-- end: refusals -->

## What is left

| What | Story |
| --- | --- |
| A job waited for, its log followed, its result as DataFrames, and its provenance | QUA-468 |
| A server that checks the token it is sent | QUA-345 |
