#!/usr/bin/env python3
"""The storage conformance cases of docs/storage-conformance.md, written
independently of the Rust code under test.

A reference model of the candle, journal and segment ports, taken from the
documents (docs/storage-conformance.md, docs/segment-spec.md,
docs/storage-schema.md), runs every scenario and records what each step
must return. The segment packer is the one docs/segment-spec.md prints; it
is checked against every case of tests/fixtures/segment_cases.json before
anything is written.

Run with ``python3 -I spikes/storage-contract/conformance_cases.py`` from
the repository root; it writes tests/fixtures/storage_conformance_cases.json.
Stdlib only.
"""

import hashlib
import json
import pathlib
import re
import struct
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = ROOT / "tests" / "fixtures" / "storage_conformance_cases.json"
SEGMENT_CASES = ROOT / "tests" / "fixtures" / "segment_cases.json"

I64_MIN, I64_MAX = -(2**63), 2**63 - 1
MINUTE = 60_000
DAY = 86_400_000
# 2026-10-20T00:00:00Z, a UTC midnight.
T = 1_792_454_400_000
# 29 decimal places: more than a Decimal holds.
INEXACT = "1.00000000000000000000000000001"
# The schema every backend creates; a new store logs one step to it from 0.
SCHEMA_VERSION = 5
WINDOW_BARS = 1440
STEP_MS = {
    "1s": 1_000,
    "1m": MINUTE,
    "3m": 3 * MINUTE,
    "5m": 5 * MINUTE,
    "15m": 15 * MINUTE,
    "30m": 30 * MINUTE,
    "1h": 60 * MINUTE,
    "2h": 120 * MINUTE,
    "4h": 240 * MINUTE,
    "6h": 360 * MINUTE,
    "8h": 480 * MINUTE,
    "12h": 720 * MINUTE,
    "1d": DAY,
    "3d": 3 * DAY,
    "1w": 7 * DAY,
}

# The markets the scenarios name: the same symbol on both venues and every
# family, beside another symbol and other intervals.
MARKETS = {
    "spot": ("binance_spot", "BTCUSDT", "1m"),
    "linear": ("binance_linear", "BTCUSDT", "1m"),
    "inverse": ("binance_inverse", "BTCUSD", "1m"),
    "bybit_spot": ("bybit_spot", "BTCUSDT", "1m"),
    "bybit_linear": ("bybit_linear", "BTCUSDT", "1m"),
    "bybit_inverse": ("bybit_inverse", "BTCUSD", "1m"),
    "eth": ("binance_spot", "ETHUSDT", "1m"),
    "spot_5m": ("binance_spot", "BTCUSDT", "5m"),
    "eth_5m": ("binance_spot", "ETHUSDT", "5m"),
    "spot_3d": ("binance_spot", "BTCUSDT", "3d"),
    "spot_1w": ("binance_spot", "BTCUSDT", "1w"),
}

SERIES_RULE = (
    "candle i of a series, for i from 0 to count - 1 (only those in range [a, b) when a "
    "range is given, and none listed in skip): open_time_ms = from + i * spacing_ms "
    "(spacing_ms defaults to the market's interval step); close_time_ms = open_time_ms + "
    "step - 1; open, high, low and close are the close text when one is given, otherwise "
    "\"{100 + i % 50}.{i % 100, two digits}\"; volume is \"0.50000000\"; trades is "
    "open_time_ms modulo 97, never negative; candles are written in rising i"
)

# --- The packer of docs/segment-spec.md, "In Python" -------------------------

