"""A job's provenance read from Python, against servers of the test's own
(docs/python-jobs.md): what `Job.provenance()` gives of what a server
recorded, each field in the package's own types, the identity of a job's
inputs taken again here and held to the one the server gave, and every
answer that is refused.

A stand-in here answers a job's provenance route with the protocol's own
example, read from the crate's documentation of it, or with whatever a test
would have a server send in its place. What the client reads of a real
`serve run`'s job is in `test_handle_jobs.py`.
"""

import copy
import hashlib
import json
import pickle
import re
from decimal import Decimal

import pytest

import quantforge_py as qf
from conftest import REPOSITORY
from quantforge_py import _provenance
from standin import JOB, StandIn, refusal, written

PROVENANCE = f"/v1/backtests/{JOB}/provenance"
SOURCE = (REPOSITORY / "src" / "protocol" / "jobs" / "provenance.rs").read_text(encoding="utf-8")
EMPTY = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"


def example() -> dict:
    """The protocol's own example of a provenance, as the crate documents
    it, of the stand-in's job."""
    (text,) = re.findall(r'/// let json = r#"(\{"job_id".*\})"#;', SOURCE)
    return {**json.loads(text), "job_id": JOB}


def identity(inputs: dict) -> str:
    """The identity of a job's inputs, as the protocol's documentation
    says Python takes it."""
    written = json.dumps(inputs, separators=(",", ":"), ensure_ascii=False).encode()
    return "sha256:" + hashlib.sha256(written).hexdigest()


def recorded(**changed) -> dict:
    """The example with members of its inputs changed, under the identity
    of what they then are."""
    answer = example()
    for path, value in changed.items():
        at = answer["inputs"]
        *above, last = path.split("__")
        for name in above:
            at = at[name]
        at[last] = value
    answer["identity"] = identity(answer["inputs"])
    return answer


@pytest.fixture
def server():
    with StandIn() as stand_in:
        stand_in.says("GET", f"/v1/backtests/{JOB}", (200, written()))
        stand_in.says("GET", PROVENANCE, (200, example()))
        yield stand_in


@pytest.fixture
def job(server):
    return qf.Client(server.url).backtest.get(JOB)


def raised(kind, call):
    with pytest.raises(kind) as caught:
        call()
    assert type(caught.value) is kind
    return caught.value


def test_a_provenance_gives_what_a_job_ran_on_in_the_packages_own_types(server, job):
    provenance = job.provenance()
    assert type(provenance) is qf.Provenance
    assert server.routes[2:] == [f"GET {PROVENANCE}"]
    answered = example()
    inputs = answered["inputs"]
    # What the result depends on.
    assert provenance.code_hash == inputs["code_hash"] == EMPTY
    assert provenance.market == qf.MarketId("binance_spot", "BTCUSDT", "1m")
    assert (provenance.start_open_ms, provenance.end_open_ms) == (1792454400000, 1792540740000)
    assert provenance.bar_count == 1440
    (segment,) = provenance.segments
    assert type(provenance.segments) is tuple and type(segment) is qf.ProvenanceSegment
    assert segment.hash == (
        "sha256:40f256656d99aa4df38ff7225d2846a06a5d164becf8f72d32015661914b211f"
    )
    assert (segment.first_open_ms, segment.last_open_ms, segment.bar_count) == (
        1792454400000,
        1792540740000,
        1440,
    )
    assert provenance.config == qf.BacktestConfig(
        initial_cash=Decimal("10000"), fee_bps=Decimal("10"), close_out_at_end=True
    )
    assert (provenance.engine, provenance.python) == ("0.3.0", "3.12")
    assert provenance.deps == {"numpy": "2.1.3"}
    assert provenance.proto_version == 1
    # And beside it.
    assert provenance.job_id == JOB
    assert provenance.identity == answered["identity"] == identity(inputs)
    assert provenance.entry == "sma_cross.py:SmaCross"
    assert provenance.params == {"fast": 20, "slow": 50}
    assert (provenance.recorded_at_ms, provenance.backend, provenance.schema_version) == (
        1792540801500,
        "postgres",
        11,
    )
    assert provenance.results is None
    # Each of a type that says what it is.
    kinds = {
        "job_id": str,
        "identity": str,
        "code_hash": str,
        "market": qf.MarketId,
        "start_open_ms": int,
        "end_open_ms": int,
        "bar_count": int,
        "segments": tuple,
        "config": qf.BacktestConfig,
        "engine": str,
        "python": str,
        "deps": dict,
        "proto_version": int,
        "entry": str,
        "params": dict,
        "recorded_at_ms": int,
        "backend": str,
        "schema_version": int,
        "results": type(None),
    }
    assert [name.removeprefix("_") for name in qf.Provenance.__slots__] == list(kinds)
    for name, kind in kinds.items():
        assert type(getattr(provenance, name)) is kind, name
    assert repr(provenance) == f"<Provenance of job {JOB}: {answered['identity']}>"
    assert repr(segment) == f"<ProvenanceSegment {segment.hash} of 1440 bars>"


