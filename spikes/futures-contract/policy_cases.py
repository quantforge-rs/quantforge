#!/usr/bin/env python3
"""An independent model of an automation profile's policy (ADR 0002 sections 5 and 6).

It writes the shared cases the Rust implementation is held to:

    python3 -I spikes/futures-contract/policy_cases.py --write tests/fixtures/policy_cases.json

The model here shares no code with the Rust one. It knows the policy's rules:
defaults and what an asset inherits from them, the changes an operator makes
and the revision each one stamps, the refusals, what an admitted entry carries,
which stop an entry uses, and what a venue's instrument list says about a
policy. Every case runs once on each of the four product lines, so the same
operations are held to the same answers on Binance and on Bybit, linear and
inverse.

Standard library only. Run it with `python3 -I`, from the repository root.
"""

import argparse
import copy
import json
import sys
from decimal import Decimal

FIXTURE_FORMAT = 1
POLICY_FORMAT = 1
SNAPSHOT_FORMAT = 1
# A made-up key fingerprint: sixteen hex digits, plain enough that a secret
# scanner does not take a fixture's fingerprint for a key.
FINGERPRINT = "00000000000000a1"

# ---------------------------------------------------------------------------
# The four product lines, each with a small list of instruments as a venue's
# adapter would normalize them.
# ---------------------------------------------------------------------------


def money(amount, asset):
    return {"amount": amount, "asset": asset}


def spec(exchange, symbol, settle, base, quote, *, contract="Perpetual", status="Trading",
         unit, tick, step, min_qty, max_qty, min_notional=None, face=None, face_source=None,
         leverage=None):
    """One instrument, in the stored form of the Rust `InstrumentSpec`."""
    quantity = lambda value: {unit: value}
    return {
        "id": {"exchange": exchange, "symbol": symbol, "settle": settle},
        "base": base,
        "quote": quote,
        "contract": contract,
        "status": status,
        "delist_at_ms": None,
        "inverse": None if face is None else {"face_value": face, "source": face_source},
        "tick": tick,
        "step": quantity(step),
        "market_step": quantity(step),
        "min_qty": quantity(min_qty),
        "market_min_qty": quantity(min_qty),
        "market_max_qty": quantity(max_qty),
        "min_notional": None if min_notional is None else money(min_notional, quote),
        "leverage": leverage,
        "observed_at_ms": 1792454400000,
    }


BYBIT_LEVERAGE = {"min": "1", "max": "100.00", "step": "0.01"}
DATED = {"Dated": {"delivery_ms": 1798156800000}}


