#!/usr/bin/env python3
"""The transfer cases of docs/storage-migration.md, written independently
of the Rust code under test.

A store's tables as rows of plain values: candles of three product lines on
two venues, a run journal with every identity source, and segments with
their index and dirty marks. Then, for every table and every market of a
market table, what a verification reads from either store: the number of
rows, the first and last value of an integer key, and the checksum of the
canonical text docs/storage-migration.md defines. A worked example shows the
canonical text of one row.

The segments are real: payloads from the packer of docs/segment-spec.md,
checked against every case of tests/fixtures/segment_cases.json before
anything is written, each stored as one zstd frame of raw blocks, which any
zstd decoder reads.

Run with ``python3 -I spikes/storage-contract/transfer_cases.py`` from the
repository root; it writes tests/fixtures/storage_transfer_cases.json.
Stdlib only.
"""

import hashlib
import json
import pathlib
import re
import struct

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "fixtures" / "storage_transfer_cases.json"
SEGMENT_CASES = ROOT / "tests" / "fixtures" / "segment_cases.json"

FORMAT = 1
MINUTE = 60_000
DAY = 86_400_000
# 2026-10-20T00:00:00Z, a UTC midnight.
T = 1_792_454_400_000
I64_MIN = -(2**63)
WINDOW_BARS = 1440
STEP_MS = {"1m": MINUTE, "5m": 5 * MINUTE}
# 29 decimal places: more than a Decimal holds. A transfer copies it as it is.
INEXACT = "1.00000000000000000000000000001"

# The tables a transfer copies, in the order it copies them, each with its
# columns in the order of the DDL, the column its rows are read in order of,
# and whether that order runs within each market.
TABLES = [
    (
        "candles",
        ["exchange", "symbol", "interval", "open_time_ms", "close_time_ms", "open", "high",
         "low", "close", "volume", "trades"],
        "open_time_ms",
        True,
    ),
    (
        "bot_runs",
        ["run_id", "exchange", "symbol", "interval", "strategy_name", "status", "state_json",
         "started_at_ms", "updated_at_ms", "stopped_at_ms", "last_error", "environment",
         "account_kind", "account_id", "identity_source", "identity_at_ms",
         "legacy_state_json"],
        "run_id",
        False,
    ),
    (
        "order_events",
        ["seq", "run_id", "symbol", "side", "order_type", "status", "order_id",
         "client_order_id", "transact_time_ms", "raw_json", "legacy_raw_json"],
        "seq",
        False,
    ),
    (
        "closed_trades",
        ["seq", "run_id", "symbol", "entry_time_ms", "exit_time_ms", "entry_price",
         "exit_price", "qty", "gross_quote_pnl", "entry_order_id", "exit_order_id"],
        "seq",
        False,
    ),
    ("segments", ["hash", "format", "payload_len", "data"], "hash", False),
    (
        "segment_index",
        ["exchange", "symbol", "interval", "window_start_ms", "first_open_ms",
         "last_open_ms", "bar_count", "hash"],
        "window_start_ms",
        True,
    ),
    ("segment_dirty", ["id", "exchange", "symbol", "interval", "window_start_ms"], "id", False),
    ("bundles", ["code_hash", "size_bytes", "data", "stored_at_ms"], "code_hash", False),
    (
        "jobs",
        ["seq", "job_id", "state", "bundle_hash", "request_json", "submitted_at_ms",
         "started_at_ms", "finished_at_ms", "results_json", "failure_json"],
        "seq",
        False,
    ),
]

# --- The canonical text of docs/storage-migration.md --------------------------


def encode(value) -> bytes:
    """One value: n for NULL, else a tag, the byte length, a colon and the bytes."""
    if value is None:
        return b"n"
    if isinstance(value, bool):
        raise TypeError("no booleans in a store")
    if isinstance(value, int):
        tag, data = b"i", str(value).encode("ascii")
    elif isinstance(value, str):
        tag, data = b"t", value.encode("utf-8")
    elif isinstance(value, bytes):
        tag, data = b"b", value
    else:
        raise TypeError(f"no such value: {value!r}")
    return tag + str(len(data)).encode("ascii") + b":" + data