def test_the_identity_is_the_hash_the_protocol_takes_of_the_inputs_as_they_were_received(
    server, job
):
    # The inputs the crate's own test takes an identity of, with a symbol
    # that is not ASCII and a control character JSON has to escape, and the
    # hash that test holds the engine to, which is Python's.
    (expected,) = re.findall(
        r'fn the_identity_is_the_hash_python_takes_of_the_same_members\(\) \{.*?"(sha256:\w+)"',
        SOURCE,
        re.S,
    )
    inputs = {
        "code_hash": EMPTY,
        "data": {
            "exchange": "binance",
            "category": "spot",
            "symbol": "币安\x01USDT",
            "interval": "4h",
            "start_open_ms": 0,
            "end_open_ms": 14_400_000,
            "bar_count": 2,
            "segments": [
                {"hash": EMPTY, "first_open_ms": 0, "last_open_ms": 14_400_000, "bar_count": 2}
            ],
        },
        "backtest": {"initial_cash": "100.50", "fee_bps": "0", "close_out_at_end": False},
        "runtime": {
            "engine": "0.2.0",
            "python": "3.11",
            "deps": {"numpy": "2.4.6", "pandas": "3.1.1"},
        },
        "proto_version": 1,
    }
    assert _provenance._identity_of(inputs) == expected == identity(inputs)
    server.says("GET", PROVENANCE, (200, {**example(), "inputs": inputs, "identity": expected}))
    provenance = job.provenance()
    assert provenance.identity == expected
    assert str(provenance.market.symbol) == "币安\x01USDT"
    assert provenance.config.initial_cash.as_tuple().exponent == -2
    assert provenance.deps == {"numpy": "2.4.6", "pandas": "3.1.1"}
    # However the server wrote them: escaped where JSON need not, and with
    # space between them, they are the same members.
    spaced = json.dumps({**example(), "inputs": inputs, "identity": expected}, indent=2)
    assert "\\u5e01" in spaced
    server.says("GET", PROVENANCE, (200, spaced.encode(), {"Content-Type": "application/json"}))
    assert job.provenance() == provenance
    # A member a later revision adds to the inputs is one of them: the
    # identity is taken over what was received.
    wider = recorded(seed=7)
    server.says("GET", PROVENANCE, (200, wider))
    assert job.provenance().identity == wider["identity"] != example()["identity"]


def test_a_provenance_whose_inputs_are_not_the_ones_its_identity_names_is_refused(server, job):
    for path, value in [
        ("code_hash", "sha256:" + "0" * 64),
        ("data__end_open_ms", 1792540740001),
        ("data__bar_count", 1439),
        ("backtest__initial_cash", "10000.0"),
        ("backtest__close_out_at_end", False),
        ("runtime__engine", "0.3.1"),
        ("runtime__deps", {}),
        ("proto_version", 2),
        ("seed", 7),
    ]:
        altered = {**recorded(**{path: value}), "identity": example()["identity"]}
        server.says("GET", PROVENANCE, (200, altered))
        error = raised(qf.RemoteError, job.provenance)
        assert str(error) == (
            f"the server at {server.url}/ answered GET {PROVENANCE} with a body the protocol "
            f"does not read: the identity of job {JOB}'s provenance is not the hash of its "
            f"inputs: they hash to {identity(altered['inputs'])}, and it says "
            f"{example()['identity']}; is it a QuantForge server?"
        ), path
    # The order of the members is part of what is hashed.
    reordered = example()
    reordered["inputs"] = dict(reversed(reordered["inputs"].items()))
    server.says("GET", PROVENANCE, (200, reordered))
    assert "is not the hash of its inputs" in str(raised(qf.RemoteError, job.provenance))
    # What is beside the inputs is no part of it: where and when it was
    # recorded, and what the code hash fixes already.
    beside = example()
    beside["recorded"] = {"at_ms": 5, "backend": "sqlite", "schema_version": 12}
    beside["strategy"] = {"entry": "s.py:S", "params": {}}
    server.says("GET", PROVENANCE, (200, beside))
    moved = job.provenance()
    assert moved.identity == example()["identity"]
    assert (moved.backend, moved.schema_version, moved.entry, moved.params) == (
        "sqlite",
        12,
        "s.py:S",
        {},
    )


