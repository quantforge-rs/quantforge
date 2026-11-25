"""A server's backtest jobs as a client holds them: `Job`, the handle on
one job, and `JobStatus`, where a job stood when the server was last asked,
which the package publishes; a status read strictly from what the protocol
writes; and the server's jobs listed and got (`docs/python-client.md`).

What a job runs is given in the package's own types, built by the engine's
own rules: its market a `MarketId`, and how its fills are simulated a
`BacktestConfig`, every amount an exact decimal.

This module is private: import its names from `quantforge_py`.
"""

from ._errors import _published
from ._native import BacktestConfig, MarketId
from ._wire import _member, _object, _optional, _quoted, _Unreadable, _wrong_type

# The states the protocol names, and those in which a job has ended.
_QUEUED, _RUNNING = "queued", "running"
_SUCCEEDED, _FAILED, _CANCELLED = "succeeded", "failed", "cancelled"
_ENDED = (_SUCCEEDED, _FAILED, _CANCELLED)
_STATES = (_QUEUED, _RUNNING, *_ENDED)

# What a job's ID and the digits of a content hash are written with.
_LETTERS = "abcdefghijklmnopqrstuvwxyz"
_ID_CHARACTERS = frozenset(_LETTERS + _LETTERS.upper() + "0123456789-_")
_HEX = frozenset("0123456789abcdef")
_HASH_PREFIX = "sha256:"


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
    digits = bundle.removeprefix(_HASH_PREFIX)
    if len(bundle) != len(_HASH_PREFIX) + 64 or len(digits) != 64 or not _HEX.issuperset(digits):
        raise _Unreadable(f"the `bundle` of {what} is no content hash")
    data = _member(submission, "data", dict, what)
    exchange, category, symbol, interval = (
        _member(data, name, str, what) for name in ("exchange", "category", "symbol", "interval")
    )
    start = _member(data, "start_open_ms", int, what)
    end = _member(data, "end_open_ms", int, what)
    # The engine's own types, built by the engine's own rules: a product
    # line, an interval or an amount it does not hold is its to refuse.
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
    settings = _member(submission, "backtest", dict, what)
    try:
        config = BacktestConfig(
            initial_cash=_amount(settings, "initial_cash", what),
            fee_bps=_amount(settings, "fee_bps", what),
            close_out_at_end=_member(settings, "close_out_at_end", bool, what),
        )
    except ValueError as refused:
        raise _Unreadable(f"{what} has settings the engine refuses ({refused})") from None

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
        return f"<JobStatus {self._job_id} {self._state}>"


@_final
@_published
class Job:
    """One backtest job a server holds: its ID, where it stood when the
    server was last asked, and what can be asked of it.

    `Client.backtest` gives a job, by `get` or in `list`, and none is built.
    Reading an attribute never asks the server: `refresh` and `cancel` do,
    and keep what it answers as `status`.

    - `id`: the job's ID, the server's name for it.
    - `status`: where the job stood when the server was last asked, a
      `JobStatus`.
    """

    __slots__ = ("_wire", "_status")

    def __init__(self, *args, **kwargs):
        raise TypeError(
            "a Job cannot be built: Client.backtest gives one, by get(job_id) or in list()"
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

    def __repr__(self):
        return f"<Job {self.id} {self._status.state}>"


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

    def __repr__(self):
        return f"<backtests of {self._wire.server.shown}>"
