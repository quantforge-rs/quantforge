#!/usr/bin/env python3
"""The execution store's cases of docs/execution-store.md, written
independently of the Rust code under test.

Every scenario runs on the reference model of execution_model.py, which
spells each record as a store keeps it and decides every unit, lease call
and operation in the order the document fixes, and records what each step
must answer. Each scenario also says what it expects of the steps that
matter, so the model and the scenario's author have to agree before
anything is written.

Run with ``python3 -I spikes/storage-contract/execution_cases.py`` from the
repository root; it writes tests/fixtures/execution_cases.json. Stdlib
only.
"""

import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from execution_model import (  # noqa: E402
    ACCOUNT, FINGERPRINT, INTENT_TERMINAL, RECORDED_INTENT_STATES, REPLACE_STATES,
    REPLACE_TERMINAL, ROOT, T, Alert, Line, Store, Unit, conflict_event, cycle_record,
    event_record, fill_record, incident_record, intent_record, judged, moved, operation_record,
    order_record, order_ref, position_of, replacement, reservation_record, scope_columns,
    signal_event, signal_record, snapshot, venue_order,
)

OUT = ROOT / "tests" / "fixtures" / "execution_cases.json"
FORMAT = 1

# One venue account with two profiles on its linear line and one on its
# inverse line, the same profile name and symbol on another venue, the
# first account again on testnet, and its Spot line, which has no profile.
SYMBOLS = {"SOLUSDT": "USDT", "BTCUSDT": "USDT", "ETHUSDT": "USDT", "XRPUSDT": "USDT"}
BYBIT = Line("bybit", "linear", "demo", "venue", "71002233", "main", "k3x9p2", SYMBOLS)
SECOND = BYBIT.other_profile("second", "a1b2c3")
BINANCE = Line("binance", "linear", "demo", "key_fingerprint", FINGERPRINT, "main", "zzzzzz",
               {"SOLUSDT": "USDT"})
COIN = Line("bybit", "inverse", "demo", "venue", "71002233", "coin", "000000", {"BTCUSD": "BTC"})
TESTNET = Line("bybit", "linear", "testnet", "venue", "71002233", "main", "k3x9p2", SYMBOLS)
SPOT = Line("bybit", "spot", "demo", "venue", "71002233", None, None,
            {"BTCUSDT": "USDT", "BTC/USDT": "USDT", "BTC": "USDT"})
LINES = {"bybit": BYBIT, "bybit second": SECOND, "binance": BINANCE, "bybit coin": COIN,
         "bybit testnet": TESTNET, "bybit spot": SPOT}

USDT = {"Asset": "USDT"}
BTC = {"Asset": "BTC"}


class Names:
    """What the fixture names once and its steps refer to: scopes,
    accounts, positions, and every record in its stored form."""

    def __init__(self):
        self.records = {}
        self.counts = {}
        self.positions = {}

    def line(self, line):
        return next(name for name, known in LINES.items() if known is line)

    def record(self, record):
        known = (record.kind, record.text)
        if known not in self.records:
            self.counts[record.kind] = self.counts.get(record.kind, 0) + 1
            self.records[known] = f"{record.kind}-{self.counts[record.kind]}"
        return self.records[known]

    def position(self, line, symbol):
        name = f"{self.line(line)}/{symbol}"
        self.positions[name] = line.key(symbol)
        return name

    def position_named(self, at):
        return next(name for name, key in self.positions.items() if position_of(key) == at)


NAMES = Names()


def at(line, symbol):
    """The position a line's symbol is traded on, as the model keys it."""
    NAMES.position(line, symbol)
    return position_of(line.key(symbol))


def match(**equals):
    return lambda row: all(row[name] == value for name, value in equals.items())


def scoped(line, **more):
    return match(**scope_columns(line.scope()), **more)


def of_account(line, **more):
    columns = scope_columns(line.scope())
    return match(**{name: columns[name] for name in ACCOUNT}, **more)


class Scenario:
    """One scenario: its steps, each with the answer the model gives."""

    def __init__(self, identifier, doc, bound="demo", active_at=None):
        self.id = identifier
        self.doc = doc
        self.steps = []
        self.store = Store()
        if bound is not None:
            self.bind(bound)
        if active_at is not None:
            self.activate(active_at)

    def step(self, op, **fields):
        self.steps.append({"op": op, **fields})

    def expected(self, got, want, what):
        if want is not None and got != want:
            raise SystemExit(f"{self.id}: {what} is {got!r}, and the scenario says {want!r}")

    # The store's record.

    def bind(self, environment):
        assert self.store.bind(environment) is None
        self.step("bind", environment=environment)

    def activate(self, at_ms):
        assert self.store.change_activation("active", at_ms) is None
        self.step("activate", at_ms=at_ms)

    def deactivate(self, at_ms):
        assert self.store.change_activation("deactivated", at_ms) is None
        self.step("deactivate", at_ms=at_ms)

    def reopen(self):
        self.step("reopen")

    def raw_select(self, sql, expect):
        self.step("raw_select", sql=sql, expect=expect)

    def count(self, table, expect):
        got = len(self.store.rows[table])
        self.expected(got, expect, f"the rows of {table}")
        self.raw_select(f"SELECT CAST(COUNT(*) AS TEXT) FROM {table}", [[str(got)]])

    def column(self, table, column, expect=None):
        """One column of every row of a table, in the order they were
        numbered, as text."""
        first = next(iter(self.store.rows[table][0])) if self.store.rows[table] else "seq"
        got = [[None if row[column] is None else str(row[column])]
               for row in self.store.rows[table]]
        if expect is not None:
            self.expected([value for (value,) in got], expect, f"{table}.{column}")
        self.raw_select(f"SELECT CAST({column} AS TEXT) FROM {table} ORDER BY {first}", got)

    # Leases.

    def lease_json(self, lease):
        if lease is None:
            return None
        return {**lease, "position": NAMES.position_named(lease["position"])}

    def fence_json(self, fence):
        position, holder, fence_token = fence
        return {"position": NAMES.position_named(position), "holder": holder,
                "token": fence_token}

    def acquire(self, line, symbol, holder, now_ms, expect, ttl_ms=30_000):
        """Asks for a position's lease; answers its fence where granted."""
        where = at(line, symbol)
        kind, answer = self.store.acquire(where, line.profile, holder, ttl_ms, now_ms)
        self.expected(kind, expect, f"the lease asked by {holder}")
        asked = {"position": NAMES.position(line, symbol), "profile": line.profile,
                 "holder": holder, "ttl_ms": ttl_ms, "now_ms": now_ms}
        if kind == "refused":
            self.step("acquire_lease", **asked, refused=answer)
            return None
        self.step("acquire_lease", **asked, expect={kind: self.lease_json(answer)})
        return (where, answer["holder"], answer["fence"]) if kind == "granted" else None

    def renew(self, fence, now_ms, expect, ttl_ms=30_000):
        kind, answer = self.store.renew(fence, ttl_ms, now_ms)
        asked = {"fence": self.fence_json(fence), "ttl_ms": ttl_ms, "now_ms": now_ms}
        if kind == "refused":
            self.expected("refused", expect, "the renewal")
            self.step("renew_lease", **asked, refused=answer)
            return
        self.expected("renewed" if answer else "not its holder's", expect, "the renewal")
        self.step("renew_lease", **asked, expect=self.lease_json(answer))

    def release(self, fence, expect):
        got = self.store.release(fence)
        self.expected(got, expect, "the release")
        self.step("release_lease", fence=self.fence_json(fence), expect=got)

    def lease(self, line, symbol):
        held = self.store.lease(at(line, symbol))
        self.step("lease", position=NAMES.position(line, symbol), expect=self.lease_json(held))

    # Units.

    def stored_json(self, found):
        if found is None:
            return None
        record, revision = found
        return {"record": NAMES.record(record), "revision": revision}

    def commit(self, unit, expect):
        """Commits a unit; `expect` names what the scenario means it to
        come to: applied, duplicate, conflict, stale, fenced or refused."""
        answer = self.store.commit(unit)
        kind = answer if isinstance(answer, str) else next(iter(answer))
        self.expected(kind, expect, "the unit")
        asked = {
            "at_ms": unit.at_ms,
            "fence": None if unit.fence is None else self.fence_json(unit.fence),
            "pool": unit.pool,
            "delivery": None if unit.delivery is None else {
                "record": NAMES.record(unit.delivery[0]),
                "conflict_event": NAMES.record(unit.delivery[1]),
            },
            "changes": [{"record": NAMES.record(record), "expect": expected}
                        for record, expected in unit.changes],
        }
        if kind == "refused":
            self.step("commit", unit=asked, refused=answer["refused"])
        elif kind in ("duplicate", "conflict"):
            held = answer[kind]
            stored = self.stored_json((held["record"], held["revision"]))
            self.step("commit", unit=asked, expect={kind: stored})
        else:
            self.step("commit", unit=asked, expect=answer)
        return answer

    def begin(self, line, kind, symbol, at_ms, expect, key=None, signal=None, replaces=None):
        """Asks for an operation's number; answers it."""
        answer = self.store.begin_operation(line, kind, symbol, at_ms, key=key, signal=signal,
                                            replaces=replaces)
        asked = {"scope": NAMES.line(line), "kind": kind, "key": key,
                 "instrument": line.instrument(symbol), "signal": signal,
                 "replacement": replaces}
        if answer[0] == "refused":
            self.expected("refused", expect, "the operation")
            self.step("begin_operation", operation=asked, at_ms=at_ms, refused=answer[1])
            return None
        _, record, revision = answer
        self.expected(record.doc["id"], expect, "the operation's number")
        self.step("begin_operation", operation=asked, at_ms=at_ms,
                  expect=self.stored_json((record, revision)))
        return record

    # Reads.

    def read_one(self, op, kind, where, expect_state=None, **asked):
        found = self.store.one(kind, where)
        self.step(op, **asked, expect=self.stored_json(found))
        return found

    def read_many(self, op, kind, where, expect, limit=None, **asked):
        found = self.store.stored(kind, where, limit)
        self.expected(len(found), expect, f"what {op} lists")
        if limit is not None:
            asked["limit"] = limit
        self.step(op, **asked, expect=[self.stored_json(one) for one in found])
        return found

    def signal(self, line, alert):
        return self.read_one("signal", "signal", scoped(line, signal_id=alert.id),
                             scope=NAMES.line(line), signal=alert.id)

    def received_signals(self, limit, expect):
        return self.read_many("received_signals", "signal", match(state="received"), expect,
                              limit=limit)

    def deliveries(self, line, alert, expect):
        rows = [row for row in self.store.rows["signal_deliveries"]
                if scoped(line, signal_id=alert.id)(row)]
        self.expected([row["disposition"] for row in rows], expect, "the deliveries")
        members = ("disposition", "payload_hash", "payload", "fire_time_ms", "received_at_ms")
        self.step("deliveries", scope=NAMES.line(line), signal=alert.id,
                  expect=[{name: row[name] for name in members} for row in rows])

    def intent(self, line, alert):
        return self.read_one("intent", "intent", scoped(line, signal_id=alert.id),
                             scope=NAMES.line(line), signal=alert.id)

    def position_intents(self, line, symbol, expect):
        where = at(line, symbol)
        unsettled = RECORDED_INTENT_STATES - INTENT_TERMINAL
        names = ("venue", "category", "environment", "account_kind", "account_id", "symbol")
        return self.read_many(
            "position_intents", "intent",
            lambda row: tuple(row[n] for n in names) == where and row["state"] in unsettled,
            expect, position=NAMES.position(line, symbol))

    def unsettled_intents(self, expect):
        unsettled = RECORDED_INTENT_STATES - INTENT_TERMINAL
        return self.read_many("unsettled_intents", "intent",
                              lambda row: row["state"] in unsettled, expect)

    def operation(self, number):
        return self.read_one("operation", "operation", match(operation_id=number), id=number)

    def unsettled_replacements(self, expect):
        unsettled = REPLACE_STATES - REPLACE_TERMINAL
        return self.read_many("unsettled_replacements", "operation",
                              lambda row: row["replace_state"] in unsettled, expect)

    def order(self, line, client_order_id):
        return self.read_one("order", "order",
                             of_account(line, client_order_id=client_order_id),
                             scope=NAMES.line(line), client_order_id=client_order_id)

    def orders(self, operation, expect):
        return self.read_many("orders", "order", match(operation_id=operation), expect,
                              operation=operation)

    def fill(self, line, symbol, trade):
        found = self.store.one("fill", of_account(line, symbol=symbol, trade_id=trade))
        self.step("fill", scope=NAMES.line(line), instrument=line.instrument(symbol),
                  trade=trade, expect=None if found is None else NAMES.record(found[0]))

    def fills(self, cycle, expect):
        found = self.store.stored("fill", match(cycle_id=cycle))
        self.expected(len(found), expect, "the cycle's fills")
        self.step("fills", cycle=cycle, expect=[NAMES.record(record) for record, _ in found])

    def cycle(self, number):
        return self.read_one("cycle", "cycle", match(cycle_id=number), id=number)

    def open_cycle(self, line, symbol):
        where = at(line, symbol)
        names = ("venue", "category", "environment", "account_kind", "account_id", "symbol")
        return self.read_one(
            "open_cycle", "cycle",
            lambda row: tuple(row[n] for n in names) == where and row["open_slot"] == 1,
            position=NAMES.position(line, symbol))

    def open_cycles(self, expect):
        return self.read_many("open_cycles", "cycle", match(open_slot=1), expect)

    def pool(self, line, pool, expect):
        """A pool's revision and what is held against it; `expect` is the
        revision and how many reservations are held."""
        held = self.store.pool(line.account(), pool)
        self.expected((held["revision"], len(held["held"])), expect, "the pool")
        self.step("pool", account=NAMES.line(line), pool=pool, expect={
            "revision": held["revision"],
            "held": [NAMES.record(record) for record in held["held"]],
            "total": held["total"],
        })
        return held["revision"]

    def reservation(self, operation):
        return self.read_one("reservation", "reservation", match(operation_id=operation),
                             operation=operation)

    def incident(self, line, key):
        return self.read_one("incident", "incident", of_account(line, incident_key=key),
                             scope=NAMES.line(line), key=key)

    def open_incidents(self, expect):
        return self.read_many("open_incidents", "incident", match(state="open"), expect)

    def event(self, line, key):
        return self.read_one("event", "event", of_account(line, event_key=key),
                             scope=NAMES.line(line), key=key)

    def pending_events(self, limit, expect):
        return self.read_many("pending_events", "event", match(delivery="pending"), expect,
                              limit=limit)

    def json(self):
        return {"id": self.id, "doc": self.doc, "steps": self.steps}

    def unit_json(self, unit):
        """A unit as a step names it, without an answer."""
        self.commit(unit, None)
        return self.steps.pop()["unit"]