def test_what_a_succeeded_job_produced_is_listed_with_its_provenance(server, job, refused):
    produced = [
        {"name": "metrics", "media_type": "application/json", "hash": EMPTY, "size_bytes": 117},
        {
            "name": "trades",
            "media_type": "application/vnd.apache.arrow.file",
            "hash": "sha256:" + "ab" * 32,
            "size_bytes": 1882,
        },
    ]
    server.says("GET", PROVENANCE, (200, {**example(), "results": produced}))
    results = job.provenance().results
    assert type(results) is tuple and [type(each) for each in results] == [qf.ResultFile] * 2
    assert [(each.name, each.media_type, each.hash, each.size_bytes) for each in results] == [
        tuple(each.values()) for each in produced
    ]
    # Until it has, and for a `null` in their place, there are none.
    for none in ({**example(), "results": None}, example()):
        server.says("GET", PROVENANCE, (200, none))
        assert job.provenance().results is None
    server.says("GET", PROVENANCE, (200, {**example(), "results": []}))
    assert job.provenance().results == ()


def test_two_records_of_different_kinds_are_never_equal_whatever_they_hold(server, job):
    # A segment and a result's file each hold a hash and three more fields.
    # Two that held the same four values would still be two different
    # things: a record equals one of its own kind alone.
    held = dict(zip(("first", "second", "third", "fourth"), ("sha256:" + "0" * 64, 1, 2, 3)))
    segment = qf.ProvenanceSegment._of(
        hash=held["first"],
        first_open_ms=held["second"],
        last_open_ms=held["third"],
        bar_count=held["fourth"],
    )
    file = qf.ResultFile._of(
        name=held["first"],
        media_type=held["second"],
        hash=held["third"],
        size_bytes=held["fourth"],
    )
    assert segment._fields() == file._fields() == tuple(held.values())
    assert segment != file and file != segment and not segment == file
    # Nor one of anything else that says it holds the same.
    import types

    for other in (
        tuple(held.values()),
        list(held.values()),
        types.SimpleNamespace(_fields=lambda: tuple(held.values())),
        None,
    ):
        assert segment != other and other != segment and file != other
    # And one equals another of its kind that holds the same.
    again = qf.ProvenanceSegment._of(
        hash=held["first"], first_open_ms=1, last_open_ms=2, bar_count=3
    )
    assert segment == again and segment is not again
    assert job.provenance().segments[0] != segment


def test_a_provenance_is_a_value_that_is_never_built_changed_or_derived_from(server, job, refused):
    provenance, again = job.provenance(), job.provenance()
    assert provenance == again and provenance is not again and not provenance != again
    # What it gives of a dict is the caller's own: changing it changes
    # nothing in the provenance.
    deps, params = provenance.deps, provenance.params
    deps["numpy"] = "9"
    params["fast"] = [1]
    params.setdefault("nested", {})["deep"] = 1
    assert provenance.deps == {"numpy": "2.1.3"} and provenance.params == {"fast": 20, "slow": 50}
    assert provenance == again
    server.says(
        "GET",
        PROVENANCE,
        (
            200,
            {**example(), "strategy": {"entry": "s.py:S", "params": {"grid": {"steps": [1, 2]}}}},
        ),
    )
    nested = job.provenance()
    nested.params["grid"]["steps"].append(3)
    assert nested.params == {"grid": {"steps": [1, 2]}} and nested != provenance
    for kind, said in [
        (
            qf.Provenance,
            "a Provenance cannot be built: Job.provenance() gives what a server recorded of a job",
        ),
        (
            qf.ProvenanceSegment,
            "a ProvenanceSegment cannot be built: a Provenance gives one for each segment its "
            "job's bars came from",
        ),
    ]:
        refused(TypeError, said, kind)
        with pytest.raises(TypeError, match="not an acceptable base type"):
            type("Derived", (kind,), {})
    segment = provenance.segments[0]
    for value, name in [(provenance, "identity"), (provenance, "deps"), (segment, "hash")]:
        kind = type(value).__name__
        refused(
            AttributeError,
            f"attribute '{name}' of 'quantforge_py.{kind}' objects is not writable",
            setattr,
            value,
            name,
            "another",
        )
        refused(
            AttributeError,
            f"'quantforge_py.{kind}' object has no attribute 'ledger'",
            setattr,
            value,
            "ledger",
            1,
        )
        with pytest.raises(TypeError, match="unhashable type"):
            hash(value)
        for copied in (copy.copy, copy.deepcopy, pickle.dumps):
            refused(TypeError, f"cannot pickle 'quantforge_py.{kind}' object", copied, value)
    assert segment == again.segments[0] and segment is not again.segments[0]