def canonical(table: str, columns: list, rows: list) -> bytes:
    out = b"quantforge transfer %d\n" % FORMAT
    out += table.encode("utf-8") + b"\n" + ",".join(columns).encode("utf-8") + b"\n"
    for row in rows:
        out += b"".join(encode(value) for value in row) + b"\n"
    return out


def checksum(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def order_key(value):
    """Integers by value, text by its UTF-8 bytes, as SQLite compares them."""
    return value.encode("utf-8") if isinstance(value, str) else value


# --- The packer of docs/segment-spec.md, "In Python" -------------------------

EXACT = re.compile(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?")
DECIMALS = ("open", "high", "low", "close", "volume")


def exact(text: str) -> bool:
    if len(text) > 31 or not EXACT.fullmatch(text):
        return False
    unsigned = text.removeprefix("-")
    if text.startswith("-") and set(unsigned) <= {"0", "."}:
        return False
    integer, _, fraction = unsigned.partition(".")
    return len(fraction) <= 28 and int(integer + fraction) < 2**96


def payload(candles: list) -> bytes:
    n = len(candles)
    if not 1 <= n <= WINDOW_BARS:
        raise ValueError(f"{n} candles do not fit a segment")
    out = bytearray(b"QFSEGMNT") + struct.pack("<II", 1, n)
    out += struct.pack(f"<{n}q", *(c["open_time_ms"] for c in candles))
    out += struct.pack(f"<{n}q", *(c["close_time_ms"] for c in candles))
    out += struct.pack(f"<{n}Q", *((c["trades"] or 0) % 2**64 for c in candles))
    out += bytes(c["trades"] is not None for c in candles)
    for column in DECIMALS:
        texts = [c[column] for c in candles]
        if not all(exact(text) for text in texts):
            raise ValueError(f"a {column} is not an exact decimal")
        out += bytes(len(text) for text in texts) + "".join(texts).encode("ascii")
    return bytes(out)


def check_packer() -> None:
    cases = json.loads(SEGMENT_CASES.read_text())["cases"]
    for case in cases:
        packed = payload(case["candles"])
        if packed.hex() != case["payload_hex"] or checksum(packed) != case["hash"]:
            raise SystemExit(f"the packer disagrees with segment case {case['name']!r}")
    print(f"packer: all {len(cases)} segment cases reproduced")


def zstd_frame(data: bytes) -> bytes:
    """One zstd frame (RFC 8878) holding data in raw blocks: single segment,
    a four-byte content size, no checksum, no dictionary."""
    out = bytearray(struct.pack("<I", 0xFD2FB528))
    out.append(0b1010_0000)
    out += struct.pack("<I", len(data))
    blocks = [data[i : i + 128 * 1024] for i in range(0, len(data), 128 * 1024)] or [b""]
    for index, block in enumerate(blocks):
        last = int(index == len(blocks) - 1)
        out += (len(block) << 3 | last).to_bytes(3, "little")
        out += block
    return bytes(out)


# --- The store ---------------------------------------------------------------

SPOT = ("binance_spot", "BTCUSDT", "1m")
LINEAR = ("bybit_linear", "BTCUSDT", "1m")
INVERSE = ("binance_inverse", "BTCUSD", "5m")
# A symbol Binance lists, not ASCII: its bytes sort after every ASCII symbol.
UNICODE = ("binance_spot", "币安人生USDT", "1m")
ETH = ("binance_spot", "ETHUSDT", "1m")


def window_start(interval: str, open_time_ms: int) -> int:
    span = WINDOW_BARS * STEP_MS[interval]
    return open_time_ms // span * span


def candle(market, open_time_ms, close, trades, **columns):
    exchange, symbol, interval = market
    row = {
        "exchange": exchange,
        "symbol": symbol,
        "interval": interval,
        "open_time_ms": open_time_ms,
        "close_time_ms": open_time_ms + STEP_MS[interval] - 1,
        "open": close,
        "high": close,
        "low": close,
        "close": close,
        "volume": "1.25000000",
        "trades": trades,
    }
    row.update(columns)
    return row


CANDLES = [
    candle(SPOT, T, "67012.10", 42, open="67000.00", high="67020.50", low="66990.00"),
    # A trade count of zero is known; None is not.
    candle(SPOT, T + MINUTE, "67015.00", 0),
    candle(SPOT, T + 2 * MINUTE, "67013.5", None, volume="0.5"),
    candle(SPOT, T + DAY, "67100.10", 7),
    candle(SPOT, T + DAY + MINUTE, "67101.00", 8),
    candle(LINEAR, T, "67010.0", 3),
    candle(LINEAR, T + MINUTE, "67011.25", 4),
    # 28 decimal places, the most a Decimal holds, and their scale kept; a
    # u64 trade count past i64 stored wrapped, as both backends store it.
    candle(INVERSE, T, "1.1000000000000000000000000000", -1,
           volume="0.0000000000000000000000000001"),
    candle(INVERSE, T + 5 * MINUTE, "1.10", I64_MIN),
    candle(UNICODE, T, "0.1234", 1),
    # Before 1970, and text no Decimal holds: copied as stored.
    candle(ETH, -MINUTE, "3933.07", 5),
    candle(ETH, T, INEXACT, 6, open="3940.00", high="3941.00", low="3939.00"),
]


def packed(market, open_times):
    rows = [c for c in CANDLES if (c["exchange"], c["symbol"], c["interval"]) == market
            and c["open_time_ms"] in open_times]
    data = payload(rows)
    return checksum(data), data


# The window of the spot market's first day, packed when it held two candles
# and again with three: the replaced segment stays stored.
CURRENT_HASH, CURRENT = packed(SPOT, {T, T + MINUTE, T + 2 * MINUTE})
REPLACED_HASH, REPLACED = packed(SPOT, {T, T + MINUTE})

SEGMENTS = [
    {"hash": CURRENT_HASH, "format": 1, "payload_len": len(CURRENT), "data": zstd_frame(CURRENT)},
    {"hash": REPLACED_HASH, "format": 1, "payload_len": len(REPLACED),
     "data": zstd_frame(REPLACED)},
]

SEGMENT_INDEX = [
    {
        "exchange": SPOT[0], "symbol": SPOT[1], "interval": SPOT[2],
        "window_start_ms": window_start("1m", T), "first_open_ms": T,
        "last_open_ms": T + 2 * MINUTE, "bar_count": 3, "hash": CURRENT_HASH,
    },
]


def dirty(mark_id, market, open_time_ms):
    exchange, symbol, interval = market
    return {"id": mark_id, "exchange": exchange, "symbol": symbol, "interval": interval,
            "window_start_ms": window_start(interval, open_time_ms)}


# Revisions with gaps, as a store that refreshed in between leaves them:
# copied as they are, never renumbered.
SEGMENT_DIRTY = [
    dirty(3, INVERSE, T),
    dirty(7, LINEAR, T),
    dirty(12, SPOT, T + DAY),
    dirty(13, ETH, -MINUTE),
]

# The serde tokens of a run's state_json (dual channels: wire forms in the
# columns, tokens in the JSON).
TOKENS = {
    "binance_spot": "BinanceSpot", "bybit_linear": "BybitLinear",
    "binance_inverse": "BinanceInverse", "1m": "M1", "5m": "M5",
    "dry_run": "DryRun", "live": "Live", "stopped": "Stopped", "failed": "Failed",
}
FLAT = {"qty": "0", "entry_price": None, "entry_time_ms": None, "entry_order_id": None}


def state(run_id, market, mode, status, started, updated, position=FLAT, error=None):
    exchange, symbol, interval = market
    out = {
        "run_id": run_id,
        "market": {"exchange": TOKENS[exchange], "symbol": symbol, "interval": TOKENS[interval]},
        "strategy_name": "sma_cross",
        "strategy_config": {"kind": "sma_cross", "fast": 20, "slow": 50},
        "execution_mode": TOKENS[mode] if mode else None,
        "status": TOKENS[status],
        "last_processed_open_time_ms": T + 2 * MINUTE,
        "started_at_ms": started,
        "updated_at_ms": updated,
        "stopped_at_ms": updated,
        "last_error": error,
        "position": position,
    }
    if mode is None:
        del out["execution_mode"]
    return json.dumps(out, separators=(",", ":"), ensure_ascii=False)


def run(run_id, market, status, state_json, started, updated, environment=None, account=None,
        source="recorded", identity_at=None, legacy=None, error=None):
    exchange, symbol, interval = market
    return {
        "run_id": run_id, "exchange": exchange, "symbol": symbol, "interval": interval,
        "strategy_name": "sma_cross", "status": status, "state_json": state_json,
        "started_at_ms": started, "updated_at_ms": updated, "stopped_at_ms": updated,
        "last_error": error, "environment": environment,
        "account_kind": "key_fingerprint" if account else None, "account_id": account,
        "identity_source": source, "identity_at_ms": identity_at, "legacy_state_json": legacy,
    }


OPEN = {"qty": "0.00150", "entry_price": "67012.34000000", "entry_time_ms": T + MINUTE,
        "entry_order_id": "1321003749386327552"}
ADOPTED_BEFORE = state("run-adopted", SPOT, None, "stopped", T + 3_000, T + 90_000)
LEGACY = state("run-legacy", INVERSE, None, "stopped", T + 4_000, T + 95_000)

BOT_RUNS = [
    # Byte order puts "run-B" (0x42) before "run-a" (0x61); a linguistic
    # collation would not.
    run("run-B", LINEAR, "stopped",
        state("run-B", LINEAR, "dry_run", "stopped", T + 1_000, T + 61_000),
        T + 1_000, T + 61_000, environment="testnet", identity_at=T + 1_000),
    run("run-a", SPOT, "stopped",
        state("run-a", SPOT, "live", "stopped", T + 2_000, T + 120_000, position=OPEN),
        T + 2_000, T + 120_000, environment="testnet", account="0123456789abcdef",
        identity_at=T + 2_000),
    run("run-adopted", SPOT, "stopped",
        state("run-adopted", SPOT, "live", "stopped", T + 3_000, T + 90_000),
        T + 3_000, T + 90_000, environment="testnet", account="fedcba9876543210",
        source="adopted", identity_at=T + 100_000, legacy=ADOPTED_BEFORE),
    # Written before schema 5: no identity, and no execution mode in its
    # state, kept until an operator adopts it.
    run("run-legacy", INVERSE, "stopped", LEGACY, T + 4_000, T + 95_000, source="legacy",
        legacy=LEGACY),
    run("run-ü", UNICODE, "failed",
        state("run-ü", UNICODE, "dry_run", "failed", T + 5_000, T + 65_000,
              error="the exchange refused: ü"),
        T + 5_000, T + 65_000, identity_at=T + 5_000, error="the exchange refused: ü"),
]


def fill(price, qty, trade_id):
    return {"price": price, "qty": qty, "commission": "0.00000150",
            "commission_asset": "BTC", "trade_id": trade_id}


def raw_order(symbol, side, status, order_id, client, transact, executed, quote, fills,
              raw):
    return json.dumps({
        "symbol": symbol, "side": side, "order_type": "MARKET", "status": status,
        "order_id": order_id, "client_order_id": client, "requested_qty": None,
        "requested_quote_qty": "100.00", "executed_qty": executed,
        "cumulative_quote_qty": quote, "avg_price": None, "transact_time_ms": transact,
        "fills": fills, "raw": raw,
    }, separators=(",", ":"))


def event(seq, run_id, symbol, side, status, order_id, client, transact, raw_json,
          legacy=None):
    return {"seq": seq, "run_id": run_id, "symbol": symbol, "side": side,
            "order_type": "MARKET", "status": status, "order_id": order_id,
            "client_order_id": client, "transact_time_ms": transact, "raw_json": raw_json,
            "legacy_raw_json": legacy}


BUY_ID, SELL_ID = "1321003749386327552", "1321003749386327600"
ORDER_EVENTS = [
    event(1, "run-a", "BTCUSDT", "BUY", "FILLED", BUY_ID, "qf-b-run-a-1", T + MINUTE + 1,
          raw_order("BTCUSDT", "Buy", "Filled", BUY_ID, "qf-b-run-a-1", T + MINUTE + 1,
                    "0.00150", "100.5180000", [fill("67012.00", "0.00150", "88001")],
                    {"orderId": 1321003749386327552})),
    event(2, "run-a", "BTCUSDT", "SELL", "FILLED", SELL_ID, "qf-s-run-a-2", T + 2 * MINUTE + 1,
          raw_order("BTCUSDT", "Sell", "Filled", SELL_ID, "qf-s-run-a-2", T + 2 * MINUTE + 1,
                    "0.00150", "100.6500000", None, {})),
    # Its venue ID was a number before schema 5: the original stays beside it.
    event(5, "run-legacy", "BTCUSD", "BUY", "FILLED", "7", None, T + 4_100,
          raw_order("BTCUSD", "Buy", "Filled", "7", None, T + 4_100, "0.010", "11.00",
                    [fill("1100.0", "0.010", "70")], {}),
          legacy=raw_order("BTCUSD", "Buy", "Filled", 7, None, T + 4_100, "0.010", "11.00",
                           [fill("1100.0", "0.010", 70)], {})),
    event(9, "run-adopted", "BTCUSDT", "BUY", "PARTIALLY_FILLED", "a1b2c3-d4", "qf-b-adopt",
          T + 3_100,
          raw_order("BTCUSDT", "Buy", "PartiallyFilled", "a1b2c3-d4", "qf-b-adopt", T + 3_100,
                    "0.0005", "33.50", [], {"note": "ünïcode"})),
    # An answer that named no order: every ID unknown.
    event(10, "run-B", "BTCUSDT", "BUY", "NEW", None, None, None,
          raw_order("BTCUSDT", "Buy", "New", None, None, None, None, None, None, {})),
]


def trade(seq, run_id, symbol, entry, entry_price, exit_price, qty, pnl, entry_id, exit_id):
    return {"seq": seq, "run_id": run_id, "symbol": symbol, "entry_time_ms": entry,
            "exit_time_ms": entry + MINUTE, "entry_price": entry_price,
            "exit_price": exit_price, "qty": qty, "gross_quote_pnl": pnl,
            "entry_order_id": entry_id, "exit_order_id": exit_id}


CLOSED_TRADES = [
    trade(1, "run-a", "BTCUSDT", T + MINUTE, "67012.34000000", "67100.10", "0.00150",
          "0.131640000000", BUY_ID, SELL_ID),
    trade(4, "run-legacy", "BTCUSD", T + 4_000, "1100.0", "1050.0", "0.010", "-0.5", None, None),
]

# Two bundles: their bytes are copied as they are, NUL bytes and all, and
# their names compare byte by byte.
BUNDLE_BYTES = [bytes(range(256)) * 3 + b"\x00end", b"quantforge.toml\x00" + bytes(511) + b"x"]


def bundle(data, stored):
    return {"code_hash": "sha256:" + hashlib.sha256(data).hexdigest(), "size_bytes": len(data),
            "data": data, "stored_at_ms": stored}


BUNDLES = [bundle(BUNDLE_BYTES[0], T + 10_000), bundle(BUNDLE_BYTES[1], T + 20_000)]


def request(bundle_row, symbol, category="spot"):
    data = {"exchange": "binance", "category": category, "symbol": symbol, "interval": "1m",
            "start_open_ms": T, "end_open_ms": T + 2 * MINUTE}
    submission = {"bundle": bundle_row["code_hash"], "data": data,
                  "backtest": {"initial_cash": "10000", "fee_bps": "10",
                               "close_out_at_end": True}}
    return json.dumps(submission, separators=(",", ":"), ensure_ascii=False)


def job(seq, job_id, state, bundle_row, submitted, started=None, finished=None, results=None,
        failure=None, symbol="BTCUSDT"):
    return {"seq": seq, "job_id": job_id, "state": state, "bundle_hash": bundle_row["code_hash"],
            "request_json": request(bundle_row, symbol), "submitted_at_ms": submitted,
            "started_at_ms": started, "finished_at_ms": finished, "results_json": results,
            "failure_json": failure}


# Every phase, numbered with gaps the copy keeps; one job's market is the
# non-ASCII symbol of the candles.
JOBS = [
    job(1, "0" * 31 + "1", "succeeded", BUNDLES[0], T + 30_000, T + 31_000, T + 32_000,
        results="[]"),
    job(2, "f" * 32, "failed", BUNDLES[1], T + 40_000, None, T + 40_500,
        failure='{"code":"unavailable","message":"this server cannot run Python bundles yet"}'),
    job(5, "0123456789abcdef0123456789abcdef", "cancelled", BUNDLES[0], T + 50_000, T + 51_000,
        T + 52_000),
    job(9, "a" * 32, "running", BUNDLES[0], T + 60_000, T + 61_000, symbol=UNICODE[1]),
    job(10, "b" * 32, "queued", BUNDLES[1], T + 70_000),
]

ROWS = {
    "candles": CANDLES,
    "bot_runs": BOT_RUNS,
    "order_events": ORDER_EVENTS,
    "closed_trades": CLOSED_TRADES,
    "segments": SEGMENTS,
    "segment_index": SEGMENT_INDEX,
    "segment_dirty": SEGMENT_DIRTY,
    "bundles": BUNDLES,
    "jobs": JOBS,
}

# --- What a verification reads ------------------------------------------------


def json_value(value):
    return {"hex": value.hex()} if isinstance(value, bytes) else value


def scopes():
    out = []
    for table, columns, key, by_market in TABLES:
        rows = [[row[column] for column in columns] for row in ROWS[table]]
        position = columns.index(key)
        if by_market:
            groups = {}
            for row in rows:
                groups.setdefault(tuple(row[:3]), []).append(row)
            markets = sorted(groups, key=lambda market: tuple(s.encode("utf-8") for s in market))
            parts = [(list(market), groups[market]) for market in markets]
        else:
            parts = [(None, rows)]
        for market, part in parts:
            part = sorted(part, key=lambda row: order_key(row[position]))
            keys = [row[position] for row in part]
            integer = all(isinstance(k, int) for k in keys) and bool(keys)
            out.append({
                "table": table,
                "market": market,
                "rows": len(part),
                "first": keys[0] if integer else None,
                "last": keys[-1] if integer else None,
                "checksum": checksum(canonical(table, columns, part)),
            })
    return out


def worked_example():
    table, columns = "closed_trades", TABLES[3][1]
    rows = [[CLOSED_TRADES[1][column] for column in columns]]
    text = canonical(table, columns, rows)
    return {
        "table": table,
        "rows": rows,
        "canonical": text.decode("utf-8"),
        "checksum": checksum(text),
    }


def main() -> None:
    check_packer()
    for row in CANDLES:
        if row["exchange"] == "binance_spot" and row["symbol"] == "ETHUSDT":
            continue
        for column in DECIMALS:
            assert exact(row[column]), row
    fixture = {
        "format": FORMAT,
        "doc": (
            "A store's tables as rows: integers, text and blobs ({\"hex\": ...}), with null for "
            "NULL; and, for every table and every market of a market table, in the order a "
            "verification reports them, the rows it reads, the first and last value of an "
            "integer key, and the checksum of the canonical text of docs/storage-migration.md. "
            "Written by spikes/storage-contract/transfer_cases.py, never by the Rust code under "
            "test."
        ),
        "environment": "testnet",
        "tables": {
            table: {
                "columns": columns,
                "rows": [[json_value(row[c]) for c in columns] for row in ROWS[table]],
            }
            for table, columns, _, _ in TABLES
        },
        "scopes": scopes(),
        "worked_example": worked_example(),
    }
    OUT.write_text(json.dumps(fixture, indent=2, ensure_ascii=False) + "\n")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(fixture['scopes'])} scopes")


if __name__ == "__main__":
    main()
