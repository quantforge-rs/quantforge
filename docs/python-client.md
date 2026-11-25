# The Python client

`Client` reaches a QuantForge server, `quantforge serve run`, from Python:
it lists the server's backtest jobs, gets one by its ID, and cancels one,
with the standard library alone. This page is the contract of it: which
server a client reaches and with which token, what it agrees on with the
server and when, what a job and its status are, what is raised for each way
a request fails, and what the client never does.

It applies two pages and restates neither. What a server's routes take and
answer is the protocol, which the `quantforge` crate's own documentation
specifies (`quantforge::protocol`). Which token goes where is the command
line's rule for `--remote-token`, in
[configuration](configuration.md#remote-backtests), which a client follows.

```python
>>> import quantforge_py as qf
```

`qf` is what the examples use. They ask a server that runs on this machine,
at the address `serve run` listens on unless it is told otherwise, and that
holds four jobs: one that succeeded, one that failed, one that is running,
and one queued behind it.

## Contents

- [The client](#the-client)
- [Which server it reaches](#which-server-it-reaches)
- [The token](#the-token)
- [The handshake](#the-handshake)
- [Jobs](#jobs)
- [A job's status](#a-jobs-status)
- [Refreshing and cancelling](#refreshing-and-cancelling)
- [What is raised](#what-is-raised)
- [What is asked again](#what-is-asked-again)
- [Connections](#connections)
- [Refusals](#refusals)
- [What is left](#what-is-left)

## The client

As Python's `inspect.signature` gives them:

<!-- begin: signatures -->
```text
Client(base_url, token=None)
Client.handshake()
Client.backtest.list(*, limit=None)
Client.backtest.get(job_id)
Job.refresh()
Job.cancel()
```
<!-- end: signatures -->

<!-- begin: members -->
| Member | What it is |
| --- | --- |
| `Client(base_url, token=None)` | A client of the server that answers at `base_url`. It asks the server nothing |
| `handshake()` | Asks the server which protocol versions it speaks, and agrees on one: what the client's first use does, done now |
| `url` | The server's URL, as the client asks it |
| `backtest` | The server's backtest jobs: `list` and `get` |
| `backtest.list(*, limit=None)` | The jobs the server holds, the last submitted first, as a list of `Job` |
| `backtest.get(job_id)` | The job of that ID, as a `Job` |
| `server_version` | The server's release, as it names it; `None` until the server has been asked |
| `proto_version` | The protocol version agreed with the server; `None` until the server has been asked |
<!-- end: members -->

```python
>>> client = qf.Client("http://127.0.0.1:9600")
>>> client
<Client http://127.0.0.1:9600/>
>>> client.url
'http://127.0.0.1:9600/'
```

- **Making a client asks nothing.** It checks its arguments and reads the
  token; the server is first asked when the client is first used.
- **A client is not a value.** It equals itself alone, is not copied or
  pickled, and no class is derived from it. It holds no connection: each
  request is made on one of its own, so a client is used by any number of
  threads at once, and has nothing to close.
- **Reading an attribute never asks the server.** `handshake`, `list` and
  `get` do, and a job's `refresh` and `cancel`.

## Which server it reaches

**`base_url` is where the server answers:** `http://` or `https://` with a
host, such as `http://127.0.0.1:9600`, as the command line's `--remote`
takes it.

- **A server behind a proxy that serves it under a path is named with the
  path:** `https://qf.example/lab` asks `https://qf.example/lab/version`
  and so on. A path is given a `/` at its end where it has none.
- **A URL holds no query, no fragment, and no user name or password.** The
  credential a server takes is the [token](#the-token), and a URL that
  carries one is refused without being shown.
- **`url` is the URL as the client asks it,** with a port only where it is
  not the scheme's own.

## The token

**A token is sent with every request, as `Authorization: Bearer`, and
never shown.** It is the command line's `--remote-token`:

<!-- begin: settings -->
| Argument | Variable | Default | What it is |
| --- | --- | --- | --- |
| `base_url` | — | `required` | Where the server answers |
| `token` | `QF_REMOTE_TOKEN` | none | The token sent with every request |
<!-- end: settings -->

- **The argument wins over the variable, and the variable over none.**
  `token=None`, or `token` left out, reads `QF_REMOTE_TOKEN`.
- **The variable is read as Python has it,** from `os.environ`, when a
  client is made. Importing the package reads none, and a client that is
  made keeps the token it was made with.
- **An empty token is no token,** and so is one of nothing but space. An
  argument that is given is the token, though it is empty: `token=""`
  sends none, whatever the variable holds. The space around a token is not
  part of it.
- **A token is printable ASCII without a space inside it,** which is what a
  header's value holds. Any other is refused, by the name of the argument
  or of the variable it came from.
- **A token goes only where no one between can read it:** over `https`, or
  over `http` to this machine, which is `localhost` or a loopback address.
  Anywhere else a client with a token is refused as it is made, before
  anything is sent.
- **`localhost` has to be this machine.** A name leads where the system's
  resolver says, so before a token goes over `http` the client checks that
  the connection it opened ends on this machine, at a loopback address. On
  one that does not, nothing is sent, and the request is a `RemoteError`
  that says where the name led.
- **No message, `repr` or exception holds the token.** A client says only
  that it has one: `<Client https://qf.example/, with a token>`.
- **No server checks a token yet.** A QuantForge server answers whoever
  reaches it, until the story that gives it authentication
  ([what is left](#what-is-left)); a proxy in front of one can check the
  header today.

```bash
export QF_REMOTE_TOKEN='the-token'
```

```python
exported = qf.Client("https://qf.example")                # sends the variable's token
other = qf.Client("https://qf.example", token="another")  # the argument wins
plain = qf.Client("http://lab.example:9600", token="")    # sends none
```

## The handshake

**A client agrees on a protocol version with its server before it asks
anything else:** `GET /version`, which names no version and which every
server answers alike, and then every request under the newest version both
sides speak. This package speaks version 1.

```python
>>> client.server_version, client.proto_version
(None, None)
>>> client.handshake()
>>> client.server_version, client.proto_version
('0.2.0', 1)
```

- **It happens when a client is first used,** by whatever is first asked of
  it, and `handshake()` does it now: a script that wants to know whether
  its server is there, and is one it can talk to, asks before it does
  anything else.
- **A server is asked once.** What was agreed is kept for as long as the
  client is, and a second `handshake()` asks nothing.
- **A server that speaks no version this package speaks is a
  `VersionMismatch`,** which names both ranges and gives the server's as
  `supported`. Nothing is asked of such a server under any version.
- **`server_version` is for people to read.** What a client asks is chosen
  by the protocol version, never by the release.
- **A version the server stops speaking is agreed on again.** A server
  that refuses a request for its version, as one upgraded since the
  handshake would, is a `VersionMismatch` too, and the client's next use
  asks it which versions it speaks now.

## Jobs

**`client.backtest` is the server's backtest jobs.** A job backtests a
strategy bundle on candles the server stores; it is submitted by the
command line's `backtest --remote` today
([what is left](#what-is-left)).

```python
>>> jobs = client.backtest.list()
>>> [job.status.state for job in jobs]
['queued', 'running', 'failed', 'succeeded']
>>> queued, running, failed, succeeded = jobs
>>> client.backtest.list(limit=1)[0].id == queued.id
True
>>> job = client.backtest.get(running.id)
>>> job.id == running.id, job.status == running.status, job is running
(True, True, False)
```

- **`list()` gives the jobs the server holds, the last submitted first,**
  each as a `Job`: at most `limit` of them, and 100 where it is left out.
  The server says which limits it takes, 1 to 1,000, and refuses any other.
- **`get(job_id)` gives the job of that ID,** with the status the server
  just gave. An ID the server holds no job of is a `NotFound`.
- **A job's ID is the server's name for it:** 1 to 64 ASCII letters,
  digits, `-` or `_`, by the protocol's rule. Text that is none is refused
  before anything is asked.

<!-- begin: job members -->
| Member | What it is |
| --- | --- |
| `id` | The job's ID |
| `status` | Where the job stood when the server was last asked, a `JobStatus` |
| `refresh()` | Asks the server where the job stands, keeps the answer as `status`, and gives it |
| `cancel()` | Asks the server to cancel the job, keeps the status it answers as `status`, and gives it |
<!-- end: job members -->

- **A `Job` is a handle on one job, and not a value.** `client.backtest`
  gives one, and none is built. Two handles on one job are two handles:
  each keeps what it was last given.
- **Reading a job asks nothing.** `status` is what the server last
  answered this handle, a moment or an hour ago; `refresh()` asks again.
- **`repr()` of a job is its ID and the state it was last given,** as in
  `<Job 57917a9825f44d75ad4a2e6329d00ec7 running>`.

## A job's status

**A `JobStatus` is where a job stood when the server was asked:** its
state, its times, why it failed, and what it runs.

<!-- begin: status fields -->
| Field | What it is |
| --- | --- |
| `job_id` | The job's ID |
| `state` | `queued`, `running`, `succeeded`, `failed` or `cancelled` |
| `ended` | Whether the job has ended: succeeded, failed or cancelled |
| `submitted_at_ms` | When the server accepted the job |
| `started_at_ms` | When a worker started it, or `None` |
| `finished_at_ms` | When it ended, or `None` |
| `error_code` | The protocol's code for why it failed, or `None` |
| `error_message` | The server's message for why it failed, or `None` |
| `bundle` | The code hash of the strategy bundle it runs |
| `market` | The market it runs on, a `MarketId` |
| `start_open_ms` | The open time of the first bar it runs on |
| `end_open_ms` | The open time of the last bar it runs on |
| `config` | How its fills are simulated, a `BacktestConfig` |
<!-- end: status fields -->

```python
>>> status = job.status
>>> status.state, status.ended
('running', False)
>>> status.submitted_at_ms <= status.started_at_ms, status.finished_at_ms
(True, None)
>>> status.market
MarketId(ExchangeId('binance_spot'), Symbol('BTCUSDT'), Interval('1m'))
>>> status.start_open_ms, status.end_open_ms
(1767268800000, 1767270540000)
>>> status.config
BacktestConfig(initial_cash=Decimal('10000'), fee_bps=Decimal('10'), close_out_at_end=True)
>>> status.bundle[:7], len(status.bundle)
('sha256:', 71)
>>> failed.status.error_code
'invalid_request'
>>> print(failed.status.error_message)
the strategy "fragile" failed in on_bar, at the bar that opened at 1767268800000: RuntimeError: no; its traceback is in the job's log
```

What a status holds in each state:

<!-- begin: states -->
| `state` | `started_at_ms` | `finished_at_ms` | `error_code`, `error_message` | `ended` |
| --- | --- | --- | --- | --- |
| `queued` | `None` | `None` | `None` | `False` |
| `running` | A time | `None` | `None` | `False` |
| `succeeded` | A time | A time | `None` | `True` |
| `failed` | A time, or `None` where it failed before it started | A time | The code and the message | `True` |
| `cancelled` | A time, or `None` where it was cancelled before it started | A time | `None` | `True` |
<!-- end: states -->

- **A status is a value, and never changes.** Two are equal when every
  field is, an amount by its value. It is a record, as the engine's are,
  and no key of a dict; none is built, copied or pickled.
- **Every time is milliseconds since the Unix epoch, UTC,** as every `_ms`
  field of the package is, by the server's clock.
- **What a job runs is given in the package's own types,** built by the
  engine's own rules: the market a `MarketId`, whose product line is the
  protocol's exchange and family together, and the settings a
  `BacktestConfig`, each amount an exact `decimal.Decimal` with the digits
  the server wrote ([the Python types](python-types.md)).
- **`error_code` says who is at fault:** `invalid_request` when the bundle
  or its data is, so that running it again unchanged fails again, and
  `internal` or `unavailable` when the server is.
- **`error_message` is the server's own, word for word,** and what a
  bundle made it say is in it: a strategy's name, an exception's text.
  Treat it as text a stranger wrote before it is shown where control
  characters matter, as the command line does when it prints one.
- **A state a later revision of the protocol adds is given as the server
  wrote it.** `ended` is `True` for the three the protocol has that end a
  job, and `False` for any other.
- **A status the protocol does not write is refused, and never guessed
  at:** a member that is missing or of another type, an amount that is not
  an exact decimal, a market this version of the package does not know.
  The answer that holds it is a `RemoteError`, a whole list for one such
  job.

## Refreshing and cancelling

```python
>>> queued.cancel().state
'cancelled'
>>> queued.status.started_at_ms is None, queued.status.ended
(True, True)
>>> running.cancel().state
'cancelled'
>>> running.status.started_at_ms <= running.status.finished_at_ms
True
>>> job.status.state
'running'
>>> job.refresh().state
'cancelled'
>>> job.status == running.status
True
```

- **`refresh()` asks the server where the job stands,** keeps the answer
  as the job's `status`, and gives it.
- **`cancel()` asks the server to cancel the job,** and keeps and gives
  the status it answers, which is `cancelled`. A queued job never starts.
  A running one's worker is stopped, and freed for the next job.
- **Cancelling a job that is cancelled already changes nothing,** and
  answers it as it was.
- **A cancel is answered `cancelled`, or it is no answer.** One that says
  the job is queued, running, succeeded or failed is a status the protocol
  does not write for a cancel, and is refused as any such answer is.
- **A job that has succeeded or failed is not cancelled.** The server
  refuses, a `Conflict`, and the job's `status` stays as it was:

```python
>>> try:
...     succeeded.cancel()
... except qf.Conflict as refused:
...     said = f"job {succeeded.id} has already succeeded; only a queued or running job can be cancelled"
...     str(refused) == said, refused.code, refused.status
...
(True, 'conflict', 409)
>>> succeeded.status.state
'succeeded'
```

- **A call that fails leaves the job as it was last given,** whatever it
  failed with.

## What is raised

**Every failure of a request is a `RemoteError`, or an exception derived
from it,** one for each way a caller acts on one:

<!-- begin: exceptions -->
| Exception | When | What to do |
| --- | --- | --- |
| `RemoteError` | The request did not get its answer: every exception below, and any failure that is none of them, such as the server's own (`internal`) or an answer that is not QuantForge's | Read it |
| `AuthError` | The server refused the request's credentials | Give the client the server's token |
| `ValidationError` | The server refused the request as malformed or as naming something invalid | Change the request: sent again as it is, it fails again |
| `NotFound` | What the request names does not exist on the server | Name something else |
| `Conflict` | The request contradicts the state of what it names | Ask how it stands |
| `ServerUnavailable` | The server could not be reached, or cannot answer now | Ask again later |
| `VersionMismatch` | The server and this package share no protocol version | Upgrade the older of the two |
<!-- end: exceptions -->

```python
>>> client.backtest.list(limit=0)
Traceback (most recent call last):
    ...
quantforge_py.ValidationError: limit 0 is not between 1 and 1000
>>> client.backtest.get("00000000000000000000000000000000")
Traceback (most recent call last):
    ...
quantforge_py.NotFound: the server holds no job 00000000000000000000000000000000
```

- **A refusal says what the server said, word for word.** `str()` of the
  exception is the message of the protocol's error envelope, with nothing
  added.
- **`code` is the protocol's code,** as the server wrote it, such as
  `"invalid_request"`, and **`status` the HTTP status** of the answer.
  `code` is `None` where no QuantForge server refused the request, and
  `status` where no answer came, or the answer was no refusal.
- **A `VersionMismatch` has `supported` too:** the versions the server
  speaks, the oldest and the newest, as a tuple of two `int`s, or `None`
  where it did not say.
- **An exception is raised by the code, and never by the status.** Every
  code the protocol has:

<!-- begin: codes -->
| The protocol's code | HTTP status | Raises |
| --- | --- | --- |
| `invalid_request` | 400 | `ValidationError` |
| `unauthorized` | 401 | `AuthError` |
| `not_found` | 404 | `NotFound` |
| `method_not_allowed` | 405 | `RemoteError` |
| `conflict` | 409 | `Conflict` |
| `unsupported_version` | 400 | `VersionMismatch` |
| `limit_exceeded` | 429 | `RemoteError` |
| `rate_limited` | 429 | `RemoteError` |
| `unavailable` | 503 | `ServerUnavailable` |
| `internal` | 500 | `RemoteError` |
<!-- end: codes -->

- **A code a later revision of the protocol adds is a `RemoteError`,**
  with that code and the server's message.

**Where no QuantForge server answered,** the message is the client's own,
and names the server by its URL:

<!-- begin: without an envelope -->
| What came | Raises |
| --- | --- |
| No answer: no connection, or one that broke, timed out or failed its TLS before the answer was whole | `ServerUnavailable` |
| `401` or `403` without the protocol's error envelope | `AuthError` |
| `502`, `503` or `504` without the protocol's error envelope | `ServerUnavailable` |
| Any other status that is no success without it, a redirect among them | `RemoteError` |
| A success whose body is not what the protocol says the route answers | `RemoteError` |
| An answer longer than any QuantForge server sends, by the length it states or by what came | `RemoteError` |
| A connection for a token over `http` that does not end on this machine | `RemoteError` |
<!-- end: without an envelope -->

A proxy in front of a server answers in its place where it refuses a
request, or cannot reach the server: its `401` is an `AuthError` and its
`502` a `ServerUnavailable`, so a caller acts on them as on the server's
own. Anything else that is not QuantForge's says so, and asks whether the
URL names a QuantForge server:

```python
>>> nowhere = qf.Client("http://127.0.0.1:9")
>>> try:
...     nowhere.backtest.list()
... except qf.ServerUnavailable as unreachable:
...     said = str(unreachable)
...     said.startswith("could not reach the server at http://127.0.0.1:9/: ")
...     unreachable.code, unreachable.status
...
True
(None, None)
```

**A `ServerUnavailable` may be asked again.** A read that failed changed
nothing. A cancel whose answer never came may have been taken all the same:
asked again, it cancels the job or finds it cancelled.

## What is asked again

**A request is sent again while the server answers `503` with
`Retry-After`,** a few times, waiting as long as the server asks: the
server's own answer where its store could not answer in time, and a
proxy's. Every request a client makes today may be: a read changes
nothing, and a cancel nothing more the second time.

<!-- begin: limits -->
| What | Limit |
| --- | --- |
| Opening a connection, TLS included | 10 seconds |
| The server's silence while it answers | 60 seconds |
| Tries of one request while the server answers `503` with `Retry-After` | 5 |
| One wait between two tries | The seconds `Retry-After` names, 5 at most |
| One answer | 64 MiB |
<!-- end: limits -->

- **A `503` that names no whole number of seconds is raised at once,** and
  so is every other refusal.
- **After the last try the `503` is raised,** as the `ServerUnavailable` it
  is.
- **A call can take as long as its tries and its waits together.** None of
  these is an argument yet.

## Connections

- **Every connection goes to the server itself.** A client reads no proxy
  from the environment, `HTTPS_PROXY` and its kin, and uses none. The
  command line's `--remote` does read them.
- **No redirect is followed.** A QuantForge server never redirects, and a
  token goes to the URL it was given for or nowhere: an answer that
  redirects is a `RemoteError` that says so.
- **Over `https` the server's certificate is checked** against what the
  system trusts and the name the URL gives, by Python's own defaults
  (`ssl.create_default_context`). One that fails is a `ServerUnavailable`,
  and nothing is sent.
- **Each request is made on a connection of its own,** and closes it.
- **The package imports nothing of the network until a client asks its
  server.** `import quantforge_py` is as fast with a client in it as
  without, and a program that never makes one pays nothing for it.

## Refusals

A value of the wrong type is a `TypeError`, and one the rules refuse a
`ValueError`, before anything is asked. Each row is what Python raises for
the call, where `client` is a client:

<!-- begin: refusals -->
| Call | Raises |
| --- | --- |
| `qf.Client("127.0.0.1:9600")` | `ValueError: base_url must start with http:// or https://, such as http://127.0.0.1:9600` |
| `qf.Client("")` | `ValueError: base_url is empty; name the server, such as http://127.0.0.1:9600` |
| `qf.Client("http://")` | `ValueError: base_url names no host; name the server, such as http://127.0.0.1:9600` |
| `qf.Client("http://127.0.0.1:port")` | `ValueError: base_url is not a URL the client reads; name the server, such as http://127.0.0.1:9600` |
| `qf.Client("http://127.0.0.1:9600/?debug=1")` | `ValueError: base_url holds a query or a fragment; name the server and the path it is served under, and nothing more` |
| `qf.Client("http://127.0.0.1:9600/my lab")` | `ValueError: base_url holds a space or a character that is not printed; name the server, such as http://127.0.0.1:9600` |
| `qf.Client("https://ann:PASSWORD@qf.example")` | `ValueError: base_url holds a user name or a password, which the client does not send; give the server's token as token, or in QF_REMOTE_TOKEN` |
| `qf.Client(b"http://127.0.0.1:9600")` | `TypeError: base_url must be a str, not bytes` |
| `qf.Client("http://127.0.0.1:9600", token=7)` | `TypeError: token must be a str or None, not int` |
| `qf.Client("http://127.0.0.1:9600", token="two words")` | `ValueError: token holds a character other than printable ASCII, or a space inside it: a token is sent in a header` |
| `qf.Client("http://qf.example:9600", token="TOKEN")` | `ValueError: a token goes only over https, or over http to this machine, and the server at http://qf.example:9600/ is neither; nothing was sent` |
| `client.backtest.get("no such job")` | `ValueError: invalid job id 'no such job': expected 1 to 64 ASCII letters, digits, '-', or '_'` |
| `client.backtest.get(7)` | `TypeError: job_id must be a str, not int` |
| `client.backtest.list(limit="3")` | `TypeError: limit must be an int or None, not str` |
| `qf.Job()` | `TypeError: a Job cannot be built: Client.backtest gives one, by get(job_id) or in list()` |
| `qf.JobStatus()` | `TypeError: a JobStatus cannot be built: it is what a server answered of a job` |
<!-- end: refusals -->

A refusal that comes from the variable names the variable in the
argument's place: `QF_REMOTE_TOKEN holds a character other than printable
ASCII, or a space inside it: a token is sent in a header`. A token the
variable holds that may not go to the server says whose it is, and how a
client is made without it: `a token goes only over https, or over http to
this machine, and the server at http://qf.example:9600/ is neither; nothing
was sent. The token is QF_REMOTE_TOKEN's: pass token="" to send none`.

## What is left

| What | Story |
| --- | --- |
| A bundle built from a strategy's files and submitted as a job, from Python | QUA-462 |
| A job waited for, its log followed, its result as DataFrames, and its provenance | QUA-468 |
| A server that checks the token it is sent | QUA-345 |