def lines():
    """Each line: who its profile is, the symbols the cases use, and its instruments."""
    def identity(venue, category, account):
        return {
            "id": "main",
            "account_ref": {"venue": venue, "environment": "Demo", "venue_account_id": account},
            "category": category,
            "owner_tag": "k3x9p2",
        }

    binance_linear = [
        spec("BinanceLinear", "SOLUSDT", "USDT", "SOL", "USDT", unit="Base", tick="0.0100",
             step="0.01", min_qty="0.01", max_qty="80000", min_notional="5"),
        spec("BinanceLinear", "BTCUSDT", "USDT", "BTC", "USDT", unit="Base", tick="0.10",
             step="0.001", min_qty="0.001", max_qty="120", min_notional="50"),
        spec("BinanceLinear", "BTCUSDT_261225", "USDT", "BTC", "USDT", contract=DATED,
             unit="Base", tick="0.10", step="0.001", min_qty="0.001", max_qty="120",
             min_notional="50"),
        spec("BinanceLinear", "ETHUSDC", "USDC", "ETH", "USDC", unit="Base", tick="0.01",
             step="0.001", min_qty="0.001", max_qty="2000", min_notional="20"),
        spec("BinanceLinear", "LUNAUSDT", "USDT", "LUNA", "USDT", status={"Other": "SETTLING"},
             unit="Base", tick="0.0001", step="1", min_qty="1", max_qty="100000",
             min_notional="5"),
        spec("BinanceLinear", "XAUUSDT", "USDT", "XAU", "USDT", contract="TradFiPerpetual",
             unit="Base", tick="0.01", step="0.001", min_qty="0.001", max_qty="500",
             min_notional="5"),
    ]
    bybit_linear = [
        spec("BybitLinear", "SOLUSDT", "USDT", "SOL", "USDT", unit="Base", tick="0.010",
             step="0.1", min_qty="0.1", max_qty="12000.0", min_notional="5",
             leverage=BYBIT_LEVERAGE),
        spec("BybitLinear", "BTCUSDT", "USDT", "BTC", "USDT", unit="Base", tick="0.10",
             step="0.001", min_qty="0.001", max_qty="119.000", min_notional="5",
             leverage=BYBIT_LEVERAGE),
        spec("BybitLinear", "BTC-26DEC26", "USDT", "BTC", "USDT", contract=DATED, unit="Base",
             tick="0.50", step="0.001", min_qty="0.001", max_qty="10", min_notional="5",
             leverage={"min": "1", "max": "30.00", "step": "0.01"}),
        spec("BybitLinear", "ETHPERP", "USDC", "ETH", "USDC", unit="Base", tick="0.01",
             step="0.01", min_qty="0.01", max_qty="500", min_notional="5",
             leverage=BYBIT_LEVERAGE),
        spec("BybitLinear", "LUNAUSDT", "USDT", "LUNA", "USDT", status={"Other": "Closed"},
             unit="Base", tick="0.0001", step="1", min_qty="1", max_qty="100000",
             min_notional="5", leverage=BYBIT_LEVERAGE),
    ]
    binance_inverse = [
        spec("BinanceInverse", "BTCUSD_PERP", "BTC", "BTC", "USD", unit="Contracts", tick="0.1",
             step="1", min_qty="1", max_qty="60000", face="100", face_source="Metadata"),
        spec("BinanceInverse", "ETHUSD_PERP", "ETH", "ETH", "USD", unit="Contracts", tick="0.01",
             step="1", min_qty="1", max_qty="500000", face="10", face_source="Metadata"),
        spec("BinanceInverse", "BTCUSD_261225", "BTC", "BTC", "USD", contract=DATED,
             unit="Contracts", tick="0.1", step="1", min_qty="1", max_qty="60000", face="100",
             face_source="Metadata"),
        spec("BinanceInverse", "LUNAUSD_PERP", "LUNA", "LUNA", "USD",
             status={"Other": "PENDING_TRADING"}, unit="Contracts", tick="0.0001", step="1",
             min_qty="1", max_qty="100000", face="10", face_source="Metadata"),
    ]
    bybit_inverse = [
        spec("BybitInverse", "BTCUSD", "BTC", "BTC", "USD", unit="Contracts", tick="0.10",
             step="1", min_qty="1", max_qty="1000000", face="1", face_source="DocumentedRule",
             leverage=BYBIT_LEVERAGE),
        spec("BybitInverse", "SOLUSD", "SOL", "SOL", "USD", unit="Contracts", tick="0.010",
             step="1", min_qty="1", max_qty="500000", face="1", face_source="DocumentedRule",
             leverage={"min": "1", "max": "50.00", "step": "0.01"}),
        spec("BybitInverse", "BTCUSDZ26", "BTC", "BTC", "USD", contract=DATED, unit="Contracts",
             tick="0.50", step="1", min_qty="1", max_qty="1000000", face="1",
             face_source="DocumentedRule", leverage=BYBIT_LEVERAGE),
        spec("BybitInverse", "LUNAUSD", "LUNA", "LUNA", "USD", status={"Other": "Closed"},
             unit="Contracts", tick="0.0001", step="1", min_qty="1", max_qty="100000", face="1",
             face_source="DocumentedRule", leverage=BYBIT_LEVERAGE),
    ]
    return [
        {
            "name": "binance linear",
            "identity": identity("Binance", "Linear", {"Venue": "9001234"}),
            "use": {"main": "SOLUSDT", "second": "BTCUSDT", "dated": "BTCUSDT_261225",
                    "halted": "LUNAUSDT", "other_settle": "ETHUSDC"},
            "instruments": binance_linear,
        },
        {
            "name": "binance inverse",
            "identity": identity("Binance", "Inverse", {"Venue": "9001234"}),
            "use": {"main": "BTCUSD_PERP", "second": "ETHUSD_PERP", "dated": "BTCUSD_261225",
                    "halted": "LUNAUSD_PERP", "other_settle": None},
            "instruments": binance_inverse,
        },
        {
            "name": "bybit linear",
            "identity": identity("Bybit", "Linear", {"KeyFingerprint": FINGERPRINT}),
            "use": {"main": "SOLUSDT", "second": "BTCUSDT", "dated": "BTC-26DEC26",
                    "halted": "LUNAUSDT", "other_settle": "ETHPERP"},
            "instruments": bybit_linear,
        },
        {
            "name": "bybit inverse",
            "identity": identity("Bybit", "Inverse", {"KeyFingerprint": FINGERPRINT}),
            "use": {"main": "BTCUSD", "second": "SOLUSD", "dated": "BTCUSDZ26",
                    "halted": "LUNAUSD", "other_settle": None},
            "instruments": bybit_inverse,
        },
    ]


# ---------------------------------------------------------------------------
# The model.
# ---------------------------------------------------------------------------

EXCHANGES = {
    ("Binance", "Linear"): "BinanceLinear",
    ("Binance", "Inverse"): "BinanceInverse",
    ("Bybit", "Linear"): "BybitLinear",
    ("Bybit", "Inverse"): "BybitInverse",
}
LINE_NAMES = {
    "BinanceLinear": "binance linear",
    "BinanceInverse": "binance inverse",
    "BybitLinear": "bybit linear",
    "BybitInverse": "bybit inverse",
}


def stored(document):
    """A document, or a snapshot, as a store keeps it: compact JSON, keys in their own order."""
    return json.dumps(document, separators=(",", ":"), ensure_ascii=False)


def same(left, right):
    """Whether two optional decimals are the same number, whatever their scale."""
    if left is None or right is None:
        return left is None and right is None
    return Decimal(left) == Decimal(right)


def same_stop(left, right):
    """Whether two assets' fallback settings are the same."""
    if isinstance(left, dict) and isinstance(right, dict):
        return same(left["Percent"], right["Percent"])
    return left == right


def exchange_of(policy):
    identity = policy["identity"]
    return EXCHANGES[(identity["account_ref"]["venue"], identity["category"])]


