#!/usr/bin/env python3
"""Independent Decimal oracle for ADR 0002's risk sizing and accounting rules.

Every worked example in ADR 0002 comes from this script, never from the PRD's
illustrative figures, and the Rust implementation (`model::futures::sizing`) is
held to the cases it writes. The cases apply the rules of ADR 0002 sections 6,
9 and 12 to inputs in the shared model's stored form: what a venue's adapter
makes of its instrument metadata, its leverage tiers and an account. The rules
are:

- R = B x r, refused unless the capital is in the instrument's settlement
  asset;
- the stop is the alert's absolute stop if given, side-checked, else the
  percentage fallback E x (1 - p) for LONG or E x (1 + p) for SHORT, else
  refused;
- the stop rounds to the tick away from the entry, giving S';
- linear quantity is R / |E - S'|;
- inverse contracts are R / (F x |1/E - 1/S'|);
- both are floored to the market step, then checked against the minimum
  quantity, minimum notional and market-order maximum;
- the position, in the settlement asset at E, must fit the leverage bracket:
  at most the largest cap among the tiers whose maximum leverage is at least
  the chosen one;
- the initial margin plus taker fees for opening and closing must fit the
  available margin of the venue's pool minus reservations, and a pool in USD
  is measured at the venue's index price of the settlement asset;
- an entry is refused when the settlement asset's wallet holds less than R,
  and warned about when it holds less than B;
- at submission the entry is refused if the mark price or the book has already
  crossed the stop, or if the risk at the expected fill exceeds 1.25 x R;
- after the fills, the risk taken is the loss from each fill's price to the
  stop, reported beside what the filled quantity would have risked at E.

An input sizing cannot trust is a fault, apart from the refusals: rules, an
account or prices that are for something else, too old, missing, ambiguous or
invalid. Faults are looked for first, in one fixed order.

It checks itself in three ways:
- each result is tested for the invariants the rules promise: on the grid,
  within budget, the floor is tight, the stop on the protective side, the
  position within its leverage bracket and its margin within the pool;
- every answer is worked out again in exact rational arithmetic (fractions), a
  second implementation that shares no arithmetic with the first, and the two
  must agree on every refusal and every figure;
- hand-computed expectations (stops, quantities, linear amounts) are compared
  exactly.

    python3 -I spikes/futures-contract/sizing.py
    python3 -I spikes/futures-contract/sizing.py --write spikes/futures-contract/sizing-cases.json
    python3 -I spikes/futures-contract/sizing.py --write tests/fixtures/sizing_cases.json
    python3 -I spikes/futures-contract/sizing.py --doc docs/risk-sizing.md

The two files are the same cases: the first is the one ADR 0002 links, the
second the one the crate's tests read, since `spikes/` is not packaged.
`--doc` rewrites the lists of that page which repeat the cases: its worked
examples and its lists of cases. The crate's tests hold the page to the
fixture either way.

Standard library only; no network.
"""

import argparse
import copy
import json
import math
import sys
import textwrap
from decimal import ROUND_CEILING, ROUND_FLOOR, ROUND_HALF_EVEN, Context, Decimal, localcontext
from fractions import Fraction

D = Decimal
FORMAT = 2
PRECISION = 60
AMOUNT_PLACES = 12
AMOUNT_QUANTUM = D("1e-12")
RATIO_PLACES = D("1e-8")
DRIFT_LIMIT = D("1.25")
# The largest mantissa an exact decimal of the shared model holds: 96 bits.
MANTISSA_LIMIT = 2 ** 96

# 2026-10-20T00:00:00Z: the clock every case is sized at.
NOW_MS = 1792454400000
# How old an input may be, in milliseconds, before sizing no longer trusts it.
FRESHNESS_MS = {"prices": 5000, "account": 10000, "rules": 3600000}
# How old each input is in a case that says nothing else. All differ, so a
# limit applied to the wrong input shows.
AGES_MS = {"rules": 60000, "brackets": 90000, "fees": 120000, "account": 2000, "index": 1000,
           "mark": 300, "quote": 200}

# A made-up key fingerprint: sixteen hex digits, plain enough that a secret
# scanner does not take a fixture's fingerprint for a key.
FINGERPRINT = "00000000000000a1"

# Rules recorded from the venues' public metadata on 2026-10-22 (see
# universe.py); fees are illustrative taker rates, not venue quotes.
RULES = {
    "binance linear BTCUSDT": {"tick": "0.10", "step": "0.001", "min_qty": "0.001", "max_mkt": "120", "min_notional": "50"},
    "binance linear SOLUSDT": {"tick": "0.0100", "step": "0.01", "min_qty": "0.01", "max_mkt": "80000", "min_notional": "5"},
    "binance inverse BTCUSD_PERP": {"tick": "0.1", "step": "1", "min_qty": "1", "max_mkt": "60000", "face": "100"},
    "binance inverse ETHUSD_PERP": {"tick": "0.01", "step": "1", "min_qty": "1", "max_mkt": "500000", "face": "10"},
    "bybit linear SOLUSDT": {"tick": "0.010", "step": "0.1", "min_qty": "0.1", "max_mkt": "12000.0", "min_notional": "5"},
    "bybit inverse BTCUSD": {"tick": "0.10", "step": "1", "min_qty": "1", "max_mkt": "5000000", "min_notional": "5", "face": "1"},
    "bybit inverse ETHUSD": {"tick": "0.01", "step": "1", "min_qty": "1", "max_mkt": "2000000", "min_notional": "5", "face": "1"},
    "bybit inverse SOLUSD": {"tick": "0.01", "step": "1", "min_qty": "1", "max_mkt": "1000000", "min_notional": "5", "face": "1"},
}

SETTLE = {
    "binance linear BTCUSDT": "USDT",
    "binance linear SOLUSDT": "USDT",
    "binance inverse BTCUSD_PERP": "BTC",
    "binance inverse ETHUSD_PERP": "ETH",
    "bybit linear SOLUSDT": "USDT",
    "bybit inverse BTCUSD": "BTC",
    "bybit inverse ETHUSD": "ETH",
    "bybit inverse SOLUSD": "SOL",
}

BASE = {
    "binance linear BTCUSDT": "BTC",
    "binance linear SOLUSDT": "SOL",
    "binance inverse BTCUSD_PERP": "BTC",
    "binance inverse ETHUSD_PERP": "ETH",
    "bybit linear SOLUSDT": "SOL",
    "bybit inverse BTCUSD": "BTC",
    "bybit inverse ETHUSD": "ETH",
    "bybit inverse SOLUSD": "SOL",
}

# Leverage brackets as tiers of (maximum leverage, cap). A cap bounds the
# position in its settlement asset at E: USDT for linear, the coin for
# inverse. A leverage may hold up to the largest cap among the tiers that
# allow it. Bybit's tiers are its public risk limits on 2026-10-22. Binance
# serves brackets per account, to signed requests only, so the Binance tiers
# here are illustrative.
BRACKETS = {
    "binance linear BTCUSDT": [
        ("125", "50000"), ("100", "600000"), ("50", "3000000"), ("20", "12000000"), ("10", "70000000"),
        ("5", "100000000"),
    ],
    "binance linear SOLUSDT": [("75", "10000"), ("50", "100000"), ("25", "500000"), ("10", "2000000"), ("5", "10000000")],
    "binance inverse BTCUSD_PERP": [("125", "50"), ("100", "100"), ("50", "200"), ("20", "500"), ("10", "1000")],
    "binance inverse ETHUSD_PERP": [("75", "500"), ("50", "2000"), ("25", "5000"), ("10", "20000")],
    "bybit linear SOLUSDT": [
        ("100.00", "50000"), ("90.00", "100000"), ("80.00", "250000"), ("75.00", "400000"),
        ("70.00", "550000"), ("65.00", "700000"), ("55.00", "850000"), ("50.00", "1000000"),
        ("45.00", "2000000"), ("40.00", "3000000"), ("35.00", "4000000"), ("30.00", "6000000"),
        ("25.00", "8000000"), ("20.00", "10000000"), ("19.00", "12000000"), ("18.00", "14000000"),
        ("17.00", "16000000"), ("16.00", "18000000"), ("15.00", "20000000"), ("14.00", "22000000"),
        ("13.00", "24000000"), ("12.00", "26000000"), ("11.00", "28000000"), ("10.00", "30000000"),
        ("9.00", "35000000"), ("8.00", "40000000"), ("7.00", "45000000"), ("6.00", "50000000"),
        ("5.00", "60000000"), ("4.00", "80000000"), ("3.00", "100000000"), ("2.00", "150000000"),
        ("1.43", "200000000"), ("1.28", "250000000"), ("1.00", "300000000"),
    ],
    "bybit inverse BTCUSD": [
        ("100.00", "150"), ("66.67", "300"), ("50.00", "450"), ("40.00", "600"), ("33.34", "750"),
        ("28.58", "900"), ("25.00", "1050"), ("22.23", "1200"), ("20.00", "1350"), ("18.19", "1500"),
    ],
    "bybit inverse ETHUSD": [
        ("100.00", "500"), ("50.00", "3000"), ("33.34", "6000"), ("25.00", "9000"), ("20.00", "12000"),
        ("16.67", "15000"), ("14.29", "18000"), ("12.50", "21000"), ("11.12", "24000"), ("10.00", "27000"),
        ("9.10", "30000"),
    ],
    "bybit inverse SOLUSD": [
        ("50.00", "200"), ("45.00", "400"), ("40.00", "700"), ("35.00", "1100"), ("30.00", "1500"),
        ("25.00", "1900"), ("20.00", "2300"), ("15.00", "3100"), ("10.00", "4600"), ("5.00", "6100"),
        ("4.00", "7600"), ("3.00", "38000"), ("2.00", "76000"), ("1.50", "115000"), ("1.00", "150000"),
    ],
}

# ---------------------------------------------------------------------------
# The venues' values in the shared model's stored form: what an adapter hands
# sizing. Nothing below this block reads RULES or BRACKETS.
# ---------------------------------------------------------------------------

EXCHANGES = {
    ("binance", "linear"): "BinanceLinear",
    ("binance", "inverse"): "BinanceInverse",
    ("bybit", "linear"): "BybitLinear",
    ("bybit", "inverse"): "BybitInverse",
}
# The leverage each venue's metadata gives an instrument, where it gives one.
LEVERAGE_FILTERS = {
    "bybit linear SOLUSDT": {"min": "1", "max": "100.00", "step": "0.01"},
    "bybit inverse BTCUSD": {"min": "1", "max": "100.00", "step": "0.01"},
    "bybit inverse ETHUSD": {"min": "1", "max": "100.00", "step": "0.01"},
    "bybit inverse SOLUSD": {"min": "1", "max": "50.00", "step": "0.01"},
}
# One account a venue: Binance issues an account id, Bybit's is known by its
# key's fingerprint.
ACCOUNT_IDS = {"binance": {"Venue": "9001234"}, "bybit": {"KeyFingerprint": FINGERPRINT}}
# What each venue calls the index it values a settlement asset at, for an
# account whose margin is one USD pool. The names are illustrative: each
# adapter records the one it read.
USD_SOURCES = {
    ("binance", "linear"): "asset index USDTUSD",
    ("bybit", "linear"): "USDT index price",
    ("bybit", "inverse"): "index price of the instrument",
}
MAKER_FEE = "0.0002"


def money(amount, asset):
    return {"amount": amount, "asset": asset}


def spec(key):
    """An instrument as the Rust `InstrumentSpec` stores it."""
    venue, family, symbol = key.split()
    rules = RULES[key]
    unit = "Base" if family == "linear" else "Contracts"
    quote = "USDT" if family == "linear" else "USD"
    inverse = None
    if family == "inverse":
        # Binance's metadata carries the contract size; Bybit documents 1 USD.
        source = "Metadata" if venue == "binance" else "DocumentedRule"
        inverse = {"face_value": rules["face"], "source": source}
    return {
        "id": {"exchange": EXCHANGES[venue, family], "symbol": symbol, "settle": SETTLE[key]},
        "base": BASE[key],
        "quote": quote,
        "contract": "Perpetual",
        "status": "Trading",
        "delist_at_ms": None,
        "inverse": inverse,
        "tick": rules["tick"],
        "step": {unit: rules["step"]},
        "market_step": {unit: rules["step"]},
        "min_qty": {unit: rules["min_qty"]},
        "market_min_qty": {unit: rules["min_qty"]},
        "market_max_qty": {unit: rules["max_mkt"]},
        "min_notional": money(rules["min_notional"], quote) if "min_notional" in rules else None,
        "leverage": LEVERAGE_FILTERS.get(key),
        "observed_at_ms": NOW_MS - AGES_MS["rules"],
    }


def tiers(key):
    """An instrument's leverage brackets as the Rust `LeverageBracket` stores them."""
    return [{"max_leverage": most, "cap": money(cap, SETTLE[key])} for most, cap in BRACKETS[key]]


INSTRUMENTS = {key: spec(key) for key in RULES}
TIERS = {key: tiers(key) for key in BRACKETS}


def account_ref(venue):
    return {"venue": venue.capitalize(), "environment": "Demo", "venue_account_id": ACCOUNT_IDS[venue]}


def identity(key):
    """The profile an instrument's cases are sized for, as `ProfileIdentity` stores it."""
    venue, family, _ = key.split()
    return {"id": "main", "account_ref": account_ref(venue), "category": family.capitalize(),
            "owner_tag": "k3x9p2"}


def wallet(amount, asset, available=None):
    """A wallet: what it holds, and what of that is free to use."""
    return {"balance": money(amount, asset), "available": money(available or amount, asset)}


def snapshot_of(case):
    """The policy snapshot an entry of `case` is admitted under, or None where the profile
    holds no capital in the settlement asset. The profile is made (revision 1), given its
    capital (2), then the asset with settings of its own (3)."""
    key = case["instrument"]
    if case["capital"]["asset"] != SETTLE[key]:
        return None
    asset = lambda value: {"value": value, "source": "Asset"}
    return {
        "format": 1,
        "profile": "main",
        "account": account_ref(key.split()[0]),
        "revision": 3,
        "instrument": copy.deepcopy(INSTRUMENTS[key]["id"]),
        "risk_percent": asset(case["risk_percent"]),
        "stop_percent": asset(case.get("stop_percent")),
        "leverage": asset(case["leverage"]),
        "capital": money(case["capital"]["amount"], case["capital"]["asset"]),
        "capital_revision": 2,
    }


