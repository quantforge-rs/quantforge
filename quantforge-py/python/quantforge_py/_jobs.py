"""A server's backtest jobs as a client holds them: `Job`, the handle on
one job, and `JobStatus`, where a job stood when the server was last asked,
which the package publishes; a status read strictly from what the protocol
writes; and the server's jobs listed, got and submitted
(`docs/python-client.md`, `docs/python-bundles.md`).

A job is waited for here. What else is asked of one is a module of its
own: its log followed (`quantforge_py._logs`), its result
(`quantforge_py._results`) and its provenance
(`quantforge_py._provenance`), each of which `docs/python-jobs.md` is the
contract of.

What a job runs is given in the package's own types, built by the engine's
own rules: its market a `MarketId`, and how its fills are simulated a
`BacktestConfig`, every amount an exact decimal.

This module is private: import its names from `quantforge_py`.
"""

import operator
import time

from ._errors import _published
from ._native import BacktestConfig, MarketId
from ._wire import _member, _object, _optional, _quoted, _Unreadable, _wrong_type

# The states the protocol names, and those in which a job has ended.
_QUEUED, _RUNNING = "queued", "running"
_SUCCEEDED, _FAILED, _CANCELLED = "succeeded", "failed", "cancelled"
_ENDED = (_SUCCEEDED, _FAILED, _CANCELLED)
_STATES = (_QUEUED, _RUNNING, *_ENDED)

# The members of a market, as the protocol writes one.
_MARKET = ("exchange", "category", "symbol", "interval")

# What a job's ID and the digits of a content hash are written with.
_LETTERS = "abcdefghijklmnopqrstuvwxyz"
_ID_CHARACTERS = frozenset(_LETTERS + _LETTERS.upper() + "0123456789-_")
_HEX = frozenset("0123456789abcdef")
_HASH_PREFIX = "sha256:"

# The most a count is that the protocol holds in 32 bits: the bars of a
# segment, and the number of a protocol version and of a store's schema.
_MOST_IN_32_BITS = 2**32 - 1

# How long `Job.wait` waits before it asks its server again: this long at
# first, twice as long each time after, and never longer than the second.
_WAIT_SECONDS = 0.1
_MOST_WAIT_SECONDS = 2.0


def _final(cls):
    """Refuses a class derived from `cls`, in the words Python refuses one
    derived from a type of the extension module, and refuses to pickle or
    copy one of `cls`, as it refuses one of those."""
    named = f"{cls.__module__}.{cls.__qualname__}"

    def derived(subclass, **given):
        raise TypeError(f"type '{named}' is not an acceptable base type")

    def reduced(self, protocol):
        raise TypeError(f"cannot pickle '{named}' object")

    cls.__init_subclass__ = classmethod(derived)
    cls.__reduce_ex__ = reduced
    return cls


def _not_writable(value, name):
    """Refuses a change to an attribute of a value, as Python refuses one of
    a type of the extension module."""
    kind = type(value)
    named = f"{kind.__module__}.{kind.__qualname__}"
    if hasattr(kind, name):
        raise AttributeError(f"attribute '{name}' of '{named}' objects is not writable")
    raise AttributeError(f"'{named}' object has no attribute '{name}'")


def _given(slot, said):
    """A field of a record: what the record was given for it, under the
    field's own name, with what the class says of it."""
    return property(operator.attrgetter(slot), doc=said)


class _Record:
    """What the records a server's answers are read into share. A record is
    what a server answered, and none is built; it is a value, and never
    changes; two are equal when every field is; and it is no key, as the
    engine's own records are none.

    A record's class names its fields in `__slots__`, each with a `_`
    before it, and says in `_BUILT` why none is built."""

    __slots__ = ()

    def __init__(self, *args, **kwargs):
        raise TypeError(self._BUILT)

    @classmethod
    def _of(cls, **fields):
        record = object.__new__(cls)
        for name in cls.__slots__:
            object.__setattr__(record, name, fields[name.removeprefix("_")])
        return record

    def __setattr__(self, name, value):
        _not_writable(self, name)

    def __delattr__(self, name):
        _not_writable(self, name)

    def _fields(self):
        return tuple(operator.attrgetter(*self.__slots__)(self))

    def __eq__(self, other):
        if type(other) is not type(self):
            return NotImplemented
        return self._fields() == other._fields()

    __hash__ = None


def _is_job_id(text):
    """Whether text is a job's ID by the protocol's rule: 1 to 64 ASCII
    letters, digits, `-` or `_`."""
    return 1 <= len(text) <= 64 and _ID_CHARACTERS.issuperset(text)


