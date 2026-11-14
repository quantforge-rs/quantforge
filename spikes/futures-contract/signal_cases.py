#!/usr/bin/env python3
"""An independent model of the TradingView alert payload (ADR 0002 sections 5 and 9).

It writes the shared cases the Rust implementation is held to:

    python3 -I spikes/futures-contract/signal_cases.py --write tests/fixtures/signal_cases.json
    python3 -I spikes/futures-contract/signal_cases.py --doc docs/signal-payload.md

The model shares no code with the Rust one. It knows the payload's rules:

- what a body must be to be a signal at all, and every problem with one that
  is not, each by its code and its field;
- a signal's ID, built only from what TradingView renders the same way on
  every resend, and the canonical payload whose hash tells a repeated
  delivery from a conflicting one;
- how a signal is resolved for the profile its URL selects: the profile it
  names, the venue and family it may assert, an entry's age, its ticker
  against the profile's assets and the venue's own instrument list, and the
  policy an entry is admitted under;
- which exit closes which entry's position, what a repeated or opposite entry
  is, and the receipt a delivery is answered with.

It also holds the JSON schema of `docs/signal-schema.json` to the cases, with a
validator of its own for the part of JSON Schema that file uses: the schema
accepts everything the rules accept, and each case says what the schema alone
would say of it.

`--doc` rewrites what the contract page repeats of the cases, between its
markers. Standard library only. Run it with `python3 -I`, from the repository
root.
"""

import argparse
import calendar
import copy
import datetime
import hashlib
import json
import math
import re
import sys
import textwrap

FIXTURE_FORMAT = 1
SIGNAL_VERSION = 1
MAX_SIGNAL_BYTES = 4096
MAX_DEPTH = 8
AGE_LIMIT_S = {"default": 60, "min": 5, "max": 300}
FUTURE_ALLOWANCE_MS = 5000
SCHEMA_PATH = "docs/signal-schema.json"

# A made-up key fingerprint: sixteen hex digits, plain enough that a secret
# scanner does not take a fixture's fingerprint for a key.
FINGERPRINT = "00000000000000a1"

# Every field of the format, in the order the canonical payload writes them.
FIELDS = ["version", "kind", "profile", "source", "ticker", "exchange", "category", "side",
          "bar_time", "fire_time", "price", "stop"]
# What only an entry carries.
ENTRY_ONLY = ["price", "stop"]
COMMON_REQUIRED = ["version", "kind", "profile", "source", "ticker", "side", "bar_time", "fire_time"]
REQUIRED = {"entry": COMMON_REQUIRED + ["price"], "exit": COMMON_REQUIRED}
# What a delivery's ID and hash leave out: the one field a resend may render again.
NOT_IN_CANONICAL = ["fire_time"]
ID_FIELDS = ["source", "kind", "side", "ticker", "bar_time"]

CODES = ["too_large", "not_json", "not_an_object", "duplicate_field", "unknown_field",
         "misplaced_field", "missing_field", "wrong_type", "unsupported_version", "invalid_value",
         "bar_after_fire"]

KINDS = ["entry", "exit"]
SIDES = ["long", "short"]
CATEGORIES = ["linear", "inverse"]
RECEIPT_STATES = ["received", "admitted", "rejected", "duplicate", "noop"]
# How a ticker names an instrument.
SPELLINGS = ["venue_symbol", "tradingview"]

# ---------------------------------------------------------------------------
# Reading a body.
# ---------------------------------------------------------------------------


def held_by_a_double(text):
    """A JSON number counts as one only while a double holds it: beyond that, readers
    disagree on what it is, so the body is not JSON here."""
    if math.isinf(float(text)):
        raise ValueError("{} is a number no double holds".format(text[:20]))
    return text


class RawInt:
    """A JSON integer as it was written."""

    def __init__(self, text):
        self.text = held_by_a_double(text)


class RawFloat:
    def __init__(self, text):
        self.text = held_by_a_double(text)


class Pairs:
    """A JSON object as its members were written: duplicates kept, in order."""

    def __init__(self, items):
        self.items = items


def refuse_constant(name):
    raise ValueError("{} is not JSON".format(name))


def too_deep(text):
    depth, in_string, escaped = 0, False, False
    for char in text:
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif char in "[{":
            depth += 1
            if depth > MAX_DEPTH:
                return True
        elif char in "]}":
            depth -= 1
    return False


def has_surrogate(value):
    if isinstance(value, str):
        return any(0xD800 <= ord(char) <= 0xDFFF for char in value)
    if isinstance(value, Pairs):
        return any(has_surrogate(key) or has_surrogate(member) for key, member in value.items)
    if isinstance(value, list):
        return any(has_surrogate(member) for member in value)
    return False


def parse(body):
    """The body's JSON value, or the code of why it has none."""
    if len(body) > MAX_SIGNAL_BYTES:
        return None, "too_large"
    try:
        text = body.decode("utf-8", errors="strict")
    except UnicodeDecodeError:
        return None, "not_json"
    if too_deep(text):
        return None, "not_json"
    try:
        value = json.loads(text, object_pairs_hook=Pairs, parse_int=RawInt, parse_float=RawFloat,
                           parse_constant=refuse_constant)
    except (ValueError, RecursionError):
        return None, "not_json"
    if has_surrogate(value):
        return None, "not_json"
    if not isinstance(value, Pairs):
        return None, "not_an_object"
    return value, None


# ---------------------------------------------------------------------------
# One field at a time.
# ---------------------------------------------------------------------------

