"""A reference model of the execution store of docs/execution-store.md,
written independently of the Rust code under test.

Three things, each taken from the documents and none from the code: how
every record is spelled where a store keeps it (its document, and the
columns the document decides); the state machines, read from the parity
matrix (docs/parity/matrix.toml); and the store itself: what a unit, a
lease call, an operation and a change of activation come to, in the order
docs/execution-store.md fixes.

Imported by execution_cases.py, which runs the scenarios, and by
transfer_cases.py, which copies a store's execution tables. Stdlib only.
"""

import datetime
import decimal
import hashlib
import json
import pathlib
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
MATRIX = ROOT / "docs" / "parity" / "matrix.toml"

# 2026-10-20T00:00:00Z, a UTC midnight.
T = 1_792_454_400_000
MINUTE = 60_000
I64_MAX = 2**63 - 1
# A made-up key fingerprint: sixteen hex digits, plain enough that a secret
# scanner does not take a fixture's fingerprint for a key.
FINGERPRINT = "00000000000000a1"


def dumps(value) -> str:
    """A document as a store keeps it: compact, members in their order."""
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False)


def token(word: str) -> str:
    """A wire word as a document spells it: outcome_unknown, OutcomeUnknown."""
    return "".join(part.capitalize() for part in word.split("_"))


def wire(spelled: str) -> str:
    """A document's word as a column spells it: OutcomeUnknown, outcome_unknown."""
    out = []
    for letter in spelled:
        if letter.isupper() and out:
            out.append("_")
        out.append(letter.lower())
    return "".join(out)


def base36(number: int) -> str:
    digits = "0123456789abcdefghijklmnopqrstuvwxyz"
    out = ""
    while True:
        number, digit = divmod(number, 36)
        out = digits[digit] + out
        if number == 0:
            return out


def iso(ms: int) -> str:
    moment = datetime.datetime.fromtimestamp(ms / 1000, tz=datetime.timezone.utc)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


# --- The state machines, as the parity matrix freezes them --------------------


def machine(name: str):
    table = tomllib.loads(MATRIX.read_text())["states"][name]
    edges = {tuple(edge.split(" -> ")) for edge in table["transitions"]}
    return set(table["states"]), set(table["terminal"]), edges


INTENT_STATES, INTENT_TERMINAL, INTENT_EDGES = machine("signal_intent")
_, _, PROTECTION_EDGES = machine("futures_position")
REPLACE_STATES, REPLACE_TERMINAL, REPLACE_EDGES = machine("spot_replace")
_, _, NOTIFICATION_EDGES = machine("notification")
_, _, INCIDENT_EDGES = machine("incident")
_, _, STORE_EDGES = machine("store")

# A signal is judged once: admitted, rejected, or asking for nothing.
SIGNAL_EDGES = {("received", to) for to in ("admitted", "rejected", "noop")}
assert SIGNAL_EDGES <= INTENT_EDGES, "the inbox's moves are the matrix's"
# An entry whose outcome is open blocks every other entry on its position
# (ADR 0002 section 5): sent, and not known to have ended.
UNRESOLVED = {"submitting", "outcome_unknown"}
# The states an intent record is ever kept in: admitted, and what follows.
RECORDED_INTENT_STATES = INTENT_STATES - {"received", "duplicate"}

# --- Whose record it is -------------------------------------------------------


class Line:
    """One profile on one product line of one venue account, or, without a
    profile, the account's own line (Spot)."""

    def __init__(self, venue, category, environment, account_kind, account_id, profile, owner,
                 settles):
        self.venue = venue
        self.category = category
        self.environment = environment
        self.account_kind = account_kind
        self.account_id = account_id
        self.profile = profile
        self.owner = owner
        self.settles = settles

    def scope(self):
        return {
            "venue": token(self.venue),
            "category": token(self.category),
            "environment": token(self.environment),
            "account": {token(self.account_kind): self.account_id},
            "profile": self.profile,
        }

    def account(self):
        return {
            "venue": token(self.venue),
            "environment": token(self.environment),
            "venue_account_id": {token(self.account_kind): self.account_id},
        }

    def instrument(self, symbol):
        return {
            "exchange": token(self.venue) + token(self.category),
            "symbol": symbol,
            "settle": self.settles[symbol],
        }

    def key(self, symbol):
        return {"account": self.account(), "instrument": self.instrument(symbol)}

    def client_order_id(self, epoch, operation, leg, attempt=1):
        """Layout 1 of ADR 0002: qf1-<owner>-<epoch>-<operation>-<leg><attempt>."""
        letter = {"entry": "e", "close": "c"}[leg]
        return f"qf1-{self.owner}-{base36(epoch)}-{base36(operation)}-{letter}{base36(attempt)}"

    def other_profile(self, profile, owner):
        return Line(self.venue, self.category, self.environment, self.account_kind,
                    self.account_id, profile, owner, self.settles)


def scope_columns(scope):
    (kind, account_id), = scope["account"].items()
    return {
        "venue": wire(scope["venue"]),
        "category": wire(scope["category"]),
        "environment": wire(scope["environment"]),
        "account_kind": wire(kind),
        "account_id": account_id,
        "profile_id": scope["profile"],
    }


ACCOUNT = ("venue", "category", "environment", "account_kind", "account_id")


def position_of(key):
    """A position as its columns name it: the account, the family and the
    symbol, whoever's profile acts on it."""
    account, instrument = key["account"], key["instrument"]
    (kind, account_id), = account["venue_account_id"].items()
    category = wire(instrument["exchange"])[len(wire(account["venue"])) + 1:]
    return (wire(account["venue"]), category, wire(account["environment"]), wire(kind),
            account_id, instrument["symbol"])


def pool_text(pool):
    return "account_usd" if pool == "AccountUsd" else "asset:" + pool["Asset"]


def pool_of(account, pool):
    """One pool of one venue account, across its product lines."""
    (kind, account_id), = account["venue_account_id"].items()
    return (wire(account["venue"]), wire(account["environment"]), wire(kind), account_id,
            pool_text(pool))


# --- The records, as a store keeps them ---------------------------------------


