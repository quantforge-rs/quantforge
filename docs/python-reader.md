# The candle reader

`CandleReader` reads the candles a store holds, by market and time range,
into a `CandleBatch`: the candles `quantforge data sync` stored, in SQLite
or in PostgreSQL, with no step between the command line's store and Python.
This page is the contract of it: which store a reader opens, what it reads,
what it says of a range that held nothing, what it never does to a store,
and what it refuses.

It applies three pages and restates none. What a market and a batch are is
[the Python types](python-types.md). Which backend is selected and how a
PostgreSQL URL is written is the command line's
[configuration](configuration.md#storage-backends), which a reader resolves
by the same rules. What a store holds is the
[storage schema](storage-schema.md).

```python
>>> import datetime
>>> import quantforge_py as qf
>>> url = "postgres://reader:PASSWORD@db.example.com/market"
```

`qf` is what the examples use, and `url` stands for a PostgreSQL URL where
one is refused. The examples read a store the command line wrote where they
run, at its default path: five minutes of `BTCUSDT` on Binance Spot, from
noon on the first day of 2026, UTC.

## Contents

- [The reader](#the-reader)
- [Which store it opens](#which-store-it-opens)
- [Reading candles](#reading-candles)
- [A range of time](#a-range-of-time)
- [A range that holds nothing](#a-range-that-holds-nothing)
- [A market is named in full](#a-market-is-named-in-full)
- [What a reader never does](#what-a-reader-never-does)
- [One SQLite in a process](#one-sqlite-in-a-process)
- [Closing, threads and processes](#closing-threads-and-processes)
- [Credentials](#credentials)
- [Refusals](#refusals)
- [What is raised](#what-is-raised)
- [PostgreSQL](#postgresql)

## The reader

As Python's `inspect.signature` gives them:

<!-- begin: signatures -->
```text
CandleReader(*, storage=None, db=None, postgres_url=None, postgres_pool_size=None, postgres_connect_timeout_secs=None)
CandleReader.load(market, *, start=None, end=None)
CandleReader.close()
```
<!-- end: signatures -->

<!-- begin: members -->
| Member | What it is |
| --- | --- |
| `CandleReader(...)` | Opens the store the settings select, to read it |
| `load(market, *, start=None, end=None)` | The candles of a `MarketId` that opened from `start` to `end`, as a `CandleBatch` that names the market |
| `close()` | Closes the store: its files, or its connections |
| `backend` | The backend the store is on: `sqlite` or `postgres` |
| `location` | Where the store is: a file's path, or `postgres://HOST:PORT/DATABASE`, without a user or a password |
| `closed` | Whether the reader is closed |
<!-- end: members -->

```python
>>> reader = qf.CandleReader()
>>> reader
<CandleReader sqlite data/market.sqlite>
>>> reader.backend, reader.location, reader.closed
('sqlite', 'data/market.sqlite', False)
```

- **A reader is an open store, and not a value.** Two readers of one store
  are two readers: one equals itself alone, and none is copied or pickled.
- **It is a context manager.** `with qf.CandleReader() as reader:` gives
  the reader and closes it as the block ends, whatever the block raises.
- **It is the one type of the package that holds a store,** and with
  PostgreSQL the one that holds a password
  ([credentials](#credentials)). A batch it gives holds neither.

## Which store it opens

**A reader opens the store the `quantforge` command line would.** Each
argument is the command line's flag of the same name, and an argument that
is left out is read from that flag's variable:

<!-- begin: settings -->
| Argument | Variable | Default | What it is |
| --- | --- | --- | --- |
| `storage` | `QF_STORAGE` | `sqlite` | The backend: `sqlite` or `postgres` |
| `db` | `QF_DB` | `data/market.sqlite` | The SQLite file, a `str` or an `os.PathLike`. Refused beside `postgres` |
| `postgres_url` | `QF_POSTGRES_URL` | none | `postgres://USER:PASSWORD@HOST:PORT/DATABASE`, required by `postgres` |
| `postgres_pool_size` | `QF_POSTGRES_POOL_SIZE` | `2` | The most connections the reader opens, an `int` from 1 to 262143, the most a PostgreSQL server serves |
| `postgres_connect_timeout_secs` | `QF_POSTGRES_CONNECT_TIMEOUT_SECS` | `5` | Seconds opening a connection may take, TLS included, an `int` from 1 |
<!-- end: settings -->

- **An argument wins over its variable, and a variable over the default,**
  setting by setting. `None` leaves an argument out.
- **With nothing set, it is SQLite at `data/market.sqlite`,** the command
  line's own default: a path that does not start at the root is looked for
  where the process runs, as the command line looks for it.
- **The file is settled to a full path as the reader opens,** by the
  system's own rules: on Windows, a path with a drive and no root, such as
  `C:market.sqlite`, names a file in that drive's own directory. A reader
  keeps reading the file it found then, wherever the process changes its
  directory to afterwards.
- **A variable is read as Python has it,** from `os.environ`, when a reader
  is opened. Importing the package reads none.
- **A variable that is read counts, though it is empty or belongs to the
  other backend.** An empty one is refused, never taken for the default,
  and one that is not what its setting takes is refused whichever backend
  is selected, as the command line refuses it. A number is digits and
  nothing around them, as the command line reads one.
- **A variable is not read where its argument is given.** The command line
  refuses a malformed variable even beside its flag; a reader given the
  argument leaves the variable alone.
- **The rules are the command line's,** applied by the same code
  ([configuration](configuration.md#storage-backends)). A backend is
  selected or is SQLite, never guessed: a PostgreSQL setting beside no
  backend is refused, since the reader would quietly open SQLite; `db` or
  `QF_DB` beside `postgres` is refused; and `postgres` without a URL is
  refused. With `storage="sqlite"` the PostgreSQL URL is not so much as
  read.
- **Selecting PostgreSQL never ends in a SQLite file.** A reader that
  cannot reach its database fails, and the SQLite-only build, whose
  `STORAGE_BACKENDS` is `("sqlite",)`, refuses `postgres` as `not in this
  build`. Neither opens, reads or makes a file in its place.

A reader that names its file:

```python
>>> with qf.CandleReader(db="data/market.sqlite") as named:
...     named.location
...
'data/market.sqlite'
```

## Reading candles

**`load` gives the candles of a market, oldest first, as a `CandleBatch`
that names the market:**

```python
>>> market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
>>> candles = reader.load(market)
>>> candles
CandleBatch(<5 candles>, market=MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m')))
>>> candles.close
(Decimal('81940.01000000'), Decimal('81955.25000000'), Decimal('81900.00000000'), Decimal('81900.10000000'), Decimal('81912.50000000'))
>>> candles.open_time_ms[0], candles.close_time_ms[0], candles.trades
(1767268800000, 1767268859999, (412, 3, 0, 18, 7))
```

- **Every value is the one the store holds.** A price and a volume are a
  `decimal.Decimal` with the digits the exchange wrote, trailing zeros
  included: a store keeps a decimal as text, and the reader reads the text.
  A time is the engine's millisecond, and a trade count the count.
- **Nothing is a float.** A float appears only where a batch is asked for
  one, through `to_numpy` or a DataFrame
  ([candles as NumPy arrays](python-numpy.md),
  [candles and results as pandas DataFrames](python-pandas.md)).
- **A count the store does not hold is `None`,** and never zero.
- **Stored text that no decimal holds exactly is refused,** as a
  `ValueError` that quotes it, and never rounded. No command stores such
  text.
- **A load that runs past PostgreSQL's limit on a statement, 30 seconds,
  is cut off** as a `ConnectionError`, as every command's is: read a long
  history in parts. A million one-minute candles take a few seconds.
- **The order is the candles' own:** by the time each opened, oldest first,
  however and in whatever order they were stored.
- **The batch is a batch as any other.** It runs through `run_backtest` on
  the market it names, and gives arrays and a frame as any batch does:

```python
>>> class Hold(qf.Strategy):
...     def on_bar(self, ctx, bar):
...         return qf.TargetPosition.LONG_ALL_IN
...
>>> config = qf.BacktestConfig(initial_cash=10000, fee_bps=0, close_out_at_end=True)
>>> qf.run_backtest(candles, Hold(), config).trade_count
1
```

- **A reader reads the store as it is when it is asked.** It keeps no
  candle between two calls, so what a `data sync` stored since the reader
  opened is in the next batch.

## A range of time

**`start` and `end` bound the candles by the time each opened, and both
ends are part of the range.** A bound that is left out, or `None`, bounds
nothing: with neither, `load` gives every candle of the market.

```python
>>> noon = datetime.datetime(2026, 1, 1, 12, 0, tzinfo=datetime.timezone.utc)
>>> minute = datetime.timedelta(minutes=1)
>>> reader.load(market, start=noon + minute, end=noon + 3 * minute).open_time_ms
(1767268860000, 1767268920000, 1767268980000)
>>> reader.load(market, start="2026-01-01T12:03:00Z").open_time_ms
(1767268980000, 1767269040000)
>>> reader.load(market, end=1767268860000).open_time_ms
(1767268800000, 1767268860000)
```

A bound is written in one of the three ways QuantForge writes a time:

<!-- begin: bounds -->
| A bound | Is read as |
| --- | --- |
| An `int` | Milliseconds since the Unix epoch, UTC, as every `_ms` field of the package is |
| A `datetime.datetime` with a time zone | The instant it names, in whatever zone it is written |
| A `str` | An RFC 3339 time such as `2026-01-01T00:00:00Z`, as the command line reads `--start` and `--end` |
| `None`, or left out | No bound |
<!-- end: bounds -->

- **A `datetime` without a time zone is refused.** It names no instant, and
  no zone is assumed for it.
- **A time between two milliseconds bounds the range where it stands,**
  as a `datetime` and as text. A candle opens on a whole millisecond, so as
  a start such a time is the millisecond after it, and as an end the one
  before: no candle outside the two instants is read, and none inside is
  left out. A type derived from `datetime` that holds more than
  microseconds, as a pandas `Timestamp` does, is read with all it holds.
- **Text is read by the engine's own parser,** the one behind `--start` and
  `--end`, and refused in its words where it is no RFC 3339 time. The
  refusal quotes the text, to its first 80 characters, but never text that
  holds an `@` or an `=`, as a URL or a connection string with a password
  does. Text with a lone surrogate is refused before it is read, and is not
  quoted either.
- **A `float` is no time,** and neither is a `bool` or a `datetime.date`.

## A range that holds nothing

**A market the store holds nothing of, and a range that holds no candle,
give an empty batch, and no exception.** The batch names the market that
was asked for. One warning on the logger named `quantforge_py` says which
of the two it was, and what the store holds instead:

```python
>>> import logging
>>> import sys
>>> shown = logging.StreamHandler(sys.stdout)
>>> logging.getLogger("quantforge_py").addHandler(shown)
>>> reader.load(qf.MarketId("binance_spot", "ETHUSDT", "1m"))
data/market.sqlite holds no candles of binance_spot ETHUSDT 1m
CandleBatch(<0 candles>, market=MarketId(ExchangeId('binance_spot'), Symbol('ETHUSDT'), Interval('1m')))
>>> len(reader.load(market, start="2026-01-02T00:00:00Z"))
data/market.sqlite holds no candles of binance_spot BTCUSDT 1m from 2026-01-02T00:00:00Z on; it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z
0
>>> logging.getLogger("quantforge_py").removeHandler(shown)
```

The handler above is this page's, so that the warning shows here. With no
logging set up, Python writes a warning to the standard error stream, and a
program routes or silences the logger as it does any other.

Each sentence starts with the store's location, and then says:

<!-- begin: nothing found -->
| Asked for | The sentence |
| --- | --- |
| A market the store holds nothing of, whatever the range | `holds no candles of binance_spot ETHUSDT 1m` |
| A range from one time to another | `holds no candles of binance_spot BTCUSDT 1m from 2026-01-02T00:00:00Z to 2026-01-03T00:00:00Z; it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z` |
| A range with a start alone | `holds no candles of binance_spot BTCUSDT 1m from 2026-01-02T00:00:00Z on; it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z` |
| A range with an end alone | `holds no candles of binance_spot BTCUSDT 1m up to 2025-12-31T00:00:00Z; it holds them from 2026-01-01T12:00:00Z to 2026-01-01T12:04:00Z` |
| A range that ends before it starts | `holds no candles of binance_spot BTCUSDT 1m from 2026-01-03T00:00:00Z to 2026-01-02T00:00:00Z: the range ends before it starts` |
| A range whose two bounds lie inside one millisecond | `holds no candles of binance_spot BTCUSDT 1m between two times inside the millisecond that begins at 2026-01-01T12:02:00Z: a candle opens on a whole millisecond` |
<!-- end: nothing found -->

- **A range inside what the store holds can hold nothing too:** a store
  keeps what was synced, and a stretch that never was is a hole in it. The
  sentence is the same, and the times it gives are those of the first and
  the last candle the store holds of the market.
- **A range that holds a candle says nothing.**
- **The sentence never says a range holds nothing where the store holds
  candles in it.** It reads the store again to say what it holds, and where
  a writer stored candles in the range between the load that found none
  and that second read, it says that the range held none as it was read:
  `held no candles of binance_spot BTCUSDT 1m from 2026-01-02T00:00:00Z on
  as they were read; it holds them from ... to ... now`.
- **Where the store cannot be read again to say what it holds,** the
  empty batch is given all the same, and the warning says why: `held no
  candles of binance_spot BTCUSDT 1m as they were read, and what it holds
  besides could not be read: ...`, with what the store said.
- **It is a log record, and not a Python warning,** so no setting of the
  `warnings` module turns it into an exception.

## A market is named in full

**`load` takes a `MarketId`, and nothing that stands for one:** the product
line, the symbol and the interval, which is the key a store keeps candles
under.

- **A product line names its venue and its family,** such as
  `binance_spot` or `bybit_linear`, so Spot, linear and inverse candles of
  one symbol are three markets, and so are one symbol's candles on two
  venues. Each is read alone, and nothing of one reaches another, on either
  backend.
- **The Spot line is named as it always was.** `binance_spot` is the name a
  store has kept Binance's Spot candles under since before there were other
  families, so a store an earlier release wrote is read under the same key.
- **A venue's name alone is not a market.** `binance` names no family, and
  nothing takes it for the Spot line: `qf.ExchangeId("binance")` is refused
  with the lines there are, so a family is never assumed.
- **The key holds no settlement asset.** A series of candles is named by
  its line, its symbol and its interval, and a line's family is what tells
  a linear contract's candles from an inverse one's
  ([shared-model.md](shared-model.md)).
- **Any line can be read, and Spot is what a candle strategy runs on.**
  `run_backtest` refuses a batch of a futures line
  ([running a backtest](python-types.md#running-a-backtest)).

## What a reader never does

**A reader opens the store read-only.** It differs from a command in what
it leaves alone:

- **It creates no database and no schema.** A command creates a SQLite
  file that is not there, and the schema in an empty PostgreSQL database. A
  reader refuses both: a file that does not exist is a `FileNotFoundError`,
  and a database that holds no store a `ValueError`. Opening a reader where
  there is no store never leaves an empty one behind.
- **It migrates nothing.** A command brings a store an earlier release
  wrote to its own schema as it opens it. A reader refuses a store at
  another schema than its build reads. A store an earlier release wrote is
  refused with the command that brings it forward: any command of the
  `quantforge` command line of the reader's version, such as `quantforge
  store status`. A store a later release wrote is refused as every command
  of the reader's version refuses it, since none of them can read it: it
  takes the package and the command line of that later release. A package
  built on a newer engine therefore never moves a store out from under the
  command line that writes it.
- **It writes nothing.** Every connection is read-only, SQLite's by how the
  file is opened and PostgreSQL's by its session, so the backend itself
  refuses a statement that would write.
- **It reads no unfinished copy.** A store that holds part of another
  ([storage-migration.md](storage-migration.md)) is refused until the copy
  is finished, as every command but the copy refuses it: as a reader opens,
  and before and after every load, since a reader may be open while a copy
  runs. A copy writes its record before its first row and finishes it after
  its last, so a load counts only where the record was the same before it
  and after it: a load that finds the copy unfinished is refused, and one
  that a copy began and finished during is made again, after it.
- **It finishes no writer's work.** Candles a writer committed to the log
  of a SQLite store, and that no one moved into the database since, are
  read where they are, and left there. A write a writer left unfinished, in
  the rollback journal of a store an operator switched to one, is no
  reader's to roll back: the store is refused with the command that does
  it, such as `quantforge store status`.

After a release changes the schema, one command of the new command line has
to open a store before a reader of the new package reads it.

**A reader of a SQLite store may leave two files beside it,** an empty
`-wal` file and a `-shm` file, as SQLite has any reader of such a database
do. The database file itself keeps every byte it had. A database in a
directory the reader cannot write to is refused, with what to do about it.

**What is there must be a file.** A directory, a pipe or a device at the
path is refused before it is opened (opening a pipe would wait for good),
and so is a path the process may not reach, such as one inside a directory
it may not enter, where the file may well be.

## One SQLite in a process

**In a process that reads a SQLite store through a `CandleReader`, do not
open that same file with Python's `sqlite3` module, or with anything built
on it, such as `pandas.read_sql` over a `sqlite3` connection or
SQLAlchemy's SQLite dialect.** The package carries its own copy of SQLite,
the engine's, which a reader of a SQLite store uses. A process that has one
file open through two copies of SQLite loses its locks on it each time
either copy closes it, which SQLite names as a way to corrupt a database
([How To Corrupt An SQLite Database File](https://www.sqlite.org/howtocorrupt.html#multiple_copies_of_sqlite_linked_into_the_same_application)).
While `quantforge data sync` or any other process writes the store, either
copy can then read candles that are not in it, a write made through the
other copy can be lost, and the process can end on a bus error.

- **Another process is safe:** the `sqlite3` command line, or a second
  Python process that uses the `sqlite3` module.
- **A reader alone is safe,** with any number of readers and threads in its
  process, beside any number of processes that write the store.
- **PostgreSQL is safe,** with no file to lock.

## Closing, threads and processes

- **`close()` closes the store,** its files or its connections, and so does
  leaving a `with` block. A reader that is no longer referenced closes too.
  Closing one again does nothing.
- **A closed reader loads nothing:** `load` raises `ValueError: the reader
  is closed`. It still says which store it was.
- **A call waits without the interpreter.** Opening a reader and loading
  release it while the store is asked, so other Python threads run
  meanwhile.
- **A reader is used by any number of threads at once.** With PostgreSQL
  they share its connections, at most `postgres_pool_size` of them: a call
  that finds none free waits for one, and fails as a `ConnectionError`
  where none frees in five seconds.
- **A reader belongs to the process that opened it.** A forked child has
  none of a pool's threads, so `load` on a reader it inherited is a
  `RuntimeError` that says so, on either backend, and the child opens a
  reader of its own. A child that lives on holds the parent's connections
  until it ends, as a forked process holds whatever its parent had open.
- **An interrupt waits for the call.** Ctrl-C during a load or as a reader
  opens is raised once the call returns, which its time limits bound: five
  seconds for a lock or a free connection, the connect timeout, and
  PostgreSQL's limit on a statement.

## Credentials

**A reader is for the process that reads its own store:** the trusted
local client, which already holds what selects the store.

- **No message holds a password or a user.** A refusal names the argument
  or the variable a value came from and never the value, a store is shown
  as `postgres://HOST:PORT/DATABASE`, and a connection the server refuses
  is told in words of QuantForge's own. A PostgreSQL URL written where
  another setting belongs is refused without being shown, in any of the
  forms it is written in elsewhere: in capitals, with a driver's name after
  a `+`, quoted, or with a `/` that `pathlib` took out of its `//`. A path
  that holds an `@` or an `=`, as a URL or a connection string with a
  password does, is not shown where no SQLite database is found at it:
  where nothing is there, where what is there is no file or no SQLite
  database, and where the path cannot be reached, as on Windows, which may
  refuse a `:` in a name. Nor is the full path it was looked for at, which
  holds it, nor the path where it could not be looked for at all, as when
  the directory the process runs in is gone. Text that
  is no UTF-8 is refused without being quoted, since Python's own error
  would carry all of it.
- **A password is written as the URL form asks:** `@`, `:`, `/` and `%`
  inside it percent-encoded. One whose `@` and `/` were left as they are
  would leave the rest of itself where the URL names the database, which is
  shown: a database name with an `@` that is not percent-encoded is refused
  for that reason, by the command line too.
- **The password belongs in `QF_POSTGRES_URL`,** set by whatever starts the
  program, which keeps it out of the code and out of a notebook.
- **What a reader gives carries nothing of the store.** A `CandleBatch` is
  candles and the market they are of: no connection, no URL and no setting.
- **A strategy that runs remotely is handed its candles.** A server's job
  worker receives the candles of its job from the server, which reads them
  from its own store. It is started with an empty environment and opens no
  store, so a strategy bundle needs no reader, and is given no database
  credential or handle ([worker-isolation.md](worker-isolation.md)).

## Refusals

A value of the wrong type is a `TypeError`, and one the settings' rules
refuse a `ValueError`, before anything is opened. Each row is what Python
raises for the call, where `url` is a PostgreSQL URL, `reader` an open
reader and `market` a `MarketId`:

<!-- begin: refusals -->
| Call | Raises |
| --- | --- |
| `qf.CandleReader(storage="mysql")` | `ValueError: storage must be sqlite or postgres` |
| `qf.CandleReader(storage=1)` | `TypeError: storage must be a str, not int` |
| `qf.CandleReader(db="")` | `ValueError: db is empty; name the SQLite database file, or leave it unset` |
| `qf.CandleReader(db=b"market.sqlite")` | `TypeError: db must be a str or an os.PathLike of one, not bytes` |
| `qf.CandleReader(db=url)` | `ValueError: db takes a SQLite file's path, and holds a PostgreSQL URL: give the URL as postgres_url or in QF_POSTGRES_URL, with storage="postgres" or QF_STORAGE=postgres` |
| `qf.CandleReader(storage="postgres")` | `ValueError: postgres storage needs a connection URL: postgres://USER:PASSWORD@HOST:PORT/DATABASE; give it as postgres_url, or in QF_POSTGRES_URL, which keeps a password out of the code` |
| `qf.CandleReader(postgres_url=url)` | `ValueError: postgres_url is a PostgreSQL setting, but no storage backend is selected, so this would use SQLite; select the postgres backend to use it, or the sqlite backend explicitly to keep SQLite: pass storage="postgres" or storage="sqlite", or set QF_STORAGE` |
| `qf.CandleReader(storage="postgres", postgres_url=url, db="market.sqlite")` | `ValueError: db names a SQLite file, but the postgres backend is selected; unset it, or select the sqlite backend` |
| `qf.CandleReader(storage="postgres", postgres_url="market")` | `ValueError: postgres_url is not a usable PostgreSQL URL: it does not parse as a URL (relative URL without a base); write postgres://USER:PASSWORD@HOST:PORT/DATABASE` |
| `qf.CandleReader(storage="postgres", postgres_url=url, postgres_pool_size=0)` | `ValueError: postgres_pool_size must be a whole number from 1 to 262143, the most connections a PostgreSQL server serves` |
| `qf.CandleReader(storage="postgres", postgres_url=url, postgres_pool_size="2")` | `TypeError: postgres_pool_size must be an int, not str` |
| `qf.CandleReader(db="no/such.sqlite")` | ``FileNotFoundError: there is no SQLite database at no/such.sqlite (looked for at ...): a reader creates none, and `quantforge data sync` does; db, or QF_DB, names another file`` |
| `reader.load("binance_spot BTCUSDT 1m")` | `TypeError: market must be a MarketId, not str` |
| `reader.load(market, start=1.5)` | `TypeError: start must be an int of milliseconds, a datetime.datetime with a time zone, a str or None, not float` |
| `reader.load(market, end=datetime.datetime(2026, 1, 1))` | `TypeError: end must be a datetime.datetime with a time zone, not one without: a time without a zone is never read as an instant; give it one, such as datetime.timezone.utc` |
| `reader.load(market, start="")` | `ValueError: invalid start '': invalid rfc3339 timestamp: empty; write an RFC 3339 time, such as 2026-01-01T00:00:00Z` |
| `reader.load(market, start=2**63)` | `ValueError: invalid start 9223372036854775808: must be between -9223372036854775808 and 9223372036854775807` |
<!-- end: refusals -->

A refusal that comes from a variable names the variable in the argument's
place: `QF_STORAGE must be sqlite or postgres`. A file that is not there is
looked for where the process runs when its path does not start at the root,
and the refusal says where it looked: the full path the `...` above stands
for.

## What is raised

A failure is told by what a caller does about it, the same on either
backend:

<!-- begin: raises -->
| What happened | Raises |
| --- | --- |
| An argument of the wrong type | `TypeError` |
| Settings the rules refuse; a time that is none | `ValueError` |
| The SQLite file is not there | `FileNotFoundError` |
| What is at the path is no file, or the path cannot be reached | `ValueError` |
| The store is there and is not one this reader reads: no database, a database without a store, another schema, an unfinished copy, a journal a writer left unfinished | `ValueError` |
| The store holds text no decimal holds exactly | `ValueError` |
| The store was not reached in time: the server is not there, refuses the URL's user or password, or names no such database; no connection came free; a lock outlasted the wait; a load ran past PostgreSQL's limit on a statement | `ConnectionError` |
| The reader is closed | `ValueError` |
| The reader is used in another process than the one that opened it | `RuntimeError` |
| Anything else the store fails with | `RuntimeError`, with what the store said |
<!-- end: raises -->

A `ConnectionError` changed nothing and read nothing, and the call may be
made again. A market or a range that holds nothing raises none of these: it
gives [an empty batch](#a-range-that-holds-nothing).

## PostgreSQL

The same code reads either backend. The command line that stores the
candles is told which, and so is the reader, by the same two variables:

```bash
export QF_STORAGE=postgres
export QF_POSTGRES_URL='postgres://USER:PASSWORD@HOST:5432/DATABASE'
quantforge data sync --symbol BTCUSDT --interval 1m --start 2026-01-01T12:00:00Z --end 2026-01-01T12:04:00Z
```

```python
with qf.CandleReader() as reader:
    reader.location  # 'postgres://HOST:5432/DATABASE'
    candles = reader.load(qf.MarketId("binance_spot", "BTCUSDT", "1m"))
```

Or by its arguments, which win over the variables:
`qf.CandleReader(storage="postgres", postgres_url=url)`.

- **What is read is the same.** The two backends hold the same candles
  under the same keys, and a batch read from one equals the batch read from
  the other, digit for digit.
- **The database is one a command wrote.** A reader creates no schema: the
  first `data sync` does.
- **The role needs to read, and nothing else.** A reader issues no
  statement that writes, in sessions the server itself keeps read-only.
- **How the URL is written,** which `sslmode` reaches which host, and that
  a password in it is never shown, is the command line's
  [configuration](configuration.md#the-postgresql-url).