NAME = re.compile(r"[a-z0-9][a-z0-9_-]{0,31}\Z")
EXCHANGE = re.compile(r"[A-Z0-9_]{1,32}\Z")
TIME = re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})T([0-9]{2}):([0-9]{2}):([0-9]{2})Z\Z")
DECIMAL = re.compile(r"(0|[1-9][0-9]*)(\.[0-9]+)?\Z")
WHOLE = re.compile(r"(0|[1-9][0-9]*)\Z")
TICKER_ASCII = set("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
# Beyond ASCII a ticker takes any character but the C1 controls and the spaces
# and invisible format characters, listed by code point so that every
# implementation draws the same line.
TICKER_REFUSED = [(0x80, 0xA0), (0x1680, 0x1680), (0x2000, 0x200F), (0x2028, 0x202F),
                  (0x205F, 0x206F), (0x3000, 0x3000), (0xFEFF, 0xFEFF)]
TICKER_MAX = 40
MANTISSA_LIMIT = 2 ** 96


def ticker_ok(text):
    if not 1 <= len(text) <= TICKER_MAX:
        return False
    for char in text:
        point = ord(char)
        if point < 0x80:
            if char not in TICKER_ASCII:
                return False
        elif any(low <= point <= high for low, high in TICKER_REFUSED):
            return False
    return True


def time_ms(text):
    """The instant a time names, in milliseconds since the epoch, or None."""
    match = TIME.match(text)
    if not match:
        return None
    year, month, day, hour, minute, second = (int(part) for part in match.groups())
    if year < 1970:
        return None
    try:
        datetime.datetime(year, month, day, hour, minute, second)
    except ValueError:
        return None
    return calendar.timegm((year, month, day, hour, minute, second)) * 1000


def price_ok(text):
    """A positive exact decimal: at most 28 decimal places and 96 bits of digits."""
    match = DECIMAL.match(text)
    if not match:
        return False
    fraction = (match.group(2) or ".")[1:]
    mantissa = int(match.group(1) + fraction)
    return len(fraction) <= 28 and 0 < mantissa < MANTISSA_LIMIT


TEXT_RULES = {
    "kind": lambda text: text in KINDS,
    "profile": lambda text: bool(NAME.match(text)),
    "source": lambda text: bool(NAME.match(text)),
    "ticker": ticker_ok,
    "exchange": lambda text: bool(EXCHANGE.match(text)),
    "category": lambda text: text in CATEGORIES,
    "side": lambda text: text in SIDES,
    "bar_time": lambda text: time_ms(text) is not None,
    "fire_time": lambda text: time_ms(text) is not None,
    "price": price_ok,
    "stop": price_ok,
}


def judge(field, value):
    """The code of what is wrong with one field's value, or None."""
    if field == "version":
        whole = isinstance(value, RawInt) and WHOLE.match(value.text)
        if not whole or int(value.text) >= 2 ** 53:
            return "wrong_type"
        return None if int(value.text) == SIGNAL_VERSION else "unsupported_version"
    if not isinstance(value, str):
        return "wrong_type"
    return None if TEXT_RULES[field](value) else "invalid_value"


def rank(problem):
    code, field = problem
    if field is None:
        place = (-1, "")
    elif field in FIELDS:
        place = (FIELDS.index(field), "")
    else:
        place = (len(FIELDS), field)
    return (place, CODES.index(code))


def read(body):
    """("signal", fields) for a body that is a signal, else ("problems", [(code, field)])."""
    value, code = parse(body)
    if code:
        return "problems", [(code, None)]
    seen, twice = {}, []
    for key, member in value.items:
        if key in seen and key not in twice:
            twice.append(key)
        seen.setdefault(key, member)
    problems = [("duplicate_field", key) for key in twice]
    kind = None
    if "kind" not in twice and isinstance(seen.get("kind"), str) and seen["kind"] in KINDS:
        kind = seen["kind"]
    fields = {}
    for key, member in seen.items():
        if key in twice:
            continue
        if key not in FIELDS:
            problems.append(("unknown_field", key))
        elif kind == "exit" and key in ENTRY_ONLY:
            problems.append(("misplaced_field", key))
        else:
            wrong = judge(key, member)
            if wrong:
                problems.append((wrong, key))
            else:
                fields[key] = member
    for key in REQUIRED[kind] if kind else COMMON_REQUIRED:
        if key not in seen:
            problems.append(("missing_field", key))
    if "bar_time" in fields and "fire_time" in fields:
        if time_ms(fields["bar_time"]) > time_ms(fields["fire_time"]):
            problems.append(("bar_after_fire", "bar_time"))
    if problems:
        return "problems", sorted(set(problems), key=rank)
    fields["version"] = SIGNAL_VERSION
    return "signal", fields


def canonical(fields):
    """The payload a delivery is known by: every field it has but the fire time, in the
    format's order, written as compact JSON."""
    kept = {key: fields[key] for key in FIELDS if key in fields and key not in NOT_IN_CANONICAL}
    return json.dumps(kept, separators=(",", ":"), ensure_ascii=False)


def payload_hash(fields):
    return "sha256:" + hashlib.sha256(canonical(fields).encode("utf-8")).hexdigest()


def signal_id(fields):
    return ":".join(fields[key] for key in ID_FIELDS)


# ---------------------------------------------------------------------------
# The schema, read with a validator for the part of JSON Schema it uses.
# ---------------------------------------------------------------------------

SCHEMA_KEYWORDS = {"$schema", "$id", "$defs", "$ref", "title", "description", "type", "const",
                   "enum", "pattern", "minLength", "maxLength", "required", "properties",
                   "additionalProperties", "oneOf"}


def same_json(left, right):
    """JSON's equality: numbers by value, and a boolean equal only to a boolean."""
    if isinstance(left, bool) or isinstance(right, bool):
        return isinstance(left, bool) and isinstance(right, bool) and left == right
    number = (int, float)
    if isinstance(left, number) and isinstance(right, number):
        return left == right
    return type(left) is type(right) and left == right


def ecma(pattern):
    """A schema's pattern as Python reads it. The schema anchors every pattern, and Python's
    `$` also matches before a final newline where ECMA-262's does not."""
    assert pattern.startswith("^") and pattern.endswith("$") and "$" not in pattern[:-1], pattern
    return pattern[:-1] + r"\Z"


def schema_accepts(schema, value, root):
    for keyword in schema:
        assert keyword in SCHEMA_KEYWORDS, "the schema uses {}, which this validator does not read".format(keyword)
    if "$ref" in schema:
        prefix = "#/$defs/"
        assert schema["$ref"].startswith(prefix), schema["$ref"]
        if not schema_accepts(root["$defs"][schema["$ref"][len(prefix):]], value, root):
            return False
    if "oneOf" in schema and sum(schema_accepts(one, value, root) for one in schema["oneOf"]) != 1:
        return False
    kind = schema.get("type")
    assert kind in (None, "object", "string", "integer"), kind
    if kind == "object" and not isinstance(value, dict):
        return False
    if kind == "string" and not isinstance(value, str):
        return False
    if kind == "integer":
        # A number without a fractional part, however it is written: 1.0 is an integer.
        whole = isinstance(value, int) or (isinstance(value, float) and value.is_integer())
        if isinstance(value, bool) or not whole:
            return False
    if "const" in schema and not same_json(value, schema["const"]):
        return False
    if "enum" in schema and not any(same_json(value, one) for one in schema["enum"]):
        return False
    if isinstance(value, str):
        if "pattern" in schema and not re.search(ecma(schema["pattern"]), value):
            return False
        if len(value) < schema.get("minLength", 0) or len(value) > schema.get("maxLength", len(value)):
            return False
    if isinstance(value, dict):
        properties = schema.get("properties", {})
        if any(key not in value for key in schema.get("required", [])):
            return False
        if schema.get("additionalProperties") is False and any(key not in properties for key in value):
            return False
        if any(key in value and not schema_accepts(sub, value[key], root) for key, sub in properties.items()):
            return False
    return True


def schema_verdict(schema, body):
    """Whether the schema accepts the body, read as any JSON reader would: None where the
    body is no JSON at all."""
    try:
        value = json.loads(body.decode("utf-8", errors="strict"), parse_constant=refuse_constant)
    except (ValueError, RecursionError):
        return None
    return schema_accepts(schema, value, schema)


# ---------------------------------------------------------------------------
# The four product lines, each with a profile, its policy and what its venue lists.
# ---------------------------------------------------------------------------


def spec(exchange, symbol, settle, base, quote, *, contract="Perpetual", status="Trading", unit,
         face=None, face_source=None):
    """One instrument, in the stored form of the Rust `InstrumentSpec`. Only its identity,
    its assets, its kind and its status matter here; the trading rules are placeholders."""
    quantity = lambda value: {unit: value}
    return {
        "id": {"exchange": exchange, "symbol": symbol, "settle": settle},
        "base": base,
        "quote": quote,
        "contract": contract,
        "status": status,
        "delist_at_ms": None,
        "inverse": None if face is None else {"face_value": face, "source": face_source},
        "tick": "0.01",
        "step": quantity("1"),
        "market_step": quantity("1"),
        "min_qty": quantity("1"),
        "market_min_qty": quantity("1"),
        "market_max_qty": quantity("1000000"),
        "min_notional": None,
        "leverage": None,
        "observed_at_ms": 1792454400000,
    }


DATED = {"Dated": {"delivery_ms": 1798156800000}}


def linear(exchange, symbol, base, quote="USDT", **more):
    return spec(exchange, symbol, quote, base, quote, unit="Base", **more)


def inverse(exchange, symbol, base, face, source, **more):
    return spec(exchange, symbol, base, base, "USD", unit="Contracts", face=face, face_source=source, **more)


def defaults(enabled, risk=None, stop=None, leverage=None):
    return {"defaults": {"enabled": enabled, "risk_percent": risk, "stop_percent": stop, "leverage": leverage}}


def capital(amount, asset):
    return {"capital": {"amount": amount, "asset": asset}}


def asset(symbol, enabled=True, risk=None, stop="Inherit", leverage=None):
    """An asset given settings of its own. Its stop is "Inherit", "Off" or {"Percent": p}."""
    return {"asset": {"symbol": symbol, "enabled": enabled, "risk_percent": risk, "stop": stop,
                      "leverage": leverage}}


def lines():
    def identity(profile, venue, category, account, tag):
        return {"id": profile,
                "account_ref": {"venue": venue, "environment": "Demo", "venue_account_id": account},
                "category": category, "owner_tag": tag}

    binance, bybit = {"Venue": "9001234"}, {"KeyFingerprint": FINGERPRINT}
    return [
        {"name": "binance linear",
         "identity": identity("bn-linear", "Binance", "Linear", binance, "k3x9p2"),
         # Made at revision 1; each change is the next revision.
         "changes": [
             defaults(True, risk="1", stop="2", leverage="5"),
             capital("1000", "USDT"),
             asset("SOLUSDT", risk="0.5", leverage="3"),
             asset("1000SHIBUSDT", enabled=False),
             asset("LUNAUSDT"),
         ],
         "instruments": [
             linear("BinanceLinear", "SOLUSDT", "SOL"),
             linear("BinanceLinear", "BTCUSDT", "BTC"),
             linear("BinanceLinear", "1000SHIBUSDT", "1000SHIB"),
             linear("BinanceLinear", "BTCUSDT_261225", "BTC", contract=DATED),
             linear("BinanceLinear", "ETHUSDC", "ETH", quote="USDC"),
             linear("BinanceLinear", "LUNAUSDT", "LUNA", status={"Other": "SETTLING"}),
             linear("BinanceLinear", "XAUUSDT", "XAU", contract="TradFiPerpetual"),
             linear("BinanceLinear", "\u5e01\u5b89\u4eba\u751fUSDT", "\u5e01\u5b89\u4eba\u751f"),
         ]},
        {"name": "binance inverse",
         "identity": identity("bn-inverse", "Binance", "Inverse", binance, "m4y8q3"),
         "changes": [
             capital("0.5", "BTC"),
             asset("BTCUSD_PERP", risk="1", stop="Off", leverage="5"),
             asset("ETHUSD_PERP", enabled=False, stop="Off"),
         ],
         "instruments": [
             inverse("BinanceInverse", "BTCUSD_PERP", "BTC", "100", "Metadata"),
             inverse("BinanceInverse", "ETHUSD_PERP", "ETH", "10", "Metadata"),
             inverse("BinanceInverse", "SOLUSD_PERP", "SOL", "10", "Metadata"),
             inverse("BinanceInverse", "BTCUSD_261225", "BTC", "100", "Metadata", contract=DATED),
         ]},
        {"name": "bybit linear",
         "identity": identity("by-linear", "Bybit", "Linear", bybit, "n5z7r4"),
         "changes": [
             defaults(True, risk="1", leverage="5"),
             capital("1000", "USDT"),
             asset("SOLUSDT", stop={"Percent": "2"}),
             asset("BTCUSDT"),
         ],
         "instruments": [
             linear("BybitLinear", "SOLUSDT", "SOL"),
             linear("BybitLinear", "BTCUSDT", "BTC"),
             linear("BybitLinear", "XRPUSDT", "XRP"),
             linear("BybitLinear", "SHIB1000USDT", "SHIB1000"),
             linear("BybitLinear", "BTC-26DEC26", "BTC", contract=DATED),
             linear("BybitLinear", "ETHPERP", "ETH", quote="USDC"),
         ]},
        {"name": "bybit inverse",
         "identity": identity("by-inverse", "Bybit", "Inverse", bybit, "p6w5s5"),
         "changes": [
             defaults(True, risk="1", stop="3", leverage="5"),
             capital("0.5", "BTC"),
             capital("10", "SOL"),
             asset("BTCUSD", risk="2", leverage="10"),
         ],
         "instruments": [
             inverse("BybitInverse", "BTCUSD", "BTC", "1", "DocumentedRule"),
             inverse("BybitInverse", "SOLUSD", "SOL", "1", "DocumentedRule"),
             inverse("BybitInverse", "ETHUSD", "ETH", "1", "DocumentedRule"),
             inverse("BybitInverse", "BTCUSDZ26", "BTC", "1", "DocumentedRule", contract=DATED),
         ]},
        # Not a venue's list: one that names a pair twice, to show what is refused.
        {"name": "a list naming a pair twice",
         "identity": identity("twice", "Bybit", "Linear", bybit, "q7v4t6"),
         "changes": [
             defaults(True, risk="1", stop="2", leverage="5"),
             capital("1000", "USDT"),
             asset("SOLUSDT"),
         ],
         "instruments": [
             linear("BybitLinear", "SOLUSDT", "SOL"),
             linear("BybitLinear", "SOLUSDT2", "SOL"),
         ]},
    ]


LINES = {line["name"]: line for line in lines()}


def listing(line, symbol):
    return next(listed for listed in line["instruments"] if listed["id"]["symbol"] == symbol)


def policy_of(line):
    """The profile's policy once every change is made: its defaults, the assets with
    settings of their own, its capital, and the revision each part is of."""
    policy = {"revision": 1,
              "defaults": {"enabled": False, "risk_percent": None, "stop_percent": None, "leverage": None},
              "assets": {}, "capital": {}}
    for change in line["changes"]:
        policy["revision"] += 1
        if "defaults" in change:
            policy["defaults"] = dict(change["defaults"])
        elif "capital" in change:
            policy["capital"][change["capital"]["asset"]] = {"amount": change["capital"]["amount"],
                                                             "revision": policy["revision"]}
        else:
            own = dict(change["asset"])
            # An asset is held as the venue lists it, settlement asset included.
            own["settle"] = listing(line, own["symbol"])["id"]["settle"]
            policy["assets"][own["symbol"]] = own
        usable = lambda settings: settings["risk_percent"] and settings["leverage"]
        base = policy["defaults"]
        assert not base["enabled"] or usable(base), "enabled defaults need a risk and a leverage"
        for own in policy["assets"].values():
            has = (own["risk_percent"] or base["risk_percent"]) and (own["leverage"] or base["leverage"])
            assert not own["enabled"] or has, "{} is enabled without a risk or a leverage".format(own["symbol"])
    return policy


POLICIES = {name: policy_of(line) for name, line in LINES.items()}


def universe_refusal(listed):
    """Universe rule 1: a crypto perpetual in trading status, its linear form settled in USDT."""
    if listed["contract"] != "Perpetual":
        return "outside_universe"
    if listed["status"] != "Trading":
        return "outside_universe"
    if listed["id"]["exchange"].endswith("Linear") and listed["id"]["settle"] != "USDT":
        return "outside_universe"
    return None


def entry_snapshot(line, policy, listed):
    """(snapshot, None) for the policy an entry on `listed` is admitted under, in the stored
    form of the Rust `PolicySnapshot`, or (None, refusal)."""
    symbol, settle = listed["id"]["symbol"], listed["id"]["settle"]
    own, base = policy["assets"].get(symbol), policy["defaults"]
    if own and own["settle"] != settle:
        return None, "ambiguous_selection"
    if not (own["enabled"] if own else base["enabled"]):
        return None, "not_armed" if own else "unknown_ticker"
    outside = universe_refusal(listed)
    if outside:
        return None, outside
    held = policy["capital"].get(settle)
    if not held:
        return None, "no_capital_in_settlement_asset"

    def pick(name):
        if own and own[name] is not None:
            return {"value": own[name], "source": "Asset"}
        return {"value": base[name], "source": "Defaults"}

    stop = own["stop"] if own else "Inherit"
    if stop == "Inherit":
        stop_percent = {"value": base["stop_percent"], "source": "Defaults"}
    elif stop == "Off":
        stop_percent = {"value": None, "source": "Asset"}
    else:
        stop_percent = {"value": stop["Percent"], "source": "Asset"}
    return {
        "format": 1,
        "profile": line["identity"]["id"],
        "account": line["identity"]["account_ref"],
        "revision": policy["revision"],
        "instrument": listed["id"],
        "risk_percent": pick("risk_percent"),
        "stop_percent": stop_percent,
        "leverage": pick("leverage"),
        "capital": {"amount": held["amount"], "asset": settle},
        "capital_revision": held["revision"],
    }, None


# ---------------------------------------------------------------------------
# Resolving a signal for the profile its URL selected.
# ---------------------------------------------------------------------------


def find(line, policy, ticker):
    """(instrument, spelling, None) for the instrument a ticker names, or (None, None,
    refusal). A ticker is the venue's own symbol of an asset the profile has settings for,
    or TradingView's name for a perpetual: its base and quote assets and `.P`. Nothing else
    is tried, so a symbol the profile was never given settings for names nothing."""
    for listed in line["instruments"]:
        if listed["id"]["symbol"] == ticker and ticker in policy["assets"]:
            return listed, "venue_symbol", None
    if not ticker.endswith(".P"):
        return None, None, "unknown_ticker"
    pair = ticker[:-2]
    named = [listed for listed in line["instruments"]
             if listed["contract"] in ("Perpetual", "TradFiPerpetual")
             and listed["base"] + listed["quote"] == pair]
    if not named:
        return None, None, "unknown_ticker"
    if len(named) > 1:
        return None, None, "ambiguous_selection"
    return named[0], "tradingview", None


def age_refusal(kind, fire_ms, now_ms, limit_s):
    """Only an entry is held to the clock: one fired further ahead of it than the
    allowance, or longer ago than the profile's limit, is refused. An exit never is: no
    clock holds back a close."""
    if kind != "entry":
        return None
    if fire_ms - now_ms > FUTURE_ALLOWANCE_MS:
        return "future_timestamp"
    if now_ms - fire_ms > limit_s * 1000:
        return "stale"
    return None


# The parts of a signal a refusal can be over, in the order the rules are looked for.
RESOLUTION_ORDER = ["profile", "exchange", "category", "fire_time", "ticker", "policy", "stop"]


def fallback_of(policy, listed):
    """The fallback stop the policy gives an instrument, enabled or not."""
    own = policy["assets"].get(listed["id"]["symbol"])
    stop = own["stop"] if own else "Inherit"
    if stop == "Inherit":
        return policy["defaults"]["stop_percent"]
    return None if stop == "Off" else stop["Percent"]


def failing(line, fields, now_ms, limit_s):
    """Every part of a signal that breaks its rule, each rule asked on its own. A signal is
    refused over the first of them; the rest show which rule comes before which. The
    policy and the stop can be asked only once the ticker names an instrument."""
    identity, policy = line["identity"], POLICIES[line["name"]]
    broken = []
    if fields["profile"] != identity["id"]:
        broken.append("profile")
    if "exchange" in fields and fields["exchange"] != identity["account_ref"]["venue"].upper():
        broken.append("exchange")
    if "category" in fields and fields["category"] != identity["category"].lower():
        broken.append("category")
    if age_refusal(fields["kind"], time_ms(fields["fire_time"]), now_ms, limit_s):
        broken.append("fire_time")
    listed, _, refusal = find(line, policy, fields["ticker"])
    if refusal:
        broken.append("ticker")
    elif fields["kind"] == "entry":
        if entry_snapshot(line, policy, listed)[1]:
            broken.append("policy")
        if "stop" not in fields and fallback_of(policy, listed) is None:
            broken.append("stop")
    return broken


def resolve(line, fields, now_ms, limit_s):
    """What a signal asks of the profile its URL selected: an entry with the policy it is
    admitted under, or an exit, on one instrument; or the first refusal, with the part of
    the signal it is over."""
    identity, policy = line["identity"], POLICIES[line["name"]]
    if fields["profile"] != identity["id"]:
        return {"refused": "ambiguous_selection", "over": "profile"}
    if "exchange" in fields and fields["exchange"] != identity["account_ref"]["venue"].upper():
        return {"refused": "ambiguous_selection", "over": "exchange"}
    if "category" in fields and fields["category"] != identity["category"].lower():
        return {"refused": "ambiguous_selection", "over": "category"}
    fire_ms = time_ms(fields["fire_time"])
    late = age_refusal(fields["kind"], fire_ms, now_ms, limit_s)
    if late:
        return {"refused": late, "over": "fire_time"}
    listed, spelling, refusal = find(line, policy, fields["ticker"])
    if refusal:
        return {"refused": refusal, "over": "ticker"}
    resolved = {
        "kind": fields["kind"],
        "side": fields["side"],
        "instrument": listed["id"],
        "account": identity["account_ref"],
        "spelling": spelling,
        "signal_id": signal_id(fields),
        "signal_time_ms": fire_ms,
    }
    if fields["kind"] == "entry":
        snapshot, refusal = entry_snapshot(line, policy, listed)
        if refusal:
            return {"refused": refusal, "over": "policy"}
        if "stop" not in fields and snapshot["stop_percent"]["value"] is None:
            return {"refused": "no_stop", "over": "stop"}
        resolved["expected_price"] = fields["price"]
        resolved["stop"] = fields.get("stop")
        resolved["policy"] = snapshot
    return {"resolved": resolved}


# ---------------------------------------------------------------------------
# The payload cases.
# ---------------------------------------------------------------------------

T0 = "2026-10-20T00:00:00Z"   # a bar's time
T1 = "2026-10-20T00:15:00Z"   # the fire time of an alert on that bar's close
T1_MS = 1792455300000


def body_of(fields, **changes):
    """A payload as a template renders it: one member a line, in the format's order.
    A change of None takes the member out."""
    merged = dict(fields)
    merged.update(changes)
    ordered = [key for key in FIELDS if key in merged] + [key for key in merged if key not in FIELDS]
    members = ",\n".join("  {}: {}".format(json.dumps(key), json.dumps(merged[key], ensure_ascii=False))
                         for key in ordered if merged[key] is not None)
    return "{\n" + members + "\n}"


def entry(profile, ticker, side, price, **more):
    fields = {"version": 1, "kind": "entry", "profile": profile, "source": "trend",
              "ticker": ticker, "side": side, "bar_time": T0, "fire_time": T1, "price": price}
    fields.update(more)
    return fields


def exit_(profile, ticker, side, **more):
    fields = {"version": 1, "kind": "exit", "profile": profile, "source": "trend",
              "ticker": ticker, "side": side, "bar_time": T0, "fire_time": T1}
    fields.update(more)
    return fields


BASE = entry("by-linear", "SOLUSDT.P", "long", "234.23", stop="200.00")
BASE_EXIT = exit_("by-linear", "SOLUSDT.P", "long")
LARGEST = str(MANTISSA_LIMIT - 1)

# (id, note, body): a body is text, or bytes where it is not text at all.
VALID = [
    ("bn-linear-long-entry", "Binance USD-M LONG entry, the alert's own stop",
     body_of(entry("bn-linear", "SOLUSDT.P", "long", "234.23", stop="200.00"))),
    ("bn-linear-short-entry", "Binance USD-M SHORT entry without a stop: the profile's fallback applies",
     body_of(entry("bn-linear", "SOLUSDT.P", "short", "234.23"))),
    ("bn-linear-long-exit", "Binance USD-M exit of a LONG",
     body_of(exit_("bn-linear", "SOLUSDT.P", "long"))),
    ("bn-linear-short-exit", "Binance USD-M exit of a SHORT, as a take-profit alert sends it",
     body_of(exit_("bn-linear", "SOLUSDT.P", "short", source="take-profit"))),
    ("bn-inverse-long-entry", "Binance COIN-M LONG entry: TradingView's BTCUSD.P is the venue's BTCUSD_PERP",
     body_of(entry("bn-inverse", "BTCUSD.P", "long", "84123.4", stop="81600.0"))),
    ("bn-inverse-short-entry", "Binance COIN-M SHORT entry, naming the venue's own symbol",
     body_of(entry("bn-inverse", "BTCUSD_PERP", "short", "84123.4", stop="86650.0"))),
    ("bn-inverse-long-exit", "Binance COIN-M exit of a LONG",
     body_of(exit_("bn-inverse", "BTCUSD.P", "long"))),
    ("bn-inverse-short-exit", "Binance COIN-M exit of a SHORT",
     body_of(exit_("bn-inverse", "BTCUSD.P", "short"))),
    ("by-linear-long-entry", "Bybit linear LONG entry, the alert's own stop",
     body_of(BASE)),
    ("by-linear-short-entry", "Bybit linear SHORT entry, asserting its exchange and its family",
     body_of(entry("by-linear", "SOLUSDT.P", "short", "234.23", stop="240.00", exchange="BYBIT",
                   category="linear"))),
    ("by-linear-long-exit", "Bybit linear exit of a LONG",
     body_of(BASE_EXIT)),
    ("by-linear-short-exit", "Bybit linear exit of a SHORT",
     body_of(exit_("by-linear", "SOLUSDT.P", "short"))),
    ("by-inverse-long-entry", "Bybit inverse LONG entry without a stop: the profile's fallback applies",
     body_of(entry("by-inverse", "BTCUSD.P", "long", "84123.4"))),
    ("by-inverse-short-entry", "Bybit inverse SHORT entry, the alert's own stop",
     body_of(entry("by-inverse", "BTCUSD.P", "short", "84123.4", stop="86650.0"))),
    ("by-inverse-long-exit", "Bybit inverse exit of a LONG, naming the venue's own symbol",
     body_of(exit_("by-inverse", "BTCUSD", "long"))),
    ("by-inverse-short-exit", "Bybit inverse exit of a SHORT",
     body_of(exit_("by-inverse", "BTCUSD.P", "short"))),
    ("compact", "The same signal on one line: spacing is not part of a payload",
     json.dumps(BASE, separators=(",", ":"))),
    ("as-large-as-a-body-may-be", "The same signal, spaced out to the size limit exactly",
     body_of(BASE)[:-2] + " " * (MAX_SIGNAL_BYTES - len(body_of(BASE))) + "\n}"),
    ("members-in-another-order", "The same signal with its members in another order",
     body_of({key: BASE[key] for key in reversed(FIELDS) if key in BASE})),
    ("a-later-fire-time", "The same signal fired again 10 seconds later, as a resend may render it: "
     "the same ID and the same hash",
     body_of(BASE, fire_time="2026-10-20T00:15:10Z")),
    ("another-price", "The same ID with another price: another hash, so a conflict if it follows the first",
     body_of(BASE, price="234.24")),
    ("price-with-trailing-zeros", "A price keeps the digits it was sent with: 234.230 is not 234.23 to the hash",
     body_of(BASE, price="234.230")),
    ("another-stop", "The same ID with another stop: another hash",
     body_of(BASE, stop="201.00")),
    ("without-its-stop", "The same ID without its stop: another hash",
     body_of(BASE, stop=None)),
    ("another-bar", "The same alert on the next bar: another ID",
     body_of(BASE, bar_time="2026-10-20T00:15:00Z", fire_time="2026-10-20T00:30:00Z")),
    ("another-source", "Another alert on the same bar: another ID",
     body_of(BASE, source="breakout")),
    ("bar-time-is-the-fire-time", "An alert fired the second its bar opened",
     body_of(BASE, fire_time=T0)),
    ("non-ascii-ticker", "A ticker beyond ASCII, as a venue lists some",
     body_of(entry("bn-linear", "\u5e01\u5b89\u4eba\u751fUSDT.P", "long", "0.1234"))),
    ("smallest-price", "The smallest price an exact decimal holds",
     body_of(BASE, price="0." + "0" * 27 + "1", stop=None)),
    ("largest-price", "The largest price an exact decimal holds",
     body_of(BASE, price=LARGEST, stop=None)),
    ("leap-day", "A bar on a leap day",
     body_of(BASE, bar_time="2028-02-29T23:59:59Z", fire_time="2028-03-01T00:00:00Z")),
    ("century-leap-day", "A leap day in 2000: a year divisible by 400 is a leap year",
     body_of(BASE, bar_time="2000-02-29T12:00:00Z", fire_time="2000-02-29T12:00:00Z")),
    ("months-of-31-days", "The last day of January and of March",
     body_of(BASE, bar_time="2026-01-31T00:00:00Z", fire_time="2026-03-31T23:59:59Z")),
    ("more-months-of-31-days", "Of May and of July",
     body_of(BASE, bar_time="2026-05-31T00:00:00Z", fire_time="2026-07-31T23:59:59Z")),
    ("the-other-months-of-31-days", "Of August and of October",
     body_of(BASE, bar_time="2026-08-31T00:00:00Z", fire_time="2026-10-31T23:59:59Z")),
    ("december-31", "And of December, after the last day of February", body_of(
        BASE, bar_time="2026-02-28T00:00:00Z", fire_time="2026-12-31T23:59:59Z")),
    ("months-of-30-days", "The last day of April and of June",
     body_of(BASE, bar_time="2026-04-30T00:00:00Z", fire_time="2026-06-30T23:59:59Z")),
    ("more-months-of-30-days", "Of September and of November",
     body_of(BASE, bar_time="2026-09-30T00:00:00Z", fire_time="2026-11-30T23:59:59Z")),
    ("first-second", "The first second a time can name",
     body_of(BASE, bar_time="1970-01-01T00:00:00Z", fire_time="1970-01-01T00:00:00Z")),
    ("last-second", "The last second a time can name",
     body_of(BASE, bar_time="9999-12-31T23:59:59Z", fire_time="9999-12-31T23:59:59Z")),
    ("longest-names", "The longest profile, source and ticker: the ID still fits",
     body_of(BASE, profile="p" * 32, source="s" * 32, ticker="T" * 40)),
    ("widest-ticker", "Forty characters are forty characters, however many bytes each takes",
     body_of(BASE, ticker="\u5e01" * 40)),
    ("escaped-text", "Text written with JSON escapes is the text it spells",
     body_of(BASE).replace('"trend"', '"tr\\u0065nd"').replace('"long"', '"l\\u006fng"')),
]


def without(fields, *names):
    return {key: value for key, value in fields.items() if key not in names}


def raw(text):
    return text.encode("utf-8")


def nested(arrays):
    """A body whose version is `arrays` empty lists, one inside the other."""
    return '{"version":' + "[" * arrays + "]" * arrays + "}"


# (id, note, body, expected problems as (code, field) pairs)
INVALID = [
    ("empty", "An empty body", "", [("not_json", None)]),
    ("plain-text", "An alert message that is not JSON: TradingView sends it as text/plain",
     "SOLUSDT.P Crossing 234.23", [("not_json", None)]),
    ("truncated", "A payload cut short", body_of(BASE)[:-10], [("not_json", None)]),
    ("two-values", "Two JSON values", body_of(BASE) + body_of(BASE), [("not_json", None)]),
    ("trailing-comma", "A comma after the last member", body_of(BASE)[:-2] + ",\n}", [("not_json", None)]),
    ("single-quotes", "Single quotes", body_of(BASE).replace('"', "'"), [("not_json", None)]),
    ("a-comment", "A comment", "// entry\n" + body_of(BASE), [("not_json", None)]),
    ("byte-order-mark", "A byte order mark before the object", b"\xef\xbb\xbf" + raw(body_of(BASE)),
     [("not_json", None)]),
    ("not-utf8", "Bytes that are not UTF-8", raw(body_of(BASE)).replace(b"trend", b"tr\xffnd"),
     [("not_json", None)]),
    ("lone-surrogate", "An escape that names half a character",
     body_of(BASE).replace("trend", "tr\\ud800nd"), [("not_json", None)]),
    ("raw-control-character", "A control character inside a string, unescaped",
     body_of(BASE).replace("trend", "tr\x01nd"), [("not_json", None)]),
    ("nan", "NaN is not JSON", body_of(BASE).replace('"234.23"', "NaN"), [("not_json", None)]),
    ("number-with-a-leading-zero", "A number with a leading zero is not JSON",
     body_of(BASE).replace('"version": 1', '"version": 01'), [("not_json", None)]),
    ("number-no-double-holds", "A number too large for a double is not read as one",
     body_of(BASE).replace('"version": 1', '"version": 1e999'), [("not_json", None)]),
    ("integer-no-double-holds", "Nor an integer of four hundred digits",
     body_of(BASE).replace('"version": 1', '"version": 1' + "0" * 399), [("not_json", None)]),
    ("nested-too-deep", "Nesting past the limit is not read at all",
     nested(MAX_DEPTH), [("not_json", None)]),
    ("too-large", "A body over the size limit, whatever it holds",
     body_of(BASE)[:-2] + " " * MAX_SIGNAL_BYTES + "\n}", [("too_large", None)]),
    ("a-byte-too-large", "By as little as a byte",
     body_of(BASE)[:-2] + " " * (MAX_SIGNAL_BYTES + 1 - len(body_of(BASE))) + "\n}", [("too_large", None)]),
    ("an-array", "An array of signals: one delivery is one signal", "[" + body_of(BASE) + "]",
     [("not_an_object", None)]),
    ("a-string", "A JSON string", json.dumps(body_of(BASE)), [("not_an_object", None)]),
    ("a-number", "A JSON number", "1", [("not_an_object", None)]),
    ("null", "A JSON null", "null", [("not_an_object", None)]),
    ("duplicate-side", "A member written twice: readers disagree on which one counts",
     body_of(BASE)[:-2] + ',\n  "side": "short"\n}', [("duplicate_field", "side")]),
    ("duplicate-kind", "A kind written twice says nothing of what the payload is",
     body_of(without(BASE, "price", "stop"))[:-2] + ',\n  "kind": "exit"\n}', [("duplicate_field", "kind")]),
    ("duplicate-unknown", "An unknown member written twice is reported once, as written twice",
     body_of(BASE)[:-2] + ',\n  "note": "a",\n  "note": "b"\n}', [("duplicate_field", "note")]),
    ("unknown-field", "A member the format does not define",
     body_of(BASE, comment="breakout"), [("unknown_field", "comment")]),
    ("field-in-capitals", "A member's name is written as the format writes it",
     body_of(without(BASE, "side"), Side="long"), [("missing_field", "side"), ("unknown_field", "Side")]),
    ("asks-for-a-quantity", "A payload cannot size an entry: there is no quantity to send",
     body_of(BASE, qty="10"), [("unknown_field", "qty")]),
    ("asks-for-more-risk", "A payload cannot raise the operator's risk",
     body_of(BASE, risk_percent="5"), [("unknown_field", "risk_percent")]),
    ("asks-for-leverage", "Nor choose the leverage",
     body_of(BASE, leverage="50"), [("unknown_field", "leverage")]),
    ("asks-for-live-trading", "Nor switch live trading on",
     body_of(BASE, mode="live"), [("unknown_field", "mode")]),
    ("names-an-account", "Nor choose an account or its credentials",
     body_of(BASE, account="9001234", api_key="none"), [("unknown_field", "account"), ("unknown_field", "api_key")]),
    ("names-an-environment", "Nor an environment",
     body_of(BASE, environment="mainnet"), [("unknown_field", "environment")]),
    ("exit-with-a-price", "An exit carries no price", body_of(BASE_EXIT, price="250.00"),
     [("misplaced_field", "price")]),
    ("exit-with-a-stop", "Nor a stop", body_of(BASE_EXIT, stop="200.00"), [("misplaced_field", "stop")]),
    ("no-version", "No version", body_of(without(BASE, "version")), [("missing_field", "version")]),
    ("no-profile", "No profile: every payload names the profile it is for",
     body_of(without(BASE, "profile")), [("missing_field", "profile")]),
    ("no-price", "An entry without its expected price", body_of(without(BASE, "price")),
     [("missing_field", "price")]),
    ("no-kind", "No kind: a price is then not missed, since only an entry needs one",
     body_of(without(BASE, "kind", "price")), [("missing_field", "kind")]),
    ("an-empty-object", "An object with nothing in it", "{}",
     [("missing_field", key) for key in COMMON_REQUIRED]),
    ("brackets-in-text", "Brackets inside a string open nothing: the body is read, and its member "
     "found unknown", body_of(BASE, note="[{" * 20), [("unknown_field", "note")]),
    ("brackets-after-an-escaped-quote", "Nor do brackets after a quote the string escapes",
     body_of(BASE, note='"' + "[{" * 20), [("unknown_field", "note")]),
    ("brackets-after-an-escaped-backslash", "A backslash the string escapes does not escape the quote "
     "that ends it", body_of(BASE, note="[{" * 20 + "\\", more="[{" * 20),
     [("unknown_field", "more"), ("unknown_field", "note")]),
    ("nested-to-the-limit", "Nesting up to the limit is read, and found to be the wrong type",
     nested(MAX_DEPTH - 1),
     [("wrong_type", "version")] + [("missing_field", key) for key in COMMON_REQUIRED if key != "version"]),
    ("version-as-text", "The version is a number, not text", body_of(BASE, version="1"),
     [("wrong_type", "version")]),
    ("version-with-a-fraction", "And a whole one", body_of(BASE).replace('"version": 1', '"version": 1.0'),
     [("wrong_type", "version")]),
    ("version-with-an-exponent", "Written as one", body_of(BASE).replace('"version": 1', '"version": 1e0'),
     [("wrong_type", "version")]),
    ("version-with-a-sign", "Without a sign", body_of(BASE).replace('"version": 1', '"version": -0'),
     [("wrong_type", "version")]),
    ("version-below-zero", "And not below zero", body_of(BASE, version=-1), [("wrong_type", "version")]),
    ("version-true", "A boolean is not a number", body_of(BASE, version=True), [("wrong_type", "version")]),
    ("version-beyond-exact-integers", "A version past 2^53 - 1 is not an integer every reader holds",
     body_of(BASE, version=2 ** 53), [("wrong_type", "version")]),
    ("price-as-a-number", "A price without its quotes is a JSON number, which a float may already have "
     "rounded: write \"{{close}}\", in quotes", body_of(BASE).replace('"234.23"', "234.23"),
     [("wrong_type", "price")]),
    ("stop-is-null", "A stop left out is left out, not null",
     body_of(BASE).replace('"200.00"', "null"), [("wrong_type", "stop")]),
    ("side-as-a-list", "A value that is a list", body_of(BASE).replace('"long"', '["long"]'),
     [("wrong_type", "side")]),
    ("kind-as-a-number", "A kind that is no text says nothing of what the payload is",
     body_of(without(BASE, "price", "stop")).replace('"entry"', "1"), [("wrong_type", "kind")]),
    ("profile-as-an-object", "A value that is an object",
     body_of(BASE).replace('"by-linear"', '{"id": "by-linear"}'), [("wrong_type", "profile")]),
    ("source-as-a-number", "A source that is a number", body_of(BASE, source=7), [("wrong_type", "source")]),
    ("ticker-as-a-number", "A ticker that is a number", body_of(BASE, ticker=1000), [("wrong_type", "ticker")]),
    ("exchange-is-null", "An assertion left out is left out, not null",
     body_of(BASE).replace('"kind"', '"exchange": null,\n  "kind"'), [("wrong_type", "exchange")]),
    ("category-as-a-boolean", "A family that is a boolean", body_of(BASE, category=True),
     [("wrong_type", "category")]),
    ("bar-time-as-a-number", "A time as a number of milliseconds", body_of(BASE, bar_time=1792454400000),
     [("wrong_type", "bar_time")]),
    ("fire-time-as-a-number", "Or of seconds", body_of(BASE, fire_time=1792455300),
     [("wrong_type", "fire_time")]),
    ("a-later-version", "A version this format is not", body_of(BASE, version=2),
     [("unsupported_version", "version")]),
    ("version-zero", "Version zero", body_of(BASE, version=0), [("unsupported_version", "version")]),
    ("largest-exact-version", "The largest version an exact integer holds is still not this one",
     body_of(BASE, version=2 ** 53 - 1), [("unsupported_version", "version")]),
    ("kind-buy", "A kind is entry or exit, never an order's buy or sell", body_of(BASE, kind="buy"),
     [("invalid_value", "kind")]),
    ("kind-in-capitals", "In lowercase", body_of(BASE, kind="Entry"), [("invalid_value", "kind")]),
    ("side-buy", "A side is long or short", body_of(BASE, side="buy"), [("invalid_value", "side")]),
    ("side-flat", "And never flat", body_of(BASE_EXIT, side="flat"), [("invalid_value", "side")]),
    ("side-in-capitals", "In lowercase", body_of(BASE, side="LONG"), [("invalid_value", "side")]),
    ("profile-in-capitals", "A profile is named as it was created", body_of(BASE, profile="By-Linear"),
     [("invalid_value", "profile")]),
    ("profile-starting-with-a-hyphen", "Starting with a letter or a digit", body_of(BASE, profile="-linear"),
     [("invalid_value", "profile")]),
    ("profile-too-long", "In at most 32 characters", body_of(BASE, profile="p" * 33),
     [("invalid_value", "profile")]),
    ("profile-empty", "And at least one", body_of(BASE, profile=""), [("invalid_value", "profile")]),
    ("source-with-a-space", "A source is one word", body_of(BASE, source="trend follower"),
     [("invalid_value", "source")]),
    ("source-with-a-colon", "Without the colon an ID is joined with", body_of(BASE, source="trend:fast"),
     [("invalid_value", "source")]),
    ("source-too-long", "Of at most 32 characters", body_of(BASE, source="s" * 33),
     [("invalid_value", "source")]),
    ("source-with-a-line-break", "On one line", body_of(BASE, source="trend\n"),
     [("invalid_value", "source")]),
    ("ticker-in-lowercase", "A ticker is written as TradingView renders it", body_of(BASE, ticker="solusdt.p"),
     [("invalid_value", "ticker")]),
    ("ticker-with-its-exchange", "Without the exchange before it", body_of(BASE, ticker="BYBIT:SOLUSDT.P"),
     [("invalid_value", "ticker")]),
    ("ticker-with-a-space", "And without spaces", body_of(BASE, ticker="SOL USDT"),
     [("invalid_value", "ticker")]),
    ("ticker-with-a-hidden-space", "Seen or unseen", body_of(BASE, ticker="SOLUSDT\u200b.P"),
     [("invalid_value", "ticker")]),
    ("ticker-with-a-wide-space", "In any script", body_of(BASE, ticker="\u5e01\u5b89\u3000USDT.P"),
     [("invalid_value", "ticker")]),
    ("ticker-with-a-quote", "Nor quotes", body_of(BASE, ticker='SOL"USDT'), [("invalid_value", "ticker")]),
    ("ticker-empty", "An empty ticker", body_of(BASE, ticker=""), [("invalid_value", "ticker")]),
    ("ticker-too-long", "A ticker of more than 40 characters", body_of(BASE, ticker="A" * 41),
     [("invalid_value", "ticker")]),
    ("ticker-unrendered", "A placeholder TradingView did not fill in", body_of(BASE, ticker="{{ticker}}"),
     [("invalid_value", "ticker")]),
    ("exchange-in-lowercase", "An exchange is written as {{exchange}} renders it", body_of(BASE, exchange="bybit"),
     [("invalid_value", "exchange")]),
    ("exchange-empty", "And is not empty", body_of(BASE, exchange=""), [("invalid_value", "exchange")]),
    ("category-spot", "A signal trades futures: its family is linear or inverse",
     body_of(BASE, category="spot"), [("invalid_value", "category")]),
    ("category-in-capitals", "In lowercase", body_of(BASE, category="Linear"),
     [("invalid_value", "category")]),
    ("time-with-an-offset", "A time is UTC, written with Z", body_of(BASE, fire_time="2026-10-20T02:15:00+02:00"),
     [("invalid_value", "fire_time")]),
    ("time-with-fractions", "To the second", body_of(BASE, fire_time="2026-10-20T00:15:00.000Z"),
     [("invalid_value", "fire_time")]),
    ("time-with-a-space", "With T between the day and the hour", body_of(BASE, fire_time="2026-10-20 00:15:00Z"),
     [("invalid_value", "fire_time")]),
    ("time-in-lowercase", "In capitals", body_of(BASE, fire_time="2026-10-20t00:15:00z"),
     [("invalid_value", "fire_time")]),
    ("time-ending-in-a-small-z", "The Z as well", body_of(BASE, fire_time="2026-10-20T00:15:00z"),
     [("invalid_value", "fire_time")]),
    ("time-with-a-letter", "A letter where a digit belongs", body_of(BASE, bar_time="2O26-10-20T00:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-without-its-seconds", "A time to the minute", body_of(BASE, fire_time="2026-10-20T00:15Z"),
     [("invalid_value", "fire_time")]),
    ("time-as-milliseconds", "As TradingView renders it, not as a number of milliseconds",
     body_of(BASE, bar_time="1792454400000"), [("invalid_value", "bar_time")]),
    ("time-of-no-day", "A day the calendar does not have", body_of(BASE, bar_time="2026-02-30T00:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-of-no-leap-day", "A leap day in a year without one", body_of(BASE, bar_time="2026-02-29T00:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-of-no-century-leap-day", "2100 is not a leap year", body_of(
        BASE, bar_time="2100-02-29T00:00:00Z", fire_time="2100-03-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-no-april-31", "April has 30 days", body_of(
        BASE, bar_time="2026-04-31T00:00:00Z", fire_time="2026-05-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-no-june-31", "So has June", body_of(
        BASE, bar_time="2026-06-31T00:00:00Z", fire_time="2026-07-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-no-september-31", "And September", body_of(
        BASE, bar_time="2026-09-31T00:00:00Z", fire_time="2026-10-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-no-november-31", "And November", body_of(
        BASE, bar_time="2026-11-31T00:00:00Z", fire_time="2026-12-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-day-32", "No month has 32", body_of(
        BASE, bar_time="2026-10-32T00:00:00Z", fire_time="2026-11-01T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-of-no-month", "A month the calendar does not have", body_of(BASE, bar_time="2026-13-01T00:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-of-day-zero", "Day zero", body_of(BASE, bar_time="2026-10-00T00:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-of-no-hour", "An hour the clock does not have", body_of(BASE, bar_time="2026-10-20T24:00:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-of-no-minute", "A minute the clock does not have", body_of(BASE, bar_time="2026-10-19T23:60:00Z"),
     [("invalid_value", "bar_time")]),
    ("time-with-a-leap-second", "A leap second", body_of(BASE, fire_time="2026-12-31T23:59:60Z"),
     [("invalid_value", "fire_time")]),
    ("time-before-1970", "A time before 1970", body_of(BASE, bar_time="1969-12-31T23:59:59Z"),
     [("invalid_value", "bar_time")]),
    ("time-in-other-digits", "Digits of another script", body_of(
        BASE, bar_time="\u0662\u0660\u0662\u0666-10-20T00:00:00Z"), [("invalid_value", "bar_time")]),
    ("time-unrendered", "A placeholder TradingView did not fill in", body_of(BASE, bar_time="{{time}}"),
     [("invalid_value", "bar_time")]),
    ("price-zero", "A price of nothing", body_of(BASE, price="0"), [("invalid_value", "price")]),
    ("price-zero-with-places", "However it is written", body_of(BASE, price="0.000"),
     [("invalid_value", "price")]),
    ("price-below-zero", "A price below zero", body_of(BASE, price="-234.23"), [("invalid_value", "price")]),
    ("price-with-a-plus", "A price with a plus sign", body_of(BASE, price="+234.23"),
     [("invalid_value", "price")]),
    ("price-with-an-exponent", "A price with an exponent", body_of(BASE, price="2.3423e2"),
     [("invalid_value", "price")]),
    ("price-with-a-separator", "A price with a thousands separator", body_of(BASE, price="84,123.4"),
     [("invalid_value", "price")]),
    ("price-with-a-leading-zero", "A price with a leading zero", body_of(BASE, price="0234.23"),
     [("invalid_value", "price")]),
    ("price-with-no-integer-part", "A price that starts with its point", body_of(BASE, price=".5"),
     [("invalid_value", "price")]),
    ("price-ending-in-its-point", "Or ends with it", body_of(BASE, price="234."),
     [("invalid_value", "price")]),
    ("price-with-spaces", "A price with spaces around it", body_of(BASE, price=" 234.23"),
     [("invalid_value", "price")]),
    ("price-below-the-smallest", "A price of 29 decimal places, however few its digits",
     body_of(BASE, price="0." + "0" * 28 + "1"), [("invalid_value", "price")]),
    ("price-too-precise", "More decimal places than an exact decimal holds",
     body_of(BASE, price="1." + "0" * 28 + "1"), [("invalid_value", "price")]),
    ("price-with-too-many-zeros", "Even when the places beyond the 28th are zeros",
     body_of(BASE, price="1." + "0" * 29), [("invalid_value", "price")]),
    ("price-too-large", "More digits than an exact decimal holds", body_of(BASE, price=str(MANTISSA_LIMIT)),
     [("invalid_value", "price")]),
    ("price-unrendered", "A placeholder TradingView did not fill in", body_of(BASE, price="{{close}}"),
     [("invalid_value", "price")]),
    ("stop-not-a-number", "A stop the indicator had no value for: the alert is refused, and nothing "
     "falls back to the profile's percentage", body_of(BASE, stop="NaN"), [("invalid_value", "stop")]),
    ("stop-na", "However the missing value is written", body_of(BASE, stop="na"), [("invalid_value", "stop")]),
    ("stop-empty", "An empty stop", body_of(BASE, stop=""), [("invalid_value", "stop")]),
    ("stop-zero", "A stop of nothing", body_of(BASE, stop="0"), [("invalid_value", "stop")]),
    ("bar-after-fire", "A bar that opens after its alert fired: the two times are swapped",
     body_of(BASE, bar_time=T1, fire_time=T0), [("bar_after_fire", "bar_time")]),
    ("bar-a-second-after-fire", "By as little as a second",
     body_of(BASE, bar_time="2026-10-20T00:15:01Z"), [("bar_after_fire", "bar_time")]),
    ("several-problems", "Everything wrong with a payload is said at once",
     body_of(BASE, kind="exit", side="buy", version=3, note="x"),
     [("unsupported_version", "version"), ("invalid_value", "side"), ("misplaced_field", "price"),
      ("misplaced_field", "stop"), ("unknown_field", "note")]),
    ("unknown-fields-in-order", "Unknown members are listed after the known ones, in the order of "
     "their names",
     body_of(BASE, zeta="1", Alpha="2", alpha="3"),
     [("unknown_field", "Alpha"), ("unknown_field", "alpha"), ("unknown_field", "zeta")]),
]

# ---------------------------------------------------------------------------
# The alert templates: an alert's message, as an operator pastes it.
# ---------------------------------------------------------------------------


def template(kind, side, stop=None, exchange=False):
    fields = {"version": 1, "kind": kind, "profile": "main", "source": "trend",
              "ticker": "{{ticker}}", "side": side, "bar_time": "{{time}}", "fire_time": "{{timenow}}"}
    if exchange:
        fields["exchange"] = "{{exchange}}"
    if kind == "entry":
        fields["price"] = "{{close}}"
        if stop:
            fields["stop"] = stop
    text = body_of(fields)
    # A template is the text an operator pastes as the alert's message. TradingView reads
    # a placeholder as it is written there, quotes included, so a template that names a
    # plot is JSON only once it is rendered.
    for placeholder in RENDERED_WITH:
        text = text.replace(json.dumps(placeholder)[1:-1], placeholder)
    return text


# What TradingView would put in the placeholders of an alert on a Bybit SOLUSDT.P chart.
RENDERED_WITH = {
    "{{ticker}}": "SOLUSDT.P",
    "{{exchange}}": "BYBIT",
    "{{time}}": T0,
    "{{timenow}}": T1,
    "{{close}}": "234.23",
    "{{plot_0}}": "200.5",
    '{{plot("Stop")}}': "240.75",
}
PLACEHOLDER = re.compile(r"\{\{.*?\}\}")

TEMPLATES = [
    ("entry-long", "A LONG entry at the bar's close, on the profile's fallback stop",
     template("entry", "long")),
    ("entry-short", "A SHORT entry at the bar's close, on the profile's fallback stop",
     template("entry", "short")),
    ("entry-long-with-stop", "A LONG entry whose stop is the indicator's first plot",
     template("entry", "long", stop="{{plot_0}}")),
    ("entry-short-with-stop", "A SHORT entry whose stop is the indicator's plot named Stop, asserting "
     "the chart's exchange", template("entry", "short", stop='{{plot("Stop")}}', exchange=True)),
    ("exit-long", "An exit of a LONG: an exit or a take-profit alert", template("exit", "long")),
    ("exit-short", "An exit of a SHORT", template("exit", "short")),
]

def render(text):
    """An alert's message as TradingView sends it: each placeholder replaced by its value."""
    for placeholder, value in RENDERED_WITH.items():
        text = text.replace(placeholder, value)
    return text


# ---------------------------------------------------------------------------
# Resolution, exits, deliveries, the side held, and receipts.
# ---------------------------------------------------------------------------

DEFAULT_LIMIT = AGE_LIMIT_S["default"]


def resolution(case_id, note, line, payload, *, now_ms=T1_MS + 2000, limit_s=DEFAULT_LIMIT):
    return {"id": case_id, "note": note, "line": line, "payload": payload, "now_ms": now_ms,
            "age_limit_s": limit_s}


BN, BI, BY, YI, TW = "binance linear", "binance inverse", "bybit linear", "bybit inverse", "a list naming a pair twice"

RESOLUTIONS = [
    resolution("bn-linear-long", "A Binance USD-M LONG entry on an asset with settings of its own", BN,
               "bn-linear-long-entry"),
    resolution("bn-linear-short", "A Binance USD-M SHORT entry without a stop takes the defaults' fallback",
               BN, "bn-linear-short-entry"),
    resolution("bn-linear-exit-long", "A Binance USD-M exit of a LONG", BN, "bn-linear-long-exit"),
    resolution("bn-linear-exit-short", "A Binance USD-M exit of a SHORT", BN, "bn-linear-short-exit"),
    resolution("bn-inverse-long", "TradingView's BTCUSD.P is the COIN-M BTCUSD_PERP: the perpetual of "
               "base BTC and quote USD", BI, "bn-inverse-long-entry"),
    resolution("bn-inverse-short", "The venue's own symbol names an asset the profile has settings for",
               BI, "bn-inverse-short-entry"),
    resolution("bn-inverse-exit-long", "A Binance COIN-M exit of a LONG", BI, "bn-inverse-long-exit"),
    resolution("bn-inverse-exit-short", "A Binance COIN-M exit of a SHORT", BI, "bn-inverse-short-exit"),
    resolution("by-linear-long", "A Bybit linear LONG entry", BY, "by-linear-long-entry"),
    resolution("by-linear-short", "A Bybit linear SHORT entry whose assertions agree with its profile",
               BY, "by-linear-short-entry"),
    resolution("by-linear-exit-long", "A Bybit linear exit of a LONG", BY, "by-linear-long-exit"),
    resolution("by-linear-exit-short", "A Bybit linear exit of a SHORT", BY, "by-linear-short-exit"),
    resolution("by-inverse-long", "A Bybit inverse LONG entry without a stop: BTCUSD.P is BTCUSD there, "
               "and its fallback is the defaults'", YI, "by-inverse-long-entry"),
    resolution("by-inverse-short", "A Bybit inverse SHORT entry", YI, "by-inverse-short-entry"),
    resolution("by-inverse-exit-long", "A Bybit inverse exit of a LONG, by the venue's symbol", YI,
               "by-inverse-long-exit"),
    resolution("by-inverse-exit-short", "A Bybit inverse exit of a SHORT", YI, "by-inverse-short-exit"),
    resolution("wrong-profile", "A Bybit payload delivered to the Binance profile's URL: the profile it "
               "names is not the one its URL selects", BN, "by-linear-long-entry"),
    resolution("same-id-other-profile", "The same signal ID under another profile's own URL is that "
               "profile's own signal", BN, "by-linear-long-entry-as-bn"),
    resolution("wrong-exchange", "A payload that says BYBIT on a Binance profile", BN, "bn-linear-says-bybit"),
    resolution("exchange-of-no-venue", "An exchange QuantForge trades on nowhere cannot agree with any "
               "profile", BY, "by-linear-says-coinbase"),
    resolution("wrong-category", "A payload that says inverse on a linear profile", BY,
               "by-linear-says-inverse"),
    resolution("wrong-category-on-an-exit", "An exit is held to what it asserts too", BY,
               "by-linear-exit-says-inverse"),
    resolution("inverse-ticker-on-linear", "An inverse contract's ticker on a linear profile is no "
               "instrument of it", BY, "by-linear-inverse-ticker"),
    resolution("venue-symbol-of-an-asset", "The venue's own symbol names an asset the profile has "
               "settings for", BY, "by-linear-venue-symbol"),
    resolution("on-the-defaults", "An asset with no settings of its own trades on enabled defaults, "
               "named by TradingView's ticker; a dated future that shares its base and quote is not it",
               BN, "bn-linear-btc"),
    resolution("venue-symbol-without-settings", "The venue's own symbol, as an alert on a spot chart "
               "renders it, names nothing the profile was not given settings for", BN, "bn-linear-btc-symbol"),
    resolution("venue-symbol-without-settings-exit", "Nor for an exit: the same ticker names the same "
               "thing whatever the alert asks", BN, "bn-linear-btc-symbol-exit"),
    resolution("disabled-asset", "An entry on a disabled asset", BN, "bn-linear-1000shib"),
    resolution("disabled-asset-exit", "An exit on a disabled asset still resolves: no policy holds back "
               "a close", BN, "bn-linear-1000shib-exit"),
    resolution("defaults-off", "An asset without settings, on a profile whose defaults are not enabled",
               BI, "bn-inverse-sol"),
    resolution("defaults-off-exit", "Its exit still resolves", BI, "bn-inverse-sol-exit"),
    resolution("no-fallback-stop", "An entry without a stop on an asset whose fallback is off", BI,
               "bn-inverse-entry-without-stop"),
    resolution("no-inherited-fallback-stop", "An entry without a stop on an asset that inherits a "
               "fallback the defaults do not give", BY, "by-linear-btc-without-stop"),
    resolution("own-stop-without-a-fallback", "The same entry with a stop of its own", BY,
               "by-linear-btc-with-stop"),
    resolution("thousand-contract", "A contract of a thousand coins, by its TradingView name, on the "
               "defaults", BY, "by-linear-shib1000"),
    resolution("non-ascii-ticker", "A ticker beyond ASCII", BN, "non-ascii-ticker"),
    resolution("dated-by-symbol", "A dated future's own symbol names nothing: the profile has no "
               "settings for it, and it has no TradingView perpetual name", BN, "bn-linear-dated"),
    resolution("usdc-perpetual", "ETHUSDC.P names Bybit's ETHPERP, by its base and its quote; the "
               "universe keeps a USDC contract out", BY, "by-linear-ethusdc"),
    resolution("tradfi-perpetual", "A perpetual on gold is named, and kept out by the universe", BN,
               "bn-linear-xau"),
    resolution("not-trading", "A perpetual the venue no longer trades", BN, "bn-linear-luna"),
    resolution("not-trading-exit", "Its exit still resolves, by its symbol", BN, "bn-linear-luna-exit"),
    resolution("no-capital", "An inverse contract whose coin the profile holds no capital in", YI,
               "by-inverse-eth"),
    resolution("inverse-on-the-defaults", "An inverse contract on the defaults, sized from the capital "
               "in its own coin", YI, "by-inverse-sol"),
    resolution("unknown-ticker", "A ticker the venue lists nothing for", BY, "by-linear-unknown"),
    resolution("pair-named-twice", "A list that names a pair twice is not guessed at", TW, "twice-solusdt-p"),
    resolution("pair-named-twice-by-symbol", "The venue's symbol of an asset with settings still names "
               "one of them", TW, "twice-solusdt"),
    resolution("entry-at-the-age-limit", "An entry exactly as old as the limit is taken", BY,
               "by-linear-long-entry", now_ms=T1_MS + 60000),
    resolution("entry-past-the-age-limit", "A millisecond older, and it is stale", BY,
               "by-linear-long-entry", now_ms=T1_MS + 60001),
    resolution("exit-never-too-old", "An exit an hour old is still an exit", BY,
               "by-linear-long-exit", now_ms=T1_MS + 3600000),
    resolution("entry-within-the-future-allowance", "An entry timed 5 seconds after the clock is taken",
               BY, "by-linear-long-entry", now_ms=T1_MS - 5000),
    resolution("entry-from-the-future", "A millisecond more, and it is from the future", BY,
               "by-linear-long-entry", now_ms=T1_MS - 5001),
    resolution("exit-from-the-future", "An exit timed ahead of the clock is still an exit: no clock "
               "holds back a close", BY, "by-linear-long-exit", now_ms=T1_MS - 3600000),
    resolution("a-longer-age-limit", "A profile that takes entries up to 300 seconds old", BY,
               "by-linear-long-entry", now_ms=T1_MS + 300000, limit_s=300),
    resolution("past-a-longer-age-limit", "And none older", BY,
               "by-linear-long-entry", now_ms=T1_MS + 300001, limit_s=300),
    resolution("a-shorter-age-limit", "One that takes entries up to 5 seconds old", BY,
               "by-linear-long-entry", now_ms=T1_MS + 5000, limit_s=5),
    resolution("past-a-shorter-age-limit", "And none older", BY,
               "by-linear-long-entry", now_ms=T1_MS + 5001, limit_s=5),
    resolution("wrong-profile-before-anything-else", "A payload for another profile is refused for that, "
               "however stale and whatever its ticker", BN, "by-linear-unknown", now_ms=T1_MS + 999999),
    resolution("profile-before-the-exchange", "The profile is held before what the payload asserts",
               BN, "by-linear-says-coinbase"),
    resolution("exchange-before-the-category", "An exchange that disagrees is said before a family "
               "that does", BY, "by-linear-says-neither"),
    resolution("category-before-its-age", "A family that disagrees is said before the age", BY,
               "by-linear-says-inverse", now_ms=T1_MS + 999999),
    resolution("wrong-exchange-before-its-age", "And so is an exchange", BN, "bn-linear-says-bybit",
               now_ms=T1_MS + 999999),
    resolution("stale-before-the-ticker", "A stale entry is refused for its age before its ticker is "
               "looked up", BY, "by-linear-unknown", now_ms=T1_MS + 60001),
    resolution("ticker-before-the-policy", "A ticker that names nothing is said before the policy is "
               "asked", BI, "bn-inverse-unknown"),
    resolution("policy-before-the-stop", "A disabled asset is said before the stop its entry lacks",
               BI, "bn-inverse-eth-without-stop"),
]

# Payloads the cases use beyond the examples above.
MORE_VALID = [
    ("by-linear-long-entry-as-bn", "The Bybit entry's own text but for the profile it names: the same "
     "signal ID under another profile", body_of(BASE, profile="bn-linear")),
    ("bn-linear-says-bybit", "A Binance profile's payload asserting BYBIT",
     body_of(entry("bn-linear", "SOLUSDT.P", "long", "234.23", exchange="BYBIT"))),
    ("by-linear-says-coinbase", "A payload asserting an exchange QuantForge has no venue for",
     body_of(BASE, exchange="COINBASE")),
    ("by-linear-says-inverse", "A linear profile's payload asserting inverse", body_of(BASE, category="inverse")),
    ("by-linear-exit-says-inverse", "A linear profile's exit asserting inverse",
     body_of(BASE_EXIT, category="inverse")),
    ("by-linear-inverse-ticker", "A linear profile's payload naming an inverse contract",
     body_of(BASE, ticker="BTCUSD.P")),
    ("by-linear-venue-symbol", "The venue's own symbol in place of TradingView's name",
     body_of(BASE, ticker="SOLUSDT")),
    ("bn-linear-btc", "The BTCUSDT perpetual, which the profile has no settings for",
     body_of(entry("bn-linear", "BTCUSDT.P", "long", "84123.4"))),
    ("bn-linear-btc-symbol", "The same contract by the venue's own symbol",
     body_of(entry("bn-linear", "BTCUSDT", "long", "84123.4"))),
    ("bn-linear-btc-symbol-exit", "An exit naming it so", body_of(exit_("bn-linear", "BTCUSDT", "long"))),
    ("bn-linear-1000shib", "A thousand-coin contract the profile has disabled, without a stop",
     body_of(entry("bn-linear", "1000SHIBUSDT.P", "long", "0.012345"))),
    ("bn-linear-1000shib-exit", "An exit on it", body_of(exit_("bn-linear", "1000SHIBUSDT.P", "long"))),
    ("bn-inverse-sol", "A COIN-M contract the profile has no settings for",
     body_of(entry("bn-inverse", "SOLUSD.P", "long", "234.23", stop="220.00"))),
    ("bn-inverse-sol-exit", "An exit on it", body_of(exit_("bn-inverse", "SOLUSD.P", "long"))),
    ("bn-inverse-eth-without-stop", "A COIN-M contract the profile has disabled, without a stop",
     body_of(entry("bn-inverse", "ETHUSD.P", "long", "3012.34"))),
    ("by-linear-says-neither", "A payload asserting another exchange and another family",
     body_of(BASE, exchange="BINANCE", category="inverse")),
    ("bn-inverse-entry-without-stop", "A COIN-M entry without a stop",
     body_of(entry("bn-inverse", "BTCUSD.P", "long", "84123.4"))),
    ("by-linear-btc-without-stop", "A Bybit BTCUSDT entry without a stop",
     body_of(entry("by-linear", "BTCUSDT.P", "long", "84123.4"))),
    ("by-linear-btc-with-stop", "A Bybit BTCUSDT entry with its stop",
     body_of(entry("by-linear", "BTCUSDT.P", "long", "84123.4", stop="81600.0"))),
    ("by-linear-shib1000", "Bybit's thousand-coin contract",
     body_of(entry("by-linear", "SHIB1000USDT.P", "short", "0.012345", stop="0.013000"))),
    ("bn-linear-dated", "A dated future's own symbol",
     body_of(entry("bn-linear", "BTCUSDT_261225", "long", "84123.4"))),
    ("by-linear-ethusdc", "A USDC perpetual by its TradingView name",
     body_of(entry("by-linear", "ETHUSDC.P", "long", "3012.34", stop="2900.00"))),
    ("bn-linear-xau", "A gold perpetual", body_of(entry("bn-linear", "XAUUSDT.P", "long", "2650.10"))),
    ("bn-linear-luna", "A perpetual that is settling", body_of(entry("bn-linear", "LUNAUSDT.P", "long", "0.5"))),
    ("bn-linear-luna-exit", "An exit on it, by its symbol", body_of(exit_("bn-linear", "LUNAUSDT", "long"))),
    ("by-inverse-eth", "An inverse contract in a coin without capital",
     body_of(entry("by-inverse", "ETHUSD.P", "long", "3012.34"))),
    ("by-inverse-sol", "An inverse contract on the defaults",
     body_of(entry("by-inverse", "SOLUSD.P", "short", "234.23"))),
    ("by-linear-unknown", "A ticker Bybit lists nothing for", body_of(BASE, ticker="DOGEUSDT.P")),
    ("bn-inverse-unknown", "A ticker Binance lists no COIN-M contract for, without a stop",
     body_of(entry("bn-inverse", "DOGEUSD.P", "long", "0.1234"))),
    ("twice-solusdt-p", "A .P name on a list that names its pair twice",
     body_of(BASE, profile="twice")),
    ("twice-solusdt", "The venue's symbol on that list", body_of(BASE, profile="twice", ticker="SOLUSDT")),
    ("twice-exit", "An exit under another profile of the same account", body_of(
        BASE_EXIT, profile="twice", ticker="SOLUSDT")),
    ("by-linear-btc-long-exit", "An exit of a LONG on BTCUSDT", body_of(BASE_EXIT, ticker="BTCUSDT.P")),
    ("by-linear-long-exit-by-symbol", "An exit naming the venue's symbol", body_of(BASE_EXIT, ticker="SOLUSDT")),
    ("by-linear-long-exit-next-bar", "The exit alert on the next bar", body_of(
        BASE_EXIT, bar_time="2026-10-20T00:15:00Z", fire_time="2026-10-20T00:30:00Z")),
]

# Which exit closes which entry's position: the same profile, the same account and
# instrument, the side the exit names, and an entry that fired strictly before it.
# Each is (line, payload, fire time); an exit with no entry finds nothing open.
EXITS = [
    {"id": "exit-after-its-entry", "note": "An exit that fired after the entry closes its position",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-long-exit", "2026-10-20T04:00:00Z")},
    {"id": "delayed-exit", "note": "An exit that fired before the entry, delivered late, closes nothing: "
     "the position is a newer one than the exit was for",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T04:00:00Z"),
     "exit": (BY, "by-linear-long-exit", "2026-10-20T00:15:00Z")},
    {"id": "exit-in-the-same-second", "note": "Nor does one that fired in the entry's own second: "
     "strictly before means before",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-long-exit", "2026-10-20T00:15:00Z")},
    {"id": "exit-a-second-later", "note": "One second later is later",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-long-exit", "2026-10-20T00:15:01Z")},
    {"id": "exit-of-the-other-side", "note": "An exit of a SHORT closes no LONG",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-short-exit", "2026-10-20T04:00:00Z")},
    {"id": "exit-of-another-instrument", "note": "An exit on another instrument closes nothing here",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-btc-long-exit", "2026-10-20T04:00:00Z")},
    {"id": "exit-by-the-venue-symbol", "note": "The ticker's spelling does not matter, the instrument does",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (BY, "by-linear-long-exit-by-symbol", "2026-10-20T04:00:00Z")},
    {"id": "exit-of-another-profile", "note": "Another profile's exit closes nothing, though it names "
     "the same account and instrument: a position is its own profile's",
     "entry": (BY, "by-linear-long-entry", "2026-10-20T00:15:00Z"),
     "exit": (TW, "twice-exit", "2026-10-20T04:00:00Z")},
    {"id": "exit-with-nothing-open", "note": "An exit with no position to close does nothing, and says so",
     "entry": None,
     "exit": (BY, "by-linear-long-exit", "2026-10-20T04:00:00Z")},
]

# Deliveries one after another, each on the URL of the profile named: a signal is known by
# its profile and its ID, and told apart from a conflicting one by its payload's hash.
DELIVERIES = [
    {"id": "repeated-delivery", "note": "TradingView resends a delivery it got a 5xx for: the second is "
     "the first again, and nothing is done twice",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "by-linear-long-entry")]},
    {"id": "resend-with-a-later-fire-time", "note": "A resend whose fire time was rendered again is "
     "still the same delivery",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "a-later-fire-time")]},
    {"id": "same-id-another-price", "note": "The same ID with another price is not the same signal: a "
     "conflict, refused",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "another-price")]},
    {"id": "same-id-another-scale", "note": "Nor is one whose price is written with another scale",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "price-with-trailing-zeros")]},
    {"id": "same-id-another-stop", "note": "Nor one with another stop, or with none",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "another-stop"),
               ("by-linear", "without-its-stop")]},
    {"id": "conflict-then-the-original-again", "note": "A conflict changes nothing: the first delivery "
     "is still the one known",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "another-price"),
               ("by-linear", "by-linear-long-entry")]},
    {"id": "same-id-two-profiles", "note": "The same ID under two profiles is two signals: neither is a "
     "repeat of the other",
     "steps": [("by-linear", "by-linear-long-entry"), ("bn-linear", "by-linear-long-entry-as-bn"),
               ("by-linear", "by-linear-long-entry"), ("bn-linear", "by-linear-long-entry-as-bn")]},
    {"id": "entry-then-exit", "note": "An entry and an exit on one bar are two signals, and a repeated "
     "exit is a duplicate like any other",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "by-linear-long-exit"),
               ("by-linear", "by-linear-long-exit")]},
    {"id": "next-bar", "note": "The same alerts on the next bar are new signals",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "another-bar"),
               ("by-linear", "by-linear-long-exit"), ("by-linear", "by-linear-long-exit-next-bar")]},
    {"id": "two-alerts-one-bar", "note": "Two alerts with their own names on one bar are two signals",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "another-source")]},
    {"id": "spacing-and-order", "note": "Spacing, the order of members and escapes are not part of a signal",
     "steps": [("by-linear", "by-linear-long-entry"), ("by-linear", "compact"),
               ("by-linear", "members-in-another-order"), ("by-linear", "escaped-text")]},
]