class Alert:
    """What one delivery brings: the signal, and its canonical payload
    (docs/signal-payload.md): every field in the format's order but
    fire_time, compact."""

    def __init__(self, line, kind, source, symbol, minute, price=None, stop=None, side="long",
                 exchange=None, category=None):
        self.kind = kind
        self.symbol = symbol
        self.side = side
        self.price, self.stop = price, stop
        ticker = symbol + ".P"
        bar = T + minute * MINUTE
        fields = {"version": 1, "kind": kind, "profile": line.profile, "source": source,
                  "ticker": ticker}
        if exchange is not None:
            fields["exchange"] = exchange
        if category is not None:
            fields["category"] = category
        fields["side"] = side
        fields["bar_time"] = iso(bar)
        if kind == "entry":
            fields["price"] = price
            if stop is not None:
                fields["stop"] = stop
        self.payload = dumps(fields)
        self.id = f"{source}:{kind}:{side}:{ticker}:{iso(bar)}"
        self.hash = "sha256:" + hashlib.sha256(self.payload.encode("utf-8")).hexdigest()
        self.fire_time_ms = bar + 5_000
        action = "Exit"
        if kind == "entry":
            action = {"Entry": {"expected_price": price, "stop": stop}}
        self.signal = {
            "id": self.id,
            "payload_hash": self.hash,
            "profile": line.profile,
            "ticker": ticker,
            "exchange": exchange,
            "category": None if category is None else token(category),
            "side": token(side),
            "action": action,
            "bar_time_ms": bar,
            "fire_time_ms": self.fire_time_ms,
        }


class Rec:
    """One record: its kind and its document."""

    def __init__(self, kind, doc):
        self.kind = kind
        self.doc = doc

    @property
    def text(self):
        return dumps(self.doc)

    def __eq__(self, other):
        return isinstance(other, Rec) and (self.kind, self.text) == (other.kind, other.text)

    def __hash__(self):
        return hash((self.kind, self.text))

    def but(self, **members):
        """The record with some members of its document replaced."""
        return Rec(self.kind, {**self.doc, **members})

    def within(self, member, **inner):
        """The record with members of one inner object replaced."""
        return Rec(self.kind, {**self.doc, member: {**self.doc[member], **inner}})


def signal_record(line, alert, received_at_ms, state="received", refusal=None, note=None,
                  judged_at_ms=None):
    """A signal in its profile's inbox. `refusal` is a refusal's name and
    its message."""
    refused = None
    if refusal is not None:
        refused = {"refusal": token(refusal[0]), "message": refusal[1]}
    return Rec("signal", {
        "format": 1,
        "scope": line.scope(),
        "signal": alert.signal,
        "payload": alert.payload,
        "received_at_ms": received_at_ms,
        "state": token(state),
        "refusal": refused,
        "note": note,
        "judged_at_ms": judged_at_ms,
    })


def judged(record, state, at_ms, refusal=None, note=None):
    refused = None
    if refusal is not None:
        refused = {"refusal": token(refusal[0]), "message": refusal[1]}
    return record.but(state=token(state), refusal=refused, note=note, judged_at_ms=at_ms)


def snapshot(line, symbol, capital=("15000", "USDT"), revision=4):
    """The policy an entry is admitted under (docs/automation-policy.md)."""
    sourced = lambda value: {"value": value, "source": "Defaults"}
    return {
        "format": 1,
        "profile": line.profile,
        "account": line.account(),
        "revision": revision,
        "instrument": line.instrument(symbol),
        "risk_percent": sourced("1"),
        "stop_percent": sourced("3"),
        "leverage": sourced("5"),
        "capital": {"amount": capital[0], "asset": capital[1]},
        "capital_revision": revision,
    }


def operation_in(client_order_id):
    """The operation a layout 1 client order ID names."""
    return int(client_order_id.split("-")[3], 36)


def intent_record(line, alert, policy="default", cycle=None):
    """A signal's intent as its admission records it: admitted, with
    nothing sent for it yet, so that no operation carries it out."""
    kind = {
        "profile": line.profile,
        "signal": alert.id,
        "signal_time_ms": alert.fire_time_ms,
        "key": line.key(alert.symbol),
        "side": token(alert.side),
    }
    if alert.kind == "entry":
        kind["expected_price"] = alert.price
        kind["stop"] = alert.stop
        if policy == "default":
            policy = snapshot(line, alert.symbol)
    elif policy == "default":
        policy = None
    return Rec("intent", {
        "format": 1,
        "scope": line.scope(),
        "intent": {
            "format": 1,
            "kind": {token(alert.kind): kind},
            "state": "Admitted",
            "policy": policy,
            "attempts": [],
        },
        "operation": None,
        "cycle": cycle,
    })


def moved(intent, state, attempt=None, cycle="same"):
    """An intent's successor: in another state, with one more attempt, in a
    cycle. Its operation is the one its latest attempt was sent under,
    which the attempt's client order ID names."""
    inner = dict(intent.doc["intent"])
    inner["state"] = token(state)
    operation = intent.doc["operation"]
    if attempt is not None:
        identifier, at_ms = attempt
        inner["attempts"] = inner["attempts"] + [
            {"client_order_id": identifier, "sent_at_ms": at_ms}
        ]
        operation = operation_in(identifier)
    return Rec("intent", {
        **intent.doc,
        "intent": inner,
        "operation": operation,
        "cycle": intent.doc["cycle"] if cycle == "same" else cycle,
    })


def operation_record(number, line, kind, symbol, key=None, signal=None, cycle=None,
                     replacement=None):
    return Rec("operation", {
        "format": 1,
        "id": number,
        "scope": line.scope(),
        "kind": token(kind),
        "key": key,
        "instrument": line.instrument(symbol),
        "signal": signal,
        "cycle": cycle,
        "replacement": replacement,
    })


def order_ref(client_order_id=None, venue_order_id=None):
    return {"client_order_id": client_order_id, "venue_order_id": venue_order_id}


def replacement(original, state="pending", replacing=None):
    return {"original": original, "replacement": replacing, "state": token(state)}