def new_policy(identity):
    return {
        "format": POLICY_FORMAT,
        "identity": copy.deepcopy(identity),
        "revision": 1,
        "label": None,
        "defaults": {"enabled": False, "risk_percent": None, "stop_percent": None,
                     "leverage": None, "revision": 1},
        "assets": [],
        "capital": [],
    }


def find_asset(policy, symbol):
    for asset in policy["assets"]:
        if asset["instrument"]["symbol"] == symbol:
            return asset
    return None


def find_capital(policy, asset):
    for capital in policy["capital"]:
        if capital["amount"]["asset"] == asset:
            return capital
    return None


def risk_of(policy, asset):
    return asset["risk_percent"] if asset["risk_percent"] is not None \
        else policy["defaults"]["risk_percent"]


def leverage_of(policy, asset):
    return asset["leverage"] if asset["leverage"] is not None else policy["defaults"]["leverage"]


def fault(policy):
    """Why a document is not one a store may keep, if it is not."""
    defaults = policy["defaults"]
    if defaults["enabled"] and (defaults["risk_percent"] is None or defaults["leverage"] is None):
        return ("the defaults are enabled without both a risk percentage and a leverage; set "
                "them, or disable the defaults")
    for asset in policy["assets"]:
        symbol = asset["instrument"]["symbol"]
        if risk_of(policy, asset) is None:
            missing = "a risk percentage"
        elif leverage_of(policy, asset) is None:
            missing = "a leverage"
        else:
            continue
        if asset["enabled"]:
            return (f"asset {symbol} is enabled without {missing}, its own or the defaults'; "
                    "set one, or disable the asset")
    return None


def edit_setting(holder, key, edit):
    """An edit of one optional setting: absent keeps it, "clear" removes it, {"set": x} sets it."""
    if edit is None:
        return
    holder[key] = None if edit == "clear" else edit["set"]


def stop_setting(value):
    """A fixture's spelling of an asset's fallback, as the document holds it."""
    if value == "inherit":
        return "Inherit"
    if value == "off":
        return "Off"
    return {"Percent": value["percent"]}


def defaults_equal(left, right):
    return (left["enabled"] == right["enabled"] and same(left["risk_percent"], right["risk_percent"])
            and same(left["stop_percent"], right["stop_percent"])
            and same(left["leverage"], right["leverage"]) and left["revision"] == right["revision"])


def asset_equal(left, right):
    return (left is not None and right is not None and left["instrument"] == right["instrument"]
            and left["enabled"] == right["enabled"]
            and same(left["risk_percent"], right["risk_percent"])
            and same_stop(left["stop"], right["stop"]) and same(left["leverage"], right["leverage"])
            and left["revision"] == right["revision"])


def capital_equal(left, right):
    return (left is not None and right is not None
            and left["amount"]["asset"] == right["amount"]["asset"]
            and same(left["amount"]["amount"], right["amount"]["amount"])
            and left["revision"] == right["revision"])


def policy_equal(left, right):
    return (left["label"] == right["label"] and defaults_equal(left["defaults"], right["defaults"])
            and len(left["assets"]) == len(right["assets"])
            and all(asset_equal(a, b) for a, b in zip(left["assets"], right["assets"]))
            and len(left["capital"]) == len(right["capital"])
            and all(capital_equal(a, b) for a, b in zip(left["capital"], right["capital"])))


def apply(policy, change):
    """("changed", next), ("unchanged",) or ("refused", message)."""
    profile = policy["identity"]["id"]
    line = exchange_of(policy)
    refuse = lambda reason: ("refused", f"invalid policy change: {reason}")
    nxt = copy.deepcopy(policy)
    (kind, body), = change.items()
    if kind == "label":
        nxt["label"] = body
    elif kind == "defaults":
        defaults = nxt["defaults"]
        if "enabled" in body:
            defaults["enabled"] = body["enabled"]
        for key in ("risk_percent", "stop_percent", "leverage"):
            edit_setting(defaults, key, body.get(key))
    elif kind == "add_asset":
        symbol = body["symbol"]
        exchange = body.get("exchange", line)
        if exchange != line:
            return refuse(f"{symbol} is a {LINE_NAMES[exchange]} contract, and profile {profile} "
                          f"trades {LINE_NAMES[line]}")
        if find_asset(nxt, symbol) is not None:
            return refuse(f"asset {symbol} already has settings of its own; edit them")
        nxt["assets"].append({
            "instrument": {"exchange": exchange, "symbol": symbol, "settle": body["settle"]},
            "enabled": body["enabled"],
            "risk_percent": body.get("risk_percent"),
            "stop": stop_setting(body.get("stop", "inherit")),
            "leverage": body.get("leverage"),
            "revision": policy["revision"],
        })
        nxt["assets"].sort(key=lambda asset: asset["instrument"]["symbol"].encode("utf-8"))
    elif kind in ("edit_asset", "reset_asset"):
        symbol = body if kind == "reset_asset" else body["symbol"]
        asset = find_asset(nxt, symbol)
        if asset is None:
            return refuse(f"profile {profile} holds no settings for {symbol}; add the asset first")
        if kind == "reset_asset":
            asset["risk_percent"] = None
            asset["stop"] = "Inherit"
            asset["leverage"] = None
        else:
            if "enabled" in body:
                asset["enabled"] = body["enabled"]
            edit_setting(asset, "risk_percent", body.get("risk_percent"))
            if "stop" in body:
                asset["stop"] = stop_setting(body["stop"])
            edit_setting(asset, "leverage", body.get("leverage"))
    elif kind == "set_capital":
        if Decimal(body["amount"]) <= 0:
            return refuse(f"capital of {body['amount']} {body['asset']} is not above zero")
        held = find_capital(nxt, body["asset"])
        entry = {"amount": money(body["amount"], body["asset"]), "revision": policy["revision"]}
        if held is None:
            nxt["capital"].append(entry)
            nxt["capital"].sort(key=lambda capital: capital["amount"]["asset"].encode("utf-8"))
        elif not same(held["amount"]["amount"], body["amount"]):
            held.update(entry)
    elif kind == "clear_capital":
        nxt["capital"] = [c for c in nxt["capital"] if c["amount"]["asset"] != body]
    else:
        raise AssertionError(f"unknown change {kind}")

    if policy_equal(nxt, policy):
        return ("unchanged",)
    revision = policy["revision"] + 1
    nxt["revision"] = revision
    if not defaults_equal(nxt["defaults"], policy["defaults"]):
        nxt["defaults"]["revision"] = revision
    for asset in nxt["assets"]:
        if not asset_equal(asset, find_asset(policy, asset["instrument"]["symbol"])):
            asset["revision"] = revision
    for capital in nxt["capital"]:
        if not capital_equal(capital, find_capital(policy, capital["amount"]["asset"])):
            capital["revision"] = revision
    reason = fault(nxt)
    if reason is not None:
        return refuse(reason)
    return ("changed", nxt)