EXACT = re.compile(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?")
COLUMNS = ("open", "high", "low", "close", "volume")


def exact(text: str) -> bool:
    """Whether text is a decimal in its exact form."""
    if len(text) > 31 or not EXACT.fullmatch(text):
        return False
    unsigned = text.removeprefix("-")
    if text.startswith("-") and set(unsigned) <= {"0", "."}:
        return False  # no sign on zero
    integer, _, fraction = unsigned.partition(".")
    return len(fraction) <= 28 and int(integer + fraction) < 2**96


def payload(candles: list) -> bytes:
    """The payload of 1 to 1,440 candles with rising open times."""
    n = len(candles)
    if not 1 <= n <= WINDOW_BARS:
        raise ValueError(f"{n} candles do not fit a segment")
    out = bytearray(b"QFSEGMNT") + struct.pack("<II", 1, n)
    out += struct.pack(f"<{n}q", *(c["open_time_ms"] for c in candles))
    out += struct.pack(f"<{n}q", *(c["close_time_ms"] for c in candles))
    out += struct.pack(f"<{n}Q", *(c["trades"] or 0 for c in candles))
    out += bytes(c["trades"] is not None for c in candles)
    for column in COLUMNS:
        texts = [c[column] for c in candles]
        if not all(exact(text) for text in texts):
            raise ValueError(f"a {column} is not an exact decimal")
        out += bytes(len(text) for text in texts) + "".join(texts).encode("ascii")
    return bytes(out)


def name(data: bytes) -> str:
    return "sha256:" + hashlib.sha256(data).hexdigest()


def check_packer() -> None:
    """Refuses to go on unless the packer reproduces every shared case."""
    cases = json.loads(SEGMENT_CASES.read_text())["cases"]
    for case in cases:
        packed = payload(case["candles"])
        if packed.hex() != case["payload_hex"] or name(packed) != case["hash"]:
            raise SystemExit(f"the packer disagrees with segment case {case['name']!r}")
    print(f"packer: all {len(cases)} segment cases reproduced")


# --- Building blocks ---------------------------------------------------------


def candle(open_time_ms, close, interval="1m", trades=1, **columns):
    """A candle whose open, high, low and close are `close` unless given."""
    out = {
        "open_time_ms": open_time_ms,
        "close_time_ms": open_time_ms + STEP_MS[interval] - 1,
        "open": close,
        "high": close,
        "low": close,
        "close": close,
        "volume": "1.000",
        "trades": trades,
    }
    out.update(columns)
    return out


def series_candles(market: str, series: dict) -> list:
    """The candles a series names, by SERIES_RULE."""
    step = STEP_MS[MARKETS[market][2]]
    spacing = series.get("spacing_ms", step)
    first, end = series.get("range", [0, series["count"]])
    skip = set(series.get("skip", []))
    out = []
    for i in range(first, min(end, series["count"])):
        if i in skip:
            continue
        open_time_ms = series["from"] + i * spacing
        price = series.get("close", f"{100 + i % 50}.{i % 100:02d}")
        out.append(
            {
                "open_time_ms": open_time_ms,
                "close_time_ms": open_time_ms + step - 1,
                "open": price,
                "high": price,
                "low": price,
                "close": price,
                "volume": "0.50000000",
                "trades": open_time_ms % 97,
            }
        )
    return out


def fingerprint(key: str) -> str:
    """The first 16 hex digits of the SHA-256 of an API key."""
    return hashlib.sha256(key.encode()).hexdigest()[:16]


def identity(environment=None, key=None, source="recorded"):
    account = None if key is None else {"key": key, "fingerprint": fingerprint(key)}
    return {"environment": environment, "account": account, "source": source}


LEGACY_IDENTITY = identity(source="legacy")


def refusal(contains: str) -> dict:
    return {"kind": "invalid_argument", "contains": contains}


def holds_exactly(text: str) -> bool:
    """Whether a Decimal reads text without rounding or overflow: at most
    28 decimal places and a mantissa below 2**96. QuantForge writes only
    such text; anything else was written behind the store's back, and is
    refused when read, never rounded."""
    integer, _, fraction = text.removeprefix("-").partition(".")
    return len(fraction) <= 28 and int(integer + fraction) < 2**96


CANDLE_DECIMALS = ["open", "high", "low", "close", "volume"]
TRADE_DECIMALS = ["entry_price", "exit_price", "qty", "gross_quote_pnl"]


def unreadable(rows, columns):
    """The refusal for the first stored text no Decimal holds exactly, in
    the order the rows are read, or None."""
    for row in rows:
        for column in columns:
            if not holds_exactly(row[column]):
                return refusal(row[column])
    return None


# --- The reference model -----------------------------------------------------


def window(interval: str, open_time_ms: int):
    """The window holding a candle, or None where it cannot be written."""
    span = STEP_MS[interval] * WINDOW_BARS
    start = (open_time_ms // span) * span
    end = start + span
    if start < I64_MIN or end > I64_MAX:
        return None
    return start, end


class Model:
    """What a conforming store holds, port by port."""

    def __init__(self):
        self.candles = {}  # market -> {open_time_ms: candle}
        self.dirty = {}  # market -> {window start: mark}
        self.index = {}  # market -> {window start: segment ref}
        self.segments = {}  # name -> payload
        self.marks = 0
        self.runs = {}  # run_id -> record
        self.journal = []  # (kind, run_id, name), in append order
        self.edits = {}  # (run_id, trade name) -> columns written behind the store's back
        self.environment = None  # the store's binding

    # Candles.

    def upsert(self, market, candles):
        interval = MARKETS[market][2]
        for c in candles:
            if window(interval, c["open_time_ms"]) is None:
                return None, refusal(str(c["open_time_ms"]))
        rows = self.candles.setdefault(market, {})
        changed = set()
        for c in candles:
            if rows.get(c["open_time_ms"]) != c:
                rows[c["open_time_ms"]] = dict(c)
                changed.add(window(interval, c["open_time_ms"])[0])
        for start in sorted(changed):
            self.marks += 1
            self.dirty.setdefault(market, {})[start] = self.marks
        return len(candles), None

    def rows(self, market):
        return [self.candles[market][t] for t in sorted(self.candles.get(market, {}))]

    def load(self, market, start=None, end=None, limit=None):
        out = [
            c
            for c in self.rows(market)
            if (start is None or c["open_time_ms"] >= start)
            and (end is None or c["open_time_ms"] <= end)
        ]
        out = out if limit is None else out[:limit]
        return out, unreadable(out, CANDLE_DECIMALS)

    def recent(self, market, limit):
        rows = self.rows(market)
        out = rows[len(rows) - limit :] if limit else []
        # Read newest first, then returned oldest first.
        return out, unreadable(list(reversed(out)), CANDLE_DECIMALS)

    def max_open_time(self, market):
        rows = self.rows(market)
        return rows[-1]["open_time_ms"] if rows else None

    # Segments.

    def tip_window(self, market):
        newest = self.max_open_time(market)
        return None if newest is None else window(MARKETS[market][2], newest)[0]

    def window_candles(self, market, start):
        interval = MARKETS[market][2]
        end = start + STEP_MS[interval] * WINDOW_BARS
        return [c for c in self.rows(market) if start <= c["open_time_ms"] < end]

    def pack(self, market, start):
        """('blocked', None), ('keep', None) or ('write', ref), as packing finds it."""
        candles = self.window_candles(market, start)
        try:
            data = payload(candles)
        except ValueError:
            return "blocked", None
        indexed = self.index.get(market, {}).get(start)
        if indexed is not None and indexed["hash"] == name(data):
            return "keep", None
        return "write", {
            "window_start_ms": start,
            "first_open_ms": candles[0]["open_time_ms"],
            "last_open_ms": candles[-1]["open_time_ms"],
            "bar_count": len(candles),
            "hash": name(data),
            "payload_len": len(data),
            "_payload": data,
        }

    def write_back(self, market, start, outcome):
        kind, ref = outcome
        if kind == "write":
            ref = dict(ref)
            self.segments.setdefault(ref["hash"], ref.pop("_payload"))
            self.index.setdefault(market, {})[start] = ref
        del self.dirty[market][start]

    def refresh(self, market):
        counts = {"built": 0, "unchanged": 0, "blocked": 0}
        tip = self.tip_window(market)
        if tip is None:
            return counts
        for start in sorted(w for w in self.dirty.get(market, {}) if w < tip):
            outcome = self.pack(market, start)
            if outcome[0] == "blocked":
                counts["blocked"] += 1
                continue
            counts["unchanged" if outcome[0] == "keep" else "built"] += 1
            self.write_back(market, start, outcome)
        return counts

    def resolve(self, market, first, last):
        if first > last:
            return None, refusal("after the last")
        tip = self.tip_window(market)
        if tip is None:
            return {"segments": [], "tip": {"first_open_ms": first, "last_open_ms": last}}, None
        dirty = self.dirty.get(market, {})
        tip_start = min([tip, *dirty])
        refs = [
            ref
            for start, ref in sorted(self.index.get(market, {}).items())
            if start < tip_start and ref["last_open_ms"] >= first and ref["first_open_ms"] <= last
        ]
        tail = None
        if last >= tip_start:
            tail = {"first_open_ms": max(first, tip_start), "last_open_ms": last}
        return {"segments": [dict(ref) for ref in refs], "tip": tail}, None

    def load_segment(self, hash_text):
        data = self.segments.get(hash_text)
        return None if data is None else {"bar_count": struct.unpack_from("<I", data, 12)[0]}

    # Journal.

    def create_run(self, run, ident):
        if ident["source"] == "legacy":
            return refusal("only a migration marks runs legacy")
        if run["run_id"] in self.runs:
            return refusal(f"run {run['run_id']} already exists")
        self.runs[run["run_id"]] = {
            "spec": run["name"],
            "identity": ident,
            "market": run["market"],
            "strategy": run["strategy_name"],
            "started": run["started_at_ms"],
            "updated": run["updated_at_ms"],
            "loads": True,
        }
        return None

    def seed_legacy(self, run, recorded_mode):
        self.runs[run["run_id"]] = {
            "spec": run["name"],
            "identity": LEGACY_IDENTITY,
            "market": run["market"],
            "strategy": run["strategy_name"],
            "started": run["started_at_ms"],
            "updated": run["updated_at_ms"],
            "loads": recorded_mode,
        }

    def save_run(self, run):
        record = self.runs.get(run["run_id"])
        if record is None:
            return refusal(f"run {run['run_id']} was never created")
        # The state changes; the identity, the market, the strategy and the
        # start stay as created.
        record["spec"] = run["name"]
        record["updated"] = run["updated_at_ms"]
        return None

    def loaded(self, run_id):
        record = self.runs[run_id]
        if not record["loads"]:
            return None, refusal(f"run {run_id} was recorded by QuantForge 0.2.0 without its execution mode")
        return record["spec"], None

    def load_run(self, run_id):
        return (None, None) if run_id not in self.runs else self.loaded(run_id)

    def latest_run(self, market, strategy):
        found = [
            (r["updated"], r["started"], run_id)
            for run_id, r in self.runs.items()
            if r["market"] == market and r["strategy"] == strategy
        ]
        return (None, None) if not found else self.loaded(max(found)[2])

    def identity_matches(self, run_id, current):
        recorded = self.runs[run_id]["identity"]
        if recorded["source"] == "legacy":
            return "unverified"
        if recorded["environment"] != current["environment"]:
            return "environment"
        if recorded["account"] != current["account"]:
            return "account"
        return "ok"

    def append(self, kind, run_id, item):
        self.journal.append((kind, run_id, item))

    # The store's record.

    def bind(self, environment):
        if self.environment in (None, environment):
            self.environment = environment
            return None
        return refusal(f"is bound to {self.environment}; a store keeps its environment for life")

    def legacy_runs(self):
        found = sorted(
            (r["started"], run_id) for run_id, r in self.runs.items() if r["identity"]["source"] == "legacy"
        )
        out = []
        for _, run_id in found:
            spec = RUNS[self.runs[run_id]["spec"]]
            out.append(
                {
                    "run_id": run_id,
                    "market": spec["market"],
                    "status": spec["status"],
                    "execution_mode": spec["execution_mode"] if self.runs[run_id]["loads"] else None,
                    "started_at_ms": spec["started_at_ms"],
                }
            )
        return out

    def adopt(self, run_id, ident, mode):
        if ident["source"] != "adopted":
            return refusal(f"identity source is adopted, not {ident['source']}")
        record = self.runs.get(run_id)
        if record is None:
            return refusal(f"no run found for run_id={run_id}")
        if record["identity"]["source"] != "legacy":
            return refusal(f"run {run_id} already has an identity ({record['identity']['source']})")
        if record["loads"] and mode is not None:
            return refusal(f"run {run_id} already records its execution mode")
        if not record["loads"] and mode is None:
            return refusal(f"run {run_id} recorded no execution mode")
        # A run that recorded no mode loads with the one stated, which the
        # scenarios take from its spec.
        assert mode is None or mode == RUNS[record["spec"]]["execution_mode"]
        record["identity"] = ident
        record["loads"] = True
        return None

    def listed(self, kind, run_id, limit):
        names = [item for k, r, item in self.journal if k == kind and r == run_id]
        names = list(reversed(names))[:limit]
        if kind != "trade":
            return names, None
        rows = [{**TRADES[name], **self.edits.get((run_id, name), {})} for name in names]
        return names, unreadable(rows, TRADE_DECIMALS)


# --- Scenarios ---------------------------------------------------------------


class Scenario:
    """Steps and, from the model, what each must return."""

    def __init__(self, sid, doc):
        self.id, self.doc, self.steps, self.model = sid, doc, [], Model()

    def out(self):
        return {"id": self.id, "doc": self.doc, "steps": self.steps}

    def step(self, op, **fields):
        self.steps.append({"op": op, **fields})

    def upsert(self, market, candles=None, series=None):
        rows = candles if series is None else series_candles(market, series)
        written, refused = self.model.upsert(market, rows)
        source = {"candles": candles} if series is None else {"series": series}
        if refused:
            self.step("upsert", market=market, **source, refused=refused)
        else:
            self.step("upsert", market=market, **source, written=written)

    def load(self, market, start=None, end=None, limit=None):
        expect, refused = self.model.load(market, start, end, limit)
        answer = {"refused": refused} if refused else {"expect": expect}
        self.step("load", market=market, start=start, end=end, limit=limit, **answer)

    def recent(self, market, limit):
        expect, refused = self.model.recent(market, limit)
        answer = {"refused": refused} if refused else {"expect": expect}
        self.step("recent", market=market, limit=limit, **answer)

    def max_open_time(self, market):
        self.step("max_open_time", market=market, expect=self.model.max_open_time(market))

    def refresh(self, market):
        self.step("refresh", market=market, expect=self.model.refresh(market))

    def resolve(self, market, first, last):
        plan, refused = self.model.resolve(market, first, last)
        if refused:
            self.step("resolve", market=market, first=first, last=last, refused=refused)
        else:
            self.step("resolve", market=market, first=first, last=last, expect=plan)
        return plan

    def load_segment(self, hash_text):
        self.step("load_segment", hash=hash_text, expect=self.model.load_segment(hash_text))

    def raw_select(self, sql, expect):
        self.step("raw_select", sql=sql, expect=expect)

    def raw_contains(self, sql, contains):
        self.step("raw_contains", sql=sql, contains=contains)

    def raw_execute(self, sql):
        self.step("raw_execute", sql=sql)

    def value_types(self, table, column, expect):
        self.step("value_types", table=table, column=column, expect=expect)

    def reopen(self):
        self.step("reopen")

    def create_run(self, run, ident):
        refused = self.model.create_run(RUNS[run], ident)
        extra = {"refused": refused} if refused else {}
        self.step("create_run", run=run, identity=ident, **extra)

    def save_run(self, run):
        refused = self.model.save_run(RUNS[run])
        extra = {"refused": refused} if refused else {}
        self.step("save_run", run=run, **extra)

    def seed_legacy_run(self, run, recorded_mode):
        self.model.seed_legacy(RUNS[run], recorded_mode)
        self.step("raw_execute", sql=legacy_insert(RUNS[run], recorded_mode))

    def load_run(self, run_id):
        found, refused = self.model.load_run(run_id)
        if refused:
            self.step("load_run", run_id=run_id, refused=refused)
        else:
            self.step("load_run", run_id=run_id, expect=found)

    def run_identity(self, run_id):
        record = self.model.runs.get(run_id)
        self.step("run_identity", run_id=run_id, expect=None if record is None else record["identity"])

    def latest_run(self, market, strategy="sma_cross"):
        found, refused = self.model.latest_run(market, strategy)
        if refused:
            self.step("latest_run", market=market, strategy=strategy, refused=refused)
        else:
            self.step("latest_run", market=market, strategy=strategy, expect=found)

    def identity_matches(self, run_id, current):
        expect = self.model.identity_matches(run_id, current)
        self.step("identity_matches", run_id=run_id, invocation=current, expect=expect)

    def append_order_event(self, run_id, order):
        self.model.append("order", run_id, order)
        self.step("append_order_event", run_id=run_id, order=order)

    def list_order_events(self, run_id, limit):
        expect, _ = self.model.listed("order", run_id, limit)
        self.step("list_order_events", run_id=run_id, limit=limit, expect=expect)

    def append_closed_trade(self, run_id, trade):
        self.model.append("trade", run_id, trade)
        self.step("append_closed_trade", run_id=run_id, trade=trade)

    def list_closed_trades(self, run_id, limit):
        expect, refused = self.model.listed("trade", run_id, limit)
        answer = {"refused": refused} if refused else {"expect": expect}
        self.step("list_closed_trades", run_id=run_id, limit=limit, **answer)

    def store_record(self):
        expect = {
            "schema_version": SCHEMA_VERSION,
            "environment": self.model.environment,
            "activation": "inactive",
            "activation_epoch": 0,
        }
        self.step("store_record", expect=expect)

    def migrations(self):
        self.step("migrations", expect=[{"to_version": SCHEMA_VERSION, "from_version": 0}])

    def bind_environment(self, environment):
        refused = self.model.bind(environment)
        answer = {"refused": refused} if refused else {}
        self.step("bind_environment", environment=environment, **answer)

    def legacy_runs(self):
        self.step("legacy_runs", expect=self.model.legacy_runs())

    def adopt_run(self, run_id, ident, mode=None):
        refused = self.model.adopt(run_id, ident, mode)
        answer = {"refused": refused} if refused else {}
        self.step("adopt_run", run_id=run_id, identity=ident, execution_mode=mode, **answer)


def where(market):
    exchange, symbol, interval = MARKETS[market]
    return f"exchange = '{exchange}' AND symbol = '{symbol}' AND interval = '{interval}'"


# --- Journal records ---------------------------------------------------------

POSITION_FLAT = {"qty": "0", "entry_price": None, "entry_time_ms": None, "entry_order_id": None}


def run(name, run_id, market="spot", **fields):
    out = {
        "name": name,
        "run_id": run_id,
        "market": market,
        "strategy_name": "sma_cross",
        "strategy_config": {"kind": "sma_cross", "fast": 20, "slow": 50},
        "execution_mode": "dry_run",
        "status": "running",
        "last_processed_open_time_ms": None,
        "started_at_ms": T,
        "updated_at_ms": T,
        "stopped_at_ms": None,
        "last_error": None,
        "position": POSITION_FLAT,
    }
    out.update(fields)
    return out


OPEN_POSITION = {
    "qty": "0.00120",
    "entry_price": "67012.34000000",
    "entry_time_ms": T + 60_000,
    "entry_order_id": "1321003749386327552",
}

RUNS = {
    r["name"]: r
    for r in [
        run(
            "full",
            "run-1",
            execution_mode="live",
            last_processed_open_time_ms=T + 120_000,
            updated_at_ms=T + 180_000,
            last_error="the last poll timed out",
            position=OPEN_POSITION,
            strategy_config={"kind": "sma_cross", "fast": 7, "slow": 25, "note": "ünïcode"},
        ),
        run("first", "run-1"),
        run(
            "first_stopped",
            "run-1",
            status="stopped",
            last_processed_open_time_ms=T + 120_000,
            updated_at_ms=T + 180_000,
            stopped_at_ms=T + 180_000,
        ),
        run("second", "run-2", market="eth"),
        run("older", "run-a", updated_at_ms=1_000),
        run("newer", "run-b", updated_at_ms=2_000),
        run("other_strategy", "run-c", strategy_name="other_strategy", updated_at_ms=3_000),
        run("other_interval", "run-d", market="spot_5m", updated_at_ms=4_000),
        run("other_venue", "run-e", market="bybit_spot", updated_at_ms=5_000),
        run("other_family", "run-f", market="linear", updated_at_ms=6_000),
        run("tie_m", "run-m", updated_at_ms=9_000, started_at_ms=1),
        run("tie_a", "run-a", updated_at_ms=9_000, started_at_ms=2),
        run("tie_z", "run-z", updated_at_ms=9_000, started_at_ms=2),
        run("tie_q", "run-q", updated_at_ms=9_000, started_at_ms=3),
        run("tie_m_later", "run-m", updated_at_ms=9_500, started_at_ms=1),
        # Byte order puts "B" (0x42) before "a" (0x61); a linguistic
        # collation would not.
        run("tie_upper", "run-B", updated_at_ms=9_000, started_at_ms=2),
        run("tie_lower", "run-a", updated_at_ms=9_000, started_at_ms=2),
        run(
            "legacy",
            "run-legacy",
            execution_mode="live",
            status="stopped",
            last_processed_open_time_ms=T + 60_000,
            updated_at_ms=T + 120_000,
            stopped_at_ms=T + 120_000,
            position={"qty": "0.0254", "entry_price": "3933.07", "entry_time_ms": T + 60_100, "entry_order_id": "7"},
        ),
        run("legacy_no_mode", "run-old", updated_at_ms=T + 120_000),
        # Byte order lists "run-Z" first; a linguistic collation would not.
        run("legacy_upper", "run-Z", updated_at_ms=T + 120_000),
    ]
}

# The serde tokens of the run's state_json, as a migration leaves it and as
# any build reads it: the dual channels of the enums (wire forms in columns,
# tokens in JSON).
TOKENS = {
    "binance_spot": "BinanceSpot",
    "1m": "M1",
    "dry_run": "DryRun",
    "live": "Live",
    "running": "Running",
    "stopped": "Stopped",
}


def state_json(spec, recorded_mode=True):
    exchange, symbol, interval = MARKETS[spec["market"]]
    state = {
        "run_id": spec["run_id"],
        "market": {"exchange": TOKENS[exchange], "symbol": symbol, "interval": TOKENS[interval]},
        "strategy_name": spec["strategy_name"],
        "strategy_config": spec["strategy_config"],
        "execution_mode": TOKENS[spec["execution_mode"]],
        "status": TOKENS[spec["status"]],
        "last_processed_open_time_ms": spec["last_processed_open_time_ms"],
        "started_at_ms": spec["started_at_ms"],
        "updated_at_ms": spec["updated_at_ms"],
        "stopped_at_ms": spec["stopped_at_ms"],
        "last_error": spec["last_error"],
        "position": spec["position"],
    }
    if not recorded_mode:
        del state["execution_mode"]
    return json.dumps(state, separators=(",", ":"))


def sql_text(value):
    return "NULL" if value is None else "'" + str(value).replace("'", "''") + "'"


def legacy_insert(spec, recorded_mode):
    """A run as the schema-5 migration leaves one an earlier build wrote:
    no environment, no account, source legacy, its JSON kept beside it."""
    exchange, symbol, interval = MARKETS[spec["market"]]
    text = state_json(spec, recorded_mode)
    values = [
        sql_text(spec["run_id"]),
        sql_text(exchange),
        sql_text(symbol),
        sql_text(interval),
        sql_text(spec["strategy_name"]),
        sql_text(spec["status"]),
        sql_text(text),
        str(spec["started_at_ms"]),
        str(spec["updated_at_ms"]),
        "NULL" if spec["stopped_at_ms"] is None else str(spec["stopped_at_ms"]),
        sql_text(spec["last_error"]),
        "NULL",
        "NULL",
        "NULL",
        "'legacy'",
        "NULL",
        sql_text(text),
    ]
    return (
        "INSERT INTO bot_runs(run_id, exchange, symbol, interval, strategy_name, status, "
        "state_json, started_at_ms, updated_at_ms, stopped_at_ms, last_error, environment, "
        "account_kind, account_id, identity_source, identity_at_ms, legacy_state_json) "
        f"VALUES ({', '.join(values)})"
    )


def order(name, order_id, **fields):
    out = {
        "name": name,
        "symbol": "BTCUSDT",
        "side": "BUY",
        "order_type": "MARKET",
        "status": "FILLED",
        "order_id": order_id,
        "client_order_id": f"qf-{name}",
        "requested_qty": None,
        "requested_quote_qty": "100.00",
        "executed_qty": "0.00150",
        "cumulative_quote_qty": "100.5180000",
        "avg_price": "67012.00",
        "transact_time_ms": T + 1,
        "fills": [
            {
                "price": "67012.00",
                "qty": "0.00150",
                "commission": "0.00000150",
                "commission_asset": "BTC",
                "trade_id": order_id,
            }
        ],
        "raw": {"orderId": order_id, "note": name},
    }
    out.update(fields)
    return out


ORDERS = {
    o["name"]: o
    for o in [
        order("o1", "1"),
        order("o2", "2", side="SELL", fills=None, executed_qty=None, cumulative_quote_qty=None),
        order("o3", "1321003749386327552", status="PARTIALLY_FILLED", fills=[]),
        order("o7", "7"),
        order("unknown_id", None, client_order_id=None, fills=None, avg_price=None, raw={}),
    ]
}


def trade(name, entry_time_ms, **fields):
    out = {
        "name": name,
        "symbol": "BTCUSDT",
        "entry_time_ms": entry_time_ms,
        "exit_time_ms": entry_time_ms + 60_000,
        "entry_price": "67012.34000000",
        "exit_price": "67100.10",
        "qty": "0.00150",
        "gross_quote_pnl": "0.131640000000",
        "entry_order_id": "7",
        "exit_order_id": "8",
    }
    out.update(fields)
    return out


TRADES = {
    t["name"]: t
    for t in [
        trade("t1", T),
        trade("t2", T + 120_000, entry_order_id=None, exit_order_id=None, qty="0.010", gross_quote_pnl="-0.5"),
        trade("t3", T + 240_000, entry_order_id="a1b2c3-d4", exit_order_id="00000042"),
    ]
}

# --- The candle scenarios ----------------------------------------------------


def candle_scenarios():
    out = []

    s = Scenario("candles.empty_market", "a market without candles reads nothing")
    s.upsert("eth", [candle(T, "1")])
    s.load("spot")
    s.recent("spot", 5)
    s.max_open_time("spot")
    out.append(s)

    s = Scenario(
        "candles.written_counts_statements",
        "an upsert counts the candles it was given, not the rows it added",
    )
    pair = [candle(0, "101"), candle(MINUTE, "102")]
    s.upsert("spot", [])
    s.upsert("spot", pair)
    s.upsert("spot", pair)
    s.load("spot")
    out.append(s)

    s = Scenario(
        "candles.duplicate_overwrites",
        "a candle written again under its open time replaces every other column",
    )
    s.upsert("spot", [candle(0, "105")])
    s.upsert(
        "spot",
        [
            candle(
                0,
                "106",
                close_time_ms=59_000,
                open="101",
                high="111",
                low="91",
                volume="43.21",
                trades=43,
            )
        ],
    )
    s.load("spot")
    out.append(s)

    s = Scenario(
        "candles.every_column_counts",
        "a candle written again with any one column changed is stored with that change",
    )
    stored = candle(0, "100")
    s.upsert("spot", [stored])
    for column, value in [
        ("close_time_ms", 59_000),
        ("open", "99"),
        ("high", "101"),
        ("low", "98"),
        ("close", "100.5"),
        ("volume", "2.000"),
        ("trades", 2),
    ]:
        # Each write differs from the stored row in this one column.
        stored = {**stored, column: value}
        s.upsert("spot", [stored])
        s.load("spot")
    out.append(s)

    s = Scenario("candles.later_duplicate_wins", "of two candles with one open time in a batch, the later is kept")
    s.upsert("spot", [candle(0, "1"), candle(MINUTE, "5"), candle(0, "2")])
    s.load("spot")
    out.append(s)

    s = Scenario(
        "candles.bounds_order_and_limit",
        "a load is ascending with both bounds included and keeps the oldest; recent keeps the newest, ascending",
    )
    # Closes fall as open times rise, so no other column can pass for time.
    ordered = [candle(i * MINUTE, close) for i, close in enumerate(["105", "104", "103", "102", "101"])]
    s.upsert("spot", [ordered[i] for i in (3, 0, 4, 2, 1)])
    s.load("spot", start=MINUTE, end=3 * MINUTE)
    s.load("spot")
    s.load("spot", limit=2)
    s.load("spot", start=MINUTE, limit=2)
    s.recent("spot", 2)
    s.recent("spot", 50)
    out.append(s)

    s = Scenario("candles.empty_ranges", "a range holding no candle, a zero limit and a reversed range read nothing")
    s.upsert("spot", [candle(0, "1"), candle(2 * MINUTE, "2"), candle(4 * MINUTE, "3")])
    s.load("spot", start=3 * MINUTE, end=MINUTE)
    s.load("spot", start=MINUTE, end=2 * MINUTE - 1)
    s.load("spot", limit=0)
    s.recent("spot", 0)
    s.load("spot", start=5 * MINUTE)
    s.load("spot", end=-1)
    out.append(s)

    s = Scenario("candles.exact_bounds", "a bound on a candle's open time includes it, and one past it does not")
    s.upsert("spot", [candle(0, "1"), candle(2 * MINUTE, "2"), candle(4 * MINUTE, "3")])
    s.load("spot", start=2 * MINUTE, end=2 * MINUTE)
    s.load("spot", start=2 * MINUTE + 1, end=4 * MINUTE - 1)
    s.load("spot", start=2 * MINUTE - 1, end=2 * MINUTE + 1)
    out.append(s)

    s = Scenario(
        "candles.markets_isolated",
        "the same open time on every product line, symbol and interval reads back only its own candle",
    )
    names = ["spot", "linear", "inverse", "bybit_spot", "bybit_linear", "bybit_inverse", "eth", "spot_5m"]
    for index, market in enumerate(names):
        s.upsert(market, [candle(T, f"{100 + index}.{index}0", interval=MARKETS[market][2])])
    for market in names:
        s.load(market)
        s.max_open_time(market)
    s.max_open_time("eth_5m")
    s.recent("eth_5m", 1)
    out.append(s)

    s = Scenario("candles.decimal_scale", "every decimal column keeps the text it was written with, scale included")
    scaled = candle(0, "105.5000", open="100.50", high="110.000", low="0.010", volume="12.340")
    s.upsert("spot", [scaled, candle(MINUTE, "0.0000001", volume="123456789.123456789012345678")])
    s.load("spot")
    s.raw_select(
        f"SELECT open, high, low, close, volume FROM candles WHERE {where('spot')} ORDER BY open_time_ms",
        [
            ["100.50", "110.000", "0.010", "105.5000", "12.340"],
            ["0.0000001", "0.0000001", "0.0000001", "0.0000001", "123456789.123456789012345678"],
        ],
    )
    out.append(s)

    s = Scenario("candles.unknown_trades", "an unknown trade count stays unknown and is not zero")
    s.upsert("spot", [candle(0, "1", trades=None), candle(MINUTE, "1", trades=0)])
    s.load("spot")
    s.upsert("spot", [candle(0, "1", trades=0)])
    s.load("spot", end=0)
    s.upsert("spot", [candle(0, "1", trades=None)])
    s.load("spot", end=0)
    out.append(s)

    s = Scenario("candles.trade_count_range", "every trade count from 0 to the largest unsigned 64-bit one reads back unchanged")
    counts = [0, 1, I64_MAX, I64_MAX + 1, 2**64 - 1]
    s.upsert("spot", [candle(i * MINUTE, "1", trades=count) for i, count in enumerate(counts)])
    s.load("spot")
    s.recent("spot", 2)
    out.append(s)

    s = Scenario("candles.unwritable_batch", "a batch holding a candle no segment window can hold is refused whole")
    far = candle(I64_MAX - 100, "1", close_time_ms=I64_MAX)
    s.upsert("spot", [candle(T, "1"), far])
    s.load("spot")
    s.max_open_time("spot")
    out.append(s)

    s = Scenario("candles.reopened", "a store opened again on the same database resumes from its newest candle")
    s.upsert("spot", [candle(i * MINUTE, "101") for i in range(3)])
    s.reopen()
    s.max_open_time("spot")
    s.load("spot")
    out.append(s)

    s = Scenario(
        "candles.inexact_text_refused",
        "stored text a decimal cannot hold exactly is refused by every read that returns it, never rounded",
    )
    s.upsert("spot", [candle(T + i * MINUTE, "101") for i in range(3)])
    # Written behind the store's back: 29 decimal places.
    s.raw_execute(f"UPDATE candles SET close = '{INEXACT}' WHERE {where('spot')} AND open_time_ms = {T + MINUTE}")
    s.model.candles["spot"][T + MINUTE]["close"] = INEXACT
    s.load("spot")
    s.load("spot", end=T)
    s.recent("spot", 1)
    s.recent("spot", 2)
    s.max_open_time("spot")
    out.append(s)
    return out


# --- The journal scenarios ---------------------------------------------------


def journal_scenarios():
    out = []
    testnet_a = identity("testnet", "a")

    s = Scenario("journal.round_trip", "a run reads back in full: position, cursor, error and configuration")
    s.create_run("full", testnet_a)
    s.load_run("run-1")
    s.run_identity("run-1")
    out.append(s)

    s = Scenario("journal.unknown_run", "a run never created reads nothing")
    s.create_run("first", identity())
    s.load_run("missing")
    s.run_identity("missing")
    s.list_order_events("missing", 10)
    s.list_closed_trades("missing", 10)
    s.latest_run("eth")
    out.append(s)

    s = Scenario("journal.created_once", "a run is created once, and its first identity stays")
    s.create_run("first", testnet_a)
    s.create_run("first", identity())
    s.run_identity("run-1")
    s.create_run("second", LEGACY_IDENTITY)
    s.load_run("run-2")
    out.append(s)

    s = Scenario("journal.saved_after_created", "a run is saved only after it was created")
    s.save_run("first")
    s.load_run("run-1")
    s.run_identity("run-1")
    out.append(s)

    s = Scenario("journal.save_keeps_identity", "saving replaces the state and never the identity")
    s.create_run("first", testnet_a)
    s.save_run("first_stopped")
    s.load_run("run-1")
    s.run_identity("run-1")
    s.raw_select(
        "SELECT environment, account_kind, account_id, identity_source, status FROM bot_runs "
        "WHERE run_id = 'run-1'",
        [["testnet", "key_fingerprint", fingerprint("a"), "recorded", "stopped"]],
    )
    s.raw_select("SELECT CAST(COUNT(*) AS TEXT) FROM bot_runs", [["1"]])
    out.append(s)

    s = Scenario(
        "journal.last_save_wins",
        "the last save replaces the state whatever its times, and a refused create changes nothing: one process drives a run at a time",
    )
    s.create_run("first", identity())
    s.save_run("first_stopped")
    # Older than the save before it, as a second process driving the run
    # would write: it still replaces the state.
    s.save_run("first")
    s.load_run("run-1")
    s.create_run("full", identity("testnet", "key-a"))
    s.load_run("run-1")
    s.run_identity("run-1")
    out.append(s)

    s = Scenario(
        "journal.latest_per_market",
        "the latest run is the last updated of its market and strategy; another strategy, interval, venue or family never wins",
    )
    for name in ["older", "newer", "other_strategy", "other_interval", "other_venue", "other_family"]:
        s.create_run(name, identity())
    s.latest_run("spot")
    s.latest_run("spot", "other_strategy")
    s.latest_run("spot_5m")
    s.latest_run("bybit_spot")
    s.latest_run("linear")
    s.latest_run("eth")
    out.append(s)

    s = Scenario(
        "journal.latest_tie",
        "of runs updated in the same millisecond the one started later is the latest, then the greater run ID",
    )
    # Created in the order that a store returning the first row it finds
    # would get wrong.
    s.create_run("tie_m", identity())
    s.create_run("tie_a", identity())
    s.latest_run("spot")
    s.create_run("tie_z", identity())
    s.latest_run("spot")
    s.create_run("tie_q", identity())
    s.latest_run("spot")
    s.save_run("tie_m_later")
    s.latest_run("spot")
    out.append(s)

    s = Scenario(
        "journal.latest_tie_by_bytes",
        "on a tie of both times, run IDs compare byte by byte, so run-a outranks run-B whatever the database's collation",
    )
    s.create_run("tie_lower", identity())
    s.create_run("tie_upper", identity())
    s.latest_run("spot")
    out.append(s)

    s = Scenario("journal.order_events", "order events read back newest first, up to the limit, exactly as appended")
    s.create_run("first", identity())
    for o in ["o1", "o2", "o3", "unknown_id"]:
        s.append_order_event("run-1", o)
    s.list_order_events("run-1", 10)
    s.list_order_events("run-1", 2)
    s.list_order_events("run-1", 0)
    # Venue IDs are text, however much they look like numbers.
    s.value_types("order_events", "order_id", ["text"])
    out.append(s)

    s = Scenario("journal.duplicate_event", "an order event appended twice is listed twice")
    s.create_run("first", identity())
    s.append_order_event("run-1", "o7")
    s.append_order_event("run-1", "o7")
    s.list_order_events("run-1", 10)
    out.append(s)

    s = Scenario(
        "journal.closed_trades",
        "closed trades read back newest first with their scale and their missing order IDs",
    )
    s.create_run("first", identity())
    for t in ["t1", "t2", "t3"]:
        s.append_closed_trade("run-1", t)
    s.list_closed_trades("run-1", 10)
    s.list_closed_trades("run-1", 1)
    s.value_types("closed_trades", "entry_order_id", ["text"])
    s.value_types("closed_trades", "exit_order_id", ["text"])
    out.append(s)

    s = Scenario(
        "journal.inexact_text_refused",
        "a closed trade whose stored text a decimal cannot hold exactly is refused when listed, never rounded",
    )
    s.create_run("first", identity())
    for t in ["t1", "t2"]:
        s.append_closed_trade("run-1", t)
    # Written behind the store's back: 29 decimal places.
    s.raw_execute(f"UPDATE closed_trades SET qty = '{INEXACT}' WHERE run_id = 'run-1' AND entry_time_ms = {T}")
    s.model.edits[("run-1", "t1")] = {"qty": INEXACT}
    s.list_closed_trades("run-1", 1)
    s.list_closed_trades("run-1", 10)
    out.append(s)

    s = Scenario("journal.runs_apart", "two runs' journals never mix")
    s.create_run("first", identity())
    s.create_run("second", identity())
    s.append_order_event("run-1", "o1")
    s.append_order_event("run-2", "o2")
    s.append_order_event("run-1", "o3")
    s.append_closed_trade("run-2", "t1")
    s.append_closed_trade("run-1", "t2")
    s.list_order_events("run-1", 10)
    s.list_order_events("run-2", 10)
    s.list_closed_trades("run-1", 10)
    s.list_closed_trades("run-2", 10)
    out.append(s)

    s = Scenario("journal.reopened", "a store opened again on the same database loads a run for resume")
    s.create_run("full", testnet_a)
    s.reopen()
    s.load_run("run-1")
    s.run_identity("run-1")
    out.append(s)

    s = Scenario(
        "journal.wire_forms",
        "columns hold the wire forms and the JSON the serde tokens",
    )
    s.create_run("first", identity())
    s.append_order_event("run-1", "o1")
    s.raw_select(
        "SELECT exchange, symbol, interval, strategy_name, status FROM bot_runs WHERE run_id = 'run-1'",
        [["binance_spot", "BTCUSDT", "1m", "sma_cross", "running"]],
    )
    s.raw_contains(
        "SELECT state_json FROM bot_runs WHERE run_id = 'run-1'",
        ['"BinanceSpot"', '"M1"', '"DryRun"', '"Running"'],
    )
    s.raw_select(
        "SELECT symbol, side, order_type, status, order_id FROM order_events WHERE run_id = 'run-1'",
        [["BTCUSDT", "BUY", "MARKET", "FILLED", "1"]],
    )
    s.raw_contains("SELECT raw_json FROM order_events WHERE run_id = 'run-1'", ['"Buy"', '"Filled"', '"1"'])
    out.append(s)

    s = Scenario(
        "journal.legacy_unresolved",
        "a legacy run loads with no environment and no account, is found as the latest, and is never driven",
    )
    s.seed_legacy_run("legacy", True)
    s.load_run("run-legacy")
    s.run_identity("run-legacy")
    s.latest_run("spot")
    s.identity_matches("run-legacy", identity())
    s.identity_matches("run-legacy", identity("testnet"))
    s.create_run("legacy", identity())
    s.load_run("run-legacy")
    out.append(s)

    s = Scenario(
        "journal.legacy_without_mode",
        "a legacy run that recorded no execution mode is refused on load, never given one",
    )
    s.seed_legacy_run("legacy_no_mode", False)
    s.load_run("run-old")
    s.latest_run("spot")
    s.run_identity("run-old")
    out.append(s)
    return out


# --- The store record's scenarios --------------------------------------------


def control_scenarios():
    out = []

    s = Scenario(
        "control.new_store",
        "a new store is inactive at epoch 0, bound to no environment, and logs one step to the current schema from 0",
    )
    s.store_record()
    s.migrations()
    s.reopen()
    s.store_record()
    out.append(s)

    s = Scenario(
        "control.bound_for_life",
        "a store is bound to one environment for life; binding it again to the same one changes nothing",
    )
    s.bind_environment("testnet")
    s.bind_environment("testnet")
    s.bind_environment("mainnet")
    s.reopen()
    s.store_record()
    out.append(s)

    s = Scenario(
        "control.legacy_runs_listed",
        "legacy runs are listed oldest first, then by run ID, with the mode only a run that recorded one has; recorded runs are not listed",
    )
    s.create_run("first", identity())
    s.seed_legacy_run("legacy_no_mode", False)
    s.seed_legacy_run("legacy", True)
    s.seed_legacy_run("legacy_upper", True)
    s.legacy_runs()
    out.append(s)

    s = Scenario(
        "control.adopt_a_legacy_run",
        "an adopted legacy run takes the verified identity, loads, matches only that identity, and keeps its original state beside it",
    )
    s.seed_legacy_run("legacy", True)
    s.bind_environment("testnet")
    adopted = identity("testnet", "key-a", source="adopted")
    s.adopt_run("run-legacy", adopted)
    s.run_identity("run-legacy")
    s.load_run("run-legacy")
    s.identity_matches("run-legacy", identity("testnet", "key-a"))
    s.identity_matches("run-legacy", identity("testnet", "key-b"))
    s.legacy_runs()
    s.raw_select(
        "SELECT legacy_state_json FROM bot_runs WHERE run_id = 'run-legacy'",
        [[state_json(RUNS["legacy"], True)]],
    )
    s.adopt_run("run-legacy", adopted)
    out.append(s)

    s = Scenario(
        "control.adopt_states_the_missing_mode",
        "a legacy run that recorded no execution mode is adopted only with the mode the operator states, then loads with it",
    )
    s.seed_legacy_run("legacy_no_mode", False)
    s.bind_environment("testnet")
    adopted = identity("testnet", source="adopted")
    s.adopt_run("run-old", adopted)
    s.load_run("run-old")
    s.adopt_run("run-old", adopted, mode="dry_run")
    s.load_run("run-old")
    s.run_identity("run-old")
    s.raw_select(
        "SELECT legacy_state_json FROM bot_runs WHERE run_id = 'run-old'",
        [[state_json(RUNS["legacy_no_mode"], False)]],
    )
    out.append(s)

    s = Scenario(
        "control.adopt_refusals",
        "only a legacy run is adopted, only under an adopted identity, and a run that recorded its mode refuses another; a refusal changes nothing",
    )
    s.create_run("first", identity())
    s.seed_legacy_run("legacy", True)
    adopted = identity("testnet", "key-a", source="adopted")
    s.adopt_run("run-1", adopted)
    s.adopt_run("run-none", adopted)
    s.adopt_run("run-legacy", adopted, mode="live")
    s.adopt_run("run-legacy", identity("testnet", "key-a"))
    s.run_identity("run-legacy")
    s.run_identity("run-1")
    s.legacy_runs()
    out.append(s)
    return out


# --- The segment scenarios ---------------------------------------------------


def segment_scenarios():
    out = []
    unknown = name(b"no segment")

    s = Scenario("segments.no_candles", "a market without candles is all tip")
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    out.append(s)

    s = Scenario("segments.packs_closed_windows", "a refresh packs every closed window and leaves the tip")
    s.upsert("spot", series={"from": T, "count": 2 * 1440 + 1})
    s.refresh("spot")
    s.resolve("spot", T, T + 3 * DAY)
    out.append(s)

    s = Scenario("segments.fill_order", "candles written in any order pack to the same segments")
    for end in range(2 * 1440 + 1, 0, -500):
        s.upsert("spot", series={"from": T, "count": 2 * 1440 + 1, "range": [max(0, end - 500), end]})
    s.refresh("spot")
    s.resolve("spot", T, T + 3 * DAY)
    out.append(s)

    s = Scenario("segments.nothing_changed", "a second refresh, and an identical resync, change nothing")
    s.upsert("spot", series={"from": T, "count": 1441})
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    s.refresh("spot")
    s.upsert("spot", series={"from": T, "count": 1441})
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    out.append(s)

    s = Scenario("segments.changed_back", "a window changed and changed back keeps its segment")
    s.upsert("spot", series={"from": T, "count": 1441})
    s.refresh("spot")
    s.upsert("spot", series={"from": T, "count": 1, "close": "999"})
    s.upsert("spot", series={"from": T, "count": 1441, "range": [0, 1]})
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    out.append(s)

    s = Scenario("segments.tip_until_next_window", "the window of the newest candle stays in the tip until a later window starts")
    s.upsert("spot", series={"from": T, "count": 1440})
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    s.upsert("spot", series={"from": T + DAY, "count": 1, "close": "1"})
    s.refresh("spot")
    s.resolve("spot", T, T + DAY)
    out.append(s)

    s = Scenario(
        "segments.backfill_keeps_old_segment",
        "a backfill reopens its window at once, the next refresh packs only that window, and the segment it replaced still loads",
    )
    s.upsert("spot", series={"from": T, "count": 2 * 1440 + 1, "skip": [100]})
    s.refresh("spot")
    before = s.resolve("spot", T, T + 3 * DAY)
    s.upsert("spot", series={"from": T, "count": 2 * 1440 + 1, "range": [100, 101]})
    s.resolve("spot", T, T + 3 * DAY)
    s.refresh("spot")
    s.resolve("spot", T, T + 3 * DAY)
    s.load_segment(before["segments"][0]["hash"])
    out.append(s)

    s = Scenario("segments.scale_is_a_change", "a candle written again with another scale packs to a new segment")
    s.upsert("spot", series={"from": T, "count": 1, "close": "1.10"})
    s.upsert("spot", series={"from": T + DAY, "count": 1, "close": "1"})
    s.refresh("spot")
    before = s.resolve("spot", T, T)
    s.upsert("spot", series={"from": T, "count": 1, "close": "1.1"})
    s.refresh("spot")
    s.resolve("spot", T, T)
    s.load_segment(before["segments"][0]["hash"])
    out.append(s)

    s = Scenario("segments.plan_ranges", "a plan lists the segments a range overlaps, oldest first, then the tip")
    s.upsert("spot", series={"from": T, "count": 3 * 1440 + 1})
    s.refresh("spot")
    for first, last in [
        (T - 10 * DAY, T - 1),
        (T + 30 * MINUTE, T + DAY + 10 * MINUTE),
        (T + DAY - MINUTE, T + DAY),
        (T + DAY, T + DAY),
        (T + 2 * DAY + 5, T + 4 * DAY),
        (T + 3 * DAY, T + 5 * DAY),
        (T + 10 * DAY, T + 11 * DAY),
        (T, T + 3 * DAY - 1),
        (T + 1, T),
    ]:
        s.resolve("spot", first, last)
    out.append(s)

    s = Scenario("segments.long_windows", "three-day and weekly candles stay in the tip of their long windows")
    # 2017-08-17 on the 3d grid, and a Monday, 2017-08-14, for the weekly.
    for market, first in [("spot_3d", 1_502_928_000_000), ("spot_1w", 1_502_668_800_000)]:
        s.upsert(market, series={"from": first, "count": 100, "close": "100"})
        s.refresh(market)
        s.resolve(market, first, first + 99 * STEP_MS[MARKETS[market][2]])
    out.append(s)

    s = Scenario(
        "segments.crowded_window",
        "a window holding more candles than a segment keeps itself and every later window in the tip",
    )
    s.upsert("spot", series={"from": T, "count": 1441, "spacing_ms": 30_000, "close": "1"})
    s.upsert("spot", series={"from": T + DAY, "count": 10})
    s.upsert("spot", series={"from": T + 2 * DAY, "count": 1, "close": "1"})
    s.refresh("spot")
    s.resolve("spot", T, T + 3 * DAY)
    s.refresh("spot")
    out.append(s)

    s = Scenario("segments.inexact_text", "stored text not in its exact form is left unpacked rather than corrected")
    s.upsert("spot", series={"from": T, "count": 3})
    s.upsert("spot", series={"from": T + DAY, "count": 1, "close": "1"})
    # Written behind the store's back, as an older or foreign writer might.
    s.raw_execute(f"UPDATE candles SET close = '0100.00' WHERE {where('spot')} AND open_time_ms = {T}")
    s.model.candles["spot"][T]["close"] = "0100.00"
    s.refresh("spot")
    s.resolve("spot", T, T)
    out.append(s)

    s = Scenario("segments.opens_to_its_candles", "a stored segment opens under its name to the candles it packs; an unknown name loads nothing")
    s.upsert("spot", series={"from": T, "count": 1441})
    s.refresh("spot")
    s.resolve("spot", T, T)
    s.load_segment(unknown)
    out.append(s)

    s = Scenario(
        "segments.shared_name",
        "the same candles under two product lines share one segment, listed by two index entries",
    )
    for market in ["spot", "linear"]:
        s.upsert(market, series={"from": T, "count": 3})
        s.upsert(market, series={"from": T + DAY, "count": 1, "close": "1"})
        s.refresh(market)
    plan = s.resolve("spot", T, T + 2 * MINUTE)
    s.resolve("linear", T, T + 2 * MINUTE)
    shared = plan["segments"][0]["hash"]
    s.raw_select(f"SELECT CAST(COUNT(*) AS TEXT) FROM segments WHERE hash = '{shared}'", [["1"]])
    s.raw_select(
        f"SELECT exchange FROM segment_index WHERE hash = '{shared}' ORDER BY exchange",
        [["binance_linear"], ["binance_spot"]],
    )
    out.append(s)
    return out


# --- The races, stepped by each backend --------------------------------------


def races():
    out = []
    window_start = (0 // (MINUTE * WINDOW_BARS)) * MINUTE * WINDOW_BARS

    def setup(model):
        rows = [candle(0, "1", trades=7), candle(MINUTE, "1", trades=7), candle(DAY, "1", trades=7)]
        model.upsert("spot", rows)
        return [{"op": "upsert", "market": "spot", "candles": rows}]

    model = Model()
    steps = setup(model)
    snapshot_id = model.dirty["spot"][window_start]
    snapshot = model.pack("spot", window_start)
    later = [candle(MINUTE, "2", trades=7)]
    model.upsert("spot", later)
    current = model.dirty["spot"][window_start]
    outcome = "dirty" if current != snapshot_id else "done"
    assert outcome == "dirty" and snapshot[0] == "write"
    after = []
    plan, _ = model.resolve("spot", 0, 2 * DAY)
    after.append({"op": "resolve", "market": "spot", "first": 0, "last": 2 * DAY, "expect": plan})
    after.append({"op": "refresh", "market": "spot", "expect": model.refresh("spot")})
    plan, _ = model.resolve("spot", 0, 2 * DAY)
    after.append({"op": "resolve", "market": "spot", "first": 0, "last": 2 * DAY, "expect": plan})
    out.append(
        {
            "id": "races.write_between_read_and_write_back",
            "doc": "a write between a refresh's read and its write-back keeps the window dirty, and the next refresh packs the new candles",
            "setup": steps,
            "window_start_ms": window_start,
            "steps": [
                {"op": "read", "as": "a"},
                {"op": "upsert", "market": "spot", "candles": later},
                {"op": "write_back", "of": "a", "expect": outcome},
            ],
            "after": after,
        }
    )

    model = Model()
    steps = setup(model)
    first = (model.dirty["spot"][window_start], model.pack("spot", window_start))
    second = (model.dirty["spot"][window_start], model.pack("spot", window_start))
    outcomes = []
    for mark, packed in [first, second]:
        current = model.dirty["spot"].get(window_start)
        if current is None:
            outcomes.append("elsewhere")
        elif current != mark:
            outcomes.append("dirty")
        else:
            model.write_back("spot", window_start, packed)
            outcomes.append("done")
    plan, _ = model.resolve("spot", 0, 2 * DAY)
    out.append(
        {
            "id": "races.two_refreshes",
            "doc": "of two refreshes packing one window, the later finds it done elsewhere",
            "setup": steps,
            "window_start_ms": window_start,
            "steps": [
                {"op": "read", "as": "a"},
                {"op": "read", "as": "b"},
                {"op": "write_back", "of": "a", "expect": outcomes[0]},
                {"op": "write_back", "of": "b", "expect": outcomes[1]},
            ],
            "after": [{"op": "resolve", "market": "spot", "first": 0, "last": 2 * DAY, "expect": plan}],
        }
    )
    return out


def strip(record):
    return {k: v for k, v in record.items() if k != "name"}


CASES = {
    "format": 1,
    "note": (
        "The storage conformance cases of docs/storage-conformance.md: scenarios for the "
        "candle, journal and segment ports, each step with what a conforming store returns, "
        "and the dirty-window races each backend steps through. Written by "
        "spikes/storage-contract/conformance_cases.py, not by the Rust code under test."
    ),
    "markets": {
        key: {"exchange": exchange, "symbol": symbol, "interval": interval}
        for key, (exchange, symbol, interval) in MARKETS.items()
    },
    "series_rule": SERIES_RULE,
}


def main() -> int:
    check_packer()
    cases = dict(CASES)
    cases["runs"] = {key: strip(value) for key, value in RUNS.items()}
    cases["orders"] = {key: strip(value) for key, value in ORDERS.items()}
    cases["trades"] = {key: strip(value) for key, value in TRADES.items()}
    cases["candles"] = [s.out() for s in candle_scenarios()]
    cases["journal"] = [s.out() for s in journal_scenarios()]
    cases["segments"] = [s.out() for s in segment_scenarios()]
    cases["races"] = races()
    cases["control"] = [s.out() for s in control_scenarios()]
    groups = ("candles", "journal", "segments", "races", "control")
    ids = [s["id"] for group in groups for s in cases[group]]
    if len(ids) != len(set(ids)):
        raise SystemExit("scenario ids repeat")
    OUT.write_text(json.dumps(cases, indent=1, ensure_ascii=False) + "\n")
    counts = ", ".join(f"{len(cases[g])} {g}" for g in groups)
    print(f"wrote {OUT.relative_to(ROOT)}: {counts}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