def _is_exact_decimal(text):
    """Whether text is a decimal in the protocol's exact form: digits, with
    no sign but a `-`, none on a zero, no exponent, and no zero before a
    whole part."""
    digits = text.removeprefix("-")
    whole, point, fraction = digits.partition(".")
    if not (whole.isascii() and whole.isdigit()) or (len(whole) > 1 and whole[0] == "0"):
        return False
    if point and not (fraction.isascii() and fraction.isdigit()):
        return False
    return digits == text or not set(digits) <= {"0", "."}


def _is_content_hash(text):
    """Whether text is a content hash as the protocol writes one: `sha256:`
    and 64 lowercase hex digits."""
    digits = text.removeprefix(_HASH_PREFIX)
    return len(text) == len(_HASH_PREFIX) + 64 and len(digits) == 64 and _HEX.issuperset(digits)


def _word(text):
    """What a server names, as a repr writes it: a plain word as it is, and
    any other text as Python writes a string, in part where it is very
    long. The words the protocol has are plain. One that came with a
    terminal's control characters in it is written out for a reader, and
    never to the terminal as it came."""
    if 1 <= len(text) <= 64 and _ID_CHARACTERS.issuperset(text):
        return text
    return _quoted(text)


def _count(record, name, what, most=None):
    """A count as the protocol writes one: an integer that is not negative,
    and no more than `most` where the protocol holds it in fewer bits than
    its integers have."""
    count = _member(record, name, int, what)
    if count < 0:
        raise _Unreadable(f"the `{name}` of {what} is negative")
    if most is not None and count > most:
        raise _Unreadable(f"the `{name}` of {what} is beyond the {most} the protocol holds it to")
    return count


def _market(exchange, category, symbol, interval, what):
    """The market four names of the protocol say, as the engine's own type,
    built by the engine's own rules: a product line or an interval it does
    not hold is its to refuse."""
    named = f"{exchange} {category} {symbol} {interval}"
    try:
        market = MarketId(f"{exchange}_{category}", symbol, interval)
    except ValueError as refused:
        raise _Unreadable(
            f"{what} names the market {named}, which this version of the package does not know "
            f"({refused})"
        ) from None
    if (str(market.exchange), str(market.symbol), str(market.interval)) != (
        f"{exchange}_{category}",
        symbol,
        interval,
    ):
        raise _Unreadable(f"{what} names the market {named}, which is not in its canonical form")
    return market


def _config(settings, what):
    """How fills are simulated, as the protocol writes it, as the engine's
    own type: an amount it does not hold is its to refuse."""
    try:
        return BacktestConfig(
            initial_cash=_amount(settings, "initial_cash", what),
            fee_bps=_amount(settings, "fee_bps", what),
            close_out_at_end=_member(settings, "close_out_at_end", bool, what),
        )
    except ValueError as refused:
        raise _Unreadable(f"{what} has settings the engine refuses ({refused})") from None


def _amount(record, name, what):
    """An amount as the protocol writes one: a string, in its exact form."""
    import decimal

    text = _member(record, name, str, what)
    if not _is_exact_decimal(text):
        raise _Unreadable(f"the `{name}` of {what} is no decimal in its exact form")
    return decimal.Decimal(text)


def _job_id(job_id):
    """A job's ID as a caller names one, checked by the protocol's rule
    before any request is made with it."""
    if not isinstance(job_id, str):
        raise _wrong_type("job_id", "a str", job_id)
    if not _is_job_id(job_id):
        raise ValueError(
            f"invalid job id {_quoted(job_id)}: expected 1 to 64 ASCII letters, digits, '-', or '_'"
        )
    return job_id


def _seconds(timeout):
    """How long a caller waits at the most, in seconds, or `None` for as
    long as it takes."""
    if timeout is None:
        return None
    if isinstance(timeout, bool) or not isinstance(timeout, (int, float)):
        raise _wrong_type("timeout", "a number of seconds or None", timeout)
    said = "it is how many seconds to wait at the most, or None for as long as the job takes"
    if timeout != timeout:
        raise ValueError(f"timeout is no number: {said}")
    if timeout < 0:
        raise ValueError(f"timeout is negative: {said}")
    try:
        return float(timeout)
    except OverflowError:
        # A whole number of seconds beyond any float: as long as it takes.
        return float("inf")