# An entry against the side its position already holds, where it holds one.
HELD = [
    {"held": None, "entry": "long", "refused": None, "note": "A flat position takes an entry"},
    {"held": None, "entry": "short", "refused": None, "note": "On either side"},
    {"held": "long", "entry": "long", "refused": "pyramiding", "note": "A second entry on the side held "
     "adds to a position: refused"},
    {"held": "short", "entry": "short", "refused": "pyramiding", "note": "On either side"},
    {"held": "long", "entry": "short", "refused": "reversal", "note": "An entry on the other side would "
     "reverse it: refused, the indicator sends an exit first"},
    {"held": "short", "entry": "long", "refused": "reversal", "note": "On either side"},
]

SAMPLE_ID = "trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z"
SAMPLE_EXIT_ID = "trend:exit:long:SOLUSDT.P:2026-10-20T00:00:00Z"
RECEIPTS = [
    {"id": "received", "note": "Recorded, and nothing more: no order exists yet",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "received"}, "valid": True},
    {"id": "admitted", "note": "An entry that passed admission: not yet sized, sent or filled",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "admitted"}, "valid": True},
    {"id": "rejected", "note": "Recorded and refused, with the refusal's name",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "rejected", "refusal": "stale",
              "message": "the alert fired 75 s ago, and profile by-linear takes entries up to 60 s old"},
     "valid": True},
    {"id": "duplicate", "note": "A delivery already recorded: nothing is done twice",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "duplicate"}, "valid": True},
    {"id": "noop", "note": "An exit with no position of its own to close",
     "body": {"version": 1, "signal_id": SAMPLE_EXIT_ID, "state": "noop",
              "message": "profile by-linear holds no long position on SOLUSDT that this exit is for"},
     "valid": True},
    {"id": "rejected-without-a-refusal", "note": "A rejection says why",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "rejected"}, "valid": False},
    {"id": "received-with-a-refusal", "note": "Only a rejection carries a refusal",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "received", "refusal": "stale"}, "valid": False},
    {"id": "filled", "note": "A receipt never says an order was sent or filled: those states are not its to give",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "filled"}, "valid": False},
    {"id": "another-version", "note": "A receipt of another version is not this one",
     "body": {"version": 2, "signal_id": SAMPLE_ID, "state": "received"}, "valid": False},
    {"id": "without-its-signal", "note": "A receipt names the signal it is for",
     "body": {"version": 1, "state": "received"}, "valid": False},
    {"id": "with-an-unknown-member", "note": "Like a payload, a receipt holds nothing the format does not define",
     "body": {"version": 1, "signal_id": SAMPLE_ID, "state": "received", "order_id": "1"}, "valid": False},
]
RECEIPT_FIELDS = ["version", "signal_id", "state", "refusal", "message"]