# --- What the scenarios are made of -------------------------------------------


def entry(line, source="trend", symbol="SOLUSDT", minute=0, price="234.23", stop="200.00",
          **more):
    return Alert(line, "entry", source, symbol, minute, price=price, stop=stop, **more)


def exit_(line, source="trend", symbol="SOLUSDT", minute=1):
    return Alert(line, "exit", source, symbol, minute)


def admitted(line, alert, at_ms):
    """A signal as the unit that admits it on arrival records it."""
    return signal_record(line, alert, at_ms, state="admitted", judged_at_ms=at_ms)


def admission(line, alert, at_ms, cycle=None):
    """The unit that records a delivery and admits it: the signal, its
    intent and the event that tells of it, in one write."""
    return Unit(
        at_ms,
        changes=[(intent_record(line, alert, cycle=cycle), None),
                 (signal_event(line, alert, "signal_admitted", at_ms), None)],
        delivery=(admitted(line, alert, at_ms), conflict_event(line, alert, at_ms)),
    )


def operation_key(line, alert, ordinal=1):
    return f"sig/{line.profile}/{ordinal}/{alert.id}"


def started(s, line, alert, at_ms, number, kind="entry", cycle=None):
    """Admits a signal and numbers its operation; answers its intent."""
    s.commit(admission(line, alert, at_ms, cycle=cycle), "applied")
    s.begin(line, kind, alert.symbol, at_ms, number, key=operation_key(line, alert),
            signal=alert.id)
    return intent_record(line, alert, cycle=cycle)


def submitted(line, intent, client_order_id, at_ms):
    """An intent's first attempt: the intent sent, and its order's record."""
    sent = moved(intent, "submitting", attempt=(client_order_id, at_ms))
    symbol = sent.doc["intent"]["kind"][next(iter(sent.doc["intent"]["kind"]))]["key"][
        "instrument"]["symbol"]
    order = order_record(line, sent.doc["operation"], client_order_id, symbol, at_ms)
    return sent, order


# --- The scenarios ------------------------------------------------------------