def put(target, path, value):
    """Sets `target`'s item at the dotted `path`; a number in the path indexes a list."""
    steps = [int(step) if step.isdigit() else step for step in path.split(".")]
    for step in steps[:-1]:
        target = target[step]
    target[steps[-1]] = value


def inputs_of(case):
    """Everything sizing is given for `case`, in the shared model's stored form."""
    key = case["instrument"]
    venue, family, _ = key.split()
    settle = SETTLE[key]
    usd = case.get("pool") == "usd"
    unit = "USD" if usd else settle
    pools = [{"scope": "AccountUsd" if usd else {"Asset": settle},
              "available": money(case["available_margin"], unit)}]
    pools += [{"scope": {"Asset": asset}, "available": money(available, asset)}
              for asset, available in case.get("more_pools", ())]
    wallets = []
    if case.get("settlement_wallet") is not None:
        wallets.append(wallet(case["settlement_wallet"], settle, case.get("wallet_available")))
    wallets += [wallet(balance, asset) for asset, balance in case.get("more_wallets", ())]
    index = None
    if "usd_index" in case:
        index = {"asset": settle, "price": case["usd_index"], "source": USD_SOURCES[venue, family],
                 "observed_at_ms": NOW_MS - AGES_MS["index"]}
    prices = None
    if "quote" in case:
        quote = case["quote"]
        prices = {
            "instrument": copy.deepcopy(INSTRUMENTS[key]["id"]),
            "mark": quote["mark"],
            "index": None,
            "marked_at_ms": NOW_MS - AGES_MS["mark"],
            "bid": quote["bid"],
            "ask": quote["ask"],
            "quoted_at_ms": NOW_MS - AGES_MS["quote"],
        }
    inputs = {
        "profile": identity(key),
        "policy": {
            "capital": money(case["capital"]["amount"], case["capital"]["asset"]),
            "risk_percent": case["risk_percent"],
            "stop_percent": case.get("stop_percent"),
            "leverage": case["leverage"],
        },
        "snapshot": snapshot_of(case),
        "side": "Long" if case["side"] == "long" else "Short",
        "expected_price": case["entry"],
        "signal_stop": case.get("stop"),
        "instrument": key,
        "rules_at_ms": NOW_MS - AGES_MS["rules"],
        "brackets": key,
        "brackets_at_ms": NOW_MS - AGES_MS["brackets"],
        "fees": {"maker": MAKER_FEE, "taker": case["taker_fee"]},
        "fees_at_ms": NOW_MS - AGES_MS["fees"],
        "account": {
            "account": account_ref(venue),
            "kind": "Standard",
            # Only Binance's USD-margined accounts have a multi-assets mode.
            "multi_asset_margin": usd if (venue, family) == ("binance", "linear") else None,
            "wallets": wallets,
            "pools": pools,
            "observed_at_ms": NOW_MS - AGES_MS["account"],
        },
        "reserved": money(case.get("reserved_margin", "0"), unit),
        "usd_index": index,
        "prices": prices,
        "now_ms": NOW_MS,
    }
    for path, value in case.get("alter", ()):
        put(inputs, path, copy.deepcopy(value))
    return inputs


# ---------------------------------------------------------------------------
# The cases.
# ---------------------------------------------------------------------------


def entry(case_id, note, instrument, side, capital, risk_percent, entry_price, expect, **extra):
    case = {
        "id": case_id,
        "note": note,
        "instrument": instrument,
        "side": side,
        "capital": capital,
        "risk_percent": risk_percent,
        "entry": entry_price,
        "expect": expect,
    }
    case.update(extra)
    return case


USDT = lambda amount: {"asset": "USDT", "amount": amount}

# The cases ADR 0002 records.
SIZING_CASES = [
    entry("prd-p9-binance-linear-long",
          "The PRD's p.9 trade with a 150 USDT budget, 1 % of 15,000: it reproduces every p.9 figure but the risk line",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "15000"}, "1", "234.23",
          {"status": "sized", "stop": "200.0000", "qty": "4.38", "risk": "149.9274", "notional": "1025.9274",
           "initial_margin": "205.18548"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="15000"),
    entry("prd-p9-bybit-linear-long",
          "The same trade on Bybit, whose 0.1 step keeps less of the budget",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "15000"}, "1", "234.23",
          {"status": "sized", "stop": "200.000", "qty": "4.3", "risk": "147.189", "notional": "1007.189"},
          stop="200.00", leverage="5", taker_fee="0.00055", available_margin="15000", settlement_wallet="15000"),
    entry("prd-p9-literal-15-usdt",
          "The p.9 risk line read literally, 15 USDT: 0.43821 SOL before the 0.01 step, 0.43 after",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "sized", "stop": "200.0000", "qty": "0.43", "risk": "14.7189", "notional": "100.7189"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500"),
    entry("linear-short-percent-stop",
          "SHORT with the 3 % fallback stop, rounded up to the tick",
          "binance linear BTCUSDT", "short", {"asset": "USDT", "amount": "10000"}, "1", "84123.45",
          {"status": "sized", "stop": "86647.20", "qty": "0.039", "risk": "98.42625", "notional": "3280.81455"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="5000", settlement_wallet="10000"),
    entry("binance-inverse-long",
          "COIN-M LONG, 100 USD contracts; the wallet holds less than the capital, which only warns",
          "binance inverse BTCUSD_PERP", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "sized", "stop": "81599.6", "qty": "135", "warnings": ["wallet_below_capital"]},
          stop_percent="3", leverage="5", taker_fee="0.0005", available_margin="0.3", settlement_wallet="0.3"),
    entry("binance-inverse-short",
          "COIN-M SHORT, 10 USD contracts, with an absolute stop rounded up to the tick",
          "binance inverse ETHUSD_PERP", "short", {"asset": "ETH", "amount": "2"}, "0.5", "3012.34",
          {"status": "sized", "stop": "3105.56", "qty": "100"},
          stop="3105.555", leverage="5", taker_fee="0.0005", available_margin="2", settlement_wallet="2"),
    entry("bybit-inverse-long",
          "Bybit inverse LONG, 1 USD contracts: the trade of binance-inverse-long, about 100 times the contracts "
          "(13,599 against 135), since the finer grid keeps more of the budget",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "sized", "stop": "81599.60", "qty": "13599"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="0.5", settlement_wallet="0.5"),
    entry("bybit-inverse-short",
          "Bybit inverse SHORT with the 2 % fallback stop",
          "bybit inverse SOLUSD", "short", {"asset": "SOL", "amount": "20"}, "1", "187.45",
          {"status": "sized", "stop": "191.20", "qty": "1911"},
          stop_percent="2", leverage="3", taker_fee="0.00055", available_margin="20", settlement_wallet="20"),
    entry("refuse-wrong-side-stop",
          "A LONG whose stop is above its entry",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "refused", "reason": "stop_wrong_side"},
          stop="240.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500"),
    entry("refuse-no-stop",
          "Neither a payload stop nor a fallback percentage",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "refused", "reason": "no_stop"},
          leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500"),
    entry("refuse-below-min-qty",
          "A budget too small for one step is refused, never rounded up",
          "binance linear BTCUSDT", "long", {"asset": "USDT", "amount": "100"}, "0.1", "84123.45",
          {"status": "refused", "reason": "below_min_qty"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="100", settlement_wallet="100"),
    entry("refuse-below-min-notional",
          "Enough for two steps, but 4.6846 USDT is below the 5 USDT minimum notional",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "50"}, "1", "234.23",
          {"status": "refused", "reason": "below_min_notional"},
          stop_percent="10", leverage="5", taker_fee="0.0005", available_margin="50", settlement_wallet="50"),
    entry("refuse-above-market-max",
          "198.117 BTC is above the 120 BTC market-order maximum",
          "binance linear BTCUSDT", "short", {"asset": "USDT", "amount": "10000000"}, "5", "84123.45",
          {"status": "refused", "reason": "above_market_max", "qty": "198.117"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="10000000", settlement_wallet="10000000"),
    entry("refuse-above-leverage-bracket",
          "At 100x Bybit's lowest risk tier caps SOLUSDT at 50,000 USDT, far below this 190,428.99 USDT position",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "100000"}, "1", "234.23",
          {"status": "refused", "reason": "above_leverage_bracket", "qty": "813.0", "notional": "190428.99",
           "bracket_cap": "50000"},
          stop="233.00", leverage="100", taker_fee="0.00055", available_margin="100000", settlement_wallet="100000"),
    entry("leverage-bracket-fits-at-20x",
          "The same position at 20x, whose tiers reach 10,000,000 USDT",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "100000"}, "1", "234.23",
          {"status": "sized", "stop": "233.000", "qty": "813.0", "risk": "999.99", "notional": "190428.99",
           "bracket_cap": "10000000"},
          stop="233.00", leverage="20", taker_fee="0.00055", available_margin="100000", settlement_wallet="100000"),
    entry("refuse-inverse-above-leverage-bracket",
          "At 50x Bybit caps SOLUSD at 200 SOL, and 75,135 contracts are 400.83 SOL",
          "bybit inverse SOLUSD", "short", {"asset": "SOL", "amount": "200"}, "1", "187.45",
          {"status": "refused", "reason": "above_leverage_bracket", "stop": "188.39", "qty": "75135",
           "bracket_cap": "200"},
          stop_percent="0.5", leverage="50", taker_fee="0.00055", available_margin="200", settlement_wallet="200"),
    entry("refuse-insufficient-margin",
          "331.36226955 USDT needed; 400 available minus a 100 USDT reservation leaves 300",
          "binance linear BTCUSDT", "short", {"asset": "USDT", "amount": "10000"}, "1", "84123.45",
          {"status": "refused", "reason": "insufficient_margin", "required_margin": "331.36226955"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="400", reserved_margin="100",
          settlement_wallet="400"),
    entry("refuse-linear-wallet-below-risk",
          "Cross margin funded by other collateral: 15,000 USD of margin, but 100 USDT cannot pay a 150 USDT stop-out",
          "bybit linear SOLUSDT", "long", {"asset": "USDT", "amount": "15000"}, "1", "234.23",
          {"status": "refused", "reason": "settlement_wallet_below_risk"},
          stop="200.00", leverage="5", taker_fee="0.00055", available_margin="15000", settlement_wallet="100"),
    entry("refuse-wallet-below-risk",
          "An inverse stop-out the BTC wallet could not pay",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "refused", "reason": "settlement_wallet_below_risk"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="0.5", settlement_wallet="0.004"),
    entry("refuse-capital-currency",
          "An ETH-settled instrument with capital configured only in BTC",
          "bybit inverse ETHUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "3012.34",
          {"status": "refused", "reason": "no_capital_in_settlement_asset"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="2", settlement_wallet="2"),
    entry("submit-drift-accepted",
          "At an ask of 243.00 the risk is 18.49, within 1.25 x 15 = 18.75",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "sized", "qty": "0.43", "risk_at_fill": "18.49"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "242.90", "ask": "243.00", "mark": "242.95"}),
    entry("submit-drift-refused",
          "At an ask of 244.00 the risk is 18.92, above 18.75",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "refused", "reason": "risk_drift", "risk_at_fill": "18.92"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "243.90", "ask": "244.00", "mark": "243.95"}),
    entry("submit-book-crossed",
          "A SHORT whose stop the ask has already reached",
          "bybit linear SOLUSDT", "short", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "refused", "reason": "stop_crossed"},
          stop="240.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "239.99", "ask": "240.01", "mark": "239.95"}),
    entry("submit-mark-crossed",
          "A LONG whose book is still above its stop, but whose mark price, which triggers the stop, is below it",
          "binance linear SOLUSDT", "long", {"asset": "USDT", "amount": "1500"}, "1", "234.23",
          {"status": "refused", "reason": "stop_crossed"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "200.50", "ask": "200.60", "mark": "199.95"}),
]