def receipt_ok(body):
    if any(key not in RECEIPT_FIELDS for key in body):
        return False
    version = body.get("version")
    if isinstance(version, bool) or version != SIGNAL_VERSION or not isinstance(body.get("signal_id"), str):
        return False
    if body.get("state") not in RECEIPT_STATES:
        return False
    # A receipt follows the protocol's rule for what a server sends: a member left out and
    # a null are read alike.
    if any(not isinstance(body.get(key), (str, type(None))) for key in ("refusal", "message")):
        return False
    return (body.get("refusal") is not None) == (body["state"] == "rejected")


# ---------------------------------------------------------------------------


def as_bytes(body):
    return body if isinstance(body, bytes) else body.encode("utf-8")


def payload_item(case_id, note, body, schema, expect=None):
    data = as_bytes(body)
    verdict, found = read(data)
    item = {"id": case_id, "note": note}
    try:
        item["body"] = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError:
        item["body_hex"] = data.hex()
    accepted = schema_verdict(schema, data)
    if verdict == "signal":
        assert expect is None, "{}: expected {} and it is a signal".format(case_id, expect)
        assert accepted is True, "{}: the rules take it and the schema does not".format(case_id)
        name = signal_id(found)
        assert len(name) <= 128 and not any(ord(char) < 0x20 for char in name), case_id
        # Neither spacing nor the order of members is part of a signal.
        for respelled in (json.dumps(found, ensure_ascii=False),
                          json.dumps(dict(reversed(list(found.items()))), indent=3)):
            again = read(respelled.encode("utf-8"))
            assert again == ("signal", found), "{}: {} reads as {}".format(case_id, respelled, again)
        item["signal"] = {
            "fields": found,
            "signal_id": name,
            "canonical": canonical(found),
            "hash": payload_hash(found),
            "bar_time_ms": time_ms(found["bar_time"]),
            "fire_time_ms": time_ms(found["fire_time"]),
        }
    else:
        assert expect is not None, "{}: not a signal: {}".format(case_id, found)
        assert found == sorted(set(expect), key=rank), "{}: {} and not {}".format(case_id, found, expect)
        item["problems"] = [{"code": code, "field": field} for code, field in found]
    # What the schema says of the body, read as any JSON reader would read it. It is a
    # necessary condition, never a sufficient one: it cannot see a member written twice,
    # the size of a body, a day the calendar lacks, or the order of two times.
    item["schema_accepts"] = accepted
    return item