def scenarios():
    out = []

    s = Scenario(
        "execution.delivery_recorded_once",
        "the first delivery of a signal is recorded with its admission, its intent and its event in one write; the same payload again changes nothing but the note of it, and another payload under the signal's ID is a conflict, noted with its payload and told once for each payload",
    )
    alert = entry(BYBIT)
    s.commit(admission(BYBIT, alert, T + 6_000), "applied")
    s.signal(BYBIT, alert)
    s.intent(BYBIT, alert)
    s.commit(admission(BYBIT, alert, T + 7_000), "duplicate")
    altered = entry(BYBIT, price="999.99")
    assert altered.id == alert.id and altered.hash != alert.hash
    s.commit(admission(BYBIT, altered, T + 8_000), "conflict")
    s.commit(admission(BYBIT, altered, T + 9_000), "conflict")
    third = entry(BYBIT, price="111.11", stop=None)
    s.commit(admission(BYBIT, third, T + 10_000), "conflict")
    s.deliveries(BYBIT, alert, ["new", "duplicate", "conflict", "conflict", "conflict"])
    s.pending_events(10, 3)
    s.count("signal_inbox", 1)
    s.count("execution_intents", 1)
    s.column("signal_inbox", "revision", ["1"])
    s.reopen()
    s.signal(BYBIT, alert)
    s.intent(BYBIT, alert)
    s.deliveries(BYBIT, alert, None)
    out.append(s)

    s = Scenario(
        "execution.received_then_judged",
        "a delivery may be recorded as received and judged later, once: admitted with its intent in the same write, rejected with its refusal, or asking for nothing; received signals are listed oldest first until they are judged",
    )
    first, late, nothing = entry(BYBIT), entry(BYBIT, source="late", minute=2), exit_(BYBIT)
    received = [signal_record(BYBIT, alert, T + 1_000 + index)
                for index, alert in enumerate((first, late, nothing))]
    for alert, record in zip((first, late, nothing), received):
        arrival = record.doc["received_at_ms"]
        s.commit(Unit(arrival, delivery=(record, conflict_event(BYBIT, alert, arrival))),
                 "applied")
    s.received_signals(10, 3)
    s.received_signals(2, 2)
    s.unsettled_intents(0)
    s.commit(Unit(T + 2_000, changes=[
        (judged(received[0], "admitted", T + 2_000), 1),
        (intent_record(BYBIT, first), None),
        (signal_event(BYBIT, first, "signal_admitted", T + 2_000), None),
    ]), "applied")
    stale = ("stale", "the alert fired 75 s ago, and profile main takes one at most 60 s old")
    s.commit(Unit(T + 3_000, changes=[
        (judged(received[1], "rejected", T + 3_000, refusal=stale), 1),
        (signal_event(BYBIT, late, "signal_rejected", T + 3_000), None),
    ]), "applied")
    note = "profile main holds no long position on SOLUSDT that this exit is for"
    s.commit(Unit(T + 4_000, changes=[(judged(received[2], "noop", T + 4_000, note=note), 1)]),
             "applied")
    s.received_signals(10, 0)
    for alert in (first, late, nothing):
        s.signal(BYBIT, alert)
    s.intent(BYBIT, first)
    s.intent(BYBIT, late)
    # Judged once: named at its new revision, a second judgment is no successor.
    again = judged(received[0], "rejected", T + 5_000, refusal=stale)
    s.commit(Unit(T + 5_000, changes=[(again, 1)]), "stale")
    s.commit(Unit(T + 5_000, changes=[(again, 2)]), "refused")
    s.commit(Unit(T + 5_000, changes=[(judged(received[2], "noop", T + 5_000, note=note), 2)]),
             "refused")
    s.column("signal_inbox", "state", ["admitted", "rejected", "noop"])
    s.column("signal_inbox", "refusal", [None, "stale", None])
    s.column("signal_inbox", "judged_at_ms", None)
    out.append(s)

    s = Scenario(
        "execution.unit_shape_refusals",
        "a unit that is not one a store applies is refused before anything is read: one that changes nothing, admits a signal without its intent or records an intent without its admission, or with the admission of another account's signal of the same name, first records a signal or an operation by a change, replaces a fill, changes a record twice, by its name or under the key its account knows it by, changes a position without its lease or two positions at once, reserves without the revision of its pool, or gives its position two unresolved entries or two open cycles",
    )
    alert = entry(BYBIT)
    intent = intent_record(BYBIT, alert)
    told = conflict_event(BYBIT, alert, T)
    sol, btc = at(BYBIT, "SOLUSDT"), at(BYBIT, "BTCUSDT")
    lease = (sol, "executor-a", 1)
    first = BYBIT.client_order_id(1, 1, "entry")
    order = order_record(BYBIT, 1, first, "SOLUSDT", T)
    elsewhere = order_record(BYBIT, 2, BYBIT.client_order_id(1, 2, "entry"), "BTCUSDT", T)
    fill = fill_record(BYBIT, "SOLUSDT", "t-1", first, "2", "234.25", "0.0512", T, operation=1)
    held = reservation_record(BYBIT, 1, "SOLUSDT", "205.18548", "149.9274", T)
    in_usd = reservation_record(BYBIT, 2, "SOLUSDT", "100.00", "149.9274", T, pool="AccountUsd")
    event = signal_event(BYBIT, alert, "signal_admitted", T)
    sent, _ = submitted(BYBIT, intent, first, T)
    rival = intent_record(SECOND, entry(SECOND, source="other"))
    rival_sent, _ = submitted(SECOND, rival, SECOND.client_order_id(1, 2, "entry"), T)
    for unit in (
        Unit(T),
        Unit(T, delivery=(admitted(BYBIT, alert, T), told)),
        Unit(T, changes=[(intent, None)]),
        Unit(T, changes=[(signal_record(BYBIT, alert, T), None)]),
        Unit(T, changes=[(operation_record(1, BYBIT, "entry", "SOLUSDT"), None)]),
        Unit(T, changes=[(fill, 1)], fence=lease),
        Unit(T, changes=[(event, None), (event, None)]),
        Unit(T, changes=[(order, None)]),
        Unit(T, changes=[(order, None)], fence=(btc, "executor-a", 1)),
        Unit(T, changes=[(order, None), (elsewhere, None)], fence=lease),
        Unit(T, changes=[(held, None)], fence=lease),
        Unit(T, changes=[(order, None)], fence=lease, pool=0),
        Unit(T, changes=[(held, None), (in_usd, None)], fence=lease, pool=0),
        Unit(T, delivery=(signal_record(BYBIT, alert, T), event)),
        # The other venue's profile has the same name and takes the same
        # alert: its intent is not the one this signal's admission records.
        Unit(T, delivery=(admitted(BYBIT, alert, T), told),
             changes=[(intent_record(BINANCE, entry(BINANCE)), None)]),
        Unit(T, changes=[(sent, 1), (rival_sent, 1)], fence=lease),
        Unit(T, changes=[(cycle_record(BYBIT, "SOLUSDT", 1, T), None),
                         (cycle_record(SECOND, "SOLUSDT", 2, T), None)], fence=lease),
        # An event is known within its account: under one key, the same
        # account's other profile names the same record.
        Unit(T, changes=[(event, None),
                         (event_record(SECOND, event.doc["key"], "signal_admitted", T,
                                       symbol=alert.symbol, signal=alert.id), None)]),
    ):
        s.commit(unit, "refused")
    for table in ("signal_inbox", "signal_deliveries", "execution_intents", "execution_orders",
                  "outbox_events"):
        s.count(table, 0)
    out.append(s)

    s = Scenario(
        "execution.lease_fences_writers",
        "a position's lease goes to one holder at a time under the next token; a holder asking again keeps its token; a lease that ran out still fences until another holder takes it, and from then on nothing its former holder writes is applied, renewed or released",
        active_at=T,
    )
    alert = entry(BYBIT)
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    first = BYBIT.client_order_id(1, 1, "entry")
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    s.acquire(BYBIT, "SOLUSDT", "executor-b", T + 3_000, "held")
    assert s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 10_000, "granted") == a
    s.renew(a, T + 20_000, "renewed")
    s.renew(a, T + 20_000, "refused", ttl_ms=0)
    s.acquire(BYBIT, "SOLUSDT", "executor-b", T + 20_000, "refused", ttl_ms=0)
    s.lease(BYBIT, "SOLUSDT")
    sent, order = submitted(BYBIT, intent, first, T + 21_000)
    s.commit(Unit(T + 21_000, changes=[(sent, 1), (order, None)], fence=a), "applied")
    # One millisecond before its end the lease is still its holder's.
    s.acquire(BYBIT, "SOLUSDT", "executor-b", T + 49_999, "held")
    # Past its end, and taken by no one: its holder's writes still apply.
    acknowledged = order.but(venue_order_id="8389765519")
    s.commit(Unit(T + 55_000, changes=[(acknowledged, 1)], fence=a), "applied")
    b = s.acquire(BYBIT, "SOLUSDT", "executor-b", T + 56_000, "granted")
    assert b[2] == 2
    reported = acknowledged.but(evidence=venue_order(
        BYBIT, "SOLUSDT", first, "new", "4.38", "0", T + 57_000))
    s.commit(Unit(T + 57_000, changes=[(reported, 2)], fence=a), "fenced")
    s.renew(a, T + 57_000, "not its holder's")
    s.release(a, False)
    s.order(BYBIT, first)
    s.commit(Unit(T + 58_000, changes=[(reported, 2)], fence=b), "applied")
    s.release(b, True)
    s.release(b, False)
    s.lease(BYBIT, "SOLUSDT")
    s.commit(Unit(T + 59_000, changes=[(moved(sent, "outcome_unknown"), 2)], fence=b), "fenced")
    c = s.acquire(BYBIT, "SOLUSDT", "executor-c", T + 60_000, "granted")
    assert c[2] == 3
    # Another position's lease is its own, and ends at its end to the millisecond.
    other = s.acquire(BYBIT, "BTCUSDT", "executor-b", T + 60_000, "granted")
    assert other[2] == 1
    s.acquire(BYBIT, "BTCUSDT", "executor-c", T + 89_999, "held")
    s.acquire(BYBIT, "BTCUSDT", "executor-c", T + 90_000, "granted")
    s.reopen()
    s.lease(BYBIT, "SOLUSDT")
    s.lease(BYBIT, "BTCUSDT")
    out.append(s)

    s = Scenario(
        "execution.activation_gates_execution",
        "a store that is not active records deliveries and numbers operations, and grants no lease; a deactivation stops every unit written under a lease at once, and the activation after it voids every lease, for its own holder too, so the next grant of each position takes the token after its last",
    )
    alert = entry(BYBIT)
    s.acquire(BYBIT, "SOLUSDT", "executor-a", T, "refused")
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    sol = at(BYBIT, "SOLUSDT")
    first = BYBIT.client_order_id(1, 1, "entry")
    sent, order = submitted(BYBIT, intent, first, T + 2_000)
    writes = [(sent, 1), (order, None)]
    s.commit(Unit(T + 2_000, changes=writes, fence=(sol, "executor-a", 1)), "fenced")
    s.activate(T + 10_000)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 11_000, "granted")
    s.deactivate(T + 20_000)
    s.commit(Unit(T + 21_000, changes=writes, fence=a), "fenced")
    s.renew(a, T + 21_000, "not its holder's")
    s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 21_000, "refused")
    s.lease(BYBIT, "SOLUSDT")
    s.activate(T + 30_000)
    s.lease(BYBIT, "SOLUSDT")
    s.commit(Unit(T + 31_000, changes=writes, fence=a), "fenced")
    s.renew(a, T + 31_000, "not its holder's")
    s.release(a, False)
    again = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 32_000, "granted")
    assert again[2] == 2
    s.commit(Unit(T + 33_000, changes=writes, fence=a), "fenced")
    s.commit(Unit(T + 33_000, changes=writes, fence=again), "applied")
    s.intent(BYBIT, alert)
    s.count("execution_orders", 1)
    out.append(s)

    s = Scenario(
        "execution.compare_and_swap_all_or_nothing",
        "a change names what it expects to find, no record or the revision it replaces, and a unit one of whose changes finds anything else writes nothing and answers what was found",
        active_at=T,
    )
    alert = entry(BYBIT)
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    first = BYBIT.client_order_id(1, 1, "entry")
    sent, order = submitted(BYBIT, intent, first, T + 3_000)
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, 3)], fence=a), "stale")
    s.intent(BYBIT, alert)
    s.count("execution_orders", 0)
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None)], fence=a), "applied")
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None)], fence=a), "stale")
    s.commit(Unit(T + 3_500, changes=[(order, None)], fence=a), "stale")
    acknowledged = order.but(venue_order_id="8389765519")
    told = event_record(BYBIT, "op/1/submitted:1", "order_submitted", T + 4_000,
                        symbol="SOLUSDT", signal=alert.id, operation=1,
                        facts={"client_order_id": first, "leg": "entry"})
    s.commit(Unit(T + 4_000, changes=[(told, None), (acknowledged, 2)], fence=a), "stale")
    s.event(BYBIT, "op/1/submitted:1")
    s.commit(Unit(T + 4_000, changes=[(told, None), (acknowledged, 1)], fence=a), "applied")
    s.commit(Unit(T + 5_000, changes=[(told, None)]), "stale")
    s.order(BYBIT, first)
    s.column("execution_intents", "revision", ["2"])
    s.column("execution_orders", "revision", ["2"])
    s.column("execution_intents", "created_at_ms", [str(T + 1_000)])
    s.column("execution_intents", "updated_at_ms", [str(T + 3_000)])
    s.column("execution_orders", "updated_at_ms", [str(T + 4_000)])
    out.append(s)

    s = Scenario(
        "execution.successor_rules",
        "a record is replaced only by the same record moved as its kind allows: an intent along its state machine with its policy, its attempts and its cycle kept, an order with nothing it had learned taken back, a cycle's protection along its machine, a reservation released once, an incident cleared once, an event's delivery ended once; anything else is refused and changes nothing",
        active_at=T,
    )
    alert = entry(BYBIT)
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    first = BYBIT.client_order_id(1, 1, "entry")
    sent, order = submitted(BYBIT, intent, first, T + 3_000)
    held = reservation_record(BYBIT, 1, "SOLUSDT", "205.18548", "149.9274", T + 3_000)
    opened = cycle_record(BYBIT, "SOLUSDT", 1, T + 4_000)
    acknowledged = order.but(venue_order_id="8389765519")
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None), (held, None)], fence=a, pool=0),
             "applied")
    filled = moved(sent, "filled", cycle=1)
    s.commit(Unit(T + 4_000, changes=[(filled, 2), (acknowledged, 1), (opened, None)], fence=a),
             "applied")
    released = held.but(released={"reason": "Opened", "at_ms": T + 5_000})
    s.commit(Unit(T + 5_000, changes=[(released, 1)], fence=a, pool=1), "applied")
    closed = opened.within("cycle", protection="Closed", close_reason="ExitSignal",
                           closed_at_ms=T + 6_000)
    s.commit(Unit(T + 6_000, changes=[(closed, 1)], fence=a), "applied")
    incident = incident_record(BYBIT, "op/1/unsettled_entry", "unsettled_entry",
                               "the entry of operation 1 was still unsettled 60 s after it was sent",
                               T + 7_000, symbol="SOLUSDT", operation=1)
    told = event_record(BYBIT, "incident/op/1/unsettled_entry", "incident_opened", T + 7_000,
                        facts={"incident": "op/1/unsettled_entry"})
    s.commit(Unit(T + 7_000, changes=[(incident, None), (told, None)]), "applied")
    cleared = incident.but(cleared={"at_ms": T + 8_000, "note": "flat, checked by hand"})
    delivered = told.but(delivery="Delivered", delivery_at_ms=T + 8_000)
    s.commit(Unit(T + 8_000, changes=[(cleared, 1), (delivered, 1)]), "applied")
    other_policy = snapshot(BYBIT, "SOLUSDT", revision=5)
    for change in (
        (moved(filled, "submitting"), 3),
        (filled.within("intent", policy=other_policy), 3),
        (filled.within("intent", attempts=[{"client_order_id": first, "sent_at_ms": T}]), 3),
        (filled.but(cycle=2), 3),
        (order, 2),
        (acknowledged.but(recorded_at_ms=T), 2),
        (opened, 2),
        (closed.within("cycle", side="Short"), 2),
        (released, 2),
        (released.but(risk={"amount": "1", "asset": "USDT"}), 2),
        (incident, 2),
        (cleared.but(message="another cause"), 2),
        (delivered, 2),
        (told.but(facts={"incident": "another"}, delivery="Delivered",
                  delivery_at_ms=T + 9_000), 2),
    ):
        record = change[0]
        fenced = record.kind in ("intent", "order", "cycle", "reservation")
        pool = 2 if record.kind == "reservation" else None
        s.commit(Unit(T + 9_000, changes=[change], fence=a if fenced else None, pool=pool),
                 "refused")
    # A venue's final report of an order stands: one that says the order
    # is still working is an older one arriving late.
    done = acknowledged.but(evidence=venue_order(BYBIT, "SOLUSDT", first, "filled", "2", "2",
                                                 T + 10_000))
    s.commit(Unit(T + 10_000, changes=[(done, 2)], fence=a), "applied")
    working = acknowledged.but(evidence=venue_order(BYBIT, "SOLUSDT", first, "partially_filled",
                                                    "2", "1", T + 11_000))
    s.commit(Unit(T + 11_000, changes=[(working, 3)], fence=a), "refused")
    s.commit(Unit(T + 11_000, changes=[(acknowledged, 3)], fence=a), "refused")
    s.intent(BYBIT, alert)
    s.order(BYBIT, first)
    s.cycle(1)
    s.reservation(1)
    s.incident(BYBIT, "op/1/unsettled_entry")
    s.event(BYBIT, "incident/op/1/unsettled_entry")
    s.pool(BYBIT, USDT, (2, 0))
    out.append(s)

    s = Scenario(
        "execution.one_unresolved_entry_and_one_open_cycle",
        "a position has one unresolved entry and one open cycle at a time, whoever's profile acts on it: an entry that was sent holds the slot until it is settled, a cycle until it is closed, and another position of the same account has slots of its own",
        active_at=T,
    )
    mine, theirs = entry(BYBIT), entry(SECOND, source="other")
    intent = started(s, BYBIT, mine, T + 1_000, 1)
    rival = started(s, SECOND, theirs, T + 1_500, 2)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    sent, order = submitted(BYBIT, intent, BYBIT.client_order_id(1, 1, "entry"), T + 3_000)
    theirs_sent, theirs_order = submitted(SECOND, rival, SECOND.client_order_id(1, 2, "entry"),
                                          T + 3_500)
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None)], fence=a), "applied")
    blocked = Unit(T + 3_500, changes=[(theirs_sent, 1), (theirs_order, None)], fence=a)
    s.commit(blocked, "refused")
    s.position_intents(BYBIT, "SOLUSDT", 2)
    unknown = moved(sent, "outcome_unknown")
    s.commit(Unit(T + 4_000, changes=[(unknown, 2)], fence=a), "applied")
    s.commit(blocked, "refused")
    opened = cycle_record(BYBIT, "SOLUSDT", 1, T + 5_000)
    s.commit(Unit(T + 5_000, changes=[(moved(unknown, "filled", cycle=1), 3), (opened, None)],
                  fence=a), "applied")
    s.commit(blocked, "applied")
    second_cycle = cycle_record(SECOND, "SOLUSDT", 2, T + 6_000)
    pyramid = Unit(T + 6_000, changes=[(moved(theirs_sent, "filled", cycle=2), 2),
                                       (second_cycle, None)], fence=a)
    s.commit(pyramid, "refused")
    s.open_cycle(BYBIT, "SOLUSDT")
    closed = opened.within("cycle", protection="Closed", close_reason="StopLoss",
                           closed_at_ms=T + 7_000)
    s.commit(Unit(T + 7_000, changes=[(closed, 1)], fence=a), "applied")
    s.commit(pyramid, "applied")
    s.open_cycle(BYBIT, "SOLUSDT")
    s.open_cycles(1)
    s.cycle(1)
    s.position_intents(BYBIT, "SOLUSDT", 0)
    # The account's other position is not held up by any of it.
    elsewhere = entry(BYBIT, symbol="BTCUSDT")
    other = started(s, BYBIT, elsewhere, T + 8_000, 3)
    b = s.acquire(BYBIT, "BTCUSDT", "executor-a", T + 8_000, "granted")
    other_sent, other_order = submitted(BYBIT, other, BYBIT.client_order_id(1, 3, "entry"),
                                        T + 8_500)
    s.commit(Unit(T + 8_500, changes=[(other_sent, 1), (other_order, None)], fence=b),
             "applied")
    s.column("execution_intents", "entry_slot", [None, None, "1"])
    s.column("position_cycles", "open_slot", [None, "1"])
    out.append(s)

    s = Scenario(
        "execution.reservations_and_the_pool",
        "a unit that reserves or releases margin names the revision it read the pool at, and is applied only while the pool still stands there; every reservation and every release moves the pool on, each pool of each venue account has a revision of its own, and what is held against a pool is added up exactly",
        active_at=T,
    )
    sol = started(s, BYBIT, entry(BYBIT), T + 1_000, 1)
    btc = started(s, BYBIT, entry(BYBIT, symbol="BTCUSDT"), T + 1_500, 2)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    b = s.acquire(BYBIT, "BTCUSDT", "executor-b", T + 2_000, "granted")
    assert s.pool(BYBIT, USDT, (0, 0)) == 0
    sol_sent, sol_order = submitted(BYBIT, sol, BYBIT.client_order_id(1, 1, "entry"), T + 3_000)
    sol_held = reservation_record(BYBIT, 1, "SOLUSDT", "205.18548", "149.9274", T + 3_000)
    s.commit(Unit(T + 3_000, changes=[(sol_sent, 1), (sol_order, None), (sol_held, None)],
                  fence=a, pool=0), "applied")
    btc_sent, btc_order = submitted(BYBIT, btc, BYBIT.client_order_id(1, 2, "entry"), T + 3_500)
    btc_held = reservation_record(BYBIT, 2, "BTCUSDT", "410.1", "149.9274", T + 3_500)
    sized_early = [(btc_sent, 1), (btc_order, None), (btc_held, None)]
    s.commit(Unit(T + 3_500, changes=sized_early, fence=b, pool=0), "stale")
    s.count("execution_orders", 1)
    assert s.pool(BYBIT, USDT, (1, 1)) == 1
    s.commit(Unit(T + 3_500, changes=sized_early, fence=b, pool=1), "applied")
    s.pool(BYBIT, USDT, (2, 2))
    released = sol_held.but(released={"reason": "Opened", "at_ms": T + 4_000})
    s.commit(Unit(T + 4_000, changes=[(released, 1)], fence=a, pool=1), "stale")
    s.commit(Unit(T + 4_000, changes=[(released, 1)], fence=a, pool=2), "applied")
    s.pool(BYBIT, USDT, (3, 1))
    s.reservation(1)
    s.reservation(2)
    s.reservation(9)
    # The same account's margin stated in USD, and its coin-margined line's
    # in the coin, are pools of their own; so is another venue's.
    in_usd = reservation_record(BYBIT, 1, "SOLUSDT", "100.00", "149.9274", T + 5_000,
                                pool="AccountUsd")
    s.commit(Unit(T + 5_000, changes=[(in_usd, 1)], fence=a, pool=0), "stale")
    c = s.acquire(COIN, "BTCUSD", "executor-c", T + 5_000, "granted")
    coin_held = reservation_record(COIN, 7, "BTCUSD", "0.00150000", "0.00075000", T + 5_500)
    s.commit(Unit(T + 5_500, changes=[(coin_held, None)], fence=c, pool=0), "applied")
    s.pool(COIN, BTC, (1, 1))
    s.pool(BYBIT, BTC, (1, 1))
    s.pool(BYBIT, "AccountUsd", (0, 0))
    s.pool(BINANCE, USDT, (0, 0))
    s.column("margin_reservations", "pool", ["asset:USDT", "asset:USDT", "asset:BTC"])
    s.column("margin_reservations", "state", ["released", "held", "held"])
    # A total is exact however many digits it takes: this one has 29
    # significant ones, one more than a decimal sum keeps by default.
    fine = reservation_record(COIN, 8, "BTCUSD", "1.0000000000000000000000000001", "0.00075000",
                              T + 6_000)
    s.commit(Unit(T + 6_000, changes=[(fine, None)], fence=c, pool=1), "applied")
    assert s.store.pool(COIN.account(), BTC)["total"] == "1.0015000000000000000000000001"
    s.pool(COIN, BTC, (2, 2))
    out.append(s)

    s = Scenario(
        "execution.operations_numbered_once",
        "the store numbers every operation, and an operation asked for again under its key is the one already numbered, as it stands now; a key names one operation of its account for good; a Spot replacement is an operation with no signal, no profile and no position, whose state moves only as the matrix allows and whose orders are never renamed, by their client ID or by the venue's once it is known",
    )
    alert, leaving = entry(BYBIT), exit_(BYBIT)
    key = operation_key(BYBIT, alert)
    first = s.begin(BYBIT, "entry", "SOLUSDT", T + 1_000, 1, key=key, signal=alert.id)
    assert s.begin(BYBIT, "entry", "SOLUSDT", T + 2_000, 1, key=key, signal=alert.id) == first
    s.begin(BYBIT, "close", "SOLUSDT", T + 3_000, 2, key=operation_key(BYBIT, leaving),
            signal=leaving.id)
    s.begin(BYBIT, "stop", "SOLUSDT", T + 4_000, 3)
    s.begin(BYBIT, "stop", "SOLUSDT", T + 4_000, 4)
    s.begin(BYBIT, "entry", "BTCUSDT", T + 5_000, "refused", key=key, signal=alert.id)
    s.begin(BYBIT, "close", "SOLUSDT", T + 5_000, "refused", key=key, signal=alert.id)
    s.begin(BINANCE, "entry", "SOLUSDT", T + 6_000, 5, key=key, signal=alert.id)
    s.operation(1)
    s.operation(9)
    old = order_ref("qf-old-1")
    replace = s.begin(SPOT, "spot_replace", "BTCUSDT", T + 7_000, 6, key="cmd/4f9a",
                      replaces=replacement(old))
    s.begin(SPOT, "spot_replace", "BTCUSDT", T + 8_000, 6, key="cmd/4f9a",
            replaces=replacement(old))
    s.begin(SPOT, "spot_replace", "BTCUSDT", T + 8_000, "refused", key="cmd/4f9a",
            replaces=replacement(order_ref("qf-old-2")))
    s.unsettled_replacements(1)
    unknown = replace.but(replacement=replacement(old, "outcome_unknown",
                                                  order_ref("qf-new-1")))
    s.commit(Unit(T + 9_000, changes=[(unknown, 1)]), "applied")
    s.unsettled_replacements(1)
    accepted = replace.but(replacement=replacement(old, "accepted",
                                                   order_ref("qf-new-1", "9001")))
    s.commit(Unit(T + 10_000, changes=[(accepted, 2)]), "applied")
    s.unsettled_replacements(0)
    for wrong in (
        replace.but(replacement=replacement(old, "pending", order_ref("qf-new-1", "9001"))),
        replace.but(replacement=replacement(old, "accepted", order_ref("qf-new-2", "9001"))),
        replace.but(replacement=replacement(order_ref("qf-old-2"), "accepted",
                                            order_ref("qf-new-1", "9001"))),
        # The venue's ID of the replacing order, once learned, neither
        # changes nor goes.
        replace.but(replacement=replacement(old, "accepted", order_ref("qf-new-1", "9002"))),
        replace.but(replacement=replacement(old, "accepted", order_ref("qf-new-1"))),
    ):
        s.commit(Unit(T + 11_000, changes=[(wrong, 3)]), "refused")
    # Asked again once it has moved on, the key answers the operation as it stands.
    s.begin(SPOT, "spot_replace", "BTCUSDT", T + 12_000, 6, key="cmd/4f9a",
            replaces=replacement(old))
    bound = first.but(cycle=1)
    s.commit(Unit(T + 13_000, changes=[(bound, 1)]), "applied")
    s.commit(Unit(T + 14_000, changes=[(first.but(cycle=2), 2)]), "refused")
    s.commit(Unit(T + 14_000, changes=[(first.but(signal=leaving.id, cycle=1), 2)]), "refused")
    assert s.begin(BYBIT, "entry", "SOLUSDT", T + 15_000, 1, key=key, signal=alert.id) == bound
    s.column("execution_operations", "operation_id", ["1", "2", "3", "4", "5", "6"])
    s.column("execution_operations", "replace_state",
             [None, None, None, None, None, "accepted"])
    s.column("execution_operations", "cycle_id", ["1", None, None, None, None, None])
    s.column("execution_operations", "profile_id",
             ["main", "main", "main", "main", "main", None])
    s.column("execution_operations", "created_at_ms", None)
    s.column("execution_operations", "updated_at_ms", None)
    out.append(s)

    s = Scenario(
        "execution.two_venues_one_alert",
        "the same alert, byte for byte, sent to profiles of one name that trade one symbol on two venues is two signals: each has its own inbox entry, intent, operation, lease, pool, orders, cycle and events, what happens to one changes nothing of the other, and one unit may change both",
        active_at=T,
    )
    here, there = entry(BYBIT), entry(BINANCE)
    assert (here.id, here.payload) == (there.id, there.payload)
    mine = started(s, BYBIT, here, T + 1_000, 1)
    yours = started(s, BINANCE, there, T + 1_000, 2)
    s.commit(admission(BYBIT, entry(BYBIT, price="999.99"), T + 2_000), "conflict")
    s.deliveries(BYBIT, here, ["new", "conflict"])
    s.deliveries(BINANCE, there, ["new"])
    s.signal(BYBIT, here)
    s.signal(BINANCE, there)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 3_000, "granted")
    b = s.acquire(BINANCE, "SOLUSDT", "executor-b", T + 3_000, "granted")
    assert (a[2], b[2]) == (1, 1)
    mine_sent, mine_order = submitted(BYBIT, mine, BYBIT.client_order_id(1, 1, "entry"),
                                      T + 4_000)
    yours_id = BINANCE.client_order_id(1, 2, "entry")
    yours_sent, yours_order = submitted(BINANCE, yours, yours_id, T + 4_000)
    s.commit(Unit(T + 4_000, changes=[
        (mine_sent, 1), (mine_order, None),
        (reservation_record(BYBIT, 1, "SOLUSDT", "205.18548", "149.9274", T + 4_000), None),
    ], fence=a, pool=0), "applied")
    # Each line's lease is its own: neither holder writes the other's position.
    crossed = Unit(T + 4_000, changes=[(yours_sent, 1), (yours_order, None)],
                   fence=(b[0], "executor-a", 1))
    s.commit(crossed, "fenced")
    s.commit(Unit(T + 4_000, changes=[
        (yours_sent, 1), (yours_order, None),
        (reservation_record(BINANCE, 2, "SOLUSDT", "205.18548", "149.9274", T + 4_000), None),
    ], fence=b, pool=0), "applied")
    s.pool(BYBIT, USDT, (1, 1))
    s.pool(BINANCE, USDT, (1, 1))
    s.commit(Unit(T + 5_000, changes=[(moved(mine_sent, "outcome_unknown"), 2),
                                      (mine_order.but(outcome_unknown=True), 1)], fence=a),
             "applied")
    report = venue_order(BINANCE, "SOLUSDT", yours_id, "filled", "4.38", "4.38", T + 5_000)
    s.commit(Unit(T + 5_000, changes=[
        (moved(yours_sent, "filled", cycle=2), 2),
        (yours_order.but(venue_order_id="8389765519", evidence=report), 1),
        (fill_record(BINANCE, "SOLUSDT", "t-1", yours_id, "4.38", "234.25", "0.0512",
                     T + 4_900, operation=2, cycle=2), None),
        (cycle_record(BINANCE, "SOLUSDT", 2, T + 4_900), None),
    ], fence=b), "applied")
    s.position_intents(BYBIT, "SOLUSDT", 1)
    s.position_intents(BINANCE, "SOLUSDT", 0)
    s.unsettled_intents(1)
    s.open_cycle(BYBIT, "SOLUSDT")
    s.open_cycle(BINANCE, "SOLUSDT")
    s.order(BYBIT, yours_id)
    s.order(BINANCE, yours_id)
    s.fill(BYBIT, "SOLUSDT", "t-1")
    s.fill(BINANCE, "SOLUSDT", "t-1")
    # One event key on both lines, and one trade ID: a row for each.
    for line in (BYBIT, BINANCE):
        s.event(line, f"sig/main/signal_admitted/{here.id}")
    s.pending_events(10, 3)
    s.column("signal_inbox", "venue", ["bybit", "binance"])
    s.column("execution_intents", "entry_slot", ["1", None])
    # One unit may change both lines' records of one name: they are two
    # records, each known within its own account and profile.
    later_here = entry(BYBIT, source="later", minute=3)
    later_there = entry(BINANCE, source="later", minute=3)
    assert later_here.id == later_there.id
    arrived = []
    for line, alert in ((BYBIT, later_here), (BINANCE, later_there)):
        record = signal_record(line, alert, T + 6_000)
        s.commit(Unit(T + 6_000, delivery=(record, conflict_event(line, alert, T + 6_000))),
                 "applied")
        arrived.append(record)
    stale = ("stale", "the alert fired 75 s ago, and profile main takes one at most 60 s old")
    s.commit(Unit(T + 7_000, changes=[
        (judged(arrived[0], "rejected", T + 7_000, refusal=stale), 1),
        (judged(arrived[1], "rejected", T + 7_000, refusal=stale), 1),
        (signal_event(BYBIT, later_here, "signal_rejected", T + 7_000), None),
        (signal_event(BINANCE, later_there, "signal_rejected", T + 7_000), None),
    ]), "applied")
    s.signal(BYBIT, later_here)
    s.signal(BINANCE, later_there)
    s.column("signal_inbox", "state", ["admitted", "admitted", "rejected", "rejected"])
    out.append(s)

    s = Scenario(
        "execution.orders_fills_and_cycles",
        "an order's identity is recorded before it is sent and then only learns: its venue ID, the venue's report, each fill once; an entry's first fill opens the position's cycle, numbered as the operation that opened it, an exit sent again is another order of its operation, and a fill is known by its symbol and its trade ID, each for itself",
        active_at=T,
    )
    alert = entry(BYBIT)
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted", ttl_ms=600_000)
    first = BYBIT.client_order_id(1, 1, "entry")
    sent, order = submitted(BYBIT, intent, first, T + 3_000)
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None)], fence=a), "applied")
    s.orders(1, 1)
    acknowledged = order.but(venue_order_id="8389765519")
    s.commit(Unit(T + 3_500, changes=[(acknowledged, 1)], fence=a), "applied")
    partial = acknowledged.but(evidence=venue_order(
        BYBIT, "SOLUSDT", first, "partially_filled", "4.38", "2", T + 4_000))
    one = fill_record(BYBIT, "SOLUSDT", "t-1", first, "2", "234.25", "0.0512", T + 4_000,
                      operation=1, cycle=1)
    opened = cycle_record(BYBIT, "SOLUSDT", 1, T + 4_000)
    carried = s.operation(1)[0]
    exposed = moved(sent, "partially_filled", cycle=1)
    s.commit(Unit(T + 4_500, changes=[
        (opened, None), (carried.but(cycle=1), 1), (exposed, 2), (partial, 2), (one, None),
    ], fence=a), "applied")
    s.fills(1, 1)
    s.fill(BYBIT, "SOLUSDT", "t-1")
    s.fill(BYBIT, "SOLUSDT", "t-9")
    # Partly filled, the entry is no longer on its way: its position is
    # held by the cycle its fill opened, not by the entry's slot.
    s.column("execution_intents", "entry_slot", [None])
    s.column("execution_intents", "state", ["partially_filled"])
    s.column("position_cycles", "open_slot", ["1"])
    s.position_intents(BYBIT, "SOLUSDT", 1)
    s.commit(Unit(T + 5_000, changes=[(one, None)], fence=a), "stale")
    whole = partial.but(evidence=venue_order(
        BYBIT, "SOLUSDT", first, "filled", "4.38", "4.38", T + 5_000))
    two = fill_record(BYBIT, "SOLUSDT", "t-2", first, "2.38", "234.30", "0.0609", T + 5_000,
                      operation=1, cycle=1)
    s.commit(Unit(T + 5_500, changes=[(moved(exposed, "filled"), 3), (whole, 3), (two, None)],
                  fence=a), "applied")
    s.fills(1, 2)
    s.order(BYBIT, first)
    s.open_cycle(BYBIT, "SOLUSDT")
    protected = opened.within("cycle", protection="Protected")
    s.commit(Unit(T + 6_000, changes=[(protected, 1)], fence=a), "applied")
    # The exit that closes it: bound to the cycle when it is admitted, and
    # sent twice under its own operation.
    leaving = exit_(BYBIT)
    waiting = started(s, BYBIT, leaving, T + 60_000, 2, kind="close", cycle=1)
    close_1, close_2 = (BYBIT.client_order_id(1, 2, "close", attempt) for attempt in (1, 2))
    closing, close_order = submitted(BYBIT, waiting, close_1, T + 61_000)
    s.commit(Unit(T + 61_000, changes=[(closing, 1), (close_order, None)], fence=a), "applied")
    unknown = moved(closing, "outcome_unknown")
    s.commit(Unit(T + 62_000, changes=[(unknown, 2), (close_order.but(outcome_unknown=True), 1)],
                  fence=a), "applied")
    retried = moved(unknown, "outcome_unknown", attempt=(close_2, T + 73_000))
    again = order_record(BYBIT, 2, close_2, "SOLUSDT", T + 73_000)
    s.commit(Unit(T + 73_000, changes=[(retried, 3), (again, None)], fence=a), "applied")
    s.orders(2, 2)
    s.position_intents(BYBIT, "SOLUSDT", 1)
    done = venue_order(BYBIT, "SOLUSDT", close_2, "filled", "4.38", "4.38", T + 74_000,
                       venue_order_id="8389765600", side="Sell", reduce_only=True)
    closed = protected.within("cycle", protection="Closed", close_reason="ExitSignal",
                              closed_at_ms=T + 74_500)
    s.commit(Unit(T + 75_000, changes=[
        (moved(retried, "filled"), 4),
        (again.but(venue_order_id="8389765600", evidence=done), 1),
        (fill_record(BYBIT, "SOLUSDT", "t-3", close_2, "4.38", "240.00", "0.0631", T + 74_000,
                     operation=2, cycle=1, venue_order_id="8389765600", side="Sell"), None),
        (closed, 2),
    ], fence=a), "applied")
    s.fills(1, 3)
    s.open_cycle(BYBIT, "SOLUSDT")
    s.open_cycles(0)
    s.cycle(1)
    s.cycle(2)
    s.unsettled_intents(0)
    s.column("execution_orders", "phase", ["reported", "outcome_unknown", "reported"])
    s.column("execution_orders", "venue_order_id", ["8389765519", None, "8389765600"])
    s.column("execution_fills", "recorded_at_ms",
             [str(T + 4_500), str(T + 5_500), str(T + 75_000)])
    s.column("execution_fills", "time_ms", [str(T + 4_000), str(T + 5_000), str(T + 74_000)])
    s.column("position_cycles", "protection", ["closed"])
    s.column("position_cycles", "closed_at_ms", [str(T + 74_500)])
    s.column("execution_intents", "operation_id", ["1", "2"])
    s.column("execution_intents", "cycle_id", ["1", "1"])
    # A fill is known by two things, its symbol and its trade ID. Joined
    # with a stroke these two read alike, and they are two fills.
    alike = [fill_record(SPOT, "BTC/USDT", "7", None, "0.5", "67000.00", "0.0335", T + 80_000),
             fill_record(SPOT, "BTC", "USDT/7", None, "0.5", "67000.00", "0.0335", T + 80_000)]
    s.commit(Unit(T + 80_000, changes=[(fill, None) for fill in alike]), "applied")
    s.fill(SPOT, "BTC/USDT", "7")
    s.fill(SPOT, "BTC", "USDT/7")
    out.append(s)

    s = Scenario(
        "execution.incidents_and_the_outbox",
        "an incident is raised once under its key and cleared once, with the event that tells of it written in the same unit; pending events are listed oldest first up to a limit, and an event leaves the list when its delivery ended, delivered or not known to be",
    )
    why = "the entry of operation 1 was still unsettled 60 s after it was sent"
    here = incident_record(BYBIT, "op/1/unsettled_entry", "unsettled_entry", why, T + 1_000,
                           symbol="SOLUSDT", operation=1)
    everywhere = incident_record(BYBIT, "profile/second-writer", "second_writer",
                                 "orders with owner tag k3x9p2 and another store's epoch",
                                 T + 2_000)
    events = [
        event_record(BYBIT, f"incident/{key}", "incident_opened", at_ms, facts={"incident": key})
        for key, at_ms in (("op/1/unsettled_entry", T + 1_000),
                           ("profile/second-writer", T + 2_000))
    ]
    s.commit(Unit(T + 1_000, changes=[(here, None), (events[0], None)]), "applied")
    s.commit(Unit(T + 2_000, changes=[(everywhere, None), (events[1], None)]), "applied")
    s.commit(Unit(T + 2_500, changes=[(here, None), (events[0], None)]), "stale")
    s.open_incidents(2)
    cleared = here.but(cleared={"at_ms": T + 3_000, "note": "flat, checked by hand"})
    told = event_record(BYBIT, "incident/op/1/unsettled_entry/cleared", "incident_cleared",
                        T + 3_000, facts={"incident": "op/1/unsettled_entry",
                                          "note": "flat, checked by hand"})
    s.commit(Unit(T + 3_000, changes=[(cleared, 1), (told, None)]), "applied")
    s.open_incidents(1)
    s.incident(BYBIT, "op/1/unsettled_entry")
    s.incident(BYBIT, "no/such")
    s.incident(BINANCE, "op/1/unsettled_entry")
    s.pending_events(10, 3)
    s.pending_events(2, 2)
    s.commit(Unit(T + 4_000, changes=[
        (events[0].but(delivery="Delivered", delivery_at_ms=T + 4_000), 1)]), "applied")
    s.commit(Unit(T + 5_000, changes=[
        (events[1].but(delivery="OutcomeUnknown", delivery_at_ms=T + 5_000), 1)]), "applied")
    s.pending_events(10, 1)
    s.event(BYBIT, "incident/op/1/unsettled_entry")
    s.event(BYBIT, "incident/profile/second-writer")
    s.event(BYBIT, "no/such")
    s.column("execution_incidents", "state", ["cleared", "open"])
    s.column("execution_incidents", "symbol", ["SOLUSDT", None])
    s.column("execution_incidents", "cleared_at_ms", [str(T + 3_000), None])
    s.column("outbox_events", "delivery", ["delivered", "outcome_unknown", "pending"])
    s.column("outbox_events", "delivery_at_ms", [str(T + 4_000), str(T + 5_000), None])
    out.append(s)

    s = Scenario(
        "execution.environment_of_the_store",
        "a store keeps the executions of the environment it is bound to and of no other, and of none before it is bound: a unit, an operation and a lease of another environment are refused, and nothing of them is written",
        bound=None,
    )
    alert = entry(BYBIT)
    s.commit(admission(BYBIT, alert, T), "refused")
    # A delivery on its own, recorded before it is judged, is held to it too.
    s.commit(Unit(T, delivery=(signal_record(BYBIT, alert, T), conflict_event(BYBIT, alert, T))),
             "refused")
    s.begin(BYBIT, "entry", "SOLUSDT", T, "refused", key=operation_key(BYBIT, alert),
            signal=alert.id)
    s.bind("demo")
    elsewhere = entry(TESTNET)
    s.commit(admission(TESTNET, elsewhere, T + 1_000), "refused")
    s.commit(Unit(T + 1_000, delivery=(signal_record(TESTNET, elsewhere, T + 1_000),
                                       conflict_event(TESTNET, elsewhere, T + 1_000))),
             "refused")
    s.count("signal_deliveries", 0)
    s.begin(TESTNET, "entry", "SOLUSDT", T + 1_000, "refused",
            key=operation_key(TESTNET, elsewhere), signal=elsewhere.id)
    s.activate(T + 2_000)
    s.acquire(TESTNET, "SOLUSDT", "executor-a", T + 3_000, "refused")
    s.count("signal_inbox", 0)
    s.count("execution_operations", 0)
    s.commit(admission(BYBIT, alert, T + 4_000), "applied")
    s.begin(BYBIT, "entry", "SOLUSDT", T + 4_000, 1, key=operation_key(BYBIT, alert),
            signal=alert.id)
    s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 5_000, "granted")
    s.count("signal_inbox", 1)
    out.append(s)

    s = Scenario(
        "execution.unsettled_work_listed_for_recovery",
        "every intent that is not settled is listed, oldest first, for whoever starts on the store, and a position's own are listed for whoever holds its lease; an intent leaves both lists when it is settled, and a store opened again lists what the one before it left",
        active_at=T,
    )
    sol = started(s, BYBIT, entry(BYBIT), T + 1_000, 1)
    btc = started(s, BYBIT, entry(BYBIT, symbol="BTCUSDT"), T + 1_100, 2)
    far = started(s, BINANCE, entry(BINANCE), T + 1_200, 3)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted")
    b = s.acquire(BYBIT, "BTCUSDT", "executor-a", T + 2_000, "granted")
    c = s.acquire(BINANCE, "SOLUSDT", "executor-a", T + 2_000, "granted")
    sol_sent, sol_order = submitted(BYBIT, sol, BYBIT.client_order_id(1, 1, "entry"), T + 3_000)
    btc_sent, btc_order = submitted(BYBIT, btc, BYBIT.client_order_id(1, 2, "entry"), T + 3_000)
    far_sent, far_order = submitted(BINANCE, far, BINANCE.client_order_id(1, 3, "entry"),
                                    T + 3_000)
    s.commit(Unit(T + 3_000, changes=[(sol_sent, 1), (sol_order, None)], fence=a), "applied")
    s.commit(Unit(T + 3_000, changes=[(btc_sent, 1), (btc_order, None)], fence=b), "applied")
    s.commit(Unit(T + 3_000, changes=[(far_sent, 1), (far_order, None)], fence=c), "applied")
    s.commit(Unit(T + 4_000, changes=[(moved(btc_sent, "outcome_unknown"), 2),
                                      (btc_order.but(outcome_unknown=True), 1)], fence=b),
             "applied")
    refused = ("insufficient_balance", "api error: code=Some(110007) message=ab not enough")
    s.commit(Unit(T + 4_000, changes=[(moved(far_sent, "rejected"), 2),
                                      (far_order.but(refusal={"kind": refused[0],
                                                              "message": refused[1]}), 1)],
                  fence=c), "applied")
    waiting = exit_(BYBIT)
    s.commit(admission(BYBIT, waiting, T + 5_000), "applied")
    s.unsettled_intents(3)
    s.position_intents(BYBIT, "SOLUSDT", 2)
    s.position_intents(BYBIT, "BTCUSDT", 1)
    s.position_intents(BINANCE, "SOLUSDT", 0)
    s.reopen()
    s.unsettled_intents(3)
    s.position_intents(BYBIT, "SOLUSDT", 2)
    s.lease(BYBIT, "SOLUSDT")
    s.commit(Unit(T + 40_000, changes=[(moved(moved(btc_sent, "outcome_unknown"), "not_placed"),
                                        3)], fence=b), "applied")
    s.unsettled_intents(2)
    s.position_intents(BYBIT, "BTCUSDT", 0)
    s.column("execution_intents", "state",
             ["submitting", "not_placed", "rejected", "admitted"])
    out.append(s)

    s = Scenario(
        "execution.inverse_contracts_keep_their_units",
        "a coin-margined line keeps its own units exactly: whole contracts for quantities, the coin for margin, risk and fees, each decimal with the scale it was written with, and a signal that names its exchange and family keeps both",
        active_at=T,
    )
    alert = entry(COIN, symbol="BTCUSD", price="67012.5", stop="65000.0", exchange="BYBIT",
                  category="inverse")
    capital = snapshot(COIN, "BTCUSD", capital=("0.75000000", "BTC"))
    s.commit(Unit(T + 1_000, changes=[
        (intent_record(COIN, alert, policy=capital), None),
        (signal_event(COIN, alert, "signal_admitted", T + 1_000), None),
    ], delivery=(admitted(COIN, alert, T + 1_000), conflict_event(COIN, alert, T + 1_000))),
        "applied")
    s.begin(COIN, "entry", "BTCUSD", T + 1_000, 1, key=operation_key(COIN, alert),
            signal=alert.id)
    a = s.acquire(COIN, "BTCUSD", "executor-a", T + 2_000, "granted")
    first = COIN.client_order_id(1, 1, "entry")
    sent, order = submitted(COIN, intent_record(COIN, alert, policy=capital), first, T + 3_000)
    held = reservation_record(COIN, 1, "BTCUSD", "0.00150000", "0.00075000", T + 3_000)
    s.commit(Unit(T + 3_000, changes=[(sent, 1), (order, None), (held, None)], fence=a, pool=0),
             "applied")
    report = venue_order(COIN, "BTCUSD", first, "filled", "100", "100", T + 4_000)
    s.commit(Unit(T + 4_500, changes=[
        (moved(sent, "filled", cycle=1), 2),
        (order.but(venue_order_id="8389765519", evidence=report), 1),
        (fill_record(COIN, "BTCUSD", "t-1", first, "100", "67010.0", "0.00000090", T + 4_000,
                     operation=1, cycle=1), None),
        (cycle_record(COIN, "BTCUSD", 1, T + 4_000), None),
    ], fence=a), "applied")
    s.signal(COIN, alert)
    s.intent(COIN, alert)
    s.order(COIN, first)
    s.fills(1, 1)
    s.pool(COIN, BTC, (1, 1))
    s.reservation(1)
    s.open_cycle(COIN, "BTCUSD")
    s.column("margin_reservations", "pool", ["asset:BTC"])
    s.column("execution_intents", "category", ["inverse"])
    out.append(s)

    return out