def quantity(line, amount):
    """Base units on a linear line, whole contracts on an inverse one."""
    return {"Contracts" if line.category == "inverse" else "Base": amount}


def venue_order(line, symbol, client_order_id, status, requested, filled, at_ms,
                venue_order_id="8389765519", side="Buy", reduce_only=False):
    """An order as its venue reports it."""
    return {"Futures": {
        "instrument": line.instrument(symbol),
        "venue_order_id": venue_order_id,
        "client_id": client_order_id,
        "side": side,
        "reduce_only": reduce_only,
        "origin": "Client",
        "status": token(status),
        "requested": quantity(line, requested),
        "filled": quantity(line, filled),
        "average_price": None,
        "source": "Query",
        "updated_at_ms": at_ms,
    }}


def order_record(line, operation, client_order_id, symbol, recorded_at_ms, venue_order_id=None,
                 outcome_unknown=False, refusal=None, evidence=None):
    refused = None
    if refusal is not None:
        refused = {"kind": refusal[0], "message": refusal[1]}
    return Rec("order", {
        "format": 1,
        "scope": line.scope(),
        "operation": operation,
        "client_order_id": client_order_id,
        "instrument": line.instrument(symbol),
        "recorded_at_ms": recorded_at_ms,
        "venue_order_id": venue_order_id,
        "outcome_unknown": outcome_unknown,
        "refusal": refused,
        "evidence": evidence,
    })


def fill_record(line, symbol, trade_id, client_order_id, amount, price, fee, time_ms,
                operation=None, cycle=None, venue_order_id="8389765519", side="Buy"):
    return Rec("fill", {
        "format": 1,
        "scope": line.scope(),
        "fill": {
            "trade_id": trade_id,
            "venue_order_id": venue_order_id,
            "client_id": client_order_id,
            "instrument": line.instrument(symbol),
            "origin": "Client",
            "side": side,
            "price": price,
            "quantity": quantity(line, amount),
            "fee": {"amount": fee, "asset": line.settles[symbol]},
            "realized_pnl": None,
            "time_ms": time_ms,
            "source": "Query",
        },
        "operation": operation,
        "cycle": cycle,
    })


def cycle_record(line, symbol, number, opened_at_ms, side="long", protection="protection_pending",
                 close=None):
    """A position cycle, numbered as the operation that opened it. `close`
    is its reason and time."""
    return Rec("cycle", {
        "format": 1,
        "scope": line.scope(),
        "cycle": {
            "id": number,
            "key": line.key(symbol),
            "profile": line.profile,
            "side": token(side),
            "opened_by": number,
            "protection": token(protection),
            "recorded_stop": None,
            "close_reason": None if close is None else token(close[0]),
            "pnl": None,
            "opened_at_ms": opened_at_ms,
            "closed_at_ms": None if close is None else close[1],
        },
    })


def reservation_record(line, operation, symbol, margin, risk, reserved_at_ms, pool=None,
                       released=None):
    """Margin held for an entry until its order has ended. `released` is
    the reason and the time."""
    settle = line.settles[symbol]
    pool = {"Asset": settle} if pool is None else pool
    unit = "USD" if pool == "AccountUsd" else pool["Asset"]
    return Rec("reservation", {
        "format": 1,
        "scope": line.scope(),
        "operation": operation,
        "instrument": line.instrument(symbol),
        "pool": pool,
        "margin": {"amount": margin, "asset": unit},
        "risk": {"amount": risk, "asset": settle},
        "reserved_at_ms": reserved_at_ms,
        "released": None if released is None else {"reason": token(released[0]),
                                                   "at_ms": released[1]},
    })


def incident_record(line, key, kind, message, opened_at_ms, symbol=None, operation=None,
                    cycle=None, cleared=None):
    """An incident that halts entries: on one instrument when `symbol` is
    given, else every entry of its profile. `cleared` is the time and the
    operator's note."""
    halts = "Profile" if symbol is None else {"Position": line.instrument(symbol)}
    return Rec("incident", {
        "format": 1,
        "scope": line.scope(),
        "key": key,
        "kind": token(kind),
        "halts": halts,
        "operation": operation,
        "cycle": cycle,
        "message": message,
        "opened_at_ms": opened_at_ms,
        "cleared": None if cleared is None else {"at_ms": cleared[0], "note": cleared[1]},
    })


def event_record(line, key, kind, created_at_ms, symbol=None, signal=None, operation=None,
                 cycle=None, facts=None, delivery="pending", delivery_at_ms=None):
    """One event of the outbox; its facts are kept in the order of their
    names."""
    return Rec("event", {
        "format": 1,
        "scope": line.scope(),
        "key": key,
        "kind": token(kind),
        "instrument": None if symbol is None else line.instrument(symbol),
        "signal": signal,
        "operation": operation,
        "cycle": cycle,
        "facts": dict(sorted((facts or {}).items())),
        "created_at_ms": created_at_ms,
        "delivery": token(delivery),
        "delivery_at_ms": delivery_at_ms,
    })


def conflict_event(line, alert, at_ms):
    """What the outbox is told when a delivery brings another payload under
    a recorded signal's ID: keyed by the payload, so it is told once."""
    return event_record(line, f"sig/{line.profile}/conflict:{alert.hash}/{alert.id}",
                        "signal_conflict", at_ms, signal=alert.id)


def signal_event(line, alert, kind, at_ms):
    return event_record(line, f"sig/{line.profile}/{kind}/{alert.id}", kind, at_ms,
                        symbol=alert.symbol, signal=alert.id)


# --- What a record is known by, and the row it is written as -----------------

