"""What a job ran on, as a client holds it: `Provenance` and
`ProvenanceSegment`, which the package publishes, read strictly from what
the protocol writes, with the identity of a job's inputs taken again here
and held to the one its server gave (`docs/python-jobs.md`).

As a job starts, its server reads everything the job will run on and
records it before the job runs: the bundle by its code hash, the market
and the range with every segment that holds the job's bars, how fills are
simulated, and the engine, the Python and the packages that run the bundle.
The identity of those inputs is the SHA-256 of their compact JSON, which
Python writes byte for byte as the server does, from the members as they
were received. So a provenance a client holds is one whose inputs are the
ones its identity names.

What a job ran on is given in the package's own types, as a job's status
is: its market a `MarketId`, and how its fills were simulated a
`BacktestConfig`, every amount an exact decimal.

This module is private: import its names from `quantforge_py`.
"""

from ._errors import _published
from ._jobs import (
    _MARKET,
    _MOST_IN_32_BITS,
    _config,
    _count,
    _final,
    _given,
    _is_content_hash,
    _is_job_id,
    _market,
    _Record,
)
from ._results import _file
from ._wire import _member, _object, _optional, _Unreadable

# The provenance format this version of the package reads.
_FORMAT = 1


@_final
@_published
class ProvenanceSegment(_Record):
    """One segment a job's bars came from: stored candles, named by the
    hash of their bytes.

    A `Provenance` gives one for each segment, and none is built. It is a
    value, and never changes: two are equal when every field is.

    - `hash`: the segment's name: `sha256:` and the SHA-256 of its
      payload, in hex. A server gives the segment under it, at
      `GET /v1/segments/{hash}`.
    - `first_open_ms` and `last_open_ms`: the open times of its first and
      its last bar, in milliseconds since the Unix epoch, UTC.
    - `bar_count`: how many bars it holds.
    """

    __slots__ = ("_hash", "_first_open_ms", "_last_open_ms", "_bar_count")

    _BUILT = (
        "a ProvenanceSegment cannot be built: a Provenance gives one for each segment its job's "
        "bars came from"
    )

    hash = _given("_hash", "The segment's name: the SHA-256 of its payload.")
    first_open_ms = _given("_first_open_ms", "The open time of the segment's first bar.")
    last_open_ms = _given("_last_open_ms", "The open time of the segment's last bar.")
    bar_count = _given("_bar_count", "How many bars the segment holds.")

    def __repr__(self):
        return f"<ProvenanceSegment {self._hash} of {self._bar_count} bars>"


@_final
@_published
class Provenance(_Record):
    """What a job ran on, as its server recorded it before the job ran, and
    the identity of those inputs.

    `Job.provenance()` gives one, and none is built. It is a value, and
    never changes: two are equal when every field is.

    What the job was given to run on, which the identity covers:

    - `code_hash`: the code hash of the strategy bundle, which fixes its
      code, its manifest and so its parameters.
    - `market`: the market the job ran on, a `MarketId`.
    - `start_open_ms` and `end_open_ms`: the open times of the first and
      the last bar of the job's range.
    - `bar_count`: how many bars the job ran on.
    - `segments`: the segments that hold every one of those bars, oldest
      first, a tuple of `ProvenanceSegment`. The first and the last may
      hold bars outside the range, which the job did not run on.
    - `config`: how the job's fills were simulated, a `BacktestConfig`.
    - `engine`: the version of the engine that ran the bundle, such as
      `"0.3.0"`.
    - `python`: the version of the Python that ran it, such as `"3.12"`.
    - `deps`: each package the bundle declares, with the version that ran,
      a dict of text to text. The dict is new each time it is read.
    - `proto_version`: the protocol version of the server that ran the job.

    And beside them:

    - `job_id`: the job's ID.
    - `identity`: the identity of the inputs above: `sha256:` and the
      SHA-256 of their JSON, in hex. Jobs given the same bundle, bars,
      settings and runtime have the same identity on every server and
      every store.
    - `entry` and `params`: the bundle's entry, such as
      `"sma_cross.py:SmaCross"`, and the parameters its class was built
      with, as the bundle's manifest gives them: what the code hash fixes
      already. `params` is a dict of the values JSON holds, new each time
      it is read.
    - `recorded_at_ms`, `backend` and `schema_version`: when the server
      recorded the provenance, and the backend and the schema version of
      the store it recorded it in, `"sqlite"` or `"postgres"`. They change
      with the store, and never with what the job ran on.
    - `results`: the files the job produced, once it has succeeded, a
      tuple of `ResultFile`; `None` until then.

    Every time is milliseconds since the Unix epoch, UTC.
    """

    __slots__ = (
        "_job_id",
        "_identity",
        "_code_hash",
        "_market",
        "_start_open_ms",
        "_end_open_ms",
        "_bar_count",
        "_segments",
        "_config",
        "_engine",
        "_python",
        "_deps",
        "_proto_version",
        "_entry",
        "_params",
        "_recorded_at_ms",
        "_backend",
        "_schema_version",
        "_results",
    )

    _BUILT = "a Provenance cannot be built: Job.provenance() gives what a server recorded of a job"

    job_id = _given("_job_id", "The job's ID.")
    identity = _given("_identity", "The identity of the job's inputs: the SHA-256 of their JSON.")
    code_hash = _given("_code_hash", "The code hash of the strategy bundle the job ran.")
    market = _given("_market", "The market the job ran on, a MarketId.")
    start_open_ms = _given("_start_open_ms", "The open time of the first bar of the job's range.")
    end_open_ms = _given("_end_open_ms", "The open time of the last bar of the job's range.")
    bar_count = _given("_bar_count", "How many bars the job ran on.")
    segments = _given(
        "_segments", "The segments that hold the job's bars, a tuple of ProvenanceSegment."
    )
    config = _given("_config", "How the job's fills were simulated, a BacktestConfig.")
    engine = _given("_engine", "The version of the engine that ran the bundle.")
    python = _given("_python", "The version of the Python that ran the bundle.")

    @property
    def deps(self):
        """Each package the bundle declares, with the version that ran: a
        dict of text to text, new each time it is read."""
        return dict(self._deps)

    proto_version = _given("_proto_version", "The protocol version of the server that ran the job.")
    entry = _given("_entry", "The bundle's entry: its file and its class.")

    @property
    def params(self):
        """The parameters the bundle's class was built with: a dict of the
        values JSON holds, new each time it is read."""
        import copy

        return copy.deepcopy(self._params)

    recorded_at_ms = _given("_recorded_at_ms", "When the server recorded the provenance.")
    backend = _given("_backend", "The backend of the store it was recorded in.")
    schema_version = _given("_schema_version", "The schema version of that store then.")
    results = _given(
        "_results", "The files the job produced, a tuple of ResultFile, or None until it has."
    )

    def __repr__(self):
        return f"<Provenance of job {self._job_id}: {self._identity}>"