# --- The races ----------------------------------------------------------------


def races():
    """What writers at once are handed: a store set up step by step, and
    what each writer asks of it. Who wins is not the model's to say; what
    holds whoever wins is the statement, which the suite checks."""
    out = []

    def race(identifier, doc, setup, writers):
        out.append({"id": identifier, "doc": doc, "setup": setup.steps, "writers": writers})

    def units(scenario, made):
        """Units as the writers' steps, asked of a copy of the store so
        that none of them changes what the next is decided on."""
        steps = []
        for unit in made:
            probe = Scenario(scenario.id, scenario.doc, bound=None)
            probe.store = scenario.store
            before = {table: [dict(row) for row in rows]
                      for table, rows in scenario.store.rows.items()}
            numbers, pools = dict(scenario.store.numbers), dict(scenario.store.pools)
            steps.append({"op": "commit", "unit": probe.unit_json(unit)})
            scenario.store.rows, scenario.store.numbers = before, numbers
            scenario.store.pools = pools
        return steps

    alert = entry(BYBIT)
    s = Scenario("execution.first_deliveries_racing", "")
    race(
        s.id,
        "of eight deliveries of one signal at once, one is recorded with its intent and its event, and seven are duplicates that name it: the inbox holds one signal, one intent and one event, and a note of all eight",
        s,
        units(s, [admission(BYBIT, alert, T + 6_000 + arrival) for arrival in range(8)]),
    )

    altered = entry(BYBIT, price="999.99")
    s = Scenario("execution.conflicting_deliveries_racing", "")
    race(
        s.id,
        "of eight deliveries at once under one signal ID, four with one payload and four with another, one is recorded; the three that brought its payload are duplicates and the four that brought the other are conflicts, told once",
        s,
        units(s, [admission(BYBIT, payload, T + 6_000 + arrival)
                  for arrival, payload in enumerate([alert, altered] * 4)]),
    )

    s = Scenario("execution.same_change_racing", "", active_at=T)
    intent = started(s, BYBIT, alert, T + 1_000, 1)
    a = s.acquire(BYBIT, "SOLUSDT", "executor-a", T + 2_000, "granted", ttl_ms=600_000)
    sent, order = submitted(BYBIT, intent, BYBIT.client_order_id(1, 1, "entry"), T + 3_000)
    race(
        s.id,
        "of four writers that hold one lease and write the same change at once, one is applied and three find the intent already moved: the order is recorded once",
        s,
        units(s, [Unit(T + 3_000, changes=[(sent, 1), (order, None)], fence=a)] * 4),
    )

    s = Scenario("execution.leases_racing", "", active_at=T)
    NAMES.position(BYBIT, "SOLUSDT")
    race(
        s.id,
        "of eight holders asking for one position's lease at once, one is granted it under token 1 and seven are told who holds it",
        s,
        [{"op": "acquire_lease", "position": NAMES.position(BYBIT, "SOLUSDT"),
          "profile": BYBIT.profile, "holder": f"executor-{number}", "ttl_ms": 600_000,
          "now_ms": T + 1_000} for number in range(8)],
    )

    s = Scenario("execution.reservations_racing", "", active_at=T)
    writers = []
    for number, symbol in enumerate(SYMBOLS, start=1):
        lease = s.acquire(BYBIT, symbol, f"executor-{number}", T + 1_000, "granted",
                          ttl_ms=600_000)
        held = reservation_record(BYBIT, number, symbol, f"{100 * number}.50", "149.9274",
                                  T + 2_000)
        writers.extend(units(s, [Unit(T + 2_000, changes=[(held, None)], fence=lease, pool=0)]))
    race(
        s.id,
        "four writers, each on a position of its own, reserve from one pool at once, each sizing again when the pool has moved: every reservation is made once, and the pool ends four revisions on, holding all four",
        s,
        writers,
    )

    s = Scenario("execution.operations_racing", "")
    key = operation_key(BYBIT, alert)

    def asked(key):
        return {"op": "begin_operation", "at_ms": T + 1_000, "operation": {
            "scope": NAMES.line(BYBIT), "kind": "entry", "key": key,
            "instrument": BYBIT.instrument("SOLUSDT"), "signal": alert.id, "replacement": None}}

    race(
        s.id,
        "of eight askers for an operation under one key at once, all are answered with the same number, and of four askers under keys of their own each gets a number no other has",
        s,
        [asked(key)] * 8 + [asked(f"{key}/{number}") for number in range(4)],
    )
    return out