TABLES = {
    "signal": "signal_inbox",
    "operation": "execution_operations",
    "intent": "execution_intents",
    "order": "execution_orders",
    "fill": "execution_fills",
    "cycle": "position_cycles",
    "reservation": "margin_reservations",
    "incident": "execution_incidents",
    "event": "outbox_events",
}
DOCUMENT = {
    "signal": "signal_json",
    "operation": "operation_json",
    "intent": "intent_json",
    "order": "order_json",
    "fill": "fill_json",
    "cycle": "cycle_json",
    "reservation": "reservation_json",
    "incident": "incident_json",
    "event": "event_json",
}
SCOPE = ["venue", "category", "environment", "account_kind", "account_id", "profile_id"]
# Every table's columns, in the order of the DDL of docs/storage-schema.md.
COLUMNS = {
    "signal_inbox": ["seq", *SCOPE, "signal_id", "payload_hash", "state", "refusal", "revision",
                     "signal_json", "received_at_ms", "judged_at_ms"],
    "signal_deliveries": ["seq", *SCOPE, "signal_id", "disposition", "payload_hash", "payload",
                          "fire_time_ms", "received_at_ms"],
    "execution_operations": ["operation_id", *SCOPE, "kind", "operation_key", "symbol",
                             "signal_id", "cycle_id", "replace_state", "revision",
                             "operation_json", "created_at_ms", "updated_at_ms"],
    "execution_intents": ["seq", *SCOPE, "signal_id", "kind", "symbol", "side", "state",
                          "entry_slot", "operation_id", "cycle_id", "revision", "intent_json",
                          "created_at_ms", "updated_at_ms"],
    "execution_orders": ["seq", *SCOPE, "operation_id", "client_order_id", "symbol",
                         "venue_order_id", "phase", "revision", "order_json", "recorded_at_ms",
                         "updated_at_ms"],
    "execution_fills": ["seq", *SCOPE, "symbol", "trade_id", "venue_order_id", "operation_id",
                        "cycle_id", "fill_json", "time_ms", "recorded_at_ms"],
    "position_cycles": ["cycle_id", *SCOPE, "symbol", "side", "protection", "open_slot",
                        "revision", "cycle_json", "opened_at_ms", "closed_at_ms",
                        "updated_at_ms"],
    "margin_reservations": ["seq", *SCOPE, "operation_id", "symbol", "pool", "state", "revision",
                            "reservation_json", "reserved_at_ms", "released_at_ms"],
    "execution_incidents": ["seq", *SCOPE, "incident_key", "kind", "symbol", "state", "revision",
                            "incident_json", "opened_at_ms", "cleared_at_ms"],
    "outbox_events": ["seq", *SCOPE, "event_key", "kind", "delivery", "revision", "event_json",
                      "created_at_ms", "delivery_at_ms"],
}
# The column each table's rows are numbered and read in order of.
ORDER = {table: columns[0] for table, columns in COLUMNS.items()}
# The columns a record is found by: unique together.
KEYS = {
    "signal": [*ACCOUNT, "profile_id", "signal_id"],
    "operation": ["operation_id"],
    "intent": [*ACCOUNT, "profile_id", "signal_id"],
    "order": [*ACCOUNT, "client_order_id"],
    "fill": [*ACCOUNT, "symbol", "trade_id"],
    "cycle": ["cycle_id"],
    "reservation": ["operation_id"],
    "incident": [*ACCOUNT, "incident_key"],
    "event": [*ACCOUNT, "event_key"],
}
# The columns a unit's time fills: when a row is first written, and when
# it was last.
CREATED = {"operation": "created_at_ms", "intent": "created_at_ms", "fill": "recorded_at_ms"}
UPDATED = {"operation": "updated_at_ms", "intent": "updated_at_ms", "order": "updated_at_ms",
           "cycle": "updated_at_ms"}


def intent_parts(record):
    (kind, inner), = record.doc["intent"]["kind"].items()
    return wire(kind), inner


def order_phase(doc):
    """How much is known of an order: what the venue reported comes first,
    then its refusal, then its acknowledgement."""
    if doc["evidence"] is not None:
        return "reported"
    if doc["refusal"] is not None:
        return "refused"
    if doc["venue_order_id"] is not None:
        return "acknowledged"
    return "outcome_unknown" if doc["outcome_unknown"] else "recorded"


def name_of(record):
    """What a record is known by among its kind."""
    doc = record.doc
    if record.kind == "signal":
        return f"{doc['scope']['profile']}/{doc['signal']['id']}"
    if record.kind == "operation":
        return str(doc["id"])
    if record.kind == "intent":
        _, inner = intent_parts(record)
        return f"{inner['profile']}/{inner['signal']}"
    if record.kind == "order":
        return doc["client_order_id"]
    if record.kind == "fill":
        return f"{doc['fill']['instrument']['symbol']}/{doc['fill']['trade_id']}"
    if record.kind == "cycle":
        return str(doc["cycle"]["id"])
    if record.kind == "reservation":
        return f"operation {doc['operation']}"
    return doc["key"]


def known_by(record):
    """What a signal, or the intent of one, is known by in a store: its
    scope and its signal's ID. Its name alone says its profile, which two
    accounts may both have."""
    doc = record.doc
    if record.kind == "signal":
        return (dumps(doc["scope"]), doc["signal"]["id"])
    _, inner = intent_parts(record)
    return (dumps(doc["scope"]), inner["signal"])


def position_key(scope, instrument):
    return {
        "account": {"venue": scope["venue"], "environment": scope["environment"],
                    "venue_account_id": scope["account"]},
        "instrument": instrument,
    }


def position(record):
    """The futures position a record is part of, whose lease a unit that
    changes it is written under."""
    doc = record.doc
    if record.kind == "intent":
        return position_of(intent_parts(record)[1]["key"])
    if record.kind == "cycle":
        return position_of(doc["cycle"]["key"])
    if doc["scope"]["category"] == "Spot":
        return None
    if record.kind in ("reservation", "order"):
        return position_of(position_key(doc["scope"], doc["instrument"]))
    if record.kind == "fill":
        return position_of(position_key(doc["scope"], doc["fill"]["instrument"]))
    return None