def _status(value):
    """A job's status as the protocol writes it, read strictly into a
    `JobStatus`."""
    what = "a job's status"
    record = _object(value, what)
    job_id = _member(record, "job_id", str, what)
    if not _is_job_id(job_id):
        raise _Unreadable(f"the `job_id` of {what} is no job ID")
    submitted = _member(record, "submitted_at_ms", int, what)
    submission = _member(record, "submission", dict, what)
    state = _member(record, "state", dict, what)

    what = f"job {job_id}'s submission"
    bundle = _member(submission, "bundle", str, what)
    if not _is_content_hash(bundle):
        raise _Unreadable(f"the `bundle` of {what} is no content hash")
    data = _member(submission, "data", dict, what)
    names = [_member(data, name, str, what) for name in _MARKET]
    start = _member(data, "start_open_ms", int, what)
    end = _member(data, "end_open_ms", int, what)
    # The engine's own types, built by the engine's own rules.
    market = _market(*names, what)
    config = _config(_member(submission, "backtest", dict, what), what)

    what = f"job {job_id}'s state"
    kind = _member(state, "kind", str, what)
    known = kind in _STATES
    started = finished = code = message = None
    if kind in (_RUNNING, _SUCCEEDED):
        started = _member(state, "started_at_ms", int, what)
    elif kind != _QUEUED:
        # A job fails or is cancelled before it starts as well as after, and
        # a state this version does not know says what it says.
        started = _optional(state, "started_at_ms", int, what)
    if kind in _ENDED:
        finished = _member(state, "finished_at_ms", int, what)
    elif not known:
        finished = _optional(state, "finished_at_ms", int, what)
    if kind == _SUCCEEDED:
        _member(state, "results", list, what)
    if kind == _FAILED:
        error = _member(state, "error", dict, what)
        code = _member(error, "code", str, f"job {job_id}'s error")
        message = _member(error, "message", str, f"job {job_id}'s error")
    return JobStatus._of(
        job_id=job_id,
        state=kind,
        submitted_at_ms=submitted,
        started_at_ms=started,
        finished_at_ms=finished,
        error_code=code,
        error_message=message,
        bundle=bundle,
        market=market,
        start_open_ms=start,
        end_open_ms=end,
        config=config,
    )


def _status_of(job_id):
    """Reads an answer that is the status of the job `job_id`, and of no
    other."""

    def read(value):
        status = _status(value)
        if status.job_id != job_id:
            raise _Unreadable(f"it is the status of job {status.job_id}, not of job {job_id}")
        return status

    return read


def _cancelled(job_id):
    """Reads the answer to a cancel of the job `job_id`: its status, which
    the protocol says is cancelled. One that says the job is in another of
    the protocol's states is no answer to a cancel; a state a later revision
    of the protocol adds is given as the server wrote it."""
    read = _status_of(job_id)

    def cancelled(value):
        status = read(value)
        if status.state in _STATES and status.state != _CANCELLED:
            raise _Unreadable(
                f"it says that job {job_id} is {status.state}, and a job that was cancelled is "
                "cancelled"
            )
        return status

    return cancelled


