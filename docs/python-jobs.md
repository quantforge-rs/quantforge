# A job from Python: its end, its log, its result and its provenance

A `Job` is the handle `Client` gives on one backtest job of a QuantForge
server. Beside the job's status it gives four things, each by a method that
asks the server: `wait` asks until the job has ended, `stream_logs` follows
its log line by line, `result` gives what it produced, and `provenance`
what it ran on. This page is the contract of the four, and of what they
give: `LogLine`, `JobResult`, `ResultFile`, `Provenance` and
`ProvenanceSegment`.

It applies five pages and restates none. Which server a client reaches,
with which token, and what is raised for each way a request fails is
[the Python client](python-client.md). What a server's routes take and
answer is the protocol, which the `quantforge` crate's own documentation
specifies (`quantforge::protocol::jobs`). Every byte of a result's three
files is [the result spec](result-spec.md). The two frames of a result are
those of [candles and results as pandas DataFrames](python-pandas.md), and
how a decimal becomes a float there is the
[decimal and float64 policy](decimal-float-policy.md).

```python
>>> import quantforge_py as qf
>>> client = qf.Client("http://127.0.0.1:9600")
>>> market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
>>> config = qf.BacktestConfig(initial_cash=10000, fee_bps=10, close_out_at_end=True)
>>> job = client.backtest.submit(
...     "my_strategy",
...     entry="sma_cross.py:SmaCross",
...     params={"fast": 20, "slow": 50},
...     market=market,
...     config=config,
... )
```

`qf`, `client` and `job` are what the examples use. They ask a server that
runs on this machine and stores a day of one-minute candles of `BTCUSDT`,
1,440 of them, and they run in a directory that holds `my_strategy`, with
one file, `sma_cross.py`: the SMA cross of
[the goldens](golden-parity.md). `job` is a backtest of it over the whole
day ([submitting a backtest from Python](python-bundles.md)).

## Contents