def columns_of(record):
    """Every column of a record's row that its document decides."""
    doc = record.doc
    out = scope_columns(doc["scope"])
    if record.kind == "signal":
        refusal = doc["refusal"]
        out.update(
            signal_id=doc["signal"]["id"],
            payload_hash=doc["signal"]["payload_hash"],
            state=wire(doc["state"]),
            refusal=None if refusal is None else wire(refusal["refusal"]),
            received_at_ms=doc["received_at_ms"],
            judged_at_ms=doc["judged_at_ms"],
        )
    elif record.kind == "operation":
        replaced = doc["replacement"]
        out.update(
            operation_id=doc["id"],
            kind=wire(doc["kind"]),
            operation_key=doc["key"],
            symbol=doc["instrument"]["symbol"],
            signal_id=doc["signal"],
            cycle_id=doc["cycle"],
            replace_state=None if replaced is None else wire(replaced["state"]),
        )
    elif record.kind == "intent":
        kind, inner = intent_parts(record)
        state = wire(doc["intent"]["state"])
        out.update(
            signal_id=inner["signal"],
            kind=kind,
            symbol=inner["key"]["instrument"]["symbol"],
            side=wire(inner["side"]),
            state=state,
            entry_slot=1 if kind == "entry" and state in UNRESOLVED else None,
            operation_id=doc["operation"],
            cycle_id=doc["cycle"],
        )
    elif record.kind == "order":
        out.update(
            operation_id=doc["operation"],
            client_order_id=doc["client_order_id"],
            symbol=doc["instrument"]["symbol"],
            venue_order_id=doc["venue_order_id"],
            phase=order_phase(doc),
            recorded_at_ms=doc["recorded_at_ms"],
        )
    elif record.kind == "fill":
        fill = doc["fill"]
        out.update(
            symbol=fill["instrument"]["symbol"],
            trade_id=fill["trade_id"],
            venue_order_id=fill["venue_order_id"],
            operation_id=doc["operation"],
            cycle_id=doc["cycle"],
            time_ms=fill["time_ms"],
        )
    elif record.kind == "cycle":
        cycle = doc["cycle"]
        protection = wire(cycle["protection"])
        out.update(
            cycle_id=cycle["id"],
            symbol=cycle["key"]["instrument"]["symbol"],
            side=wire(cycle["side"]),
            protection=protection,
            open_slot=None if protection == "closed" else 1,
            opened_at_ms=cycle["opened_at_ms"],
            closed_at_ms=cycle["closed_at_ms"],
        )
    elif record.kind == "reservation":
        released = doc["released"]
        out.update(
            operation_id=doc["operation"],
            symbol=doc["instrument"]["symbol"],
            pool=pool_text(doc["pool"]),
            state="held" if released is None else "released",
            reserved_at_ms=doc["reserved_at_ms"],
            released_at_ms=None if released is None else released["at_ms"],
        )
    elif record.kind == "incident":
        halts, cleared = doc["halts"], doc["cleared"]
        out.update(
            incident_key=doc["key"],
            kind=wire(doc["kind"]),
            symbol=None if halts == "Profile" else halts["Position"]["symbol"],
            state="open" if cleared is None else "cleared",
            opened_at_ms=doc["opened_at_ms"],
            cleared_at_ms=None if cleared is None else cleared["at_ms"],
        )
    else:
        out.update(
            event_key=doc["key"],
            kind=wire(doc["kind"]),
            delivery=wire(doc["delivery"]),
            created_at_ms=doc["created_at_ms"],
            delivery_at_ms=doc["delivery_at_ms"],
        )
    out[DOCUMENT[record.kind]] = record.text
    return out


def key_of(record):
    columns = columns_of(record)
    return tuple(columns[name] for name in KEYS[record.kind])


def slot_of(record):
    """The slot a record takes while it holds it: the one unresolved entry
    of its position, or its one open cycle."""
    columns = columns_of(record)
    if record.kind == "intent" and columns["entry_slot"] == 1:
        return ("entry_slot", position(record))
    if record.kind == "cycle" and columns["open_slot"] == 1:
        return ("open_slot", position(record))
    return None


def refusal(contains: str) -> dict:
    return {"kind": "invalid_argument", "contains": contains}


def final(evidence):
    """Whether a venue's report is the last its order has: an order that
    can fill no more, or a stop that has ended. A status with a reason, a
    cancellation's, is spelled as an object."""
    (kind, reported), = evidence.items()
    status = reported["status"]
    word = status if isinstance(status, str) else next(iter(status))
    working = ("New", "PartiallyFilled") if kind == "Futures" else ("Active", "Triggered")
    return word not in working