@_final
@_published
class JobStatus:
    """Where a backtest job stood when a server was last asked: its state,
    its times, why it failed, and what it runs.

    A status is what a server answered: `Client.backtest` and a `Job` give
    one, and none is built. It is a value, and never changes: two are equal
    when every field is.

    - `job_id`: the job's ID, the server's name for it.
    - `state`: `"queued"`, `"running"`, `"succeeded"`, `"failed"` or
      `"cancelled"`. A state a later revision of the protocol adds is given
      as the server wrote it.
    - `ended`: whether the job has ended: succeeded, failed or cancelled.
    - `submitted_at_ms`: when the server accepted the job.
    - `started_at_ms`: when a worker started it; `None` while it is queued,
      and where it failed or was cancelled before it started.
    - `finished_at_ms`: when it ended; `None` until it has.
    - `error_code` and `error_message`: why a failed job failed, as the
      protocol's code and the server's message; `None` otherwise.
    - `bundle`: the code hash of the strategy bundle it runs.
    - `market`: the market it runs on, a `MarketId`.
    - `start_open_ms` and `end_open_ms`: the open times of the first and the
      last bar it runs on.
    - `config`: how its fills are simulated, a `BacktestConfig`.

    Every time is milliseconds since the Unix epoch, UTC, by the server's
    clock.
    """

    __slots__ = (
        "_job_id",
        "_state",
        "_submitted_at_ms",
        "_started_at_ms",
        "_finished_at_ms",
        "_error_code",
        "_error_message",
        "_bundle",
        "_market",
        "_start_open_ms",
        "_end_open_ms",
        "_config",
    )

    def __init__(self, *args, **kwargs):
        raise TypeError("a JobStatus cannot be built: it is what a server answered of a job")

    @classmethod
    def _of(cls, **fields):
        status = object.__new__(cls)
        for name in cls.__slots__:
            object.__setattr__(status, name, fields[name.removeprefix("_")])
        return status

    def __setattr__(self, name, value):
        _not_writable(self, name)

    def __delattr__(self, name):
        _not_writable(self, name)

    @property
    def job_id(self):
        """The job's ID, the server's name for it."""
        return self._job_id

    @property
    def state(self):
        """The job's state: queued, running, succeeded, failed or cancelled,
        or what a later revision of the protocol names."""
        return self._state

    @property
    def ended(self):
        """Whether the job has ended: succeeded, failed or cancelled."""
        return self._state in _ENDED

    @property
    def submitted_at_ms(self):
        """When the server accepted the job."""
        return self._submitted_at_ms

    @property
    def started_at_ms(self):
        """When a worker started the job, or None."""
        return self._started_at_ms

    @property
    def finished_at_ms(self):
        """When the job ended, or None."""
        return self._finished_at_ms

    @property
    def error_code(self):
        """The protocol's code for why the job failed, or None."""
        return self._error_code

    @property
    def error_message(self):
        """The server's message for why the job failed, or None."""
        return self._error_message

    @property
    def bundle(self):
        """The code hash of the strategy bundle the job runs."""
        return self._bundle

    @property
    def market(self):
        """The market the job runs on, a MarketId."""
        return self._market

    @property
    def start_open_ms(self):
        """The open time of the first bar the job runs on."""
        return self._start_open_ms

    @property
    def end_open_ms(self):
        """The open time of the last bar the job runs on."""
        return self._end_open_ms

    @property
    def config(self):
        """How the job's fills are simulated, a BacktestConfig."""
        return self._config

    def _fields(self):
        return tuple(getattr(self, name) for name in self.__slots__)

    def __eq__(self, other):
        if type(other) is not JobStatus:
            return NotImplemented
        return self._fields() == other._fields()

    # A record, as the engine's are: equal by its fields, and no key.
    __hash__ = None

    def __repr__(self):
        return f"<JobStatus {self._job_id} {_word(self._state)}>"