def _identity_of(inputs):
    """The identity of a job's inputs, as the protocol takes it: the
    SHA-256 of their compact JSON, with no whitespace, its members in the
    order they were received, and text escaped only where JSON requires."""
    import hashlib
    import json

    try:
        written = json.dumps(inputs, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    except UnicodeEncodeError:
        raise _Unreadable("its inputs hold text that is no Unicode") from None
    return f"sha256:{hashlib.sha256(written).hexdigest()}"


def _hash(record, name, what):
    """A content hash as the protocol writes one."""
    named = _member(record, name, str, what)
    if not _is_content_hash(named):
        raise _Unreadable(f"the `{name}` of {what} is no content hash")
    return named


def _segment(value, what):
    """One segment of a job's bars as the protocol writes it."""
    record = _object(value, what)
    return ProvenanceSegment._of(
        hash=_hash(record, "hash", what),
        first_open_ms=_member(record, "first_open_ms", int, what),
        last_open_ms=_member(record, "last_open_ms", int, what),
        bar_count=_count(record, "bar_count", what, _MOST_IN_32_BITS),
    )


def _provenance(job_id):
    """Reads the answer of a job's provenance route: the provenance of the
    job `job_id` and of no other, in the format this version reads, its
    identity held to its inputs."""

    def read(value):
        what = "a job's provenance"
        record = _object(value, what)
        named = _member(record, "job_id", str, what)
        if not _is_job_id(named):
            raise _Unreadable(f"the `job_id` of {what} is no job ID")
        if named != job_id:
            raise _Unreadable(f"it is the provenance of job {named}, not of job {job_id}")
        what = f"job {job_id}'s provenance"
        number = _member(record, "format", int, what)
        if number != _FORMAT:
            raise _Unreadable(
                f"it is a provenance of format {number}, and this version of the package reads "
                f"format {_FORMAT}"
            )
        identity = _hash(record, "identity", what)
        inputs = _member(record, "inputs", dict, what)
        taken = _identity_of(inputs)
        if taken != identity:
            raise _Unreadable(
                f"the identity of {what} is not the hash of its inputs: they hash to {taken}, "
                f"and it says {identity}"
            )

        what = f"job {job_id}'s inputs"
        code_hash = _hash(inputs, "code_hash", what)
        data = _member(inputs, "data", dict, what)
        names = [_member(data, name, str, what) for name in _MARKET]
        start = _member(data, "start_open_ms", int, what)
        end = _member(data, "end_open_ms", int, what)
        bars = _count(data, "bar_count", what)
        segments = tuple(
            _segment(each, f"a segment of {what}") for each in _member(data, "segments", list, what)
        )
        # The engine's own types, built by the engine's own rules.
        market = _market(*names, what)
        config = _config(_member(inputs, "backtest", dict, what), what)
        runtime = _member(inputs, "runtime", dict, what)
        what = f"job {job_id}'s runtime"
        engine = _member(runtime, "engine", str, what)
        python = _member(runtime, "python", str, what)
        deps = _member(runtime, "deps", dict, what)
        for name in deps:
            _member(deps, name, str, f"the packages of {what}")
        proto_version = _count(inputs, "proto_version", f"job {job_id}'s inputs", _MOST_IN_32_BITS)

        what = f"job {job_id}'s provenance"
        strategy = _member(record, "strategy", dict, what)
        recorded = _member(record, "recorded", dict, what)
        listed = _optional(record, "results", list, what)
        said = f"job {job_id}'s strategy"
        entry = _member(strategy, "entry", str, said)
        params = _member(strategy, "params", dict, said)
        said = f"where job {job_id}'s provenance was recorded"
        at = _member(recorded, "at_ms", int, said)
        backend = _member(recorded, "backend", str, said)
        schema = _count(recorded, "schema_version", said, _MOST_IN_32_BITS)
        results = None
        if listed is not None:
            results = tuple(_file(each, f"a result file of {what}") for each in listed)
        return Provenance._of(
            job_id=job_id,
            identity=identity,
            code_hash=code_hash,
            market=market,
            start_open_ms=start,
            end_open_ms=end,
            bar_count=bars,
            segments=segments,
            config=config,
            engine=engine,
            python=python,
            deps=dict(deps),
            proto_version=proto_version,
            entry=entry,
            params=params,
            recorded_at_ms=at,
            backend=backend,
            schema_version=schema,
            results=results,
        )

    return read