def follows(new, old):
    """Why `new` may not replace `old`, or None: the same record, changed
    only as its kind allows."""
    never = lambda what: f"its {what} differs from the record it follows"
    a, b = old.doc, new.doc
    if new.kind == "signal":
        same = all(a[m] == b[m] for m in ("scope", "signal", "payload", "received_at_ms"))
        if not same:
            return never("signal")
        edge = (wire(a["state"]), wire(b["state"]))
        return None if edge in SIGNAL_EDGES else f"signal cannot move from {edge[0]} to {edge[1]}"
    if new.kind == "intent":
        before, after = a["intent"], b["intent"]
        if a["scope"] != b["scope"] or before["kind"] != after["kind"]:
            return never("signal")
        edge = (wire(before["state"]), wire(after["state"]))
        if edge[0] != edge[1] and edge not in INTENT_EDGES:
            return f"intent cannot move from {edge[0]} to {edge[1]}"
        if before["policy"] is not None and after["policy"] != before["policy"]:
            return never("policy snapshot")
        sent = len(before["attempts"])
        if after["attempts"][:sent] != before["attempts"]:
            return never("list of attempts sent")
        if a["cycle"] is not None and b["cycle"] != a["cycle"]:
            return never("position cycle")
        return None
    if new.kind == "cycle":
        before, after = a["cycle"], b["cycle"]
        fixed = ("id", "key", "profile", "side", "opened_by", "opened_at_ms")
        if any(before[m] != after[m] for m in fixed):
            return never("cycle")
        edge = (wire(before["protection"]), wire(after["protection"]))
        if edge[0] != edge[1] and edge not in PROTECTION_EDGES:
            return f"position protection cannot move from {edge[0]} to {edge[1]}"
        return None
    if new.kind == "operation":
        fixed = ("id", "scope", "kind", "key", "instrument", "signal")
        if any(a[m] != b[m] for m in fixed):
            return never("identity")
        if a["cycle"] is not None and b["cycle"] != a["cycle"]:
            return never("position cycle")
        before, after = a["replacement"], b["replacement"]
        if before is None and after is None:
            return None
        if before is None or after is None:
            return never("replacement")
        if before["original"] != after["original"]:
            return never("replaced order")
        if before["replacement"] is not None:
            was, now = before["replacement"], after["replacement"]
            # A client ID never changes. The venue's ID is learned later,
            # and once learned it neither changes nor goes.
            renamed = (now is None
                       or now["client_order_id"] != was["client_order_id"]
                       or (was["venue_order_id"] is not None
                           and now["venue_order_id"] != was["venue_order_id"]))
            if renamed:
                return never("replacing order")
        edge = (wire(before["state"]), wire(after["state"]))
        if edge[0] != edge[1] and edge not in REPLACE_EDGES:
            return f"spot replacement cannot move from {edge[0]} to {edge[1]}"
        return None
    if new.kind == "order":
        fixed = ("scope", "operation", "client_order_id", "instrument", "recorded_at_ms")
        if any(a[m] != b[m] for m in fixed):
            return never("identity")
        reopened = a["evidence"] is not None and (
            b["evidence"] is None or (final(a["evidence"]) and not final(b["evidence"])))
        forgotten = (
            (a["venue_order_id"] is not None and b["venue_order_id"] != a["venue_order_id"])
            or reopened
            or (a["outcome_unknown"] and not b["outcome_unknown"])
            or (a["refusal"] is not None and b["refusal"] != a["refusal"])
        )
        if forgotten:
            return "it takes back what the record it follows had learned of the order"
        return None
    if new.kind == "reservation":
        if {**a, "released": None} != {**b, "released": None}:
            return never("amounts or identity")
        if a["released"] is not None or b["released"] is None:
            state = lambda doc: "held" if doc["released"] is None else "released"
            return f"reservation cannot move from {state(a)} to {state(b)}"
        return None
    if new.kind == "incident":
        if {**a, "cleared": None} != {**b, "cleared": None}:
            return never("cause")
        if a["cleared"] is not None or b["cleared"] is None:
            state = lambda doc: "open" if doc["cleared"] is None else "cleared"
            return f"incident cannot move from {state(a)} to {state(b)}"
        return None
    if new.kind == "event":
        body = lambda doc: {**doc, "delivery": "Pending", "delivery_at_ms": None}
        if body(a) != body(b):
            return never("content")
        edge = (wire(a["delivery"]), wire(b["delivery"]))
        if edge not in NOTIFICATION_EDGES:
            return f"notification cannot move from {edge[0]} to {edge[1]}"
        return None
    return "is recorded once and never replaced"


# --- The unit -----------------------------------------------------------------


class Unit:
    """Everything one write changes: applied whole, or not at all.
    `changes` pairs each record with what it expects to find: None for no
    record, or the revision of the one it replaces. `delivery` is a signal
    record as the delivery brought it and the event to tell if it
    conflicts. `fence` is the position, the holder and the token of the
    lease the unit is written under; `pool` the revision its reservations
    were sized against."""

    def __init__(self, at_ms, changes=(), fence=None, pool=None, delivery=None):
        self.at_ms = at_ms
        self.changes = list(changes)
        self.fence = fence
        self.pool = pool
        self.delivery = delivery


def shape(unit):
    """Why a unit is not one a store may be asked to apply, or None."""
    if unit.delivery is None and not unit.changes:
        return "it changes nothing"
    seen = set()
    for record, expect in unit.changes:
        if record.kind in ("signal", "operation") and expect is None:
            return f"{record.kind} {name_of(record)} cannot be first recorded by a change"
        if record.kind == "fill" and expect is not None:
            return f"fill {name_of(record)} cannot be replaced by a change"
        named = (record.kind, name_of(record))
        if named in seen:
            return f"it changes {named[0]} {named[1]} twice"
        seen.add(named)
    # A position's records change only under its lease. An intent is first
    # recorded by admission, which holds none.
    held = None
    for record, expect in unit.changes:
        part_of = None if (record.kind == "intent" and expect is None) else position(record)
        if part_of is None:
            continue
        if held is not None and held != part_of:
            return "it changes two positions"
        held = part_of
    if held is not None:
        if unit.fence is None:
            return "without that position's lease"
        if unit.fence[0] != held:
            return "and is written under the lease of"
    pools = {pool_of(position_key(r.doc["scope"], r.doc["instrument"])["account"], r.doc["pool"])
             for r, _ in unit.changes if r.kind == "reservation"}
    if len(pools) > 1:
        return "it reserves from two pools"
    if pools and unit.pool is None:
        return "without naming the revision its pool was read at"
    if not pools and unit.pool is not None:
        return "it names a pool's revision and changes no reservation"
    # An admitted signal and its intent are written together.
    signals = [r for r, _ in unit.changes if r.kind == "signal"]
    if unit.delivery is not None:
        arrived, told = unit.delivery
        if told.doc["kind"] != "SignalConflict" or told.doc["scope"] != arrived.doc["scope"]:
            return "its delivery's conflict event is not a signal conflict of the same profile"
        signals.insert(0, arrived)
    # A signal is its scope's: two accounts' profiles of one name that take
    # one alert hold two signals, and each is paired with its own intent.
    names = [known_by(signal) for signal in signals]
    if len(set(names)) != len(names):
        return "twice"
    admitted = [known_by(signal) for signal in signals if signal.doc["state"] == "Admitted"]
    recorded = []
    for record, expect in unit.changes:
        if record.kind != "intent" or expect is not None:
            continue
        named = known_by(record)
        if record.doc["intent"]["state"] != "Admitted" or named not in admitted:
            return "is first recorded as admitted, in the unit that admits its signal"
        recorded.append(named)
    if any(named not in recorded for named in admitted):
        return "without recording its intent"
    # A position has one unresolved entry and one open cycle at a time, so
    # a unit gives its position at most one of each.
    taken = []
    for record, _ in unit.changes:
        slot = slot_of(record)
        if slot is None:
            continue
        if slot[0] in taken:
            return ("it gives its position two unresolved entries" if slot[0] == "entry_slot"
                    else "it gives its position two open cycles")
        taken.append(slot[0])
    return None