def sourced(own, default):
    if own is not None:
        return {"value": own, "source": "Asset"}
    if default is not None:
        return {"value": default, "source": "Defaults"}
    return None


def resolve(policy, instrument):
    """What an instrument trades on, or the reason the profile cannot say."""
    profile = policy["identity"]["id"]
    line = exchange_of(policy)
    symbol = instrument["symbol"]
    if instrument["exchange"] != line:
        return None, (f"{symbol} is a {LINE_NAMES[instrument['exchange']]} contract, and profile "
                      f"{profile} trades {LINE_NAMES[line]}")
    own = find_asset(policy, symbol)
    if own is not None and own["instrument"]["settle"] != instrument["settle"]:
        return None, (f"profile {profile} holds {symbol} as settled in "
                      f"{own['instrument']['settle']}, and the venue lists it settled in "
                      f"{instrument['settle']}")
    defaults = policy["defaults"]
    if own is None or own["stop"] == "Inherit":
        stop = {"value": defaults["stop_percent"], "source": "Defaults"}
    elif own["stop"] == "Off":
        stop = {"value": None, "source": "Asset"}
    else:
        stop = {"value": own["stop"]["Percent"], "source": "Asset"}
    capital = find_capital(policy, instrument["settle"])
    return {
        "revision": policy["revision"],
        "configured": own is not None,
        "enabled": defaults["enabled"] if own is None else own["enabled"],
        "risk_percent": sourced(own and own["risk_percent"], defaults["risk_percent"]),
        "stop_percent": stop,
        "leverage": sourced(own and own["leverage"], defaults["leverage"]),
        "capital": None if capital is None else {
            "amount": capital["amount"]["amount"],
            "asset": capital["amount"]["asset"],
            "revision": capital["revision"],
        },
    }, None


def outside_universe(listed):
    """Why universe rule 1 leaves a listed instrument out, if it does."""
    contract = listed["contract"]
    if contract == "TradFiPerpetual":
        return "TradFi perpetuals are outside the v0.3 universe"
    if isinstance(contract, dict):
        return "dated futures are outside the v0.3 universe"
    if listed["status"] != "Trading":
        return f"the venue lists it as {listed['status']['Other']}, not trading"
    settle = listed["id"]["settle"]
    if listed["id"]["exchange"].endswith("Linear") and settle != "USDT":
        return f"linear contracts settled in {settle} are refused; only USDT is"
    return None


def entry(policy, listed):
    """("snapshot", document) or ("refused", refusal, message)."""
    profile = policy["identity"]["id"]
    instrument = listed["id"]
    symbol = instrument["symbol"]
    effective, reason = resolve(policy, instrument)
    if effective is None:
        return ("refused", "ambiguous_selection", reason)
    if not effective["enabled"]:
        if effective["configured"]:
            return ("refused", "not_armed",
                    f"{symbol} is disabled in profile {profile}; enable the asset to take "
                    "entries on it")
        return ("refused", "unknown_ticker",
                f"profile {profile} has no settings for {symbol}, and its defaults are not "
                "enabled")
    outside = outside_universe(listed)
    if outside is not None:
        return ("refused", "outside_universe", f"{symbol}: {outside}")
    if effective["capital"] is None:
        return ("refused", "no_capital_in_settlement_asset",
                f"profile {profile} has no capital in {instrument['settle']}, which {symbol} "
                "settles in")
    assert effective["risk_percent"] is not None and effective["leverage"] is not None
    return ("snapshot", {
        "format": SNAPSHOT_FORMAT,
        "profile": profile,
        "account": copy.deepcopy(policy["identity"]["account_ref"]),
        "revision": policy["revision"],
        "instrument": copy.deepcopy(instrument),
        "risk_percent": effective["risk_percent"],
        "stop_percent": effective["stop_percent"],
        "leverage": effective["leverage"],
        "capital": money(effective["capital"]["amount"], effective["capital"]["asset"]),
        "capital_revision": effective["capital"]["revision"],
    })