@_final
@_published
class Job:
    """One backtest job a server holds: its ID, where it stood when the
    server was last asked, and what can be asked of it.

    `Client.backtest` gives a job, by `get`, in `list` or for one it
    submits, and none is built. Reading an attribute never asks the server:
    its methods do. `refresh`, `cancel` and `wait` keep what the server
    answers as `status`.

    - `id`: the job's ID, the server's name for it.
    - `status`: where the job stood when the server was last asked, a
      `JobStatus`.
    - `refresh()` and `cancel()`: ask where the job stands, and ask for it
      to be cancelled.
    - `wait(timeout=None)`: asks until the job has ended.
    - `stream_logs(*, after=0, on_progress=None)`: follows the job's log,
      line by line, to its end.
    - `result()`: what the job produced, once it has succeeded, a
      `JobResult`.
    - `provenance()`: what the job ran on, as its server recorded it before
      the job ran, a `Provenance`.
    """

    __slots__ = ("_wire", "_status")

    def __init__(self, *args, **kwargs):
        raise TypeError(
            "a Job cannot be built: Client.backtest gives one, by get(job_id), in list(), or "
            "for a job it submits"
        )

    @classmethod
    def _of(cls, wire, status):
        job = object.__new__(cls)
        object.__setattr__(job, "_wire", wire)
        object.__setattr__(job, "_status", status)
        return job

    def __setattr__(self, name, value):
        _not_writable(self, name)

    def __delattr__(self, name):
        _not_writable(self, name)

    @property
    def id(self):
        """The job's ID, the server's name for it."""
        return self._status.job_id

    @property
    def status(self):
        """Where the job stood when the server was last asked."""
        return self._status

    def _asked(self, method, route, read):
        """Asks a route of this job that answers its status, and keeps the
        answer as `read` makes of it."""
        status = self._wire.ask(method, route, read(self.id))
        object.__setattr__(self, "_status", status)
        return status

    def refresh(self):
        """Asks the server where the job stands, keeps the answer as
        `status`, and gives it."""
        return self._asked("GET", f"backtests/{self.id}", _status_of)

    def cancel(self):
        """Asks the server to cancel the job, keeps the status it answers
        as `status`, and gives it: `cancelled`, by the protocol.

        A queued job never starts, and a running one's worker is stopped. A
        job that is cancelled already stays so. One that has succeeded or
        failed is a `Conflict`, and `status` stays as it was.

        That a cancel returns says that the server took it. A state a later
        revision of the protocol answers one with is given as the server
        wrote it, and its `ended` is `False`: read `ended` where the job has
        to have ended.
        """
        return self._asked("POST", f"backtests/{self.id}/cancel", _cancelled)

    def wait(self, timeout=None):
        """Asks the server where the job stands until it has ended, keeps
        each answer as `status`, and gives the last: the status of a job
        that has succeeded, failed or been cancelled. A job the handle
        already holds as ended is given as it is, and nothing is asked.

        `timeout` is how many seconds to wait at the most, an `int` or a
        `float`, or `None` for as long as the job takes. A job that has not
        ended by then is a `TimeoutError`: the job goes on, and `status`
        is what the server last answered. The time is looked at between two
        requests, and one that is under way is not cut short. The last is
        made as the time runs out, so that a job that ended by then is
        given as ended.

        The server is asked again after a tenth of a second, and then after
        twice as long each time, two seconds at the most. A request that
        fails is raised as it is, and the job waited for again by calling
        again.

        A job that failed is given like any other: read `state`. A state a
        later revision of the protocol adds does not end a job here, as its
        `ended` is `False`.
        """
        seconds = _seconds(timeout)
        status = self._status
        if status.ended:
            return status
        deadline = None if seconds is None else time.monotonic() + seconds
        pause = _WAIT_SECONDS
        while True:
            status = self.refresh()
            if status.ended:
                return status
            nap = pause
            if deadline is not None:
                left = deadline - time.monotonic()
                if left <= 0:
                    raise TimeoutError(
                        f"job {self.id} had not ended after {seconds:g} seconds: it was "
                        f"{status.state} when the server was last asked"
                    )
                nap = min(pause, left)
            time.sleep(nap)
            pause = min(pause * 2, _MOST_WAIT_SECONDS)

    def stream_logs(self, *, after=0, on_progress=None):
        """Follows the job's log to its end: gives an iterator of its
        lines, each a `LogLine`, first the ones the server already holds
        and then each new one as the job writes it. It ends once the job
        has ended and its last line was given, so a log followed late is
        the whole log, and one followed from the start is every line as it
        comes.

        - `after`: the number of the last line the caller already holds:
          the first line given is the one after it. 0, which it is unless
          it is given, gives the log from its first line.
        - `on_progress`: called with `bars_done` and `bars_total`, two
          `int`s, whenever the server says the job has come further than
          it last said; or `None`, and nothing is.

        Nothing is asked until the iterator is first asked for a line. It
        asks the server for a stream of the log, and reads the log a page
        at a time, from the last line it gave, wherever the stream cannot
        be had or breaks off: no line is given twice, and none is left
        out. Lines the server no longer holds are passed over, which shows
        as a jump in `seq`. The log of a queued job begins with the job.

        Closing the iterator lets go of the connection, which Python does
        once nothing holds the iterator any more, as after a `break` out of
        a loop over it. A request that fails is raised from the loop as it
        is, and the log followed again from where it stopped by `after=`
        the `seq` of the last line given.
        """
        from ._logs import _asked, _followed

        return _followed(self._wire, self.id, *_asked(after, on_progress))

    def result(self):
        """Asks the server what the job produced, and gives it, a
        `JobResult`: the job's metrics, and its closed trades and its
        equity curve, each downloaded when it is first asked for.

        Only a job that has succeeded has a result: one that is queued or
        running, that failed or was cancelled is a `Conflict`, which says
        which. Each call asks the server: keep the result it gives.
        """
        from ._results import _result

        return _result(self._wire, self.id)

    def provenance(self):
        """Asks the server what the job ran on, and gives it, a
        `Provenance`: the bundle, the bars and the segments that hold them,
        the settings and what ran the bundle, as the server recorded them
        before the job ran, with the identity of those inputs, which is
        checked here against them.

        A job has its provenance once it has started. One that is still
        queued, or that ended before its server read its inputs, is a
        `Conflict`, which says which.
        """
        from ._provenance import _provenance

        return self._wire.ask("GET", f"backtests/{self.id}/provenance", _provenance(self.id))

    def __repr__(self):
        return f"<Job {self.id} {_word(self._status.state)}>"