def build(schema):
    payloads = [payload_item(case_id, note, body, schema) for case_id, note, body in VALID + MORE_VALID]
    payloads += [payload_item(case_id, note, body, schema, expect) for case_id, note, body, expect in INVALID]
    by_id = {item["id"]: item for item in payloads}
    assert len(by_id) == len(payloads), "two payloads share a name"
    for code in CODES:
        assert any(code == problem["code"] for item in payloads for problem in item.get("problems", [])), \
            "no payload has the problem {}".format(code)
    for field in FIELDS:
        for code in ("wrong_type", "invalid_value") if field != "version" else ("wrong_type",):
            assert any({"code": code, "field": field} in item.get("problems", []) for item in payloads), \
                "no payload has {} {}".format(code, field)

    def signal(name):
        return by_id[name]["signal"]["fields"]

    templates = []
    for name, note, text in TEMPLATES:
        placeholders = PLACEHOLDER.findall(text)
        assert all(found in RENDERED_WITH for found in placeholders), "{}: {}".format(name, placeholders)
        rendered = render(text)
        assert "{{" not in rendered, "{}: a placeholder is left in {}".format(name, rendered)
        verdict, found = read(rendered.encode("utf-8"))
        assert verdict == "signal", "{}: its rendering is not a signal: {}".format(name, found)
        templates.append({"name": name, "note": note, "template": text, "placeholders": placeholders,
                          "rendered": rendered, "signal_id": signal_id(found), "canonical": canonical(found)})

    resolutions = []
    for case in RESOLUTIONS:
        asked = (LINES[case["line"]], signal(case["payload"]), case["now_ms"], case["age_limit_s"])
        outcome, broken = resolve(*asked), failing(*asked)
        assert broken == sorted(broken, key=RESOLUTION_ORDER.index), case["id"]
        assert outcome.get("over") == (broken[0] if broken else None), "{}: {} and {}".format(
            case["id"], outcome, broken)
        resolutions.append(dict(case, breaks=broken, outcome=outcome))
    # Each rule is shown ahead of the next by a signal that breaks both. A ticker that
    # names nothing leaves no instrument for the policy or the stop to be asked about.
    for first, then in zip(RESOLUTION_ORDER, RESOLUTION_ORDER[1:]):
        shown = any(case["breaks"][:1] == [first] and then in case["breaks"] for case in resolutions)
        assert shown == (first != "ticker"), "{} before {}: {}".format(first, then, shown)
    cells = {(outcome["resolved"]["instrument"]["exchange"], outcome["resolved"]["kind"],
              outcome["resolved"]["side"])
             for outcome in (case["outcome"] for case in resolutions) if "resolved" in outcome}
    for exchange in ("BinanceLinear", "BinanceInverse", "BybitLinear", "BybitInverse"):
        for kind in KINDS:
            for side in SIDES:
                assert (exchange, kind, side) in cells, "no {} {} {} is resolved".format(exchange, kind, side)
    refusals = {(case["outcome"].get("refused"), case["outcome"].get("over")) for case in resolutions}
    for expected in [("ambiguous_selection", "profile"), ("ambiguous_selection", "exchange"),
                     ("ambiguous_selection", "category"), ("ambiguous_selection", "ticker"),
                     ("future_timestamp", "fire_time"), ("stale", "fire_time"), ("unknown_ticker", "ticker"),
                     ("unknown_ticker", "policy"), ("not_armed", "policy"), ("outside_universe", "policy"),
                     ("no_capital_in_settlement_asset", "policy"), ("no_stop", "stop")]:
        assert expected in refusals, "no resolution is refused as {} over {}".format(*expected)
    spellings = {case["outcome"]["resolved"]["spelling"] for case in resolutions if "resolved" in case["outcome"]}
    assert spellings == set(SPELLINGS), spellings
    sources = {(case["outcome"]["resolved"]["policy"][name]["source"])
               for case in resolutions if case["outcome"].get("resolved", {}).get("policy")
               for name in ("risk_percent", "stop_percent", "leverage")}
    assert sources == {"Asset", "Defaults"}, sources

    def position(line, name, fire):
        """What a resolved signal names, fired at `fire`."""
        fields = dict(signal(name), fire_time=fire)
        outcome = resolve(LINES[line], fields, time_ms(fire) + 2000, DEFAULT_LIMIT)
        assert "resolved" in outcome, "{} on {}: {}".format(name, line, outcome)
        return outcome["resolved"]

    exits = []
    for case in EXITS:
        left = position(*case["exit"])
        item = {"id": case["id"], "note": case["note"], "entry": None,
                "exit": {"line": case["exit"][0], "payload": case["exit"][1], "fire_time": case["exit"][2],
                         "fire_time_ms": left["signal_time_ms"]}}
        closes = False
        if case["entry"]:
            entered = position(*case["entry"])
            item["entry"] = {"line": case["entry"][0], "payload": case["entry"][1],
                             "fire_time": case["entry"][2], "fire_time_ms": entered["signal_time_ms"]}
            same_profile = signal(case["entry"][1])["profile"] == signal(case["exit"][1])["profile"]
            closes = (same_profile and entered["account"] == left["account"]
                      and entered["instrument"] == left["instrument"] and entered["side"] == left["side"]
                      and entered["signal_time_ms"] < left["signal_time_ms"])
        item["closes"] = closes
        exits.append(item)
    assert sum(case["closes"] for case in exits) == 3, [case["id"] for case in exits if case["closes"]]

    deliveries = []
    for case in DELIVERIES:
        known, outcomes = {}, []
        for profile, name in case["steps"]:
            fields = signal(name)
            assert fields["profile"] == profile, "{}: {} is not for {}".format(case["id"], name, profile)
            key, digest = (profile, signal_id(fields)), payload_hash(fields)
            if key not in known:
                known[key] = digest
                outcomes.append("new")
            else:
                outcomes.append("duplicate" if known[key] == digest else "conflict")
        deliveries.append({"id": case["id"], "note": case["note"],
                           "steps": [{"profile": profile, "payload": name, "outcome": outcome}
                                     for (profile, name), outcome in zip(case["steps"], outcomes)]})
    assert {step["outcome"] for case in deliveries for step in case["steps"]} == {"new", "duplicate", "conflict"}

    receipts = []
    for case in RECEIPTS:
        assert receipt_ok(case["body"]) == case["valid"], case["id"]
        receipts.append(case)
    assert {case["body"]["state"] for case in receipts if case["valid"]} == set(RECEIPT_STATES)
    return {"payloads": payloads, "templates": templates, "resolutions": resolutions, "exits": exits,
            "deliveries": deliveries, "held": HELD, "receipts": receipts}