def stop_for(snapshot, signal_stop):
    """The stop an entry uses: the alert's own first, then the fallback, else a refusal."""
    if signal_stop is not None:
        return {"signal": signal_stop}
    fallback = snapshot["stop_percent"]["value"]
    if fallback is not None:
        return {"fallback": fallback}
    return {"refused": {
        "refusal": "no_stop",
        "message": (f"the alert brings no stop, and profile {snapshot['profile']} gives "
                    f"{snapshot['instrument']['symbol']} no fallback stop percentage"),
    }}


def check(policy, instruments):
    """Every reason the profile cannot execute as its policy stands, by the venue's list."""
    line = exchange_of(policy)
    problems = []
    for asset in policy["assets"]:
        if not asset["enabled"]:
            continue
        symbol = asset["instrument"]["symbol"]
        listed = next((i for i in instruments
                       if i["id"]["exchange"] == line and i["id"]["symbol"] == symbol), None)
        found = None
        if listed is None:
            found = ("not_listed", f"the venue lists no {LINE_NAMES[line]} contract {symbol}")
        elif listed["id"]["settle"] != asset["instrument"]["settle"]:
            found = ("settlement_changed",
                     f"the venue lists it settled in {listed['id']['settle']}, and the profile "
                     f"holds it as settled in {asset['instrument']['settle']}")
        elif outside_universe(listed) is not None:
            found = ("outside_universe", outside_universe(listed))
        else:
            leverage = leverage_of(policy, asset)
            band = listed["leverage"]
            kind = None
            if leverage is not None and band is not None:
                value, low, high = Decimal(leverage), Decimal(band["min"]), Decimal(band["max"])
                if value < low or value > high:
                    kind = "leverage_out_of_range"
                elif (value - low) % Decimal(band["step"]) != 0:
                    kind = "leverage_off_step"
            if kind is not None:
                found = (kind, f"the venue takes a leverage from {band['min']} to {band['max']} "
                               f"in steps of {band['step']}, not {leverage}")
            elif find_capital(policy, listed["id"]["settle"]) is None:
                found = ("no_capital", f"the profile has no capital in {listed['id']['settle']}, "
                                       "which it settles in")
        if found is not None:
            problems.append({"subject": symbol, "kind": found[0], "message": found[1]})
    if policy["defaults"]["enabled"] and not policy["capital"]:
        problems.append({
            "subject": "defaults",
            "kind": "no_capital",
            "message": "the defaults are enabled, and the profile has no capital in any asset",
        })
    return problems


# ---------------------------------------------------------------------------
# The cases: scripts over one line, run through the model to record what each
# step answers.
# ---------------------------------------------------------------------------


class Script:
    """One case on one line: the steps asked, each with the model's answer."""

    def __init__(self, line):
        self.line = line
        self.policy = new_policy(line["identity"])
        self.steps = []
        self.documents = {1: stored(self.policy)}

    def listed(self, symbol):
        return next(i for i in self.line["instruments"] if i["id"]["symbol"] == symbol)

    def settle(self, symbol):
        return self.listed(symbol)["id"]["settle"]

    def change(self, change, expect=None):
        outcome = apply(self.policy, change)
        step = {"change": change}
        if outcome[0] == "changed":
            assert outcome[1]["revision"] == self.policy["revision"] + 1
            assert fault(outcome[1]) is None
            self.policy = outcome[1]
            step["document"] = stored(self.policy)
            self.documents[self.policy["revision"]] = step["document"]
        elif outcome[0] == "unchanged":
            step["unchanged"] = True
        else:
            step["refused"] = outcome[1]
        assert expect is None or outcome[0] == expect, (change, outcome)
        self.steps.append(step)
        return outcome[0]

    def add(self, symbol, enabled=True, **settings):
        body = {"symbol": symbol, "settle": self.settle(symbol), "enabled": enabled}
        body.update(settings)
        return self.change({"add_asset": body})

    def effective(self, symbol):
        answer, reason = resolve(self.policy, self.listed(symbol)["id"])
        assert reason is None, reason
        self.steps.append({"effective": symbol, "expect": answer})
        return answer

    def entry(self, symbol, signal_stop=None, expect=None):
        outcome = entry(self.policy, self.listed(symbol))
        step = {"entry": symbol, "signal_stop": signal_stop}
        if outcome[0] == "snapshot":
            step["snapshot"] = stored(outcome[1])
            step["stop"] = stop_for(outcome[1], signal_stop)
            got = "no_stop" if "refused" in step["stop"] else "snapshot"
        else:
            step["refused"] = {"refusal": outcome[1], "message": outcome[2]}
            got = outcome[1]
        assert expect is None or got == expect, (symbol, got, expect)
        self.steps.append(step)
        return got

    def check(self, expect=None):
        problems = check(self.policy, self.line["instruments"])
        kinds = [problem["kind"] for problem in problems]
        assert expect is None or kinds == expect, (kinds, expect)
        self.steps.append({"check": True, "problems": problems})
        return problems