def exact_sum(amounts):
    """The sum of decimal texts, exact, as text. Python adds decimals to
    28 significant digits unless told otherwise, which would round a total
    a store holds whole; here an inexact step is an error. So is a total
    outside what a store's decimals hold, 28 places and 96 bits: no case
    may be written that a store could only answer rounded."""
    with decimal.localcontext() as context:
        context.prec = 80
        context.traps[decimal.Inexact] = True
        total = sum((decimal.Decimal(amount) for amount in amounts), decimal.Decimal(0))
    sign, digits, exponent = total.as_tuple()
    mantissa = int("".join(str(digit) for digit in digits))
    if -exponent > 28 or mantissa >= 2 ** 96:
        raise ValueError(f"a total of {total} is outside a store's decimals")
    return format(total, "f")


# --- The store ----------------------------------------------------------------


class Store:
    """One execution store: its record, its tables, its leases and the
    revisions of its pools."""

    def __init__(self):
        self.environment = None
        self.activation = "inactive"
        self.epoch = 0
        self.control_log = []
        self.rows = {table: [] for table in COLUMNS}
        self.numbers = {table: 0 for table in COLUMNS}
        self.leases = {}  # position -> {"fence": last token, "lease": the lease held, or None}
        self.pools = {}  # pool -> revision

    # The store's own record.

    def bind(self, environment):
        if self.environment not in (None, environment):
            return refusal(f"is bound to {self.environment}")
        self.environment = environment
        return None

    def change_activation(self, to, at_ms):
        if to == "active":
            if self.activation == "active":
                return refusal(f"is already active, at epoch {self.epoch}")
            if self.environment is None:
                return refusal("is bound to no environment yet; bind it before it is activated")
            self.epoch += 1
            # No lease outlives the epoch that granted it: each position
            # keeps its last token, and is held by no one.
            for row in self.leases.values():
                row["lease"] = None
        elif self.activation != "active":
            return refusal(f"is {self.activation}, and only an active store is deactivated")
        assert (self.activation, to) in STORE_EDGES
        self.control_log.append(
            {"from": self.activation, "to": to, "epoch": self.epoch, "at_ms": at_ms}
        )
        self.activation = to
        return None

    def keeps(self, environment):
        """Why something of `environment` is not this store's to keep, or
        None: a store keeps the executions of the environment it is bound
        to, and of none before it is bound."""
        if self.environment is None:
            return refusal("the store is bound to no environment yet")
        if environment != self.environment:
            return refusal(f"is for {environment}, and the store is bound to {self.environment}")
        return None

    # Leases.

    def acquire(self, at, profile, holder, ttl_ms, now_ms):
        """('granted' | 'held', lease), or ('refused', refusal)."""
        refused = self.keeps(at[2])
        if refused:
            return "refused", refused
        if self.activation != "active":
            return "refused", refusal("only an active store grants a lease")
        if ttl_ms <= 0 or now_ms + ttl_ms > I64_MAX:
            return "refused", refusal("lease duration")
        row = self.leases.setdefault(at, {"fence": 0, "lease": None})
        held = row["lease"]
        live = held is not None and held["epoch"] == self.epoch and held["expires_at_ms"] > now_ms
        if live and held["holder"] != holder:
            return "held", dict(held)
        if live:
            # The asker's own: extended, under the token it has.
            held.update(profile=profile, expires_at_ms=now_ms + ttl_ms)
            return "granted", dict(held)
        # No lease, an expired one, or one an earlier epoch granted: a new
        # grant, under the next token.
        row["fence"] += 1
        row["lease"] = {"position": at, "profile": profile, "holder": holder,
                        "fence": row["fence"], "epoch": self.epoch, "acquired_at_ms": now_ms,
                        "expires_at_ms": now_ms + ttl_ms}
        return "granted", dict(row["lease"])

    def fenced(self, fence):
        """Why a unit written under `fence` is not applied, or None. Time
        is no part of it: a lease that ran out and was taken by no one
        else still fences."""
        at, holder, fence_token = fence
        held = self.leases.get(at, {}).get("lease")
        if held is None:
            return "not_held"
        if (held["holder"], held["fence"]) != (holder, fence_token):
            return {"lost": {"holder": held["holder"], "token": held["fence"]}}
        if self.activation != "active" or self.epoch != held["epoch"]:
            return {"store_not_active": {"activation": self.activation, "epoch": self.epoch}}
        return None

    def renew(self, fence, ttl_ms, now_ms):
        """('renewed', lease | None), or ('refused', refusal)."""
        if self.fenced(fence) is not None:
            return "renewed", None
        if ttl_ms <= 0 or now_ms + ttl_ms > I64_MAX:
            return "refused", refusal("a lease lasts some time")
        held = self.leases[fence[0]]["lease"]
        held["expires_at_ms"] = now_ms + ttl_ms
        return "renewed", dict(held)

    def release(self, fence):
        at, holder, fence_token = fence
        row = self.leases.get(at)
        held = None if row is None else row["lease"]
        if held is None or (held["holder"], held["fence"]) != (holder, fence_token):
            return False
        row["lease"] = None
        return True

    def lease(self, at):
        held = self.leases.get(at, {}).get("lease")
        return None if held is None else dict(held)

    # Rows.

    def find(self, record):
        """The row that holds the record `record` names, if one does."""
        wanted = key_of(record)
        for row in self.rows[TABLES[record.kind]]:
            if tuple(row[name] for name in KEYS[record.kind]) == wanted:
                return row
        return None

    def insert(self, table, columns):
        first = ORDER[table]
        if columns.get(first) is None:
            self.numbers[table] += 1
            columns[first] = self.numbers[table]
        else:
            self.numbers[table] = max(self.numbers[table], columns[first])
        row = {name: columns.get(name) for name in COLUMNS[table]}
        self.rows[table].append(row)
        self.rows[table].sort(key=lambda held: held[first])
        return row

    def first_write(self, record, at_ms):
        columns = columns_of(record)
        columns["revision"] = 1
        for stamp in (CREATED.get(record.kind), UPDATED.get(record.kind)):
            if stamp is not None:
                columns[stamp] = at_ms
        row = self.insert(TABLES[record.kind], columns)
        row["_record"] = record
        return row

    def replace(self, row, record, at_ms):
        """A row replaced: at the next revision, numbered and first
        written as it was."""
        kept = {ORDER[TABLES[record.kind]], CREATED.get(record.kind), *KEYS[record.kind]}
        for name, value in columns_of(record).items():
            if name not in kept:
                row[name] = value
        row["revision"] += 1
        if record.kind in UPDATED:
            row[UPDATED[record.kind]] = at_ms
        row["_record"] = record

    def note(self, arrived, disposition):
        doc = arrived.doc
        columns = scope_columns(doc["scope"])
        columns.update(
            signal_id=doc["signal"]["id"],
            disposition=disposition,
            payload_hash=doc["signal"]["payload_hash"],
            payload=doc["payload"] if disposition == "conflict" else None,
            fire_time_ms=doc["signal"]["fire_time_ms"],
            received_at_ms=doc["received_at_ms"],
        )
        self.insert("signal_deliveries", columns)

    # The unit.

    def commit(self, unit):
        """What a unit comes to. The order of the checks is the contract's:
        the unit's shape, the store's environment, the fence, the
        delivery, the pool's revision, each change in the unit's order,
        then the slots. Nothing is written unless all of it is."""
        wrong = shape(unit)
        if wrong:
            return {"refused": refusal(wrong)}
        records = [record for record, _ in unit.changes]
        if unit.delivery is not None:
            records.insert(0, unit.delivery[0])
        for record in records:
            refused = self.keeps(wire(record.doc["scope"]["environment"]))
            if refused:
                return {"refused": refused}
        if unit.fence is not None:
            fenced = self.fenced(unit.fence)
            if fenced is not None:
                return {"fenced": fenced}
        writes = []
        if unit.delivery is not None:
            arrived, told = unit.delivery
            held = self.find(arrived)
            if held is not None:
                # The same signal again, or another payload under its ID:
                # noted, and nothing else of the unit is applied.
                same = held["payload_hash"] == arrived.doc["signal"]["payload_hash"]
                self.note(arrived, "duplicate" if same else "conflict")
                answer = {"record": held["_record"], "revision": held["revision"]}
                if same:
                    return {"duplicate": answer}
                if self.find(told) is None:
                    self.first_write(told, unit.at_ms)
                return {"conflict": answer}
            writes.append(lambda: (self.first_write(arrived, unit.at_ms),
                                   self.note(arrived, "new")))
        if unit.pool is not None:
            reserving = next(r for r, _ in unit.changes if r.kind == "reservation")
            pool = pool_of(position_key(reserving.doc["scope"],
                                        reserving.doc["instrument"])["account"],
                           reserving.doc["pool"])
            revision = self.pools.get(pool, 0)
            if unit.pool != revision:
                return {"stale": {"pool": revision}}
            writes.append(lambda: self.pools.__setitem__(pool, revision + 1))
        for record, expect in unit.changes:
            held = self.find(record)
            # A fill is written once, and stands at revision 1 for good.
            found = None if held is None else held.get("revision", 1)
            if found != expect:
                return {"stale": {"record": {"kind": record.kind, "name": name_of(record),
                                             "found": found}}}
            if held is None:
                writes.append(lambda record=record: self.first_write(record, unit.at_ms))
                continue
            wrong = follows(record, held["_record"])
            if wrong:
                return {"refused": refusal(wrong)}
            writes.append(lambda record=record, held=held:
                          self.replace(held, record, unit.at_ms))
        for record, _ in unit.changes:
            slot = slot_of(record)
            if slot is None:
                continue
            for row in self.rows[TABLES[record.kind]]:
                holder = row["_record"]
                if slot_of(holder) == slot and name_of(holder) != name_of(record):
                    return {"refused": refusal("cannot take its position's slot")}
        for write in writes:
            write()
        return "applied"

    # Operations.

    def begin_operation(self, line, kind, symbol, at_ms, key=None, signal=None, replaces=None):
        """An operation, numbered by the store: the same one again under a
        key it already recorded. Answers ('numbered', record, revision), or
        ('refused', refusal)."""
        refused = self.keeps(line.environment)
        if refused:
            return "refused", refused
        asked = operation_record(0, line, kind, symbol, key=key, signal=signal,
                                 replacement=replaces)
        if key is not None:
            account = [scope_columns(line.scope())[name] for name in ACCOUNT]
            for row in self.rows["execution_operations"]:
                if [row[name] for name in ACCOUNT] + [row["operation_key"]] != account + [key]:
                    continue
                held = row["_record"].doc
                same = all(held[m] == asked.doc[m]
                           for m in ("scope", "kind", "instrument", "signal"))
                original = lambda doc: None if doc is None else doc["original"]
                if not same or original(held["replacement"]) != original(replaces):
                    return "refused", refusal("already names operation")
                return "numbered", row["_record"], row["revision"]
        number = self.numbers["execution_operations"] + 1
        record = asked.but(id=number)
        self.first_write(record, at_ms)
        return "numbered", record, 1

    # Reads: every list in the order its rows were numbered.

    def stored(self, kind, where, limit=None):
        found = [row for row in self.rows[TABLES[kind]] if where(row)]
        if limit is not None:
            found = found[:limit]
        return [(row["_record"], row.get("revision", 1)) for row in found]

    def one(self, kind, where):
        found = self.stored(kind, where)
        return found[0] if found else None

    def pool(self, account, pool):
        at = pool_of(account, pool)
        columns = ("venue", "environment", "account_kind", "account_id", "pool")
        held = [row["_record"] for row in self.rows["margin_reservations"]
                if tuple(row[name] for name in columns) == at and row["state"] == "held"]
        return {"revision": self.pools.get(at, 0), "held": held,
                "total": exact_sum(record.doc["margin"]["amount"] for record in held)
                if held else None}

    def table(self, table):
        """A table's rows as a transfer copies them: every column, in the
        order of the DDL."""
        return [{name: row[name] for name in COLUMNS[table]} for row in self.rows[table]]