def listed(name, statement):
    """One item of a list of the page, wrapped as the page is."""
    joined = "- `{}`: {}".format(name, statement)
    wrapped = textwrap.wrap(joined, width=78, subsequent_indent="  ", break_long_words=False,
                            break_on_hyphens=False)
    return "\n".join(wrapped)


def said(item):
    return ", ".join("`{}`".format(problem["code"]) if problem["field"] is None
                     else "`{}` `{}`".format(problem["code"], problem["field"])
                     for problem in item["problems"])


def answered(case):
    outcome = case["outcome"]
    if "refused" in outcome:
        return "`{}`".format(outcome["refused"])
    resolved = outcome["resolved"]
    return "{} {} on {}".format(resolved["kind"], resolved["side"], resolved["instrument"]["symbol"])


def rewrite_doc(path, built):
    """Rewrites what the page repeats of the cases, between its markers."""
    blocks = ["**`{}`.** {}.\n\n```json\n{}\n```".format(item["name"], item["note"], item["template"])
              for item in built["templates"]]
    valid = [item for item in built["payloads"] if "signal" in item]
    invalid = [item for item in built["payloads"] if "problems" in item]
    loose = [item for item in invalid if item["schema_accepts"]]
    steps = lambda case: ", ".join("{} {}".format(step["payload"], step["outcome"]) for step in case["steps"])
    lists = {
        "alert templates": "\n\n".join(blocks),
        "valid payloads": "\n".join(listed(item["id"], item["note"]) for item in valid),
        "invalid payloads": "\n".join(
            listed(item["id"], "{} ({})".format(item["note"], said(item))) for item in invalid),
        "beyond the schema": "\n".join(listed(item["id"], said(item)) for item in loose),
        "resolution cases": "\n".join(
            listed(case["id"], "{} ({})".format(case["note"], answered(case))) for case in built["resolutions"]),
        "exit cases": "\n".join(
            listed(case["id"], "{} ({})".format(case["note"], "closes it" if case["closes"] else "a no-op"))
            for case in built["exits"]),
        "delivery cases": "\n".join(
            listed(case["id"], "{} ({})".format(case["note"], steps(case))) for case in built["deliveries"]),
    }
    with open(path, encoding="utf-8") as handle:
        text = handle.read()
    for name, body in lists.items():
        begin, end = "<!-- begin: {} -->\n".format(name), "<!-- end: {} -->".format(name)
        head, rest = text.split(begin)
        _, tail = rest.split(end)
        text = head + begin + body + "\n" + end + tail
    with open(path, "w", encoding="utf-8", newline="\n") as handle:
        handle.write(text)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", help="write the cases as JSON here")
    parser.add_argument("--doc", help="rewrite the lists of this page that repeat the cases")
    args = parser.parse_args()
    with open(SCHEMA_PATH, "rb") as handle:
        schema_bytes = handle.read()
    schema = json.loads(schema_bytes)
    built = build(schema)
    counts = {name: len(items) for name, items in built.items()}
    valid = sum(1 for item in built["payloads"] if "signal" in item)
    print("{} payloads ({} signals, {} with problems)".format(counts["payloads"], valid, counts["payloads"] - valid))
    print(", ".join("{} {}".format(counts[name], name) for name in
                    ("templates", "resolutions", "exits", "deliveries", "held", "receipts")))
    loose = [item["id"] for item in built["payloads"] if "problems" in item and item["schema_accepts"]]
    print("the schema alone would take {} of the bodies the rules refuse: {}".format(len(loose), ", ".join(loose)))
    if args.write:
        document = {
            "format": FIXTURE_FORMAT,
            "generated_by": "spikes/futures-contract/signal_cases.py",
            "signal_version": SIGNAL_VERSION,
            "max_signal_bytes": MAX_SIGNAL_BYTES,
            "max_depth": MAX_DEPTH,
            "ticker_max": TICKER_MAX,
            "ticker_refused": TICKER_REFUSED,
            "age_limit_s": AGE_LIMIT_S,
            "future_allowance_ms": FUTURE_ALLOWANCE_MS,
            "fields": FIELDS,
            "entry_only": ENTRY_ONLY,
            "required": REQUIRED,
            "id_fields": ID_FIELDS,
            "not_in_canonical": NOT_IN_CANONICAL,
            "problem_codes": CODES,
            "receipt_states": RECEIPT_STATES,
            "receipt_fields": RECEIPT_FIELDS,
            "spellings": SPELLINGS,
            "resolution_order": RESOLUTION_ORDER,
            "schema_sha256": hashlib.sha256(schema_bytes).hexdigest(),
            "rendered_with": RENDERED_WITH,
            "lines": copy.deepcopy([LINES[name] for name in LINES]),
        }
        document.update(built)
        with open(args.write, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(document, handle, indent=2, ensure_ascii=False)
            handle.write("\n")
    if args.doc:
        rewrite_doc(args.doc, built)
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