def capital_for(script, symbol, amount=None):
    """Capital in the asset `symbol` settles in: 15,000 of a stablecoin, or 2 of a coin."""
    settle = script.settle(symbol)
    if amount is None:
        amount = "15000" if settle.startswith("USD") else "2"
    return {"set_capital": {"amount": amount, "asset": settle}}


def set_defaults(**edits):
    body = {}
    for key, value in edits.items():
        body[key] = value if key == "enabled" or value == "clear" else {"set": value}
    return {"defaults": body}


def edit_asset(symbol, **edits):
    body = {"symbol": symbol}
    for key, value in edits.items():
        body[key] = value if key in ("enabled", "stop") or value == "clear" else {"set": value}
    return {"edit_asset": body}


def case_inheritance(s):
    """An asset without settings of its own trades on the defaults, setting by setting."""
    main = s.line["use"]["main"]
    s.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    s.add(main)
    s.change(capital_for(s, main), "changed")
    s.effective(main)
    s.entry(main, None, "snapshot")
    # Its own risk percentage, and the rest still the defaults'.
    s.change(edit_asset(main, risk_percent="0.5"), "changed")
    s.effective(main)
    # A change of the defaults reaches only what the asset inherits.
    s.change(set_defaults(risk_percent="2", leverage="10"), "changed")
    s.effective(main)
    s.entry(main, None, "snapshot")
    # The defaults stop giving a fallback: the asset has none, and needs the alert's stop.
    s.change(set_defaults(stop_percent="clear"), "changed")
    s.effective(main)
    s.entry(main, None, "no_stop")
    s.entry(main, "200.00", "snapshot")
    s.check([])


def case_reset(s):
    """An asset's own settings go back to the defaults, one by one or all at once."""
    main, second = s.line["use"]["main"], s.line["use"]["second"]
    s.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    s.add(main, risk_percent="0.25", stop={"percent": "1.50"}, leverage="2")
    s.add(second, False, risk_percent="4")
    s.change(capital_for(s, main), "changed")
    s.effective(main)
    # One setting back on the defaults.
    s.change(edit_asset(main, leverage="clear"), "changed")
    s.effective(main)
    # Every setting back at once; the asset stays, and stays enabled.
    s.change({"reset_asset": main}, "changed")
    s.effective(main)
    s.entry(main, None, "snapshot")
    # Nothing left to reset: no revision is made of it.
    s.change({"reset_asset": main}, "unchanged")
    s.change(edit_asset(main, risk_percent="clear", stop="inherit"), "unchanged")
    # The other asset was never touched, and a disabled one resets as well.
    s.effective(second)
    s.change({"reset_asset": second}, "changed")
    s.effective(second)
    s.change({"reset_asset": "NOSUCH"}, "refused")


def case_stop_precedence(s):
    """The alert's stop comes first; the fallback is the asset's, the defaults', or off."""
    main, second = s.line["use"]["main"], s.line["use"]["second"]
    s.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    s.add(main)
    s.add(second, stop="off")
    s.change(capital_for(s, main), "changed")
    if s.settle(second) != s.settle(main):
        s.change(capital_for(s, second), "changed")
    # Inherited fallback, and an alert's own stop taking its place.
    s.entry(main, None, "snapshot")
    s.entry(main, "187.45", "snapshot")
    # Its own fallback.
    s.change(edit_asset(main, stop={"percent": "1.25"}), "changed")
    s.entry(main, None, "snapshot")
    # Turned off for one asset, whatever the defaults say.
    s.effective(second)
    s.entry(second, None, "no_stop")
    s.entry(second, "91000.5", "snapshot")
    # Back on the defaults.
    s.change(edit_asset(second, stop="inherit"), "changed")
    s.entry(second, None, "snapshot")


def case_disabled(s):
    """A disabled asset takes no entry and keeps its settings; enabling it takes them back."""
    main = s.line["use"]["main"]
    s.change(set_defaults(risk_percent="1", leverage="5"), "changed")
    s.add(main, False, stop={"percent": "2"})
    s.change(capital_for(s, main), "changed")
    s.effective(main)
    s.entry(main, None, "not_armed")
    s.check([])
    s.change(edit_asset(main, enabled=True), "changed")
    s.entry(main, None, "snapshot")
    s.change(edit_asset(main, enabled=False), "changed")
    s.entry(main, "150", "not_armed")
    s.effective(main)
    # Disabling again changes nothing.
    s.change(edit_asset(main, enabled=False), "unchanged")


def case_defaults_switch(s):
    """Defaults enable nothing by themselves; switched on, an asset without settings trades."""
    use = s.line["use"]
    main, second = use["main"], use["second"]
    s.entry(main, None, "unknown_ticker")
    # Enabled defaults need a risk percentage and a leverage.
    s.change(set_defaults(enabled=True), "refused")
    s.change(set_defaults(enabled=True, risk_percent="1"), "refused")
    s.change(set_defaults(risk_percent="1", stop_percent="4", leverage="3"), "changed")
    s.entry(main, None, "unknown_ticker")
    s.change(set_defaults(enabled=True), "changed")
    s.check(["no_capital"])
    s.entry(main, None, "no_capital_in_settlement_asset")
    s.change(capital_for(s, main), "changed")
    s.check([])
    s.effective(main)
    s.entry(main, None, "snapshot")
    # An asset with settings of its own answers to its own switch, not the defaults'.
    s.add(second, False)
    s.entry(second, None, "not_armed")
    # What the universe leaves out stays out, defaults or not.
    s.entry(use["dated"], None, "outside_universe")
    s.entry(use["halted"], None, "outside_universe")
    if use["other_settle"] is not None:
        s.entry(use["other_settle"], None, "outside_universe")
    # Enabled defaults cannot lose what they trade on.
    s.change(set_defaults(leverage="clear"), "refused")
    s.change(set_defaults(enabled=False, leverage="clear"), "changed")
    s.entry(main, None, "unknown_ticker")