def with_member(path: str, value) -> dict:
    """The example with one member outside its inputs changed."""
    answer = example()
    *above, last = path.split("__")
    at = answer
    for name in above:
        at = at[name]
    if value is ...:
        del at[last]
    else:
        at[last] = value
    return answer


def without_input(path: str) -> dict:
    answer = example()
    *above, last = path.split("__")
    at = answer["inputs"]
    for name in above:
        at = at[name]
    del at[last]
    answer["identity"] = identity(answer["inputs"])
    return answer


INPUTS = f"job {JOB}'s inputs"
RUNTIME = f"job {JOB}'s runtime"
WHOLE = f"job {JOB}'s provenance"
WHERE = f"where job {JOB}'s provenance was recorded"
UNREADABLE = [
    ([], "a job's provenance is not an object"),
    (with_member("job_id", "0" * 32), f"it is the provenance of job {'0' * 32}, not of job {JOB}"),
    (with_member("job_id", "no such job"), "the `job_id` of a job's provenance is no job ID"),
    (with_member("job_id", ...), "a job's provenance has no `job_id`"),
    (
        with_member("format", 2),
        "it is a provenance of format 2, and this version of the package reads format 1",
    ),
    (with_member("format", "1"), f"the `format` of {WHOLE} is not an integer"),
    (with_member("format", ...), f"{WHOLE} has no `format`"),
    (with_member("identity", "sha256:nothing"), f"the `identity` of {WHOLE} is no content hash"),
    (with_member("identity", ...), f"{WHOLE} has no `identity`"),
    (with_member("inputs", []), f"the `inputs` of {WHOLE} is not an object"),
    (with_member("inputs", ...), f"{WHOLE} has no `inputs`"),
    (with_member("strategy", ...), f"{WHOLE} has no `strategy`"),
    (with_member("strategy", "s.py:S"), f"the `strategy` of {WHOLE} is not an object"),
    (with_member("strategy__entry", ...), f"job {JOB}'s strategy has no `entry`"),
    (with_member("strategy__entry", 7), f"the `entry` of job {JOB}'s strategy is not a string"),
    (with_member("strategy__params", []), f"the `params` of job {JOB}'s strategy is not an object"),
    (with_member("recorded", ...), f"{WHOLE} has no `recorded`"),
    (with_member("recorded__backend", ...), f"{WHERE} has no `backend`"),
    (with_member("recorded__at_ms", "now"), f"the `at_ms` of {WHERE} is not an integer"),
    (with_member("recorded__backend", None), f"the `backend` of {WHERE} is not a string"),
    (with_member("recorded__schema_version", -1), f"the `schema_version` of {WHERE} is negative"),
    (with_member("results", {}), f"the `results` of {WHOLE} is not a list"),
    (with_member("results", [7]), f"a result file of {WHOLE} is not an object"),
    (
        with_member(
            "results", [{"name": "metrics", "media_type": "m", "hash": "h", "size_bytes": 1}]
        ),
        f"the `hash` of a result file of {WHOLE} is no content hash",
    ),
    (recorded(code_hash="sha256:" + "A" * 64), f"the `code_hash` of {INPUTS} is no content hash"),
    (recorded(data=[]), f"the `data` of {INPUTS} is not an object"),
    (recorded(data__symbol=7), f"the `symbol` of {INPUTS} is not a string"),
    (recorded(data__start_open_ms="0"), f"the `start_open_ms` of {INPUTS} is not an integer"),
    (recorded(data__end_open_ms=1.5), f"the `end_open_ms` of {INPUTS} is not an integer"),
    (recorded(data__bar_count=-1), f"the `bar_count` of {INPUTS} is negative"),
    (recorded(data__segments={}), f"the `segments` of {INPUTS} is not a list"),
    (recorded(data__segments=[7]), f"a segment of {INPUTS} is not an object"),
    (
        recorded(
            data__segments=[{"hash": "x", "first_open_ms": 0, "last_open_ms": 0, "bar_count": 1}]
        ),
        f"the `hash` of a segment of {INPUTS} is no content hash",
    ),
    (
        recorded(data__segments=[{"hash": EMPTY, "first_open_ms": 0, "bar_count": 1}]),
        f"a segment of {INPUTS} has no `last_open_ms`",
    ),
    (
        recorded(
            data__segments=[{"hash": EMPTY, "first_open_ms": 0, "last_open_ms": 0, "bar_count": -1}]
        ),
        f"the `bar_count` of a segment of {INPUTS} is negative",
    ),
    (
        recorded(data__interval="7m"),
        f"{INPUTS} names the market binance spot BTCUSDT 7m, which this version of the package "
        "does not know (invalid interval: 7m)",
    ),
    (
        recorded(data__symbol="btcusdt"),
        f"{INPUTS} names the market binance spot btcusdt 1m, which is not in its canonical form",
    ),
    (recorded(backtest=7), f"the `backtest` of {INPUTS} is not an object"),
    (
        recorded(backtest__fee_bps=10),
        f"the `fee_bps` of {INPUTS} is not a string",
    ),
    (
        recorded(backtest__initial_cash="1e4"),
        f"the `initial_cash` of {INPUTS} is no decimal in its exact form",
    ),
    (
        recorded(backtest__initial_cash="0"),
        f"{INPUTS} has settings the engine refuses (invalid configuration: initial_cash must be "
        "greater than 0, got 0)",
    ),
    (recorded(runtime=[]), f"the `runtime` of {INPUTS} is not an object"),
    (recorded(runtime__engine=3), f"the `engine` of {RUNTIME} is not a string"),
    (recorded(runtime__python=3.12), f"the `python` of {RUNTIME} is not a string"),
    (recorded(runtime__deps=[]), f"the `deps` of {RUNTIME} is not an object"),
    (
        recorded(runtime__deps={"numpy": 2}),
        f"the `numpy` of the packages of {RUNTIME} is not a string",
    ),
    (recorded(proto_version="1"), f"the `proto_version` of {INPUTS} is not an integer"),
    (recorded(proto_version=-1), f"the `proto_version` of {INPUTS} is negative"),
] + [
    (without_input(path), f"{what} has no `{path.rpartition('__')[2]}`")
    for path, what in [
        ("code_hash", INPUTS),
        ("data", INPUTS),
        ("data__exchange", INPUTS),
        ("data__category", INPUTS),
        ("data__interval", INPUTS),
        ("data__start_open_ms", INPUTS),
        ("data__end_open_ms", INPUTS),
        ("data__bar_count", INPUTS),
        ("data__segments", INPUTS),
        ("backtest", INPUTS),
        ("backtest__close_out_at_end", INPUTS),
        ("runtime", INPUTS),
        ("runtime__engine", RUNTIME),
        ("runtime__python", RUNTIME),
        ("runtime__deps", RUNTIME),
        ("proto_version", INPUTS),
    ]
]