class _Backtests:
    """A server's backtest jobs, as `Client.backtest` gives them."""

    __slots__ = ("_wire",)

    def __init__(self, wire):
        self._wire = wire

    def list(self, *, limit=None):
        """The jobs the server holds, the last submitted first, as a list of
        `Job`: at most `limit` of them, and where it is left out as many as
        the server lists then, 100.

        The server says which limits it takes, 1 to 1,000, and any other is
        a `ValidationError`.
        """
        if limit is not None and (not isinstance(limit, int) or isinstance(limit, bool)):
            raise _wrong_type("limit", "an int or None", limit)
        route = "backtests" if limit is None else f"backtests?limit={limit}"

        def read(value):
            what = "the list of jobs"
            return [_status(each) for each in _member(_object(value, what), "jobs", list, what)]

        return [Job._of(self._wire, status) for status in self._wire.ask("GET", route, read)]

    def get(self, job_id):
        """The job of that ID, as a `Job` with the status the server just
        gave. An ID the server holds no job of is a `NotFound`."""
        job_id = _job_id(job_id)
        return Job._of(self._wire, self._wire.ask("GET", f"backtests/{job_id}", _status_of(job_id)))

    def submit(
        self,
        strategy,
        *,
        entry=None,
        params=None,
        deps=None,
        market,
        start=None,
        end=None,
        config,
    ):
        """Submits a backtest of a strategy's files, and gives the job the
        server queued, as a `Job`.

        The files are made into a strategy bundle, which is checked here by
        the code a server checks an upload with, uploaded under the code
        hash computed here, and submitted. Nothing is uploaded for a bundle
        that is refused, a `BundleError`, and nothing is pickled or run.

        - `strategy`: a `.py` file, or the directory of a strategy's files,
          a `str` or an `os.PathLike`. A directory's own `quantforge.toml`
          is its manifest; without one, and for a file, one is written from
          `entry`, `params` and `deps`.
        - `entry`: the strategy's file in the bundle and its class, such as
          `"strategy.py:MyStrategy"`.
        - `params`: the strategy's parameters, a dict of values JSON holds:
          the class is built from them, as keyword arguments.
        - `deps`: the packages the strategy imports: a dict that pins each
          at a version, or a list of names, each pinned at the version the
          server offers.
        - `market`: the market to run on, a `MarketId`.
        - `start` and `end`: the window, each an `int` of milliseconds, a
          `datetime.datetime` with a time zone or an RFC 3339 `str`. The job
          runs on the bars the server stores between them, and one left out
          reaches the first or the last bar it stores.
        - `config`: how fills are simulated, a `BacktestConfig`.

        A submission is sent once. One that fails as a `ServerUnavailable`
        may have been taken all the same: list the server's jobs before
        submitting it again.
        """
        from ._bundles import _Draft
        from ._submit import _asked, _submitted

        asked = _asked(market, start, end, config)
        draft = _Draft.of_path(strategy, entry, params, deps)
        return Job._of(self._wire, _submitted(self._wire, draft, *asked))

    def submit_class(
        self,
        strategy,
        *,
        imports="import quantforge_py as qf",
        params=None,
        deps=None,
        market,
        start=None,
        end=None,
        config,
        source_only=False,
    ):
        """Submits a backtest of a class by its source, for a class written
        in a notebook, and gives the job the server queued, as a `Job`.

        The class's own source text is the one Python file of the bundle,
        below `imports`. Nothing else of where the class was written is
        sent, and nothing is pickled: no import, no global and no variable
        around the class, and not the class as it is in memory. The server
        runs the text.

        - `strategy`: a class derived from `Strategy`, written at the top
          level of a module or of a notebook's cell.
        - `imports`: the import statements the class's source needs, written
          above it. A name the source reads that neither they, nor the
          class, nor Python's builtins define is refused, with the name.
        - `source_only`: `True` says that the caller knows all of the above.
          Without it the call is refused before anything is read.

        `params`, `deps`, `market`, `start`, `end` and `config` are
        `submit`'s. A strategy kept in files is submitted with `submit`.
        """
        from ._bundles import _Draft
        from ._source import _class_file
        from ._submit import _asked, _submitted

        file, entry = _class_file(strategy, imports, source_only)
        asked = _asked(market, start, end, config)
        draft = _Draft.of_files([file], entry, params, deps)
        return Job._of(self._wire, _submitted(self._wire, draft, *asked))

    def __repr__(self):
        return f"<backtests of {self._wire.server.shown}>"