def case_capital(s):
    """Capital is set per settlement asset, with the revision that set it."""
    main, second = s.line["use"]["main"], s.line["use"]["second"]
    settle = s.settle(main)
    s.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    s.add(main)
    s.entry(main, None, "no_capital_in_settlement_asset")
    s.check(["no_capital"])
    s.change({"set_capital": {"amount": "0", "asset": settle}}, "refused")
    s.change({"set_capital": {"amount": "-1", "asset": settle}}, "refused")
    s.change({"set_capital": {"amount": "1500.50", "asset": settle}}, "changed")
    s.entry(main, None, "snapshot")
    # A label is a revision too, and leaves the capital's own stamp alone.
    s.change({"label": "Main account"}, "changed")
    s.entry(main, None, "snapshot")
    # The same amount with another scale is the same capital.
    s.change({"set_capital": {"amount": "1500.5", "asset": settle}}, "unchanged")
    s.change({"set_capital": {"amount": "2000", "asset": settle}}, "changed")
    s.entry(main, None, "snapshot")
    # Capital in another asset is its own, and never stands in.
    s.change({"set_capital": {"amount": "7.25", "asset": "XRP"}}, "changed")
    s.add(second)
    if s.settle(second) == settle:
        s.entry(second, None, "snapshot")
    else:
        s.entry(second, None, "no_capital_in_settlement_asset")
    s.change({"clear_capital": settle}, "changed")
    s.entry(main, None, "no_capital_in_settlement_asset")
    s.change({"clear_capital": settle}, "unchanged")
    s.change({"label": None}, "changed")


def case_invariants(s):
    """What a document holds could trade: changes that would break that are refused whole."""
    use = s.line["use"]
    main, second = use["main"], use["second"]
    other_line = "BybitInverse" if exchange_of(s.policy) != "BybitInverse" else "BinanceLinear"
    s.add(main, True)
    assert s.steps[-1].get("refused"), "an enabled asset needs a risk percentage"
    s.add(main, True, risk_percent="1")
    assert s.steps[-1].get("refused"), "and a leverage"
    s.add(main, False)
    s.add(main, False)
    assert s.steps[-1].get("refused"), "an asset is added once"
    s.change(edit_asset(main, enabled=True), "refused")
    s.change(edit_asset(main, enabled=True, risk_percent="1", leverage="4"), "changed")
    s.change(edit_asset(main, leverage="clear"), "refused")
    # With defaults behind it the same edit is taken.
    s.change(set_defaults(risk_percent="2", leverage="3"), "changed")
    s.change(edit_asset(main, leverage="clear"), "changed")
    s.effective(main)
    # And the defaults cannot then lose what the asset takes from them.
    s.change(set_defaults(leverage="clear"), "refused")
    s.change(edit_asset(second, enabled=True), "refused")
    s.change({"add_asset": {"symbol": "ADAUSD", "settle": "ADA", "exchange": other_line,
                            "enabled": False}}, "refused")
    # A refused change leaves nothing behind: the next one continues from where it was.
    s.change({"label": "Still revision by revision"}, "changed")


def case_stamps(s):
    """Each change stamps its revision on what it changed, and on nothing else."""
    main, second = s.line["use"]["main"], s.line["use"]["second"]
    s.change({"label": "Stamps"}, "changed")
    s.change(set_defaults(risk_percent="1.0", leverage="5"), "changed")
    s.add(second, False)
    s.add(main, False, risk_percent="0.75")
    s.change(capital_for(s, main), "changed")
    # The same numbers written with another scale change nothing.
    s.change(set_defaults(risk_percent="1", leverage="5.0"), "unchanged")
    s.change(edit_asset(main, risk_percent="0.750"), "unchanged")
    # A real change takes the new spelling of everything it sets.
    s.change(set_defaults(risk_percent="1.00", leverage="6"), "changed")
    s.change(edit_asset(main, enabled=True), "changed")
    s.effective(main)
    s.effective(second)


def case_venue_check(s):
    """Before a profile may execute, its policy is held to the venue's own list."""
    use = s.line["use"]
    main, second = use["main"], use["second"]
    s.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    s.add(main)
    s.change(capital_for(s, main), "changed")
    s.check([])
    # An asset the venue does not list, one it lists under another settlement asset, one the
    # universe leaves out, and one that is disabled, which is held to nothing.
    s.change({"add_asset": {"symbol": "GONEUSD", "settle": s.settle(main), "enabled": True}},
             "changed")
    s.change({"add_asset": {"symbol": second, "settle": "EUR", "enabled": True}}, "changed")
    s.add(use["dated"])
    s.add(use["halted"], False)
    s.check(None)
    s.entry(second, None, "ambiguous_selection")
    s.entry(use["dated"], None, "outside_universe")
    s.change(edit_asset("GONEUSD", enabled=False), "changed")
    s.change(edit_asset(second, enabled=False), "changed")
    s.change(edit_asset(use["dated"], enabled=False), "changed")
    s.check([])
    # A leverage the venue does not take, where its list says what it takes.
    s.change(edit_asset(main, leverage="250"), "changed")
    band = s.listed(main)["leverage"]
    s.check([] if band is None else ["leverage_out_of_range"])
    s.change(edit_asset(main, leverage="2.505"), "changed")
    s.check([] if band is None else ["leverage_off_step"])
    s.change(edit_asset(main, leverage="2.5"), "changed")
    s.check([])
    s.change({"clear_capital": s.settle(main)}, "changed")
    s.check(["no_capital"])


