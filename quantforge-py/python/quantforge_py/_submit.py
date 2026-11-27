"""A backtest job submitted to a server (`docs/python-bundles.md`): what a
client checks before it asks anything, and what it asks, in order.

1. **Before any request:** the market, the window and the settings are
   read, and the strategy's files are read, built into a bundle and checked
   against the bundle spec (`quantforge_py._bundles`).
2. **`GET /v1/runtime`:** what the server's workers run bundles on. A
   manifest that is written is written for it, and the bundle is admitted
   against it here, by the check the server makes of an upload.
3. **`GET /v1/markets`:** the bars the server stores of the market, which
   the window is read within, as the command line reads `--start` and
   `--end`.
4. **`PUT /v1/bundles/{code_hash}`:** the bundle's canonical tar, under the
   code hash the client computed. The server takes the SHA-256 of what it
   received, and its receipt has to name the same bundle.
5. **`POST /v1/backtests`:** the job, once. A submission is never sent
   again: one the server took but could not answer for would run twice.

Nothing is uploaded for a bundle the spec or the server's runtime refuses,
and nothing is submitted for a market or a window the server stores no bar
of.

This module is private.
"""

from . import _native
from ._errors import ServerUnavailable, ValidationError
from ._jobs import _HASH_PREFIX, _HEX, _status
from ._native import BacktestConfig, MarketId
from ._wire import _member, _object, _optional, _Unreadable, _wrong_type

# The media types of what a client sends: a bundle's canonical tar, and a
# submission.
_TAR = "application/x-tar"
_JSON = "application/json"

# The members of a market, as the protocol writes one.
_MARKET = ("exchange", "category", "symbol", "interval")


def _asked(market, start, end, config):
    """What a job runs on and how, as a caller names it, checked before
    anything is read or asked: the market, the two bounds of the window as
    the engine's milliseconds, and the settings."""
    if type(market) is not MarketId:
        raise _wrong_type("market", "a MarketId", market)
    start, end = _native._bounds(start, end)
    if start is not None and end is not None and start > end:
        raise ValueError(
            f"the window ends before it starts: start is {_native._rfc3339(start)} and end "
            f"{_native._rfc3339(end)}, each the whole millisecond it bounds the window at"
        )
    if type(config) is not BacktestConfig:
        raise _wrong_type("config", "a BacktestConfig", config)
    return market, start, end, config


def _named(market):
    """A market's four names as the protocol writes them: its exchange and
    its family, which together are its product line, its symbol and its
    interval."""
    line, category = str(market.exchange), str(market.exchange.category)
    return line.removesuffix(f"_{category}"), category, str(market.symbol), str(market.interval)


def _runtime(value):
    """Reads the answer of `GET /v1/runtime`, as the protocol's own type
    reads it: what the server's workers run bundles on, or `None`, and the
    answer's text, which a bundle is admitted against."""
    import json

    # An object, and a runtime that is one or is left out: the protocol's
    # own reader would take a list for either, member by member.
    what = "the server's runtime"
    _optional(_object(value, what), "runtime", dict, what)
    text = json.dumps(value, ensure_ascii=False)
    try:
        return _native._server_runtime(text), text
    except ValueError as refused:
        raise _Unreadable(f"it is no runtime as the protocol writes one ({refused})") from None


def _stored(names):
    """Reads the answer of `GET /v1/markets` for the open times of the
    first and the last bar the server stores of one market, or `None` where
    it stores none."""

    def read(value):
        what = "the list of markets"
        found = None
        for each in _member(_object(value, what), "markets", list, what):
            what = "a market"
            entry = _object(each, what)
            texts = tuple(_member(entry, name, str, what) for name in _MARKET)
            first = _member(entry, "first_open_ms", int, what)
            last = _member(entry, "last_open_ms", int, what)
            if first > last:
                raise _Unreadable(f"the first bar of {' '.join(texts)} opens after its last")
            if texts == names:
                found = first, last
        return found

    return read


def _receipt(code_hash, size):
    """Reads the answer to an upload: the server's receipt, which has to be
    of the bundle that was sent."""

    def read(value):
        what = "the receipt"
        receipt = _object(value, what)
        named = _member(receipt, "code_hash", str, what)
        held = _member(receipt, "size_bytes", int, what)
        digits = named.removeprefix(_HASH_PREFIX)
        if len(named) != len(_HASH_PREFIX) + 64 or len(digits) != 64 or not _HEX.issuperset(digits):
            raise _Unreadable(f"the `code_hash` of {what} is no content hash")
        if named != code_hash:
            raise _Unreadable(
                f"it is a receipt for bundle {named}, and the bundle uploaded is {code_hash}"
            )
        if held != size:
            raise _Unreadable(
                f"it says that bundle {code_hash} holds {held} bytes, and {size} were uploaded"
            )
        return receipt

    return read


def _queued(code_hash, market, start, end, config):
    """Reads the answer to a submission: the status of a job that runs what
    was submitted, and nothing else."""

    def read(value):
        status = _status(value)
        submitted = (code_hash, market, start, end, config)
        answered = (
            status.bundle,
            status.market,
            status.start_open_ms,
            status.end_open_ms,
            status.config,
        )
        if answered != submitted:
            raise _Unreadable(
                f"it is the status of job {status.job_id}, which does not run what was submitted"
            )
        return status

    return read


def _window(wire, market, start, end):
    """The open times of the first and the last bar a job runs on: the
    bars the server stores of `market` from `start` to `end`, each bound
    left out reaching the first or the last bar it stores."""
    names = _named(market)
    server, shown = wire.server.shown, " ".join(names)
    stored = wire.ask("GET", "markets", _stored(names))
    if stored is None:
        raise ValidationError(f"the server at {server} stores no bars of {shown}")
    first, last = stored
    opens = first if start is None else max(start, first)
    closes = last if end is None else min(end, last)
    if opens > closes:
        written = _native._rfc3339
        raise ValidationError(
            f"the server at {server} stores {shown} from {written(first)} to {written(last)}, "
            f"outside the window from {written(first if start is None else start)} to "
            f"{written(last if end is None else end)}"
        )
    return opens, closes


def _submitted(wire, draft, market, start, end, config):
    """Submits a backtest of the strategy `draft` holds, and gives the
    status the server answers of the job it queued."""
    import json

    server = wire.server.shown
    offered, answer = wire.ask("GET", "runtime", _runtime)
    if offered is None:
        raise ServerUnavailable(
            f"the server at {server} runs no Python bundles: it was started without --job-python"
        )
    built = draft.built(offered, answer, server)
    opens, closes = _window(wire, market, start, end)
    code_hash, tar = built["code_hash"], built["tar"]
    wire.ask(
        "PUT",
        f"bundles/{code_hash}",
        _receipt(code_hash, len(tar)),
        body=tar,
        media_type=_TAR,
    )
    exchange, category, symbol, interval = _named(market)
    submission = {
        "bundle": code_hash,
        "data": {
            "exchange": exchange,
            "category": category,
            "symbol": symbol,
            "interval": interval,
            "start_open_ms": opens,
            "end_open_ms": closes,
        },
        "backtest": {
            "initial_cash": format(config.initial_cash, "f"),
            "fee_bps": format(config.fee_bps, "f"),
            "close_out_at_end": config.close_out_at_end,
        },
    }
    return wire.ask(
        "POST",
        "backtests",
        _queued(code_hash, market, opens, closes, config),
        body=json.dumps(submission, separators=(",", ":")).encode("utf-8"),
        media_type=_JSON,
        again=False,
    )