@pytest.mark.parametrize(("answered", "why"), UNREADABLE)
def test_a_provenance_the_protocol_does_not_write_is_refused_and_never_guessed_at(
    server, job, answered, why
):
    server.says("GET", PROVENANCE, (200, answered))
    error = raised(qf.RemoteError, job.provenance)
    assert str(error) == (
        f"the server at {server.url}/ answered GET {PROVENANCE} with a body the protocol does "
        f"not read: {why}; is it a QuantForge server?"
    )
    assert (error.code, error.status) == (None, None)


def test_inputs_that_hold_text_that_is_no_unicode_have_no_identity(server, job):
    # A surrogate alone, which JSON's escapes can write and UTF-8 cannot.
    lone = json.dumps(example()).replace('"BTCUSDT"', '"BTC\\ud800USDT"')
    server.says("GET", PROVENANCE, (200, lone.encode(), {"Content-Type": "application/json"}))
    error = raised(qf.RemoteError, job.provenance)
    assert "its inputs hold text that is no Unicode" in str(error)


def test_a_job_that_recorded_no_provenance_has_none_in_the_servers_words(server, job):
    for stands in (
        "is still queued; its provenance is recorded as it starts",
        "is still reading its inputs; its provenance is recorded before it runs",
        "has no provenance: it ended before the server read its inputs, or a build that "
        "recorded none ran it",
    ):
        message = f"job {JOB} {stands}"
        server.says("GET", PROVENANCE, refusal(409, "conflict", message))
        error = raised(qf.Conflict, job.provenance)
        assert (str(error), error.code, error.status) == (message, "conflict", 409)
    damaged = f"the server's copy of job {JOB}'s provenance is damaged"
    server.says("GET", PROVENANCE, refusal(500, "internal", damaged))
    error = raised(qf.RemoteError, job.provenance)
    assert (str(error), error.code, error.status) == (damaged, "internal", 500)
    # Each call asks the server, and none changed the job's status.
    assert server.routes[2:] == [f"GET {PROVENANCE}"] * 4
    assert job.status.state == "queued"