CASES = [
    ("inheritance", case_inheritance),
    ("reset_to_default", case_reset),
    ("stop_precedence", case_stop_precedence),
    ("disabled_asset", case_disabled),
    ("defaults_switch", case_defaults_switch),
    ("capital", case_capital),
    ("invariants", case_invariants),
    ("revision_stamps", case_stamps),
    ("venue_check", case_venue_check),
]


def race(line):
    """Of two changes made from one revision one is stored and the other is made again on
    top of it: either order, and each alone."""
    main, second = line["use"]["main"], line["use"]["second"]
    base = Script(line)
    base.change(set_defaults(risk_percent="1", stop_percent="3", leverage="5"), "changed")
    base.add(main)
    base.add(second, False)
    left = edit_asset(main, risk_percent="0.5")
    right = edit_asset(second, enabled=True, leverage="2")

    def after(first, then):
        script = copy.deepcopy(base)
        assert script.change(first) == "changed"
        alone = stored(script.policy)
        assert script.change(then) == "changed"
        return alone, stored(script.policy)

    left_alone, left_then_right = after(left, right)
    right_alone, right_then_left = after(right, left)
    assert left_then_right != right_then_left, "the two orders stamp different revisions"
    return {
        "id": f"two_changes_from_one_revision/{line['name'].replace(' ', '_')}",
        "doc": statement(race),
        "line": line["name"],
        "base": [step["change"] for step in base.steps],
        "base_document": stored(base.policy),
        "left": left,
        "right": right,
        "left_alone": left_alone,
        "right_alone": right_alone,
        "left_then_right": left_then_right,
        "right_then_left": right_then_left,
    }


def statement(script):
    """What a script shows, as the lists of docs/automation-policy.md word it: its
    docstring, without the capital and the full stop of a sentence."""
    text = " ".join(script.__doc__.split()).rstrip(".")
    return text[0].lower() + text[1:]


def build():
    all_lines = lines()
    cases = []
    for name, script in CASES:
        for line in all_lines:
            run = Script(line)
            script(run)
            cases.append({
                "id": f"{name}/{line['name'].replace(' ', '_')}",
                "doc": statement(script),
                "line": line["name"],
                "created": run.documents[1],
                "steps": run.steps,
            })
    return {
        "format": FIXTURE_FORMAT,
        "generated_by": "spikes/futures-contract/policy_cases.py",
        "policy_format": POLICY_FORMAT,
        "snapshot_format": SNAPSHOT_FORMAT,
        "lines": all_lines,
        "cases": cases,
        "races": [race(line) for line in all_lines],
    }


def render(cases):
    """The fixture's text: one line for each instrument list, step and race, so that a
    change to the model shows as the lines it changed."""
    compact = lambda value: json.dumps(value, separators=(",", ":"), ensure_ascii=False)
    out = ["{"]
    for key in ("format", "generated_by", "policy_format", "snapshot_format"):
        out.append(f" {compact(key)}: {compact(cases[key])},")
    out.append(' "lines": [')
    for n, line in enumerate(cases["lines"]):
        head = {key: line[key] for key in ("name", "identity", "use")}
        out.append(f"  {compact(head)[:-1]},\"instruments\":[")
        rows = [f"   {compact(instrument)}" for instrument in line["instruments"]]
        out.append(",\n".join(rows))
        out.append("  ]}" + ("," if n + 1 < len(cases["lines"]) else ""))
    out.append(" ],")
    out.append(' "cases": [')
    for n, case in enumerate(cases["cases"]):
        head = {key: case[key] for key in ("id", "doc", "line", "created")}
        out.append(f"  {compact(head)[:-1]},\"steps\":[")
        out.append(",\n".join(f"   {compact(step)}" for step in case["steps"]))
        out.append("  ]}" + ("," if n + 1 < len(cases["cases"]) else ""))
    out.append(" ],")
    out.append(' "races": [')
    out.append(",\n".join(f"  {compact(race)}" for race in cases["races"]))
    out.append(" ]")
    out.append("}")
    text = "\n".join(out) + "\n"
    assert json.loads(text) == cases, "the rendered text is the cases"
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", metavar="PATH", help="write the cases to PATH")
    args = parser.parse_args()
    text = render(build())
    if args.write:
        with open(args.write, "w", encoding="utf-8", newline="\n") as out:
            out.write(text)
    cases = json.loads(text)
    steps = sum(len(case["steps"]) for case in cases["cases"])
    print(f"{len(cases['cases'])} cases, {steps} steps, {len(cases['races'])} races",
          file=sys.stderr)


if __name__ == "__main__":
    main()