# --- A store for a transfer to copy --------------------------------------------


def transfer_rows():
    """The execution tables of a store on testnet that has lived a little:
    signals of every outcome, an entry filled and its exit unknown, an
    entry refused, an unresolved entry on a coin-margined line with its
    margin held, incidents, events and a Spot replacement, on two venues.
    Answers every transferred table's rows, in the order of the DDL."""
    main = Line("bybit", "linear", "testnet", "venue", "71002233", "main", "k3x9p2", SYMBOLS)
    coin = Line("bybit", "inverse", "testnet", "key_fingerprint", FINGERPRINT, "main-2",
                "a1b2c3", {"BTCUSD": "BTC"})
    spot = Line("binance", "spot", "testnet", "key_fingerprint", FINGERPRINT, None, None,
                {"BTCUSDT": "USDT"})
    store = Store()
    assert store.bind("testnet") is None and store.change_activation("active", T) is None

    def must(answer, want="applied"):
        got = answer if isinstance(answer, str) else next(iter(answer))
        assert got == want, (answer, want)

    def lease(line, symbol, holder, now_ms):
        kind, held = store.acquire(position_of(line.key(symbol)), line.profile, holder,
                                   600_000, now_ms)
        assert kind == "granted"
        return (held["position"], held["holder"], held["fence"])

    def number(line, alert, kind, at_ms):
        answer = store.begin_operation(line, kind, alert.symbol, at_ms,
                                       key=operation_key(line, alert), signal=alert.id)
        return answer[1].doc["id"]

    # An entry: delivered, sent again, sent with another payload; sent with
    # its margin reserved, filled in two fills.
    alert = entry(main)
    must(store.commit(admission(main, alert, T + 6_000)))
    must(store.commit(admission(main, alert, T + 6_500)), "duplicate")
    must(store.commit(admission(main, entry(main, price="999.99"), T + 7_000)), "conflict")
    number(main, alert, "entry", T + 7_000)
    a = lease(main, "SOLUSDT", "executor-a", T + 7_000)
    first = main.client_order_id(1, 1, "entry")
    sent, order = submitted(main, intent_record(main, alert), first, T + 8_000)
    held = reservation_record(main, 1, "SOLUSDT", "205.18548", "149.9274", T + 8_000)
    must(store.commit(Unit(T + 8_000, changes=[(sent, 1), (order, None), (held, None)], fence=a,
                           pool=0)))
    opened = cycle_record(main, "SOLUSDT", 1, T + 9_000)
    partial = order.but(venue_order_id="8389765519", evidence=venue_order(
        main, "SOLUSDT", first, "partially_filled", "4.38", "2", T + 9_000))
    carried = store.one("operation", match(operation_id=1))[0]
    exposed = moved(sent, "partially_filled", cycle=1)
    must(store.commit(Unit(T + 9_500, changes=[
        (opened, None), (carried.but(cycle=1), 1), (exposed, 2), (partial, 1),
        (fill_record(main, "SOLUSDT", "t-1", first, "2", "234.25", "0.0512", T + 9_000,
                     operation=1, cycle=1), None),
    ], fence=a)))
    whole = partial.but(evidence=venue_order(
        main, "SOLUSDT", first, "filled", "4.38", "4.38", T + 10_000))
    must(store.commit(Unit(T + 10_500, changes=[
        (moved(exposed, "filled"), 3), (whole, 2),
        (fill_record(main, "SOLUSDT", "t-2", first, "2.38", "234.30", "0.0609", T + 10_000,
                     operation=1, cycle=1), None),
        (held.but(released={"reason": "Opened", "at_ms": T + 10_500}), 1),
        (opened.within("cycle", protection="Protected"), 1),
    ], fence=a, pool=1)))
    # Its exit: bound to the cycle, sent, and not known to have arrived.
    leaving = exit_(main)
    must(store.commit(admission(main, leaving, T + 70_000, cycle=1)))
    number(main, leaving, "close", T + 70_000)
    close = main.client_order_id(1, 2, "close")
    closing, close_order = submitted(main, intent_record(main, leaving, cycle=1), close,
                                     T + 71_000)
    must(store.commit(Unit(T + 71_000, changes=[(closing, 1), (close_order, None)], fence=a)))
    must(store.commit(Unit(T + 72_000, changes=[
        (moved(closing, "outcome_unknown"), 2), (close_order.but(outcome_unknown=True), 1),
        (event_record(main, "op/2/unknown:1", "outcome_unknown", T + 72_000, symbol="SOLUSDT",
                      signal=leaving.id, operation=2, cycle=1,
                      facts={"client_order_id": close, "state": "outcome_unknown"}), None),
    ], fence=a)))
    # An entry the venue refused: its margin is given back.
    other = entry(main, symbol="BTCUSDT")
    must(store.commit(admission(main, other, T + 80_000)))
    number(main, other, "entry", T + 80_000)
    b = lease(main, "BTCUSDT", "executor-b", T + 80_000)
    third = main.client_order_id(1, 3, "entry")
    tried, tried_order = submitted(main, intent_record(main, other), third, T + 80_500)
    wanted = reservation_record(main, 3, "BTCUSDT", "410.1", "149.9274", T + 80_500)
    must(store.commit(Unit(T + 80_500, changes=[(tried, 1), (tried_order, None),
                                                (wanted, None)], fence=b, pool=2)))
    refused = {"kind": "insufficient_balance",
               "message": "api error: code=Some(110007) message=ab not enough for new order"}
    must(store.commit(Unit(T + 81_000, changes=[
        (moved(tried, "rejected"), 2), (tried_order.but(refusal=refused), 1),
        (wanted.but(released={"reason": "NotOpened", "at_ms": T + 81_000}), 1),
    ], fence=b, pool=3)))
    # A signal rejected, one that asks for nothing, and one not judged yet.
    late, nothing = entry(main, source="late", minute=2), exit_(main, symbol="BTCUSDT", minute=3)
    stale = ("stale", "the alert fired 75 s ago, and profile main takes one at most 60 s old")
    for alert_, record in (
        (late, signal_record(main, late, T + 82_000, state="rejected", refusal=stale,
                             judged_at_ms=T + 82_000)),
        (nothing, signal_record(
            main, nothing, T + 83_000, state="noop", judged_at_ms=T + 83_000,
            note="profile main holds no long position on BTCUSDT that this exit is for")),
        (entry(main, source="queued", minute=4),
         signal_record(main, entry(main, source="queued", minute=4), T + 84_000)),
    ):
        arrival = record.doc["received_at_ms"]
        must(store.commit(Unit(arrival, delivery=(record,
                                                  conflict_event(main, alert_, arrival)))))
    # A coin-margined entry, sent and unresolved: it holds its position's
    # slot and its margin.
    hedge = entry(coin, symbol="BTCUSD", price="67012.5", stop="65000.0", exchange="BYBIT",
                  category="inverse")
    capital = snapshot(coin, "BTCUSD", capital=("0.75000000", "BTC"))
    held_intent = intent_record(coin, hedge, policy=capital)
    must(store.commit(Unit(T + 90_000, changes=[
        (held_intent, None), (signal_event(coin, hedge, "signal_admitted", T + 90_000), None),
    ], delivery=(admitted(coin, hedge, T + 90_000), conflict_event(coin, hedge, T + 90_000)))))
    number(coin, hedge, "entry", T + 90_000)
    c = lease(coin, "BTCUSD", "executor-c", T + 90_000)
    fourth = coin.client_order_id(1, 4, "entry")
    hedged, hedge_order = submitted(coin, held_intent, fourth, T + 90_500)
    must(store.commit(Unit(T + 90_500, changes=[
        (hedged, 1), (hedge_order, None),
        (reservation_record(coin, 4, "BTCUSD", "0.00150000", "0.00075000", T + 90_500), None),
    ], fence=c, pool=0)))
    # An incident cleared with a note that is not ASCII, one left open, and
    # an event delivered.
    why = "the entry of operation 3 was still unsettled 60 s after it was sent"
    raised = incident_record(main, "op/3/unsettled_entry", "unsettled_entry", why, T + 95_000,
                             symbol="BTCUSDT", operation=3)
    told = event_record(main, "incident/op/3/unsettled_entry", "incident_opened", T + 95_000,
                        facts={"incident": "op/3/unsettled_entry"})
    must(store.commit(Unit(T + 95_000, changes=[(raised, None), (told, None)])))
    must(store.commit(Unit(T + 96_000, changes=[
        (raised.but(cleared={"at_ms": T + 96_000, "note": "flat, checked by hand: ž \"ok\""}),
         1),
        (told.but(delivery="Delivered", delivery_at_ms=T + 96_000), 1),
    ])))
    must(store.commit(Unit(T + 97_000, changes=[
        (incident_record(coin, "profile/second-writer", "second_writer",
                         "orders with owner tag a1b2c3 and another store's epoch", T + 97_000),
         None)])))
    # A Spot replacement whose outcome is not known yet.
    answer = store.begin_operation(spot, "spot_replace", "BTCUSDT", T + 98_000, key="cmd/4f9a",
                                   replaces=replacement(order_ref("qf-old-1")))
    replaced = answer[1].but(replacement=replacement(order_ref("qf-old-1"), "outcome_unknown",
                                                     order_ref("qf-new-1")))
    must(store.commit(Unit(T + 99_000, changes=[(replaced, 1)])))
    return {table: store.table(table) for table in store.rows}


def main() -> None:
    cases = scenarios()
    contests = races()
    fixture = {
        "format": FORMAT,
        "doc": (
            "The execution store's scenarios: each step with the answer a conforming store "
            "gives. Steps name scopes, accounts and positions by the names below, and records "
            "by the names of `records`, each with its kind and the exact text a store keeps "
            "it as. A unit's change expects no record (null) or the revision it replaces. "
            "Written by spikes/storage-contract/execution_cases.py, never by the Rust code "
            "under test."
        ),
        "scopes": {name: line.scope() for name, line in LINES.items()},
        "accounts": {name: line.account() for name, line in LINES.items()},
        "positions": dict(sorted(NAMES.positions.items())),
        "records": {name: {"kind": kind, "stored": text}
                    for (kind, text), name in NAMES.records.items()},
        "scenarios": [case.json() for case in cases],
        "races": contests,
    }
    OUT.write_text(json.dumps(fixture, indent=2, ensure_ascii=False) + "\n")
    steps = sum(len(case.steps) for case in cases)
    print(f"wrote {OUT.relative_to(ROOT)}: {len(cases)} scenarios, {steps} steps, "
          f"{len(contests)} races, {len(NAMES.records)} records")


if __name__ == "__main__":
    main()