# What ALG-736 adds: the cell the first cases left out, the choice of stop,
# every limit met exactly and missed by one step, margin pools in USD, and the
# submission guards on inverse contracts.
SIZING_CASES += [
    entry("bybit-linear-short",
          "Bybit linear SHORT with the 4 % fallback stop: 243.5992 rounds up to 243.60",
          "bybit linear SOLUSDT", "short", USDT("15000"), "1", "234.23",
          {"status": "sized", "stop": "243.600", "qty": "16.0", "risk": "149.92", "notional": "3747.68",
           "initial_margin": "749.536"},
          stop_percent="4", leverage="5", taker_fee="0.00055", available_margin="15000", settlement_wallet="15000"),
    entry("signal-stop-overrides-the-fallback",
          "The alert's own stop is used, though the asset has a 3 % fallback that would give 227.20",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "sized", "stop": "200.0000", "qty": "4.38", "risk": "149.9274"},
          stop="200.00", stop_percent="3", leverage="5", taker_fee="0.0005", available_margin="15000",
          settlement_wallet="15000"),
    entry("refuse-wrong-side-stop-despite-the-fallback",
          "A wrong-side stop in the alert refuses the entry; the fallback never replaces it",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "stop_wrong_side"},
          stop="240.00", stop_percent="3", leverage="5", taker_fee="0.0005", available_margin="15000",
          settlement_wallet="15000"),
    entry("refuse-short-wrong-side-stop",
          "A SHORT whose stop is below its entry",
          "bybit linear SOLUSDT", "short", USDT("1500"), "1", "234.23",
          {"status": "refused", "reason": "stop_wrong_side"},
          stop="230.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500"),
    entry("refuse-stop-at-the-entry",
          "A stop at the entry price protects nothing: it is on the wrong side too",
          "bybit linear SOLUSDT", "long", USDT("1500"), "1", "234.23",
          {"status": "refused", "reason": "stop_wrong_side"},
          stop="234.23", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500"),
    entry("refuse-stop-rounds-to-nothing",
          "A stop below one tick rounds down to no price at all, so the entry has no stop the venue can hold",
          "binance linear SOLUSDT", "long", USDT("1500"), "1", "234.23",
          {"status": "refused", "reason": "no_stop"},
          stop="0.004", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500"),
    entry("linear-long-stop-off-the-tick",
          "A LONG stop of 200.008 rounds down to 200.00, away from the entry, never to the nearer 200.01",
          "bybit linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "sized", "stop": "200.000", "qty": "4.3", "risk": "147.189"},
          stop="200.008", leverage="5", taker_fee="0.00055", available_margin="15000", settlement_wallet="15000"),
    entry("linear-exact-budget",
          "150 / 37.50 is 4 exactly: the floor keeps all four, and the risk equals the budget",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "stop": "200.0000", "qty": "4.00", "risk": "150", "notional": "950",
           "initial_margin": "190", "fee_allowance": "0.95", "required_margin": "190.95"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="15000"),
    entry("inverse-exact-budget",
          "0.25 / (1/80 - 1/100) is 100 contracts exactly, and a third of a coin of margin at 3x",
          "bybit inverse SOLUSD", "long", {"asset": "SOL", "amount": "25"}, "1", "100.00",
          {"status": "sized", "stop": "80.00", "qty": "100", "risk": "0.25", "notional": "100",
           "notional_settle": "1", "initial_margin": "0.333333333333", "fee_allowance": "0.0011",
           "required_margin": "0.334433333333"},
          stop="80.00", leverage="3", taker_fee="0.00055", available_margin="20", settlement_wallet="25"),
    entry("refuse-margin-short-by-less-than-a-reported-place",
          "The same entry needs a third of a coin and a little, reported as 0.334433333333: a pool with exactly "
          "that much free is short by the thirds the figure leaves off",
          "bybit inverse SOLUSD", "long", {"asset": "SOL", "amount": "25"}, "1", "100.00",
          {"status": "refused", "reason": "insufficient_margin", "required_in_pool": "0.334433333333",
           "free_margin": "0.334433333333"},
          stop="80.00", leverage="3", taker_fee="0.00055", available_margin="0.334433333333",
          settlement_wallet="25"),
    entry("linear-margin-at-3x",
          "A third of 1,007.189 USDT has no end: the margin is rounded half to even at 12 places",
          "bybit linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "sized", "qty": "4.3", "initial_margin": "335.729666666667", "fee_allowance": "1.1079079",
           "required_margin": "336.837574566667"},
          stop="200.00", leverage="3", taker_fee="0.00055", available_margin="15000", settlement_wallet="15000"),
    entry("margin-halfway-rounds-to-the-even-digit-below",
          "An eighth of 1,023.5851000437 USDT is 127.9481375054625, halfway at the 12th place: it rounds to the "
          "even digit, here the one below",
          "binance linear SOLUSDT", "long", USDT("14990"), "1", "234.23000001",
          {"status": "sized", "qty": "4.37", "notional": "1023.5851000437", "initial_margin": "127.948137505462"},
          stop="200.00", leverage="8", taker_fee="0.0005", available_margin="14990", settlement_wallet="14990"),
    entry("margin-halfway-rounds-to-the-even-digit-above",
          "An eighth of 1,023.5851001311 USDT is 127.9481375163875: the even digit is the one above",
          "binance linear SOLUSDT", "long", USDT("14990"), "1", "234.23000003",
          {"status": "sized", "qty": "4.37", "notional": "1023.5851001311", "initial_margin": "127.948137516388"},
          stop="200.00", leverage="8", taker_fee="0.0005", available_margin="14990", settlement_wallet="14990"),
    entry("min-qty-exactly-met",
          "One step, 0.001 BTC, is the minimum quantity: allowed",
          "binance linear BTCUSDT", "long", USDT("300"), "1", "84123.45",
          {"status": "sized", "stop": "81599.70", "qty": "0.001", "risk": "2.52375", "notional": "84.12345"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="300", settlement_wallet="300"),
    entry("market-max-exactly-met",
          "302,850 / 2,523.75 is 120 BTC exactly, the market-order maximum: allowed",
          "binance linear BTCUSDT", "short", USDT("30285000"), "1", "84123.45",
          {"status": "sized", "stop": "86647.20", "qty": "120.000", "risk": "302850", "notional": "10094814"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="30285000",
          settlement_wallet="30285000"),
    entry("min-notional-exactly-met",
          "0.02 SOL at 250.00 is 5 USDT, the minimum notional: allowed",
          "binance linear SOLUSDT", "long", USDT("60"), "1", "250.00",
          {"status": "sized", "stop": "225.0000", "qty": "0.02", "risk": "0.5", "notional": "5"},
          stop_percent="10", leverage="5", taker_fee="0.0005", available_margin="60", settlement_wallet="60"),
    entry("bracket-cap-exactly-met",
          "200 SOL at 250.00 is 50,000 USDT, all that Bybit's 100x tier holds: allowed",
          "bybit linear SOLUSDT", "long", USDT("20000"), "1", "250.00",
          {"status": "sized", "stop": "249.000", "qty": "200.0", "risk": "200", "notional": "50000",
           "bracket_cap": "50000"},
          stop="249.00", leverage="100", taker_fee="0.00055", available_margin="20000", settlement_wallet="20000"),
    entry("refuse-one-step-above-the-bracket",
          "One more step, 200.1 SOL, is 50,025 USDT: above the 100x tier",
          "bybit linear SOLUSDT", "long", USDT("20010"), "1", "250.00",
          {"status": "refused", "reason": "above_leverage_bracket", "qty": "200.1", "notional": "50025",
           "bracket_cap": "50000"},
          stop="249.00", leverage="100", taker_fee="0.00055", available_margin="20010", settlement_wallet="20010"),
    entry("refuse-bracket-short-by-less-than-a-reported-place",
          "135 contracts are 0.16047853510438... BTC, reported as 0.160478535104: a bracket capped at exactly "
          "that figure does not hold them",
          "binance inverse BTCUSD_PERP", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "refused", "reason": "above_leverage_bracket", "qty": "135",
           "notional_settle": "0.160478535104", "bracket_cap": "0.160478535104"},
          stop_percent="3", leverage="5", taker_fee="0.0005", available_margin="0.3", settlement_wallet="0.3",
          alter=[("brackets", [{"max_leverage": "125", "cap": money("0.160478535104", "BTC")}])]),
    entry("refuse-leverage-above-every-tier",
          "No tier allows 150x: there is no cap to fit under",
          "binance linear BTCUSDT", "long", USDT("10000"), "1", "84123.45",
          {"status": "refused", "reason": "above_leverage_bracket", "qty": "0.039"},
          stop_percent="3", leverage="150", taker_fee="0.0005", available_margin="10000", settlement_wallet="10000"),
    entry("margin-exactly-met",
          "190.95 USDT needed, and 290.95 available less a 100 USDT reservation leaves exactly that: allowed",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "qty": "4.00", "required_margin": "190.95", "free_margin": "190.95"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="290.95", reserved_margin="100",
          settlement_wallet="15000"),
    entry("refuse-margin-one-cent-short",
          "With 290.94 available the same entry is one cent short",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "refused", "reason": "insufficient_margin", "required_margin": "190.95", "free_margin": "190.94"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="290.94", reserved_margin="100",
          settlement_wallet="15000"),
    entry("refuse-reservations-above-the-pool",
          "Reservations of 700 USDT against 400 available leave 300 less than nothing, which is not 300 free",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "refused", "reason": "insufficient_margin", "free_margin": "-300"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="400", reserved_margin="700",
          settlement_wallet="15000"),
    entry("refuse-pool-in-deficit",
          "A pool the venue reports 500 USDT short has nothing free, however large the shortfall",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "refused", "reason": "insufficient_margin", "free_margin": "-500"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="-500", settlement_wallet="15000"),
    entry("zero-taker-fee",
          "A taker fee of nothing is a fee rate like any other: no allowance, and the margin alone must fit",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "qty": "4.00", "fee_allowance": "0", "required_margin": "190"},
          stop="200.00", leverage="5", taker_fee="0", available_margin="190", settlement_wallet="15000"),
    entry("index-not-needed-is-not-looked-at",
          "An entry funded by its asset's own pool needs no index price: one handed in anyway, old and another "
          "asset's, changes nothing",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "qty": "4.00", "pool": "USDT"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="15000",
          usd_index="84100.5",
          alter=[("usd_index.asset", "BTC"), ("usd_index.observed_at_ms", NOW_MS - 3600000)]),
    entry("wallet-exactly-the-risk",
          "A wallet of 150 USDT can pay a 150 USDT stop-out: allowed, with the warning that it is below the capital",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "qty": "4.00", "warnings": ["wallet_below_capital"]},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="150"),
    entry("refuse-wallet-one-cent-below-the-risk",
          "A wallet of 149.99 USDT cannot",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "refused", "reason": "settlement_wallet_below_risk"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="149.99"),
    entry("wallet-in-use-still-holds-the-risk",
          "What counts is what the wallet holds, not what of it is free: 15,000 USDT held with 100 free pays a "
          "150 USDT stop-out",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "sized", "qty": "4.00", "warnings": []},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="15000",
          wallet_available="100"),
    entry("refuse-wallet-in-debt",
          "A wallet 200 USDT in debt holds less than nothing, which is not 200",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "237.50",
          {"status": "refused", "reason": "settlement_wallet_below_risk"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="15000", settlement_wallet="-200"),
    entry("several-wallets-and-pools",
          "A COIN-M account with a pool and a wallet for each coin: the ETH ones fund an ETH-settled entry",
          "binance inverse ETHUSD_PERP", "short", {"asset": "ETH", "amount": "2"}, "0.5", "3012.34",
          {"status": "sized", "stop": "3105.56", "qty": "100", "pool": "ETH"},
          stop="3105.555", leverage="5", taker_fee="0.0005", available_margin="2", settlement_wallet="2",
          more_pools=[("BTC", "0.0001")], more_wallets=[("BTC", "0.0001")]),
    entry("refuse-inverse-below-one-contract",
          "0.00003 BTC of budget is less than one 100 USD contract risks: refused, never rounded up to one",
          "binance inverse BTCUSD_PERP", "long", {"asset": "BTC", "amount": "0.003"}, "1", "84123.4",
          {"status": "refused", "reason": "below_min_qty", "stop": "81599.6", "qty": "0"},
          stop_percent="3", leverage="5", taker_fee="0.0005", available_margin="0.003", settlement_wallet="0.003"),
    entry("inverse-small-budget-on-one-usd-contracts",
          "The same budget on Bybit's 1 USD contracts sizes 81 of them: the face value is the venue's",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.003"}, "1", "84123.4",
          {"status": "sized", "stop": "81599.60", "qty": "81"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="0.003", settlement_wallet="0.003"),
    entry("refuse-inverse-insufficient-margin",
          "A COIN-M SHORT whose margin, in ETH, is more than the ETH pool has free",
          "binance inverse ETHUSD_PERP", "short", {"asset": "ETH", "amount": "2"}, "0.5", "3012.34",
          {"status": "refused", "reason": "insufficient_margin", "qty": "100", "pool": "ETH", "free_margin": "0.05"},
          stop="3105.555", leverage="5", taker_fee="0.0005", available_margin="0.06", reserved_margin="0.01",
          settlement_wallet="2"),
    entry("usd-pool-linear-fits",
          "Bybit cross margin: 202.5457079 USDT of margin is 202.62672618316 USD at an index of 1.0004, within 202.63",
          "bybit linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "sized", "qty": "4.3", "required_margin": "202.5457079", "pool": "USD",
           "required_in_pool": "202.62672618316", "free_margin": "202.63"},
          stop="200.00", leverage="5", taker_fee="0.00055", available_margin="202.63", settlement_wallet="15000",
          pool="usd", usd_index="1.0004"),
    entry("refuse-usd-pool-short-by-the-index",
          "With 202.60 USD free the same entry is refused, though 202.5457079 would fit unconverted",
          "bybit linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "insufficient_margin", "required_in_pool": "202.62672618316"},
          stop="200.00", leverage="5", taker_fee="0.00055", available_margin="202.60", settlement_wallet="15000",
          pool="usd", usd_index="1.0004"),
    entry("usd-pool-below-par-fits",
          "Binance multi-assets mode: 331.36226955 USDT is 331.29599709609 USD at 0.9998, within the 331.30 left",
          "binance linear BTCUSDT", "short", USDT("10000"), "1", "84123.45",
          {"status": "sized", "qty": "0.039", "required_margin": "331.36226955", "pool": "USD",
           "required_in_pool": "331.29599709609", "free_margin": "331.3"},
          stop_percent="3", leverage="10", taker_fee="0.0005", available_margin="431.30", reserved_margin="100",
          settlement_wallet="10000", pool="usd", usd_index="0.9998"),
    entry("usd-pool-inverse-fits",
          "Bybit cross margin, coin-settled: the BTC the entry needs is measured in USD at the BTC index price",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "sized", "qty": "13599", "pool": "USD"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="3000", settlement_wallet="0.5",
          pool="usd", usd_index="84100.5"),
    entry("refuse-usd-pool-inverse-short",
          "The same entry with 2,700 USD free",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "refused", "reason": "insufficient_margin", "pool": "USD"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="2700", settlement_wallet="0.5",
          pool="usd", usd_index="84100.5"),
    entry("refuse-no-settlement-wallet",
          "Margin funded entirely by other collateral: the account holds no USDT at all to pay a stop-out with",
          "bybit linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "settlement_wallet_below_risk"},
          stop="200.00", leverage="5", taker_fee="0.00055", available_margin="15000", settlement_wallet=None,
          pool="usd", usd_index="1.0004", more_wallets=[("USDC", "20000")]),
    entry("drift-exactly-at-the-limit",
          "At an ask of 250.00 the risk is 25, exactly 1.25 x 20: not above it, so accepted",
          "binance linear SOLUSDT", "long", USDT("2000"), "1", "240.00",
          {"status": "sized", "qty": "0.50", "risk": "20", "risk_at_fill": "25"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="2000", settlement_wallet="2000",
          quote={"bid": "249.90", "ask": "250.00", "mark": "249.95"}),
    entry("refuse-drift-one-tick-over",
          "One tick higher, 250.01, risks 25.005",
          "binance linear SOLUSDT", "long", USDT("2000"), "1", "240.00",
          {"status": "refused", "reason": "risk_drift", "risk_at_fill": "25.005"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="2000", settlement_wallet="2000",
          quote={"bid": "249.91", "ask": "250.01", "mark": "249.96"}),
    entry("refuse-mark-exactly-at-the-stop",
          "A mark price at the stop has reached it",
          "binance linear SOLUSDT", "long", USDT("2000"), "1", "240.00",
          {"status": "refused", "reason": "stop_crossed"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="2000", settlement_wallet="2000",
          quote={"bid": "200.50", "ask": "200.60", "mark": "200.00"}),
    entry("refuse-bid-exactly-at-the-stop",
          "So has a bid at the stop",
          "binance linear SOLUSDT", "long", USDT("2000"), "1", "240.00",
          {"status": "refused", "reason": "stop_crossed"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="2000", settlement_wallet="2000",
          quote={"bid": "200.00", "ask": "200.10", "mark": "200.30"}),
    entry("one-tick-clear-of-the-stop",
          "A bid and a mark one tick above the stop have not",
          "binance linear SOLUSDT", "long", USDT("2000"), "1", "240.00",
          {"status": "sized", "qty": "0.50", "risk_at_fill": "0.01"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="2000", settlement_wallet="2000",
          quote={"bid": "200.01", "ask": "200.02", "mark": "200.01"}),
    entry("locked-book-is-a-book",
          "A bid equal to the ask is still a book: only a bid above the ask is not",
          "binance linear SOLUSDT", "long", USDT("1500"), "1", "234.23",
          {"status": "sized", "qty": "0.43", "risk_at_fill": "18.49"},
          stop="200.00", leverage="5", taker_fee="0.0005", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "243.00", "ask": "243.00", "mark": "242.95"}),
    entry("budget-with-more-places-than-reported",
          "A budget of 1,000.0000000000005 USDT is reported as 1,000, and every comparison is made on what it is",
          "binance linear BTCUSDT", "long", USDT("1000.0000000000005"), "100", "85000.0",
          {"status": "sized", "budget": "1000", "qty": "1.000", "risk": "1000", "required_margin": "85085"},
          stop="84000.0", leverage="1", taker_fee="0.0005", available_margin="1000000",
          settlement_wallet="1000000"),
    entry("quantity-from-the-budget-before-rounding",
          "A budget of 999.9999999999995 USDT is reported as 1,000 too, and sizes 0.999 BTC at a loss of 1,000 "
          "a unit: the 1,000 reported would size a step more than the budget pays for",
          "binance linear BTCUSDT", "long", USDT("999.9999999999995"), "100", "85000.0",
          {"status": "sized", "budget": "1000", "qty": "0.999", "risk": "999"},
          stop="84000.0", leverage="1", taker_fee="0.0005", available_margin="1000000",
          settlement_wallet="1000000"),
    entry("drift-limit-from-the-budget-before-rounding",
          "From an ask of 85,250.000000000000625 the risk is exactly 1.25 times that budget, and not above it; "
          "it would be above 1.25 times the 1,000 reported",
          "binance linear BTCUSDT", "long", USDT("1000.0000000000005"), "100", "85000.0",
          {"status": "sized", "qty": "1.000", "risk_at_fill": "1250.000000000001"},
          stop="84000.0", leverage="1", taker_fee="0.0005", available_margin="1000000",
          settlement_wallet="1000000",
          quote={"bid": "85249.90", "ask": "85250.000000000000625", "mark": "85249.95"}),
    entry("refuse-wallet-below-the-budget-before-rounding",
          "A wallet of 1,000 USDT is below that budget, though not below the 1,000 reported",
          "binance linear BTCUSDT", "long", USDT("1000.0000000000005"), "100", "85000.0",
          {"status": "refused", "reason": "settlement_wallet_below_risk", "budget": "1000"},
          stop="84000.0", leverage="1", taker_fee="0.0005", available_margin="1000000",
          settlement_wallet="1000"),
    entry("short-submit-accepted",
          "A SHORT fills at the bid: 2.5 SOL from 233.90 to the 240.00 stop risk 15.25, within 18.75",
          "bybit linear SOLUSDT", "short", USDT("1500"), "1", "234.23",
          {"status": "sized", "qty": "2.5", "risk_at_fill": "15.25"},
          stop="240.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "233.90", "ask": "233.95", "mark": "233.92"}),
    entry("refuse-ask-exactly-at-the-stop",
          "A SHORT whose stop the ask has just reached",
          "bybit linear SOLUSDT", "short", USDT("1500"), "1", "234.23",
          {"status": "refused", "reason": "stop_crossed"},
          stop="240.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "239.98", "ask": "240.00", "mark": "239.90"}),
    entry("refuse-short-mark-exactly-at-the-stop",
          "A SHORT whose stop the mark price has just reached",
          "bybit linear SOLUSDT", "short", USDT("1500"), "1", "234.23",
          {"status": "refused", "reason": "stop_crossed"},
          stop="240.00", leverage="5", taker_fee="0.00055", available_margin="1500", settlement_wallet="1500",
          quote={"bid": "239.80", "ask": "239.90", "mark": "240.00"}),
    entry("inverse-submit-accepted",
          "Inverse risk at the fill is in the coin: 13,599 contracts from an ask of 84,150.5",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "sized", "qty": "13599"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="0.5", settlement_wallet="0.5",
          quote={"bid": "84150.0", "ask": "84150.5", "mark": "84149.8"}),
    entry("refuse-inverse-drift",
          "From an ask of 84,800.0 the same contracts risk more than 1.25 x 0.005 BTC",
          "bybit inverse BTCUSD", "long", {"asset": "BTC", "amount": "0.5"}, "1", "84123.4",
          {"status": "refused", "reason": "risk_drift"},
          stop_percent="3", leverage="5", taker_fee="0.00055", available_margin="0.5", settlement_wallet="0.5",
          quote={"bid": "84799.5", "ask": "84800.0", "mark": "84799.9"}),
    entry("inverse-short-submit-accepted",
          "An inverse SHORT fills at the bid",
          "bybit inverse SOLUSD", "short", {"asset": "SOL", "amount": "20"}, "1", "187.45",
          {"status": "sized", "qty": "1911"},
          stop_percent="2", leverage="3", taker_fee="0.00055", available_margin="20", settlement_wallet="20",
          quote={"bid": "187.30", "ask": "187.32", "mark": "187.31"}),
    entry("refuse-inverse-short-mark-at-the-stop",
          "An inverse SHORT whose stop the mark price has reached",
          "bybit inverse SOLUSD", "short", {"asset": "SOL", "amount": "20"}, "1", "187.45",
          {"status": "refused", "reason": "stop_crossed"},
          stop_percent="2", leverage="3", taker_fee="0.00055", available_margin="20", settlement_wallet="20",
          quote={"bid": "190.90", "ask": "191.00", "mark": "191.20"}),
]

# Each refusal with every later one waiting behind it: the order they are
# given in. Every entry here is too large for its 150x leverage, has no
# margin free and no wallet to pay a stop-out; one thing is put right at a
# time.
BEHIND = dict(leverage="150", taker_fee="0.0005", available_margin="0", settlement_wallet=None)
SIZING_CASES += [
    entry("order-01-stop-wrong-side",
          "A stop on the wrong side is refused before anything is sized",
          "binance linear SOLUSDT", "long", USDT("10"), "1", "234.23",
          {"status": "refused", "reason": "stop_wrong_side"},
          stop="240.00", **BEHIND),
    entry("order-02-no-stop",
          "No stop at all, likewise",
          "binance linear SOLUSDT", "long", USDT("10"), "1", "234.23",
          {"status": "refused", "reason": "no_stop"},
          **BEHIND),
    entry("order-03-below-min-qty",
          "With a 10 % stop, a budget of 0.1 USDT sizes nothing",
          "binance linear SOLUSDT", "long", USDT("10"), "1", "234.23",
          {"status": "refused", "reason": "below_min_qty", "stop": "210.8000", "qty": "0.00"},
          stop_percent="10", **BEHIND),
    entry("order-04-above-market-max",
          "A budget of 10,000,000 USDT sizes 426,803.24 SOL, above the market-order maximum",
          "binance linear SOLUSDT", "long", USDT("1000000000"), "1", "234.23",
          {"status": "refused", "reason": "above_market_max", "qty": "426803.24"},
          stop_percent="10", **BEHIND),
    entry("order-05-below-min-notional",
          "A budget of 0.5 USDT sizes 0.02 SOL, worth less than the minimum notional",
          "binance linear SOLUSDT", "long", USDT("50"), "1", "234.23",
          {"status": "refused", "reason": "below_min_notional", "qty": "0.02", "notional": "4.6846"},
          stop_percent="10", **BEHIND),
    entry("order-06-above-leverage-bracket",
          "A budget of 150 USDT sizes 6.40 SOL, which no bracket allows at 150x",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "above_leverage_bracket", "qty": "6.40"},
          stop_percent="10", **BEHIND),
    entry("order-07-insufficient-margin",
          "At 5x the bracket holds it, and the pool has nothing free",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "insufficient_margin", "qty": "6.40"},
          stop_percent="10", **dict(BEHIND, leverage="5")),
    entry("order-08-settlement-wallet-below-risk",
          "With margin free, the account still holds no USDT to pay a stop-out",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "settlement_wallet_below_risk", "qty": "6.40"},
          stop_percent="10", **dict(BEHIND, leverage="5", available_margin="15000")),
    entry("order-09-stop-crossed",
          "Sized; at submission the mark price is below the stop, and the ask far above the entry",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "stop_crossed", "qty": "6.40"},
          stop_percent="10", leverage="5", taker_fee="0.0005", available_margin="15000",
          settlement_wallet="15000", quote={"bid": "399.90", "ask": "400.00", "mark": "210.00"}),
    entry("order-10-risk-drift",
          "With the mark price back above the stop, the ask is still far above the entry",
          "binance linear SOLUSDT", "long", USDT("15000"), "1", "234.23",
          {"status": "refused", "reason": "risk_drift", "qty": "6.40", "risk_at_fill": "1210.88"},
          stop_percent="10", leverage="5", taker_fee="0.0005", available_margin="15000",
          settlement_wallet="15000", quote={"bid": "399.90", "ask": "400.00", "mark": "399.95"}),
]


def trust(case_id, note, base, expect, alter=(), **changes):
    """A case made from `base` by changing one thing sizing is given."""
    case = copy.deepcopy(next(case for case in SIZING_CASES if case["id"] == base))
    case.update(id=case_id, note=note, expect=expect, alter=list(alter), **changes)
    return case


def fault(kind, about):
    return {"status": "fault", "fault": {"kind": kind, "input": about}}


SAME = {"status": "sized", "qty": "4.38"}
PRD = "prd-p9-binance-linear-long"
USD = "usd-pool-linear-fits"
QUOTED = "submit-drift-accepted"
RULES_LIMIT, ACCOUNT_LIMIT, PRICES_LIMIT = (FRESHNESS_MS[key] for key in ("rules", "account", "prices"))

# What sizing cannot trust, each a fault and never a refusal, and how old an
# input may be: at its limit it is still used, a millisecond past it is not.
TRUST_CASES = [
    trust("fault-rules-of-another-instrument",
          "The rules handed in are BTCUSDT's, and the entry was admitted on SOLUSDT",
          PRD, fault("mismatched", "rules"), [("instrument", "binance linear BTCUSDT")]),
    trust("fault-account-of-another-key",
          "The account snapshot is another account's than the profile's",
          PRD, fault("mismatched", "account"), [("account.account.venue_account_id", {"Venue": "9009999"})]),
    trust("fault-rules-too-old",
          "Instrument rules read an hour and a millisecond ago",
          PRD, fault("stale", "rules"), [("rules_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    trust("fresh-rules-at-the-limit",
          "Instrument rules read exactly an hour ago are still used",
          PRD, SAME, [("rules_at_ms", NOW_MS - RULES_LIMIT)]),
    trust("fault-no-brackets",
          "The account's leverage brackets for the instrument are empty",
          PRD, fault("missing", "brackets"), [("brackets", [])]),
    trust("fault-brackets-in-another-asset",
          "A bracket's cap is in USD, and the instrument settles in USDT",
          PRD, fault("invalid", "brackets"),
          [("brackets", [{"max_leverage": "20", "cap": money("1000000", "USD")}])]),
    trust("fault-bracket-without-a-cap",
          "A bracket whose cap is zero holds nothing",
          PRD, fault("invalid", "brackets"),
          [("brackets", [{"max_leverage": "20", "cap": money("0", "USDT")}])]),
    trust("fault-brackets-too-old",
          "Leverage brackets read an hour and a millisecond ago",
          PRD, fault("stale", "brackets"), [("brackets_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    trust("fresh-brackets-at-the-limit",
          "Leverage brackets read exactly an hour ago are still used",
          PRD, SAME, [("brackets_at_ms", NOW_MS - RULES_LIMIT)]),
    trust("fault-taker-fee-below-zero",
          "A taker fee below zero would make the fee allowance a credit",
          PRD, fault("invalid", "fees"), [("fees.taker", "-0.0001")]),
    trust("fault-fees-too-old",
          "Fee rates read an hour and a millisecond ago",
          PRD, fault("stale", "fees"), [("fees_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    trust("fresh-fees-at-the-limit",
          "Fee rates read exactly an hour ago are still used",
          PRD, SAME, [("fees_at_ms", NOW_MS - RULES_LIMIT)]),
    trust("fault-account-too-old",
          "An account snapshot ten seconds and a millisecond old",
          PRD, fault("stale", "account"), [("account.observed_at_ms", NOW_MS - ACCOUNT_LIMIT - 1)]),
    trust("fresh-account-at-the-limit",
          "An account snapshot exactly ten seconds old is still used",
          PRD, SAME, [("account.observed_at_ms", NOW_MS - ACCOUNT_LIMIT)]),
    trust("fault-account-ahead-of-the-clock",
          "An account snapshot timed more than ten seconds after the clock: the clocks disagree",
          PRD, fault("stale", "account"), [("account.observed_at_ms", NOW_MS + ACCOUNT_LIMIT + 1)]),
    trust("fresh-account-ahead-at-the-limit",
          "One timed exactly ten seconds after the clock is still used",
          PRD, SAME, [("account.observed_at_ms", NOW_MS + ACCOUNT_LIMIT)]),
    trust("fault-no-pool",
          "The account reports no margin pool the entry could draw on",
          PRD, fault("missing", "pool"),
          [("account.pools", [{"scope": {"Asset": "BNB"}, "available": money("12", "BNB")}])]),
    trust("fault-two-pools",
          "The account reports both a USDT pool and a USD pool: which one funds the entry is not sizing's to guess",
          PRD, fault("ambiguous", "pool"),
          [("account.pools", [{"scope": {"Asset": "USDT"}, "available": money("15000", "USDT")},
                              {"scope": "AccountUsd", "available": money("15000", "USD")}])]),
    trust("fault-pool-in-another-unit",
          "A USD pool whose available margin is stated in USDT",
          PRD, fault("invalid", "pool"),
          [("account.pools", [{"scope": "AccountUsd", "available": money("15000", "USDT")}])]),
    trust("fault-reservation-below-zero",
          "A reservation below zero would add margin",
          PRD, fault("invalid", "reservation"), [("reserved.amount", "-1")]),
    trust("fault-reservation-in-another-unit",
          "Reservations stated in USD against a USDT pool",
          PRD, fault("invalid", "reservation"), [("reserved", money("100", "USD"))]),
    trust("fault-two-settlement-wallets",
          "Two USDT wallets in one snapshot",
          PRD, fault("ambiguous", "wallet"),
          [("account.wallets", [wallet("15000", "USDT"), wallet("100", "USDT")])]),
    trust("fault-usd-pool-without-an-index",
          "A USD pool, and no index price to measure USDT against it",
          USD, fault("missing", "usd_index"), [("usd_index", None)]),
    trust("fault-index-of-another-asset",
          "The index price handed in is BTC's, and the entry settles in USDT",
          USD, fault("mismatched", "usd_index"), [("usd_index.asset", "BTC")]),
    trust("fault-index-without-a-source",
          "An index price that does not say where it came from",
          USD, fault("invalid", "usd_index"), [("usd_index.source", " ")]),
    trust("fault-index-too-old",
          "An index price five seconds and a millisecond old",
          USD, fault("stale", "usd_index"), [("usd_index.observed_at_ms", NOW_MS - PRICES_LIMIT - 1)]),
    trust("fresh-index-at-the-limit",
          "An index price exactly five seconds old is still used",
          USD, {"status": "sized", "qty": "4.3"}, [("usd_index.observed_at_ms", NOW_MS - PRICES_LIMIT)]),
    trust("fault-prices-of-another-instrument",
          "The prices handed in at submission are BTCUSDT's",
          QUOTED, fault("mismatched", "prices"), [("prices.instrument.symbol", "BTCUSDT")]),
    trust("fault-no-bid",
          "A price snapshot without a bid",
          QUOTED, fault("missing", "prices"), [("prices.bid", None)]),
    trust("fault-no-quote-time",
          "A book that does not say when it was read",
          QUOTED, fault("missing", "prices"), [("prices.quoted_at_ms", None)]),
    trust("fault-book-crossed",
          "A bid above the ask is not a book",
          QUOTED, fault("invalid", "prices"), [("prices.bid", "243.10")]),
    trust("fault-mark-too-old",
          "A mark price five seconds and a millisecond old",
          QUOTED, fault("stale", "prices"), [("prices.marked_at_ms", NOW_MS - PRICES_LIMIT - 1)]),
    trust("fresh-mark-at-the-limit",
          "A mark price exactly five seconds old is still used",
          QUOTED, {"status": "sized", "risk_at_fill": "18.49"}, [("prices.marked_at_ms", NOW_MS - PRICES_LIMIT)]),
    trust("fault-quote-too-old",
          "A book five seconds and a millisecond old",
          QUOTED, fault("stale", "prices"), [("prices.quoted_at_ms", NOW_MS - PRICES_LIMIT - 1)]),
    trust("fresh-quote-at-the-limit",
          "A book exactly five seconds old is still used",
          QUOTED, {"status": "sized", "risk_at_fill": "18.49"}, [("prices.quoted_at_ms", NOW_MS - PRICES_LIMIT)]),
    trust("fault-mark-ahead-of-the-clock",
          "A mark price timed more than five seconds after the clock",
          QUOTED, fault("stale", "prices"), [("prices.marked_at_ms", NOW_MS + PRICES_LIMIT + 1)]),
]



def chain(prefix, base, damages):
    """One case for each damage, with every later damage done to it as well:
    the first thing wrong is the one reported, so the cases pin the order
    the inputs are looked at in."""
    cases = []
    for at, (name, expect, note, _) in enumerate(damages):
        # The later ones first, so that of two changes to one field the
        # earlier damage is the one left standing.
        changes = [change for _, _, _, later in reversed(damages[at:]) for change in later]
        cases.append(trust("{}-{:02}-{}".format(prefix, at + 1, name),
                           "{}, and everything looked at after it wrong as well".format(note),
                           base, fault(*expect), changes))
    return cases


USD_SHORT = "usd-pool-below-par-fits"
TRUST_CASES += chain("chain", USD_SHORT, [
    ("rules-of-another-instrument", ("mismatched", "rules"), "The rules are another instrument's",
     [("instrument", "binance linear SOLUSDT")]),
    ("account-of-another-key", ("mismatched", "account"), "The account snapshot is another account's",
     [("account.account.venue_account_id", {"Venue": "9009999"})]),
    ("rules-too-old", ("stale", "rules"), "The rules are too old",
     [("rules_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    ("no-brackets", ("missing", "brackets"), "There are no leverage brackets",
     [("brackets", [])]),
    ("brackets-in-another-asset", ("invalid", "brackets"), "A bracket's cap is in another asset",
     [("brackets", [{"max_leverage": "20", "cap": money("1000000", "USD")}])]),
    ("brackets-too-old", ("stale", "brackets"), "The brackets are too old",
     [("brackets_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    ("taker-fee-below-zero", ("invalid", "fees"), "The taker fee is below zero",
     [("fees.taker", "-0.0001")]),
    ("fees-too-old", ("stale", "fees"), "The fee rates are too old",
     [("fees_at_ms", NOW_MS - RULES_LIMIT - 1)]),
    ("account-too-old", ("stale", "account"), "The account snapshot is too old",
     [("account.observed_at_ms", NOW_MS - ACCOUNT_LIMIT - 1)]),
    ("no-pool", ("missing", "pool"), "The account has no pool for the entry",
     [("account.pools", [{"scope": {"Asset": "BNB"}, "available": money("12", "BNB")}])]),
    ("two-pools", ("ambiguous", "pool"), "The account has two pools for the entry",
     [("account.pools", [{"scope": {"Asset": "USDT"}, "available": money("15000", "USDT")},
                         {"scope": "AccountUsd", "available": money("15000", "USD")}])]),
    ("pool-in-another-unit", ("invalid", "pool"), "The USD pool is stated in USDT",
     [("account.pools", [{"scope": "AccountUsd", "available": money("15000", "USDT")}])]),
    ("reservation-in-another-unit", ("invalid", "reservation"), "The reservations are stated in USDT, not USD",
     [("reserved", money("100", "USDT"))]),
    ("no-index", ("missing", "usd_index"), "There is no index price for the USD pool",
     [("usd_index", None)]),
    ("index-of-another-asset", ("mismatched", "usd_index"), "The index price is another asset's",
     [("usd_index.asset", "BTC")]),
    ("index-without-a-source", ("invalid", "usd_index"), "The index price names no source",
     [("usd_index.source", "")]),
    ("index-too-old", ("stale", "usd_index"), "The index price is too old",
     [("usd_index.observed_at_ms", NOW_MS - PRICES_LIMIT - 1)]),
    ("two-settlement-wallets", ("ambiguous", "wallet"), "The account has two USDT wallets",
     [("account.wallets", [wallet("10000", "USDT"), wallet("100", "USDT")])]),
])
TRUST_CASES += chain("chain-submit", QUOTED, [
    ("prices-of-another-instrument", ("mismatched", "prices"), "The prices are another instrument's",
     [("prices.instrument.symbol", "BTCUSDT")]),
    ("no-bid", ("missing", "prices"), "The book has no bid",
     [("prices.bid", None)]),
    ("book-crossed", ("invalid", "prices"), "The bid is above the ask",
     [("prices.bid", "243.10")]),
    ("mark-too-old", ("stale", "prices"), "The mark price is too old",
     [("prices.marked_at_ms", NOW_MS - PRICES_LIMIT - 1)]),
])

# After the fills: the risk taken, beside what the filled quantity would have
# risked at the alert's expected price.
FILL_CASES = [
    {"id": "fill-at-the-expected-price", "note": "Filled whole at E: the risk is the one sized",
     "base": PRD, "fills": [("234.23", "4.38")],
     "expect": {"filled": "4.38", "expected": "149.9274", "actual": "149.9274", "deviation": "0",
                "exceeds_budget": False, "past_stop": False}},
    {"id": "fill-exactly-the-budget", "note": "A fill that risks the whole budget and no more does not exceed it",
     "base": "linear-exact-budget", "fills": [("237.50", "4.00")],
     "expect": {"expected": "150", "actual": "150", "deviation": "0", "exceeds_budget": False}},
    {"id": "fill-worse-than-expected", "note": "Filled 0.27 higher: 151.11 USDT at risk, above the 150 USDT budget",
     "base": PRD, "fills": [("234.50", "4.38")],
     "expect": {"actual": "151.11", "deviation": "1.1826", "exceeds_budget": True}},
    {"id": "fill-better-than-expected", "note": "Filled 0.23 lower: less at risk than sized",
     "base": PRD, "fills": [("234.00", "4.38")],
     "expect": {"actual": "148.92", "deviation": "-1.0074", "exceeds_budget": False}},
    {"id": "partial-fill-at-two-prices", "note": "3.38 of 4.38 SOL filled, at two prices: each fill counts at its own",
     "base": PRD, "fills": [("234.30", "2.00"), ("234.60", "1.38")],
     "expect": {"filled": "3.38", "expected": "115.6974", "actual": "116.348", "deviation": "0.6506",
                "exceeds_budget": False}},
    {"id": "fill-past-the-stop", "note": "A fill at or below a LONG's stop has nothing left to lose to it, and says so",
     "base": PRD, "fills": [("234.23", "2.38"), ("199.50", "2.00")],
     "expect": {"filled": "4.38", "actual": "81.4674", "past_stop": True}},
    {"id": "fill-exactly-at-the-stop", "note": "A fill at the stop's own price has reached it: nothing left to lose, and past it",
     "base": PRD, "fills": [("234.23", "2.38"), ("200.00", "2.00")],
     "expect": {"filled": "4.38", "actual": "81.4674", "deviation": "-68.46", "past_stop": True}},
    {"id": "short-fill-exactly-at-the-stop", "note": "The same for a SHORT, whose stop is above",
     "base": "bybit-linear-short", "fills": [("234.20", "10.0"), ("243.60", "6.0")],
     "expect": {"filled": "16.0", "actual": "94", "deviation": "-55.92", "exceeds_budget": False, "past_stop": True}},
    {"id": "fill-exactly-the-budget-before-rounding",
     "note": "A fill that risks 1,000.0000000000005 USDT of a budget of that much does not exceed it, though "
             "both are reported as 1,000",
     "base": "budget-with-more-places-than-reported", "fills": [("85000.0000000000005", "1.000")],
     "expect": {"actual": "1000", "budget": "1000", "deviation": "0", "exceeds_budget": False}},
    {"id": "fills-written-with-more-places-than-their-sum-can-keep",
     "note": "Two fills of 5 written with 28 decimal places add up to 10, which no exact decimal holds at 28 "
             "places: it is reported as 10, never as out of range",
     "base": "bybit-linear-short", "fills": [("234.23", "5." + "0" * 28), ("234.23", "5." + "0" * 28)],
     "expect": {"filled": "10", "expected": "93.7", "actual": "93.7", "deviation": "0", "exceeds_budget": False}},
    {"id": "short-fill-lower-than-expected", "note": "A SHORT filled below E has further to its stop",
     "base": "linear-short-percent-stop", "fills": [("84100.00", "0.039")],
     "expect": {"expected": "98.42625", "actual": "99.3408", "deviation": "0.91455", "exceeds_budget": False}},
    {"id": "bybit-short-fill", "note": "The Bybit SHORT, filled at two prices",
     "base": "bybit-linear-short", "fills": [("234.20", "10.0"), ("234.10", "6.0")],
     "expect": {"filled": "16.0", "expected": "149.92", "actual": "151", "deviation": "1.08",
                "exceeds_budget": True}},
    {"id": "inverse-long-two-fills", "note": "Inverse fills are summed in the coin, each at its own price",
     "base": "bybit-inverse-long", "fills": [("84150.0", "6000"), ("84200.5", "7599")],
     "expect": {"filled": "13599", "exceeds_budget": True}},
    {"id": "binance-inverse-long-fill", "note": "100 USD contracts, filled a little below E",
     "base": "binance-inverse-long", "fills": [("84100.0", "135")],
     "expect": {"filled": "135", "exceeds_budget": False}},
    {"id": "binance-inverse-short-fill", "note": "10 USD contracts sold above E: a SHORT filled better",
     "base": "binance-inverse-short", "fills": [("3013.00", "100")],
     "expect": {"filled": "100", "exceeds_budget": False}},
    {"id": "inverse-short-fill", "note": "1 USD contracts sold below E: further to the stop",
     "base": "bybit-inverse-short", "fills": [("187.00", "1911")],
     "expect": {"filled": "1911", "exceeds_budget": True}},
    {"id": "fault-no-fills", "note": "No fill, no risk to report",
     "base": PRD, "fills": [], "expect": fault("missing", "fills")},
    {"id": "fault-nothing-filled", "note": "Fills that add up to nothing",
     "base": PRD, "fills": [("234.23", "0.00")], "expect": fault("missing", "fills")},
    {"id": "fault-filled-more-than-sized", "note": "More filled than the entry was sized for: not this entry's fills",
     "base": PRD, "fills": [("234.23", "4.39")], "expect": fault("invalid", "fills")},
    {"id": "fault-fill-in-another-unit", "note": "A fill counted in contracts on a linear entry",
     "base": PRD, "fills": [("234.23", "4")], "unit": "Contracts", "expect": fault("invalid", "fills")},
]

CYCLE_CASES = [
    {"id": "prd-p10-exit", "note": "The PRD's p.10 indicator exit: its 69.07 and +6.73 % are gross, before fees",
     "family": "linear", "side": "long", "qty": "4.38", "entry": "234.23", "exit": "250.00",
     "fees": "0", "funding": "0", "expect": {"gross": "69.0726", "net": "69.0726", "return_percent": "6.73"}},
    {"id": "prd-p10-stop", "note": "The PRD's p.10 stop-loss exit: 4.38 SOL lost 149.93, the risk of a 150 USDT budget",
     "family": "linear", "side": "long", "qty": "4.38", "entry": "234.23", "exit": "200.00",
     "fees": "0", "funding": "0", "expect": {"gross": "-149.9274", "net": "-149.9274", "return_percent": "-14.61"}},
    {"id": "linear-net", "note": "Net = gross - fees + funding received (funding paid is negative)",
     "family": "linear", "side": "short", "qty": "0.039", "entry": "84123.45", "exit": "82000.00",
     "fees": "3.23940728", "funding": "-0.42", "expect": {"gross": "82.81455", "net": "79.15514272"}},
    {"id": "inverse-net", "note": "Inverse PnL is in the coin: N x F x (1/E - 1/X) for LONG",
     "family": "inverse", "side": "long", "qty": "13599", "face": "1", "entry": "84123.4", "exit": "86000.0",
     "fees": "0.00017588", "funding": "0.000005", "expect": {}},
]

# ---------------------------------------------------------------------------
# What sizing is given, read back from the stored form.
# ---------------------------------------------------------------------------


def listing(inputs):
    return INSTRUMENTS[inputs["instrument"]]


def brackets_of(inputs):
    given = inputs["brackets"]
    return TIERS[given] if isinstance(given, str) else given


def quantity_value(quantity):
    (value,) = quantity.values()
    return value


def fresh(at_ms, now_ms, limit_ms):
    """Within the limit of the clock, on either side: an input timed further
    ahead than its limit says the clocks disagree, and is trusted no more
    than an old one."""
    return abs(now_ms - at_ms) <= limit_ms


def entry_pools(account, settle):
    """The pools an entry settled in `settle` could draw on: the asset's own,
    and the account's USD pool."""
    return [pool for pool in account["pools"]
            if pool["scope"] == "AccountUsd" or pool["scope"] == {"Asset": settle}]


def distrust(inputs):
    """The first thing sizing cannot trust of what it is given, as (kind,
    input), or None. The order is part of the contract."""
    snapshot = inputs["snapshot"]
    settle = snapshot["instrument"]["settle"]
    now = inputs["now_ms"]
    if listing(inputs)["id"] != snapshot["instrument"]:
        return ("mismatched", "rules")
    account = inputs["account"]
    if account["account"] != snapshot["account"]:
        return ("mismatched", "account")
    if not fresh(inputs["rules_at_ms"], now, FRESHNESS_MS["rules"]):
        return ("stale", "rules")
    brackets = brackets_of(inputs)
    if not brackets:
        return ("missing", "brackets")
    if any(tier["cap"]["asset"] != settle or D(tier["cap"]["amount"]) <= 0 for tier in brackets):
        return ("invalid", "brackets")
    if not fresh(inputs["brackets_at_ms"], now, FRESHNESS_MS["rules"]):
        return ("stale", "brackets")
    if D(inputs["fees"]["taker"]) < 0:
        return ("invalid", "fees")
    if not fresh(inputs["fees_at_ms"], now, FRESHNESS_MS["rules"]):
        return ("stale", "fees")
    if not fresh(account["observed_at_ms"], now, FRESHNESS_MS["account"]):
        return ("stale", "account")
    pools = entry_pools(account, settle)
    if not pools:
        return ("missing", "pool")
    if len(pools) > 1:
        return ("ambiguous", "pool")
    usd = pools[0]["scope"] == "AccountUsd"
    unit = "USD" if usd else settle
    if pools[0]["available"]["asset"] != unit:
        return ("invalid", "pool")
    reserved = inputs["reserved"]
    if reserved["asset"] != unit or D(reserved["amount"]) < 0:
        return ("invalid", "reservation")
    if usd:
        index = inputs["usd_index"]
        if index is None:
            return ("missing", "usd_index")
        if index["asset"] != settle:
            return ("mismatched", "usd_index")
        if not index["source"].strip():
            return ("invalid", "usd_index")
        if not fresh(index["observed_at_ms"], now, FRESHNESS_MS["prices"]):
            return ("stale", "usd_index")
    if len([held for held in account["wallets"] if held["balance"]["asset"] == settle]) > 1:
        return ("ambiguous", "wallet")
    return None


def distrust_prices(inputs):
    """The same for the prices read just before submission."""
    prices = inputs["prices"]
    now = inputs["now_ms"]
    if prices["instrument"] != inputs["snapshot"]["instrument"]:
        return ("mismatched", "prices")
    if prices["bid"] is None or prices["ask"] is None or prices["quoted_at_ms"] is None:
        return ("missing", "prices")
    if D(prices["bid"]) > D(prices["ask"]):
        return ("invalid", "prices")
    if not fresh(prices["marked_at_ms"], now, FRESHNESS_MS["prices"]):
        return ("stale", "prices")
    if not fresh(prices["quoted_at_ms"], now, FRESHNESS_MS["prices"]):
        return ("stale", "prices")
    return None


def faulted(out, found):
    return dict(out, status="fault", fault={"kind": found[0], "input": found[1]})


def refused(out, reason):
    return dict(out, status="refused", reason=reason)


# ---------------------------------------------------------------------------
# The first implementation: Decimal at 60 digits.
# ---------------------------------------------------------------------------


def text(value):
    """Plain notation, never an exponent; keeps the value's own scale, as a
    stop on the tick or a quantity on the step should."""
    return format(value, "f")


def shown(value):
    """A derived amount: exact where it has at most 12 decimal places, else
    rounded half to even at 12. Its scale is an artifact of the arithmetic,
    so 13.6920 and 13.692 are the same risk, printed as the latter."""
    rounded = value.quantize(AMOUNT_QUANTUM, rounding=ROUND_HALF_EVEN)
    return format(rounded.normalize(), "f") if rounded else "0"


def to_grid(value, grid, rounding):
    return (value / grid).to_integral_value(rounding=rounding) * grid


def size(inputs):
    listed = listing(inputs)
    settle = listed["id"]["settle"]
    out = {"settle": settle, "warnings": []}
    snapshot = inputs["snapshot"]
    if snapshot is None:
        return refused(out, "no_capital_in_settlement_asset")
    found = distrust(inputs)
    if found:
        return faulted(out, found)
    long = inputs["side"] == "Long"
    inverse = listed["inverse"] is not None
    tick, step = D(listed["tick"]), D(quantity_value(listed["market_step"]))
    face = D(listed["inverse"]["face_value"]) if inverse else None
    e = D(inputs["expected_price"])

    capital = D(snapshot["capital"]["amount"])
    budget = capital * D(snapshot["risk_percent"]["value"]) / 100
    out["budget"] = shown(budget)

    if inputs["signal_stop"] is not None:
        s = D(inputs["signal_stop"])
        if (long and s >= e) or (not long and s <= e):
            return refused(out, "stop_wrong_side")
    elif snapshot["stop_percent"]["value"] is not None:
        p = D(snapshot["stop_percent"]["value"]) / 100
        s = e * (1 - p) if long else e * (1 + p)
    else:
        return refused(out, "no_stop")
    # Away from the entry: never tighter than the stop asked for.
    stop = to_grid(s, tick, ROUND_FLOOR if long else ROUND_CEILING).quantize(tick)
    if stop <= 0:
        return refused(out, "no_stop")
    out["stop"] = text(stop)

    if inverse:
        unit_loss = face * abs(1 / e - 1 / stop)
        # R / (F x |1/E - 1/S'|), with one division: exact whenever it ends.
        count = budget * e * stop / (face * abs(stop - e))
    else:
        unit_loss = abs(e - stop)
        count = budget / unit_loss
    qty = to_grid(count, step, ROUND_FLOOR).quantize(step)
    risk = qty * unit_loss
    out.update(qty=text(qty), unit_loss=shown(unit_loss), risk=shown(risk))
    # Nothing is below every minimum, whatever the venue lists as one.
    if qty == 0 or qty < D(quantity_value(listed["market_min_qty"])):
        return refused(out, "below_min_qty")
    if qty > D(quantity_value(listed["market_max_qty"])):
        return refused(out, "above_market_max")

    if inverse:
        notional = qty * face
        settled = notional / e
    else:
        notional = qty * e
        settled = notional
    out.update(notional=shown(notional), notional_settle=shown(settled))
    if listed["min_notional"] is not None and notional < D(listed["min_notional"]["amount"]):
        return refused(out, "below_min_notional")

    leverage = D(snapshot["leverage"]["value"])
    initial_margin = settled / leverage
    fee_allowance = settled * D(inputs["fees"]["taker"]) * 2
    required = initial_margin + fee_allowance
    out.update(initial_margin=shown(initial_margin), fee_allowance=shown(fee_allowance),
               required_margin=shown(required))
    # Leverage changes the margin, never the quantity: a position that its
    # leverage's bracket cannot hold is refused, never shrunk.
    caps = [D(tier["cap"]["amount"]) for tier in brackets_of(inputs) if D(tier["max_leverage"]) >= leverage]
    if caps:
        out["bracket_cap"] = shown(max(caps))
    if not caps or settled > max(caps):
        return refused(out, "above_leverage_bracket")

    (pool,) = entry_pools(inputs["account"], settle)
    in_pool = required
    if pool["scope"] == "AccountUsd":
        # A USD pool is measured at the venue's index price of the settlement asset.
        in_pool = required * D(inputs["usd_index"]["price"])
    free = D(pool["available"]["amount"]) - D(inputs["reserved"]["amount"])
    out.update(pool=pool["available"]["asset"], required_in_pool=shown(in_pool), free_margin=shown(free))
    if in_pool > free:
        return refused(out, "insufficient_margin")

    # A stop-out the settlement asset's wallet cannot pay would leave a debt
    # in that asset, whatever collateral funds the margin. An asset the
    # account lists no wallet for is one it holds none of.
    held = [D(wallet["balance"]["amount"]) for wallet in inputs["account"]["wallets"]
            if wallet["balance"]["asset"] == settle]
    balance = held[0] if held else D(0)
    if balance < budget:
        return refused(out, "settlement_wallet_below_risk")
    if balance < capital:
        out["warnings"].append("wallet_below_capital")

    if inputs["prices"] is not None:
        found = distrust_prices(inputs)
        if found:
            return faulted(out, found)
        bid, ask, mark = (D(inputs["prices"][key]) for key in ("bid", "ask", "mark"))
        # The stop triggers on the mark price; the touch is where a market
        # entry would fill. Either one at or past the stop refuses the entry.
        if (long and (bid <= stop or mark <= stop)) or (not long and (ask >= stop or mark >= stop)):
            return refused(out, "stop_crossed")
        fill = ask if long else bid
        if inverse:
            at_fill = qty * face * abs(1 / fill - 1 / stop)
        else:
            at_fill = qty * abs(fill - stop)
        out["risk_at_fill"] = shown(at_fill)
        if at_fill > DRIFT_LIMIT * budget:
            return refused(out, "risk_drift")
    return dict(out, status="sized")


def fill_risk(inputs, result, fills):
    """What the fills risk to the stop, beside what their quantity would have
    risked at E. A fill at or past the stop has nothing left to lose to it."""
    listed = listing(inputs)
    unit = "Contracts" if listed["inverse"] is not None else "Base"
    if not fills:
        return fault("missing", "fills")
    if any(set(fill["quantity"]) != {unit} for fill in fills):
        return fault("invalid", "fills")
    filled = sum(D(quantity_value(fill["quantity"])) for fill in fills)
    if filled == 0:
        return fault("missing", "fills")
    if filled > D(result["qty"]):
        return fault("invalid", "fills")
    long = inputs["side"] == "Long"
    e, stop = D(inputs["expected_price"]), D(result["stop"])
    face = D(listed["inverse"]["face_value"]) if unit == "Contracts" else None

    def loss(price):
        """One unit's loss from `price` to the stop; negative past the stop."""
        if face is None:
            return price - stop if long else stop - price
        return face * (1 / stop - 1 / price) if long else face * (1 / price - 1 / stop)

    actual, past = D(0), False
    for fill in fills:
        each = loss(D(fill["price"]))
        if each <= 0:
            past = True
        else:
            actual += D(quantity_value(fill["quantity"])) * each
    expected = filled * loss(e)
    budget = D(inputs["snapshot"]["capital"]["amount"]) * D(inputs["snapshot"]["risk_percent"]["value"]) / 100
    sign, digits, exponent = filled.as_tuple()
    assert sign == 0 and exponent <= 0, filled
    return {
        "status": "reported",
        # In the finest scale a fill came in, where the shared model holds it there.
        "filled": held_on_grid(int("".join(map(str, digits))), -exponent),
        "expected": shown(expected),
        "actual": shown(actual),
        "deviation": shown(actual - expected),
        "budget": shown(budget),
        "exceeds_budget": actual > budget,
        "past_stop": past,
    }


# ---------------------------------------------------------------------------
# The second implementation: fractions, sharing no arithmetic with the first.
# ---------------------------------------------------------------------------


def fr(value):
    """A decimal string as an exact fraction."""
    return Fraction(str(value))


def scaled(mantissa, scale):
    """An integer with its decimal point `scale` digits from the right."""
    digits = str(abs(mantissa)).rjust(scale + 1, "0")
    body = digits if scale == 0 else digits[:-scale] + "." + digits[-scale:]
    return ("-" if mantissa < 0 else "") + body


def scale_of(decimal_text):
    return len(decimal_text.partition(".")[2])


def on_grid(count, grid_text):
    """`count` steps of a grid, in the grid's own scale."""
    scale = scale_of(grid_text)
    return scaled(count * int(fr(grid_text) * 10 ** scale), scale)


def exactly(value, places=AMOUNT_PLACES):
    """A fraction as a derived amount: half to even at `places`, trailing
    zeros dropped."""
    mantissa, scale = round(value * 10 ** places), places
    while scale and mantissa % 10 == 0:
        mantissa, scale = mantissa // 10, scale - 1
    return scaled(mantissa, scale)


def size_exactly(inputs):
    listed = listing(inputs)
    settle = listed["id"]["settle"]
    out = {"settle": settle, "warnings": []}
    snapshot = inputs["snapshot"]
    if snapshot is None:
        return refused(out, "no_capital_in_settlement_asset")
    found = distrust(inputs)
    if found:
        return faulted(out, found)
    long = inputs["side"] == "Long"
    face = fr(listed["inverse"]["face_value"]) if listed["inverse"] is not None else None
    tick_text, step_text = listed["tick"], quantity_value(listed["market_step"])
    e = fr(inputs["expected_price"])
    capital = fr(snapshot["capital"]["amount"])
    budget = capital * fr(snapshot["risk_percent"]["value"]) / 100
    out["budget"] = exactly(budget)

    if inputs["signal_stop"] is not None:
        asked = fr(inputs["signal_stop"])
        if (asked >= e) if long else (asked <= e):
            return refused(out, "stop_wrong_side")
    elif snapshot["stop_percent"]["value"] is not None:
        share = fr(snapshot["stop_percent"]["value"]) / 100
        asked = e * (1 - share) if long else e * (1 + share)
    else:
        return refused(out, "no_stop")
    ticks = math.floor(asked / fr(tick_text)) if long else math.ceil(asked / fr(tick_text))
    if ticks <= 0:
        return refused(out, "no_stop")
    stop = ticks * fr(tick_text)
    out["stop"] = on_grid(ticks, tick_text)

    unit = abs(e - stop) if face is None else face * abs(1 / e - 1 / stop)
    steps = math.floor(budget / unit / fr(step_text))
    qty = steps * fr(step_text)
    out.update(qty=on_grid(steps, step_text), unit_loss=exactly(unit), risk=exactly(qty * unit))
    if steps == 0 or qty < fr(quantity_value(listed["market_min_qty"])):
        return refused(out, "below_min_qty")
    if qty > fr(quantity_value(listed["market_max_qty"])):
        return refused(out, "above_market_max")

    notional = qty * e if face is None else qty * face
    settled = notional if face is None else notional / e
    out.update(notional=exactly(notional), notional_settle=exactly(settled))
    if listed["min_notional"] is not None and notional < fr(listed["min_notional"]["amount"]):
        return refused(out, "below_min_notional")

    leverage = fr(snapshot["leverage"]["value"])
    margin = settled / leverage
    fees = settled * fr(inputs["fees"]["taker"]) * 2
    out.update(initial_margin=exactly(margin), fee_allowance=exactly(fees), required_margin=exactly(margin + fees))
    caps = [fr(tier["cap"]["amount"]) for tier in brackets_of(inputs) if fr(tier["max_leverage"]) >= leverage]
    if caps:
        out["bracket_cap"] = exactly(max(caps))
    if not caps or settled > max(caps):
        return refused(out, "above_leverage_bracket")

    (pool,) = entry_pools(inputs["account"], settle)
    needed = margin + fees
    if pool["scope"] == "AccountUsd":
        needed *= fr(inputs["usd_index"]["price"])
    free = fr(pool["available"]["amount"]) - fr(inputs["reserved"]["amount"])
    out.update(pool=pool["available"]["asset"], required_in_pool=exactly(needed), free_margin=exactly(free))
    if needed > free:
        return refused(out, "insufficient_margin")

    balance = sum(fr(wallet["balance"]["amount"]) for wallet in inputs["account"]["wallets"]
                  if wallet["balance"]["asset"] == settle)
    if balance < budget:
        return refused(out, "settlement_wallet_below_risk")
    if balance < capital:
        out["warnings"].append("wallet_below_capital")

    if inputs["prices"] is not None:
        found = distrust_prices(inputs)
        if found:
            return faulted(out, found)
        bid, ask, mark = (fr(inputs["prices"][key]) for key in ("bid", "ask", "mark"))
        touch = bid if long else ask
        reached = (lambda price: price <= stop) if long else (lambda price: price >= stop)
        if reached(touch) or reached(mark):
            return refused(out, "stop_crossed")
        fill = ask if long else bid
        at_fill = qty * (abs(fill - stop) if face is None else face * abs(1 / fill - 1 / stop))
        out["risk_at_fill"] = exactly(at_fill)
        if at_fill > budget * Fraction(5, 4):
            return refused(out, "risk_drift")
    return dict(out, status="sized")


def fill_risk_exactly(inputs, result, fills):
    listed = listing(inputs)
    inverse = listed["inverse"] is not None
    unit = "Contracts" if inverse else "Base"
    if not fills:
        return fault("missing", "fills")
    if any(list(fill["quantity"]) != [unit] for fill in fills):
        return fault("invalid", "fills")
    amounts = [(fr(fill["price"]), fr(quantity_value(fill["quantity"]))) for fill in fills]
    filled = sum(quantity for _, quantity in amounts)
    if filled == 0:
        return fault("missing", "fills")
    if filled > fr(result["qty"]):
        return fault("invalid", "fills")
    long = inputs["side"] == "Long"
    e, stop = fr(inputs["expected_price"]), fr(result["stop"])
    face = fr(listed["inverse"]["face_value"]) if inverse else None
    sign = 1 if long else -1

    def loss(price):
        if not inverse:
            return sign * (price - stop)
        return sign * face * (1 / stop - 1 / price)

    actual = sum(quantity * loss(price) for price, quantity in amounts if loss(price) > 0)
    expected = filled * loss(e)
    budget = fr(inputs["snapshot"]["capital"]["amount"]) * fr(inputs["snapshot"]["risk_percent"]["value"]) / 100
    places = max(scale_of(quantity_value(fill["quantity"])) for fill in fills)
    return {
        "status": "reported",
        "filled": held_on_grid(int(filled * 10 ** places), places),
        "expected": exactly(expected),
        "actual": exactly(actual),
        "deviation": exactly(actual - expected),
        "budget": exactly(budget),
        "exceeds_budget": actual > budget,
        "past_stop": any(loss(price) <= 0 for price, _ in amounts),
    }


# ---------------------------------------------------------------------------
# The checks.
# ---------------------------------------------------------------------------


def expected_of(case_id, expect, result):
    for key, value in expect.items():
        assert result.get(key) == value, "{}: {} is {!r}, expected {!r}".format(case_id, key, result.get(key), value)


def check_sizing(case, inputs, result):
    """The invariants the rules promise, then the hand-computed values."""
    if result["status"] == "sized":
        listed = listing(inputs)
        long = inputs["side"] == "Long"
        inverse = listed["inverse"] is not None
        tick, step, e = D(listed["tick"]), D(quantity_value(listed["market_step"])), D(inputs["expected_price"])
        face = D(listed["inverse"]["face_value"]) if inverse else D(1)
        stop, qty = D(result["stop"]), D(result["qty"])
        # The budget itself, not the figure reported, which is rounded.
        snapshot = inputs["snapshot"]
        budget = D(snapshot["capital"]["amount"]) * D(snapshot["risk_percent"]["value"]) / 100
        assert stop % tick == 0, "stop off the tick"
        assert qty % step == 0, "quantity off the step"
        assert (stop < e) if long else (stop > e), "stop on the wrong side"
        # The PnL formula at the stop, written independently of size().
        if inverse:
            pnl_at_stop = qty * face * ((1 / e - 1 / stop) if long else (1 / stop - 1 / e))
            loss_per_unit = face * abs(1 / e - 1 / stop)
        else:
            pnl_at_stop = qty * (stop - e) if long else qty * (e - stop)
            loss_per_unit = abs(e - stop)
        assert pnl_at_stop < 0, "a stop-out must lose"
        assert -pnl_at_stop <= budget, "risk above the budget"
        assert (qty + step) * loss_per_unit > budget, "the floor is not tight"
        # The position fits the bracket of its leverage.
        notional = qty * face / e if inverse else qty * e
        leverage = D(inputs["snapshot"]["leverage"]["value"])
        allowed = [D(tier["cap"]["amount"]) for tier in brackets_of(inputs) if D(tier["max_leverage"]) >= leverage]
        assert allowed and notional <= max(allowed), "position above its leverage bracket"
        # Its margin fits what the pool has free, and its wallet can pay a stop-out.
        assert D(result["required_in_pool"]) <= D(result["free_margin"]) + AMOUNT_QUANTUM, "margin above the pool"
        wallets = [D(wallet["balance"]["amount"]) for wallet in inputs["account"]["wallets"]
                   if wallet["balance"]["asset"] == result["settle"]]
        assert wallets and wallets[0] >= budget, "a wallet that could not pay the stop-out"
        assert D(result["budget"]) == budget.quantize(AMOUNT_QUANTUM, rounding=ROUND_HALF_EVEN), "budget"
    expected_of(case["id"], case["expect"], result)


def exact_cycle(case, result):
    long = case["side"] == "long"
    qty, e, x = fr(case["qty"]), fr(case["entry"]), fr(case["exit"])
    if case["family"] == "linear":
        gross = qty * ((x - e) if long else (e - x))
        notional = qty * e
    else:
        face = fr(case["face"])
        gross = qty * face * ((1 / e - 1 / x) if long else (1 / x - 1 / e))
        notional = qty * face / e
    net = gross - fr(case["fees"]) + fr(case["funding"])
    for key, value in (("gross", gross), ("net", net), ("entry_notional", notional)):
        assert result[key] == exactly(value), "{}: {}".format(case["id"], key)
    ratio = net / notional
    assert fr(result["return"]) == Fraction(round(ratio * 10 ** 8), 10 ** 8), "{}: return".format(case["id"])
    assert fr(result["return_percent"]) == Fraction(round(ratio * 10 ** 4), 10 ** 2), "{}: return_percent".format(case["id"])


def settle_cycle(case):
    family, long = case["family"], case["side"] == "long"
    qty, e, x = D(case["qty"]), D(case["entry"]), D(case["exit"])
    fees, funding = D(case["fees"]), D(case["funding"])
    if family == "linear":
        gross = qty * ((x - e) if long else (e - x))
        entry_notional = qty * e
    else:
        face = D(case["face"])
        gross = qty * face * ((1 / e - 1 / x) if long else (1 / x - 1 / e))
        entry_notional = qty * face / e
    net = gross - fees + funding
    ratio = net / entry_notional
    return {
        "gross": shown(gross),
        "fees": shown(fees),
        "funding": shown(funding),
        "net": shown(net),
        "entry_notional": shown(entry_notional),
        "return": text(ratio.quantize(RATIO_PLACES, rounding=ROUND_HALF_EVEN)),
        "return_percent": text((ratio * 100).quantize(D("0.01"), rounding=ROUND_HALF_EVEN)),
    }


def prd_example_checks():
    """The PRD's p.9 figures, checked against each other."""
    distance = D("234.23") - D("200.00")
    assert D("4.38") * distance == D("149.9274"), "4.38 SOL at a 34.23 stop distance risks 149.9274"
    assert text((D("15") / distance).quantize(D("0.00001"), rounding=ROUND_FLOOR)) == "0.43821", "15 USDT buys 0.43821 SOL"
    assert D("4.38") * D("234.23") == D("1025.9274"), "the position value 1,025.93"
    # Every p.9 figure but the risk line fits a 150 USDT budget, 1 % of
    # 15,000: 150 / 34.23 floors to 4.38 on a 0.01 step, and 1,025.9274 / 5
    # is the 205.19 margin at 5x.
    assert (D("150") / distance).quantize(D("0.01"), rounding=ROUND_FLOOR) == D("4.38"), "150 USDT sizes 4.38 SOL"
    assert D("1025.9274") / 5 == D("205.18548"), "the margin 205.19 at 5x"


# ---------------------------------------------------------------------------
# Exact arithmetic on its own: what an implementation's fraction helper must
# answer, whatever it is built on.
# ---------------------------------------------------------------------------


def evaluate(expression):
    """An expression's value as a fraction, or None where it divides by zero.
    A leaf is a decimal; `diff` is the distance between two values."""
    if isinstance(expression, str):
        return fr(expression)
    operator, left, right = expression
    left, right = evaluate(left), evaluate(right)
    if left is None or right is None:
        return None
    if operator == "mul":
        return left * right
    if operator == "add":
        return left + right
    if operator == "diff":
        return abs(left - right)
    assert operator == "div", operator
    return None if right == 0 else left / right


def held(mantissa, scale):
    """The decimal's text, or None where the shared model cannot hold it:
    more than 96 bits of mantissa."""
    return scaled(mantissa, scale) if abs(mantissa) < MANTISSA_LIMIT else None


def held_on_grid(mantissa, scale):
    """A value on a grid: in the grid's own scale where the shared model holds
    it there, else with its trailing zeros dropped. A grid written with more
    places than it needs must not put a value out of reach."""
    kept = held(mantissa, scale)
    if kept is not None:
        return kept
    while scale and mantissa % 10 == 0:
        mantissa, scale = mantissa // 10, scale - 1
    return held(mantissa, scale)


def arithmetic_vector(vector):
    """`vector` with the answer it must get."""
    answered = dict(vector)
    if "compare" in vector:
        left, right = evaluate(vector["compare"][0]), evaluate(vector["compare"][1])
        answered["expect"] = "less" if left < right else "greater" if left > right else "equal"
        return answered
    value = evaluate(vector["expr"])
    if value is None:
        answered["expect"] = None
        answered["undefined"] = True
    elif "places" in vector:
        mantissa, scale = round(value * 10 ** vector["places"]), vector["places"]
        while scale and mantissa % 10 == 0:
            mantissa, scale = mantissa // 10, scale - 1
        answered["expect"] = held(mantissa, scale)
    else:
        grid = vector["grid"]
        if fr(grid) == 0:
            answered["expect"] = None
        else:
            count = math.floor(value / fr(grid)) if vector["round"] == "floor" else math.ceil(value / fr(grid))
            scale = scale_of(grid)
            answered["expect"] = held_on_grid(count * int(fr(grid) * 10 ** scale), scale)
    return answered


class Lcg:
    """A fixed sequence, the same on every Python: the cases must not change
    from one run to the next."""

    def __init__(self, seed):
        self.state = seed

    def below(self, bound):
        self.state = (self.state * 6364136223846793005 + 1442695040888963407) % 2 ** 64
        return (self.state >> 16) % bound

    def decimal(self):
        # Two draws: one gives 48 bits, and a mantissa has up to 96.
        wide = (self.below(2 ** 48) << 48) | self.below(2 ** 48)
        return scaled(wide % 2 ** (1 + self.below(96)), self.below(29))

    def expression(self, depth):
        if depth == 0 or self.below(4) == 0:
            return self.decimal()
        operator = ("mul", "div", "add", "diff")[self.below(4)]
        left, right = self.expression(depth - 1), self.expression(depth - 1)
        if operator == "div" and evaluate(right) in (None, 0):
            right = "3"
        return [operator, left, right]


LARGEST = str(MANTISSA_LIMIT - 1)
SMALLEST = "0." + "0" * 27 + "1"


def arithmetic_vectors():
    third = ["div", "1", "3"]
    vectors = [
        {"note": "a third at 12 places", "expr": third, "places": 12},
        {"note": "two thirds at 12 places rounds up", "expr": ["div", "2", "3"], "places": 12},
        {"note": "half of the last place rounds to the even digit below", "expr": "0.0000000000005", "places": 12},
        {"note": "and to the even digit above", "expr": "0.0000000000015", "places": 12},
        {"note": "and below again", "expr": "0.0000000000025", "places": 12},
        {"note": "just above half rounds up", "expr": "0.00000000000050000000000001", "places": 12},
        {"note": "just below half rounds down", "expr": "0.00000000000049999999999999", "places": 12},
        {"note": "a half at no places", "expr": "0.5", "places": 0},
        {"note": "one and a half", "expr": "1.5", "places": 0},
        {"note": "two and a half", "expr": "2.5", "places": 0},
        {"note": "trailing zeros are dropped", "expr": "13.6920", "places": 12},
        {"note": "a whole number stays whole", "expr": ["mul", "0.5", "300"], "places": 12},
        {"note": "nothing is nothing", "expr": ["diff", "1.10", "1.1"], "places": 12},
        {"note": "the smallest decimal rounds away", "expr": SMALLEST, "places": 12},
        {"note": "the largest mantissa is held", "expr": LARGEST, "places": 12},
        {"note": "one more than it is not", "expr": ["add", LARGEST, "1"], "places": 12},
        {"note": "nor is it with a fraction that stays", "expr": ["add", LARGEST, "0.5"], "places": 12},
        {"note": "a product no decimal holds, brought back by a division", "places": 12,
         "expr": ["div", ["mul", LARGEST, LARGEST], LARGEST]},
        {"note": "a division by nothing has no value", "expr": ["div", "1", ["diff", "2.50", "2.5"]], "places": 12},
        {"note": "a value on the grid is its own floor", "expr": "200.00", "grid": "0.0100", "round": "floor"},
        {"note": "and its own ceiling", "expr": "200.00", "grid": "0.0100", "round": "ceil"},
        {"note": "just under the grid line floors a step down", "round": "floor", "grid": "0.01",
         "expr": ["diff", "200", SMALLEST]},
        {"note": "and ceils back to it", "expr": ["diff", "200", SMALLEST], "grid": "0.01", "round": "ceil"},
        {"note": "just over the grid line ceils a step up", "expr": ["add", "200", SMALLEST], "grid": "0.01",
         "round": "ceil"},
        {"note": "a quotient that is whole floors to itself", "expr": ["div", "150", "37.50"], "grid": "0.01",
         "round": "floor"},
        {"note": "an inverse count that is whole", "grid": "1", "round": "floor",
         "expr": ["div", "0.25", ["diff", ["div", "1", "80.00"], ["div", "1", "100.00"]]]},
        {"note": "three thirds are one", "expr": ["mul", "3", third], "grid": "1", "round": "floor"},
        {"note": "below one step floors to none of it, in the grid's scale", "expr": "0.0004", "grid": "0.001",
         "round": "floor"},
        {"note": "and ceils to one", "expr": "0.0004", "grid": "0.001", "round": "ceil"},
        {"note": "nothing is on every grid", "expr": "0", "grid": "0.10", "round": "ceil"},
        {"note": "a grid that is not a power of ten", "expr": "1.3", "grid": "0.25", "round": "floor"},
        {"note": "the same, upwards", "expr": "1.3", "grid": "0.25", "round": "ceil"},
        {"note": "a grid of nothing has no steps", "expr": "1", "grid": "0", "round": "floor"},
        {"note": "a count the grid's scale cannot hold is held without the zero that scale adds", "expr": LARGEST,
         "grid": "0.1", "round": "floor"},
        {"note": "and not where its last place is no zero", "expr": ["add", LARGEST, "0.3"], "grid": "0.1",
         "round": "floor"},
        {"note": "two fives written with 28 places are ten, on a grid of that many places", "round": "floor",
         "expr": ["add", "5." + "0" * 28, "5." + "0" * 28], "grid": SMALLEST},
        {"note": "a step written with 28 places still counts sixteen", "expr": "16.04", "round": "floor",
         "grid": "0.1" + "0" * 27},
        {"note": "the largest value on a whole grid", "expr": LARGEST, "grid": "1", "round": "floor"},
        {"note": "scale does not change a value", "compare": ["1.10", "1.1"]},
        {"note": "a third is less than 0.3334", "compare": [third, "0.3334"]},
        {"note": "and more than 0.3333", "compare": [third, "0.3333"]},
        {"note": "values beyond any decimal still compare",
         "compare": [["mul", LARGEST, LARGEST], ["add", ["mul", LARGEST, LARGEST], SMALLEST]]},
        {"note": "a difference has no sign", "compare": [["diff", "1", "3"], ["diff", "3", "1"]]},
    ]
    random = Lcg(736)
    for number in range(60):
        expression = random.expression(3)
        choice = random.below(4)
        if choice == 0:
            vectors.append({"note": "generated {}".format(number), "expr": expression, "places": random.below(29)})
        elif choice == 1:
            vectors.append({"note": "generated {}".format(number), "compare": [expression, random.expression(2)]})
        else:
            grid = scaled(1 + random.below(10 ** (1 + random.below(6))), random.below(13))
            vectors.append({"note": "generated {}".format(number), "expr": expression, "grid": grid,
                            "round": "floor" if choice == 2 else "ceil"})
    return [arithmetic_vector(vector) for vector in vectors]


# ---------------------------------------------------------------------------


def cell(case):
    venue, family, _ = case["instrument"].split()
    return "futures.sizing/{}/{}/{}".format(venue, family, case["side"])


# The page's worked examples: one sized entry for each venue, family and side.
EXAMPLES = [
    "prd-p9-binance-linear-long",
    "linear-short-percent-stop",
    "binance-inverse-long",
    "binance-inverse-short",
    "prd-p9-bybit-linear-long",
    "bybit-linear-short",
    "bybit-inverse-long",
    "bybit-inverse-short",
]


def listed(name, statement):
    """One item of a list of the page, wrapped as the page is."""
    # A number and its percent sign stay on one line.
    joined = "- `{}`: {}".format(name, statement).replace(" %", "\u00a0%")
    lines = textwrap.wrap(joined, width=78, subsequent_indent="  ", break_long_words=False,
                          break_on_hyphens=False)
    return "\n".join(lines).replace("\u00a0", " ")


def rewrite_doc(path, sizing, fills):
    """Rewrites what the page repeats of the cases, between its markers."""
    by_id = {item["id"]: item for item in sizing}
    rows = ["| Cell | Case | E | S′ | Quantity | Risk | Margin |", "| --- | --- | --- | --- | --- | --- | --- |"]
    for case_id in EXAMPLES:
        item, result = by_id[case_id], by_id[case_id]["result"]
        rows.append("| `{}` | `{}` | {} | {} | {} | {} {} | {} {} |".format(
            item["cell"], case_id, item["inputs"]["expected_price"], result["stop"], result["qty"],
            result["risk"], result["settle"], result["required_margin"], result["settle"]))
    lists = {
        "worked examples": "\n".join(rows),
        "sizing cases": "\n".join(listed(item["id"], item["note"]) for item in sizing),
        "fill cases": "\n".join(listed(item["id"], item["note"]) for item in fills),
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
    parser.add_argument("--write", help="write the cases and their results as JSON here")
    parser.add_argument("--doc", help="rewrite the lists of this page that repeat the cases")
    args = parser.parse_args()
    with localcontext(Context(prec=PRECISION)):
        prd_example_checks()
        sizing, by_id = [], {}
        for case in SIZING_CASES + TRUST_CASES:
            inputs = inputs_of(case)
            result = size(inputs)
            check_sizing(case, inputs, result)
            again = size_exactly(inputs)
            assert result == again, "{}: {!r} in Decimal, {!r} in fractions".format(case["id"], result, again)
            assert case["id"] not in by_id, "two cases named {}".format(case["id"])
            by_id[case["id"]] = (inputs, result)
            sizing.append({"id": case["id"], "note": case["note"], "cell": cell(case), "inputs": inputs,
                           "expect": case["expect"], "result": result})
        fills = []
        for case in FILL_CASES:
            inputs, sized = by_id[case["base"]]
            assert sized["status"] == "sized", "{}: its entry was not sized".format(case["id"])
            unit = case.get("unit") or ("Contracts" if listing(inputs)["inverse"] is not None else "Base")
            given = [{"price": price, "quantity": {unit: quantity}} for price, quantity in case["fills"]]
            result = fill_risk(inputs, sized, given)
            again = fill_risk_exactly(inputs, sized, given)
            assert result == again, "{}: {!r} in Decimal, {!r} in fractions".format(case["id"], result, again)
            expected_of(case["id"], case["expect"], result)
            fills.append({"id": case["id"], "note": case["note"], "entry": case["base"], "fills": given,
                          "expect": case["expect"], "result": result})
        cycles = []
        for case in CYCLE_CASES:
            result = settle_cycle(case)
            expected_of(case["id"], case["expect"], result)
            exact_cycle(case, result)
            cycles.append({"case": case, "result": result})
        arithmetic = arithmetic_vectors()
        cells = {}
        for item in sizing:
            cells.setdefault(item["cell"], set()).add(item["result"]["status"])
        assert len(cells) == 8 and all({"sized", "refused"} <= statuses for statuses in cells.values()), \
            "every venue, family and side needs an entry that is sized and one that is refused: {}".format(cells)
    for item in sizing:
        result = item["result"]
        if result["status"] == "sized":
            detail = "qty {} at stop {}, risk {} {}".format(result["qty"], result["stop"], result["risk"], result["settle"])
        elif result["status"] == "refused":
            detail = result["reason"]
        else:
            detail = "{} {}".format(result["fault"]["kind"], result["fault"]["input"])
        print("{:<44} {:<8} {}".format(item["id"], result["status"], detail))
    for item in fills:
        result = item["result"]
        detail = "{} {}".format(result["fault"]["kind"], result["fault"]["input"]) if result["status"] == "fault" \
            else "actual {} expected {} deviation {}".format(result["actual"], result["expected"], result["deviation"])
        print("{:<44} {:<8} {}".format(item["id"], result["status"], detail))
    for item in cycles:
        result = item["result"]
        print("{:<44} {:<8} gross {} net {} return {} %".format(
            item["case"]["id"], "cycle", result["gross"], result["net"], result["return_percent"]))
    print("{} sizing cases, {} fill cases, {} cycles, {} arithmetic vectors".format(
        len(sizing), len(fills), len(cycles), len(arithmetic)))
    if args.write:
        document = {
            "format": FORMAT,
            "generated_by": "spikes/futures-contract/sizing.py",
            "decimal_precision": PRECISION,
            "rounding": {
                "stop": "to the tick, away from the entry (LONG floor, SHORT ceiling)",
                "quantity": "floor to the market step",
                "amounts": "exact to 12 decimal places, half to even beyond them, trailing zeros dropped",
                "return": "8 decimal places, half to even",
            },
            "drift_limit": text(DRIFT_LIMIT),
            "freshness_ms": FRESHNESS_MS,
            "now_ms": NOW_MS,
            "rules": RULES,
            "brackets": BRACKETS,
            "instruments": INSTRUMENTS,
            "tiers": TIERS,
            "sizing": sizing,
            "fills": fills,
            "arithmetic": arithmetic,
            "cycles": cycles,
        }
        with open(args.write, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(document, handle, indent=2)
            handle.write("\n")
    if args.doc:
        rewrite_doc(args.doc, sizing, fills)
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