- [The calls](#the-calls)
- [Waiting for a job](#waiting-for-a-job)
- [A job's log](#a-jobs-log)
- [How a log is followed](#how-a-log-is-followed)
- [A job's result](#a-jobs-result)
- [The two tables](#the-two-tables)
- [The two frames](#the-two-frames)
- [Exact amounts, and floats](#exact-amounts-and-floats)
- [A job's provenance](#a-jobs-provenance)
- [The identity](#the-identity)
- [What is raised](#what-is-raised)
- [Limits](#limits)
- [Refusals](#refusals)
- [What is left](#what-is-left)

## The calls

As Python's `inspect.signature` gives them:

<!-- begin: signatures -->
```text
Job.wait(timeout=None)
Job.stream_logs(*, after=0, on_progress=None)
Job.result()
Job.provenance()
JobResult.trades_df()
JobResult.equity_df()
JobResult.to_backtest_result()
```
<!-- end: signatures -->

<!-- begin: calls -->
| Call | Asks the server | Gives |
| --- | --- | --- |
| `job.wait(timeout=None)` | Where the job stands, until it has ended | The `JobStatus` of the job once it has ended, kept as `job.status` |
| `job.stream_logs(*, after=0, on_progress=None)` | For the job's log, as a stream or a page at a time, once it is iterated | An iterator of `LogLine`, one for each line, to the log's end |
| `job.result()` | For the job's metrics, and its files' hashes and sizes | A `JobResult` |
| `job.provenance()` | For what the job ran on | A `Provenance` |
| `result.trades_df()` | For `trades.arrow`, the first time its rows are needed | The closed trades as a pandas DataFrame |
| `result.equity_df()` | For `equity.arrow`, the first time its rows are needed | The equity curve as a pandas DataFrame |
| `result.to_backtest_result()` | For whichever of the two the result does not hold yet | The result as a `BacktestResult` |
<!-- end: calls -->

- **Each is a method, because each asks the server.** Reading an attribute
  of a job, of a result or of anything a job gives asks nothing, here as
  [everywhere in the client](python-client.md#the-client).
- **What a server answers is read strictly,** as a job's status is: a
  member that is missing or of another type, an amount that is not an
  exact decimal, a file that is not the one its job lists. Such an answer
  is a `RemoteError` that says what was wrong, and is never guessed at.
- **None of the five types is built.** A `LogLine`, a `ResultFile`, a
  `Provenance` and a `ProvenanceSegment` are values: each is what a server
  answered, never changes, and equals another when every field does. They
  are records, as the engine's are, and no keys of a dict. A `JobResult`
  is no value: it holds what it has downloaded, and equals itself alone.
  None is copied or pickled, and no class is derived from one.

## Waiting for a job

```python
>>> job.wait().state
'succeeded'
>>> job.status.state, job.status.ended
('succeeded', True)
>>> job.wait(timeout=30) is job.status
True
```

- **`wait()` asks the server where the job stands until it has ended,**
  keeps each answer as the job's `status`, and gives the last one: the
  status of a job that has succeeded, failed or been cancelled.
- **A job that failed is given like any other.** Nothing is raised for it:
  its status says `failed`, and why
  ([a job's status](python-client.md#a-jobs-status)).
- **A job the handle already holds as ended is given as it is,** and
  nothing is asked: a job that has ended never changes.
- **`timeout` is how many seconds to wait at the most,** an `int` or a
  `float`, or `None`, which it is unless it is given, for as long as the
  job takes. A job that has not ended by then is Python's own
  `TimeoutError`, which is no failure of a request: the job goes on, and
  its `status` is what the server last answered.
- **The time is looked at between two requests.** The server is asked at
  least once, a timeout of 0 included, and a request that is under way is
  not cut short: a wait can outlast its timeout by one request. The last
  request is made as the time runs out, so that a job that ended by then
  is given as ended, and never raised as one that had not.
- **The server is asked again after a tenth of a second, and then after
  twice as long each time,** two seconds at the most.
- **A request that fails is raised as it is,** and the job's `status` stays
  as it was last given. The job is waited for again by calling again.
- **A state a later revision of the protocol adds does not end a job
  here,** since its `ended` is `False`: a wait for such a job lasts until
  its timeout.

## A job's log

A job's log is what the job wrote while it ran, line by line: what its
strategy printed, and what it wrote to its standard error. The job of the
examples writes nothing, so they follow another, of a class that does:

```python
>>> class Talks(qf.Strategy):
...     """Holds nothing, and says so."""
...
...     def on_start(self, ctx):
...         print("started")
...
...     def on_bar(self, ctx, bar):
...         return None
...
...     def on_finish(self, ctx):
...         print("held nothing")
...
>>> talks = client.backtest.submit_class(Talks, market=market, config=config, source_only=True)
>>> for line in talks.stream_logs():
...     print(line.seq, line.level, line.message)
...
1 info started
2 info held nothing
>>> talks.wait().state
'succeeded'
```

- **`stream_logs()` gives an iterator of the log's lines,** each a
  `LogLine`: first the lines the server already holds, then each new one
  as the job writes it. A log followed while its job runs is every line as
  it comes.
- **It ends when the log does:** once the job has ended, however it ended,
  and its last line was given. So a `for` loop over it returns when the job
  is over, and a `wait()` after it has little left to wait for.
- **A log followed late is the whole log.** A server keeps a job's log with
  the job, and gives the same lines to whoever asks for them afterwards,
  under the same numbers.
- **Nothing is asked until the iterator is first asked for a line,** and
  what it takes is checked before that, as it is called.
- **The log of a queued job begins with the job.** The loop waits until the
  job starts, and for a job that is cancelled first it ends with no line.
- **Leaving the loop lets go of the connection.** `close()` on the
  iterator does, and Python calls it as soon as nothing holds the iterator
  any more: at once after a `break`, or an exception, out of a `for` loop
  over `job.stream_logs()` itself. An iterator kept under a name of its own
  is closed by that name.

<!-- begin: line fields -->
| Field | What it is |
| --- | --- |
| `seq` | The line's number in its job's log: a server numbers a job's lines from 1, in the order it took them |
| `at_ms` | When the server took the line, in milliseconds since the Unix epoch, UTC, by the server's clock |
| `level` | How serious the line is: `debug`, `info`, `warning` or `error` |
| `message` | The line's text, as the job wrote it |
| `dropped_bytes` | How many bytes of the text the server left out, where the line was longer than it keeps of one, or `None` |
<!-- end: line fields -->

- **What a strategy prints is `info`,** and what it writes to its standard
  error `warning`. A level a later revision of the protocol adds is given
  as the server wrote it; `repr()` of a line shows a level that is no
  plain word as it shows the message, with control characters escaped.
- **`message` is the job's own text,** with every character the job wrote:
  treat it as text a stranger wrote before it is shown where control
  characters matter. `repr()` of a line shows it as Python writes text,
  with those escaped, and in part where it is very long:
  `<LogLine 1 info: 'started'>`.
- **A server keeps a bounded log.** Of a job that writes more than it
  keeps, the newest lines, and of a line longer than it keeps of one, the
  start: `dropped_bytes` says how much of a line is missing, and a jump in
  `seq` how many lines are, since a line keeps its number.

**`after` is the number of the last line the caller already holds:** the
first line given is the one after it, and 0, which it is unless it is
given, gives the log from its first line. A log that stopped being followed
is followed again from where it stopped by `after=` the `seq` of the last
line given.

```python
>>> [line.message for line in talks.stream_logs(after=1)]
['held nothing']
```

**`on_progress` is called with how far the job has come,** `bars_done` and
`bars_total`, two `int`s: the bars the strategy has been handed, of the
bars the job runs on. It is called whenever the server says the job has
come further than it last said, between the lines around that, on the
thread that iterates. An exception it raises ends the loop as itself. For a
job that has ended it is called once, with how far the job came, and not at
all for one that never started:

```python
>>> told = []
>>> lines = list(talks.stream_logs(on_progress=lambda done, total: told.append((done, total))))
>>> len(lines), told
(2, [(1440, 1440)])
```

## How a log is followed

**Over the stream a server offers, and a page at a time wherever the
stream cannot be had.** The log's route,
`GET /v1/backtests/{job_id}/logs`, answers a page of lines, and asked as a
WebSocket it sends each line as the job writes it. A client asks for the
stream, with the standard library alone, over `http` and over `https`.

- **A stream that stops is no failure.** One that cannot be opened, as
  behind a proxy that passes no WebSocket; one that breaks, falls silent
  for longer than a server may, or says something the protocol does not
  write; and one a server closes without saying that the log has ended:
  the log is read a page at a time from then on, from the last line given.
- **No line is given twice, and none is left out.** A server's number for
  a line is the cursor both ways are asked by. A line at or before the
  cursor is passed over, and lines the server no longer holds move the
  cursor past them.
- **What is wrong with a server is raised by the pages,** as the exception
  every request raises for it ([what is raised](#what-is-raised)): a
  server that is gone, a job it does not hold, a page the protocol does not
  write. Nothing a stream stopped with is raised.
- **A page is asked for again at once where it held something new,** and
  after half a second where it did not: for a job that is quiet, or has not
  started.
- **A stream is a connection of its own, made as a request's is:** to the
  server itself, with the token, and over `http` with a token only where it
  ends on this machine ([the token](python-client.md#the-token)). It asks
  for no extension and no protocol of WebSocket's, answers a server's ping,
  and says goodbye as WebSocket has each side do.
- **A server that stops ends the logs it has open.** The job it ran has
  failed, so its log ends, and the loop with it. The stream of a job that
  has not ended is closed without an end, and the page asked for in its
  place finds no server: a `ServerUnavailable`.
- **A server that is killed ends nothing.** The stream of the job it ran
  is cut off, and the page finds no server: a `ServerUnavailable` there
  too. The next server on the store fails that job, and holds none of what
  it had written, which the killed server had not stored yet.

## A job's result

```python
>>> result = job.result()
>>> result.metrics
{'initial_cash': Decimal('10000'), 'final_equity': Decimal('9921.202457428483046329087897'), 'total_return_pct': Decimal('-0.7879754257151695367091210300'), 'max_drawdown_pct': Decimal('1.8091109103945370264444945700'), 'trade_count': 15}
>>> [(file.name, file.media_type) for file in result.files]
[('metrics', 'application/json'), ('trades', 'application/vnd.apache.arrow.file'), ('equity', 'application/vnd.apache.arrow.file')]
```

**`job.result()` gives what a succeeded job produced, a `JobResult`:** its
metrics, and its closed trades and its equity curve, which are downloaded
when they are first asked for ([the two tables](#the-two-tables)).

<!-- begin: result members -->
| Member | What it is |
| --- | --- |
| `job_id` | The job's ID |
| `metrics` | The job's summary: a dict of the fields `metrics.json` holds, in its order |
| `files` | The job's three result files, a tuple of `ResultFile`: `metrics`, `trades` and `equity` |
| `trades_df()` | The closed trades as a pandas DataFrame |
| `equity_df()` | The equity curve as a pandas DataFrame |
| `to_backtest_result()` | The result as a `BacktestResult`, the type a local run gives |
<!-- end: result members -->

- **Only a job that has succeeded has a result.** One that is queued or
  running, that failed or was cancelled is a `Conflict`, in the server's
  words, which say which.
- **Each call asks the server,** and gives a result of its own: keep the
  one it gives. A result never changes, so nothing is gained by asking
  again.
- **`repr()` of a result is its job and how many trades it closed,** as in
  `<JobResult of job 57917a9825f44d75ad4a2e6329d00ec7: 15 trades>`.

**`metrics` is the job's summary,** the fields of `metrics.json`
([the result spec](result-spec.md#metricsjson)) under their own names and
in their order:

<!-- begin: metrics fields -->
| Field | Type | What it is |
| --- | --- | --- |
| `initial_cash` | `decimal.Decimal` | The cash the backtest started with |
| `final_equity` | `decimal.Decimal` | Cash plus any position still open, valued at the last bar's close, after a close-out at the end and its fee |
| `total_return_pct` | `decimal.Decimal` | The gain over the starting cash, in percent |
| `max_drawdown_pct` | `decimal.Decimal` | The largest fall of the equity curve from an earlier peak, in percent |
| `trade_count` | `int` | How many trades closed |
<!-- end: metrics fields -->

- **Each amount is exact,** a `decimal.Decimal` with the digits the server
  wrote, trailing zeros among them, and never a float.
- **The dict is new each time it is read,** and the caller's own.
- **The metrics are the ones the job's `metrics.json` holds,** though that
  file is never downloaded. The client writes the metrics it was answered
  as the result format writes that file, by the engine's own writer, and
  what it wrote has to be the file the job lists, by its size and its
  hash. Metrics that are not are refused.

**`files` is the job's three files, each a `ResultFile`:**

<!-- begin: file fields -->
| Field | What it is |
| --- | --- |
| `name` | What the file holds: `metrics`, `trades` or `equity` |
| `media_type` | The file's media type |
| `hash` | The file's name as stored content: `sha256:` and the SHA-256 of its bytes, in hex |
| `size_bytes` | How many bytes the file holds |
<!-- end: file fields -->

The same result is the same three files on every server, every store and
every release, so a file's hash names what the job produced. A file a later
revision of the protocol lists beside the three is none of this result
format's, and is left out.

## The two tables

**A table is downloaded the first time something asks for it, and not
before.** A result's metrics come with it in one small answer; its two
tables are files, and the equity curve of a year of one-minute bars is
about 22 MB. So a caller that reads the metrics of a hundred jobs downloads
no table, and one that wants the trades alone leaves the curve on the
server.

- **`trades_df()` downloads `trades.arrow`, `equity_df()` downloads
  `equity.arrow`,** and `to_backtest_result()` whichever of the two the
  result does not hold yet. Each file is downloaded once and its rows kept
  by the result, for as long as the result is. Callers that ask for a
  table at the same moment, from threads of their own, take turns: one
  downloads it, and the others wait and are given its rows.
- **A file is held to what its job lists before it is read:** it is read
  to the size the job lists and no further, and is that long and of that
  hash, or it is refused. One that states another length is not read at
  all.
- **It is then read by the engine's own reader of the result format,**
  `quantforge::protocol::results`, compiled into the package. That reader
  takes a file that is byte for byte what the format writes for its rows,
  and no other: bytes after its end, another padding, and an Arrow file of
  other columns are refused, though a general reader of Arrow would take
  them.
- **Its rows are the engine's own types:** a `ClosedTrade` for each trade,
  whose symbol is one the engine writes, and an `EquityPoint` for each
  bar. The trades are as many as the metrics count.
- **A file is asked for where the client asks for everything else:** under
  the URL the client was given, by the file's own media type. The path a
  server lists for a file is the path on the server itself, without what a
  proxy serves it under, and is not used.
- **A file that was refused is not kept.** Asked for again, it is
  downloaded again, as it is for a caller that waited its turn behind the
  one that was refused it.
- **A table is read whole, in memory, and takes more than its file.**
  While it is read it takes about ten times its file's size: the file,
  what the engine's reader makes of it to hold it to the format byte for
  byte, and its rows, a Python object each. From then on the result keeps
  the rows, for as long as it is: 64 bytes for each point of an equity
  curve, and about 200 for each trade. The curve of a year of one-minute
  bars, a file of 22 MB, takes about 200 MB while it is read and 34 MB
  from then on.
- **A result file longer than 1 GiB is not downloaded.** A year of
  one-minute bars, the most a server runs one job on unless it is told
  otherwise, makes an equity curve of a fiftieth of that. A file past it
  is fetched from the server's own route with a tool that reads Arrow
  files to disk. The limit is on the file, and no limit on memory: a
  table far below it can take more than a process has, and is read that
  way too.

```python
>>> whole = result.to_backtest_result()
>>> whole
BacktestResult(initial_cash=Decimal('10000'), final_equity=Decimal('9921.202457428483046329087897'), total_return_pct=Decimal('-0.7879754257151695367091210300'), trade_count=15, max_drawdown_pct=Decimal('1.8091109103945370264444945700'), trades=<15 trades>, equity_curve=<1440 points>)
>>> whole.trades[0].entry_price, whole.trades[0].entry_order_id
(Decimal('82659.21000000'), None)
>>> whole == result.to_backtest_result()
True
```

**`to_backtest_result()` gives the result as a `BacktestResult`,** the
type `run_backtest` gives of a local run
([the Python types](python-types.md#backtests)): the metrics as its
fields, every closed trade and every point of the curve, each amount exact.

- **It equals the result of the same run made here.** The same class, run
  by `run_backtest` over the same candles under the same settings, gives a
  `BacktestResult` equal to this one, and one whose amounts have the same
  digits. The package's tests hold a job to that on SQLite and on
  PostgreSQL, with the candles read from the server's own store.
- **A trade's two order IDs are `None`,** as a local run's are: a backtest
  sends no order to a venue, and the result format has no column of one.
- **The result is new each time,** built from the rows the `JobResult`
  keeps.

## The two frames

**`trades_df()` and `equity_df()` give the frames `trades_to_dataframe` and
`equity_to_dataframe` give of a local run's result,** column for column:
one convention for a result, wherever it was made. Each is made by that
very function, of the result `to_backtest_result()` gives, so
[candles and results as pandas DataFrames](python-pandas.md#a-results-trades)
is the contract of both.

```python
>>> trades = result.trades_df()
>>> trades.dtypes
symbol                          object
entry_time         datetime64[ms, UTC]
exit_time          datetime64[ms, UTC]
entry_price                    float64
exit_price                     float64
qty                            float64
gross_quote_pnl                float64
entry_order_id                  object
exit_order_id                   object
dtype: object
>>> len(trades), trades["entry_order_id"].tolist() == [None] * 15
(15, True)
>>> equity = result.equity_df()
>>> equity.index.name, equity.dtypes.to_dict(), len(equity)
('open_time', {'equity': dtype('float64')}, 1440)
>>> trades.equals(qf.trades_to_dataframe(whole)), equity.equals(qf.equity_to_dataframe(whole))
(True, True)
```

<!-- begin: trade columns -->
| Column | Type | What it is |
| --- | --- | --- |
| `symbol` | `object` | The symbol traded, as text |
| `entry_time` | `datetime64[ms, UTC]` | When the entry filled: the column `entry_time_ms` of `trades.arrow` |
| `exit_time` | `datetime64[ms, UTC]` | When the exit filled: the column `exit_time_ms` |
| `entry_price` | `float64` | The price the entry filled at |
| `exit_price` | `float64` | The price the exit filled at |
| `qty` | `float64` | The quantity traded, in the base asset |
| `gross_quote_pnl` | `float64` | The change in cash over the trade, in the quote asset, net of both fees |
| `entry_order_id` | `object` | `None` in every row: a backtest sends no order to a venue |
| `exit_order_id` | `object` | `None` in every row |
<!-- end: trade columns -->

<!-- begin: equity columns -->
| Column | Type | What it is |
| --- | --- | --- |
| `open_time`, the index | `datetime64[ms, UTC]` | The open time of the bar, which names it: the column `open_time_ms` of `equity.arrow` |
| `equity` | `float64` | Cash plus the position valued at the bar's close, after the bar's fill |
<!-- end: equity columns -->

- **The trades are numbered from 0,** a `RangeIndex` with no name, and the
  curve is indexed by the time each bar opened, as a frame of candles is.
- **The two order ID columns are there, and hold `None`.** A frame of a
  remote result has the columns of a local one, every one of them, so that
  code written for one runs on the other.
- **A frame is new each time, and the caller's own.** Changing one changes
  nothing in the result.
- **They need pandas and NumPy,** the package's extras, and say what to
  install where either is missing, in the words `trades_to_dataframe` says
  it, before anything is downloaded
  ([pandas itself](python-pandas.md#pandas-itself)). Everything else on
  this page needs neither.

## Exact amounts, and floats

**An amount is exact wherever it is not a frame's,** as everywhere in the
package: the same rule for a result made here and for one a server made.

<!-- begin: amounts -->
| Where | An amount is | A time is |
| --- | --- | --- |
| `result.metrics` | A `decimal.Decimal`, with the digits the server wrote | — |
| `result.to_backtest_result()` | A `decimal.Decimal`, with the digits the server wrote | An `int` of milliseconds |
| `result.trades_df()`, `result.equity_df()` | A `float64`: the float nearest to the decimal | `datetime64[ms, UTC]`, the same milliseconds |
<!-- end: amounts -->

- **A decimal is read from the text the result format writes it as,**
  exactly: the digits of `metrics.json` and of the two tables, trailing
  zeros among them, and nothing a float has been.
- **A frame's float is the float nearest to that decimal, by the engine's
  own conversion** and by nothing else
  ([the policy](decimal-float-policy.md)): the conversion a local result's
  frame is made by, applied to the same decimals, so the two frames hold
  the same floats.
- **A frame is for computing with, and is never compared for parity.** A
  job is held to a local run, and to the goldens, by the text of its
  decimals ([a frame is for computing with](python-pandas.md#a-frame-is-for-computing-with)).
- **A time is the engine's own millisecond** in a `BacktestResult`, and in
  a frame the same millisecond shown as a UTC time, never rounded.

## A job's provenance

```python
>>> provenance = job.provenance()
>>> provenance.code_hash == job.status.bundle
True
>>> provenance.market, provenance.bar_count
(MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m')), 1440)
>>> [(segment.first_open_ms, segment.last_open_ms, segment.bar_count) for segment in provenance.segments]
[(1767268800000, 1767311940000, 720), (1767312000000, 1767355140000, 720)]
>>> provenance.config == config, provenance.engine == qf.ENGINE_VERSION, provenance.deps
(True, True, {})
>>> provenance.entry, provenance.params
('sma_cross.py:SmaCross', {'fast': 20, 'slow': 50})
>>> provenance.results == result.files
True
```

**`job.provenance()` gives what the job ran on, a `Provenance`:** what its
server read for it and recorded before the job ran. A worker is handed
those inputs and nothing else, so a strategy that decides from what it is
handed depends on nothing else. One that reads a clock or draws on
randomness depends on that as well, and no provenance records it.

<!-- begin: provenance fields -->
| Field | Type | What it is |
| --- | --- | --- |
| `job_id` | `str` | The job's ID |
| `identity` | `str` | The identity of the job's inputs: `sha256:` and the SHA-256 of their JSON, in hex |
| `code_hash` | `str` | The code hash of the strategy bundle, which fixes its code, its manifest and so its parameters |
| `market` | `MarketId` | The market the job ran on |
| `start_open_ms` | `int` | The open time of the first bar of the job's range |
| `end_open_ms` | `int` | The open time of the last bar of the job's range |
| `bar_count` | `int` | How many bars the job ran on |
| `segments` | `tuple` of `ProvenanceSegment` | The segments that hold every one of those bars, oldest first |
| `config` | `BacktestConfig` | How the job's fills were simulated |
| `engine` | `str` | The version of the engine that ran the bundle, such as `0.3.0` |
| `python` | `str` | The version of the Python that ran it, such as `3.12` |
| `deps` | `dict` of `str` to `str` | Each package the bundle declares, with the version that ran |
| `proto_version` | `int` | The protocol version of the server that ran the job |
| `entry` | `str` | The bundle's entry: its file and its class |
| `params` | `dict` | The parameters the class was built with, as the values JSON holds |
| `recorded_at_ms` | `int` | When the server recorded the provenance, as the job started |
| `backend` | `str` | The backend of the store it was recorded in: `sqlite` or `postgres` |
| `schema_version` | `int` | The schema version of that store then |
| `results` | `tuple` of `ResultFile`, or `None` | The files the job produced, once it has succeeded |
<!-- end: provenance fields -->

<!-- begin: segment fields -->
| Field | Type | What it is |
| --- | --- | --- |
| `hash` | `str` | The segment's name: `sha256:` and the SHA-256 of its payload, in hex |
| `first_open_ms` | `int` | The open time of its first bar |
| `last_open_ms` | `int` | The open time of its last bar |
| `bar_count` | `int` | How many bars it holds |
<!-- end: segment fields -->

- **A job has its provenance once it has started.** One that is still
  queued, and one that ended before its server read its inputs, as a job
  cancelled while it was queued did, is a `Conflict`, in the server's
  words, which say which.
- **What a job ran on is given in the package's own types,** as its status
  is: the market a `MarketId`, the settings a `BacktestConfig` with every
  amount exact, and every time milliseconds since the Unix epoch, UTC.
- **Every bar is in a segment that is named.** A segment is stored
  candles, named by the hash of its bytes ([the segment
  spec](segment-spec.md)), which a server gives under that name at
  `GET /v1/segments/{hash}`. The first and the last of a job's segments may
  hold bars outside the job's range, which the job did not run on.
- **`entry` and `params` repeat what the code hash fixes,** for reading:
  they are the bundle's manifest's.
- **`recorded_at_ms`, `backend` and `schema_version` are the store's,** and
  change with it, never with what the job ran on.
- **`results` is what the job produced,** the files its result lists, once
  it has succeeded, and `None` until then.
- **`deps` and `params` are new each time they are read,** and the
  caller's own, as deep as they go.

## The identity

**`identity` covers everything a job is given to run on, and nothing
else:** the bundle by its code hash, the market and the range with every
segment, the settings, the engine, the Python and the packages, and the
protocol version. Jobs given the same have the same identity, on every
server and every store; where and when a provenance was recorded is no
part of it, and neither is the job.

```python
>>> again = client.backtest.submit(
...     "my_strategy",
...     entry="sma_cross.py:SmaCross",
...     params={"fast": 20, "slow": 50},
...     market=market,
...     config=config,
... )
>>> again.wait().state
'succeeded'
>>> other = again.provenance()
>>> other.identity == provenance.identity, other.job_id == provenance.job_id
(True, False)
>>> other.results == provenance.results, other == provenance
(True, False)
```

- **The identity is checked here.** It is the SHA-256 of the inputs'
  compact JSON, which Python writes byte for byte as a server does, from
  the members as they were received. The client takes it again of the
  inputs it was answered, and a provenance whose inputs are not the ones
  its identity names is refused.
- **Two jobs of one identity ran on the same thing,** and the engine is
  deterministic, so a strategy that decides from its bars alone gives them
  the same result files, hash for hash. One that draws on a clock or on
  randomness need not.
- **A provenance of another format is refused,** as a result of another
  format is: this version of the package reads provenance format 1 and
  result format 1.

## What is raised

**A request that fails is a `RemoteError`, or the exception derived from it
that the failure is,** by the table of
[the Python client](python-client.md#what-is-raised), whichever call made
it. Beside those:

<!-- begin: raises -->
| Call | Raises | When |
| --- | --- | --- |
| `job.wait(timeout)` | `TimeoutError` | The job had not ended in time; it goes on |
| `job.result()` | `Conflict` | The job has not succeeded: it is queued or running, failed or was cancelled |
| `job.provenance()` | `Conflict` | The job has recorded no provenance: it is still queued, or ended before it started |
| `job.result()`, `job.provenance()` | `RemoteError` | The answer is not what the protocol writes, is of another format, or does not hold together: metrics that are not the listed file's, an identity that is not its inputs' |
| `result.trades_df()`, `result.equity_df()`, `result.to_backtest_result()` | `RemoteError` | A file is not the one its job lists, by its size or its hash, is not what the result format writes, or is longer than is downloaded |
| `result.trades_df()`, `result.equity_df()` | `ImportError` | pandas or NumPy is missing, or is one the package's extras do not take |
| The loop over `job.stream_logs()` | A `RemoteError`, or one derived from it | The log could not be read a page at a time either |
<!-- end: raises -->

- **A refusal is the server's own, word for word,** as everywhere:
  `job 57917a9825f44d75ad4a2e6329d00ec7 is still running; only a job that
  has succeeded has a result`.
- **A call that fails leaves everything as it was.** The job's `status`
  stays as it was last given, a result keeps the tables it had, and a log
  is followed again from the last line given.
- **`TimeoutError` and `ImportError` are Python's own,** and no
  `RemoteError`: no request failed.

## Limits

<!-- begin: limits -->
| What | Limit |
| --- | --- |
| The first wait before `wait` asks again | 0.1 seconds |
| The longest wait before `wait` asks again | 2 seconds |
| A server's answer to the request for a stream | 10 seconds |
| A stream's silence | 60 seconds |
| One message of a stream | 1 MiB |
| What a stream that is let go of is read for | 1 second |
| The wait before the next page of a log, where the last held nothing new | 0.5 seconds |
| A result file | 1 GiB |
<!-- end: limits -->

- **A stream's silence is a server's longest.** A server pings a stream it
  has had nothing to say on for a while, this one for 30 seconds, so a
  stream that says nothing for 60 is a connection that is gone, and the
  log is read a page at a time.
- **The limits of a request hold for each page and each file too:** how
  long a connection may take to open, a server's longest silence, and how
  often a request is asked again while the server answers `503`
  ([what is asked again](python-client.md#what-is-asked-again)).
- **None of these is an argument yet.**

## Refusals

A value of the wrong type is a `TypeError`, and one the rules refuse a
`ValueError`, before anything is asked. Each row is what Python raises for
the call, where `job` is a job:

<!-- begin: refusals -->
| Call | Raises |
| --- | --- |
| `job.wait("5")` | `TypeError: timeout must be a number of seconds or None, not str` |
| `job.wait(True)` | `TypeError: timeout must be a number of seconds or None, not bool` |
| `job.wait(-1)` | `ValueError: timeout is negative: it is how many seconds to wait at the most, or None for as long as the job takes` |
| `job.wait(float("nan"))` | `ValueError: timeout is no number: it is how many seconds to wait at the most, or None for as long as the job takes` |
| `job.stream_logs(after="3")` | `TypeError: after must be an int, not str` |
| `job.stream_logs(after=-1)` | `ValueError: after is negative: it is the number of the last line the caller holds, or 0 for the log from its first line` |
| `job.stream_logs(after=2**63)` | `ValueError: after is beyond the number any line of a log has` |
| `job.stream_logs(on_progress="print")` | `TypeError: on_progress must be a callable or None, not str` |
| `qf.LogLine()` | `TypeError: a LogLine cannot be built: Job.stream_logs gives one for each line of a job's log` |
| `qf.JobResult()` | `TypeError: a JobResult cannot be built: Job.result() gives the result of a job that has succeeded` |
| `qf.ResultFile()` | `TypeError: a ResultFile cannot be built: a JobResult and a Provenance give one for each file a job produced` |
| `qf.Provenance()` | `TypeError: a Provenance cannot be built: Job.provenance() gives what a server recorded of a job` |
| `qf.ProvenanceSegment()` | `TypeError: a ProvenanceSegment cannot be built: a Provenance gives one for each segment its job's bars came from` |
<!-- end: refusals -->

`after` and `on_progress` are given by name, and what they are is checked
as `stream_logs` is called, though nothing is asked until the iterator is.

## What is left

| What | Story |
| --- | --- |
| Stubs for the package's types, which a type checker reads | QUA-471 |
| The goldens held to a remote run and a local one in one required check | QUA-458 |
| A server that checks the token it is sent | QUA-345 |
