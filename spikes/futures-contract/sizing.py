#!/usr/bin/env python3
"""Independent Decimal oracle for ADR 0002's risk sizing and accounting rules.

Every worked example in ADR 0002 comes from this script, never from the PRD's
illustrative figures. It applies the rules of ADR 0002 sections 6, 9 and 12 to
the cases below. The rules are:

- R = B x r, refused unless the capital is in the instrument's settlement
  asset;
- the stop is the payload's absolute stop if given, side-checked, else the
  percentage fallback E x (1 - p) for LONG or E x (1 + p) for SHORT, else
  refused;
- the stop rounds to the tick away from the entry, giving S';
- linear quantity is R / |E - S'|;
- inverse contracts are R / (F x |1/E - 1/S'|);
- both are floored to the market step, then checked against the minimum
  quantity, minimum notional and market-order maximum;
- the initial margin plus taker fees for opening and closing must fit the
  available margin minus reservations;
- an entry is refused when the settlement asset's wallet holds less than R,
  and warned about when it holds less than B;
- at submission the entry is refused if the mark price or the book has already
  crossed the stop, or if the risk at the expected fill exceeds 1.25 x R.

It checks itself in three ways:
- each result is tested for the invariants the rules promise: on the grid,
  within budget, the floor is tight, and the stop is on the protective side;
- every number is computed again in exact rational arithmetic (fractions), a
  second implementation that shares no code with the first;
- hand-computed expectations (stops, quantities, linear amounts) are compared
  exactly.

    python3 -I spikes/futures-contract/sizing.py
    python3 -I spikes/futures-contract/sizing.py --write sizing-cases.json

Standard library only; no network.
"""

import argparse
import json
import math
import sys
from decimal import ROUND_CEILING, ROUND_FLOOR, ROUND_HALF_EVEN, Context, Decimal, localcontext
from fractions import Fraction

D = Decimal
PRECISION = 60
COIN_PLACES = D("1e-12")
RATIO_PLACES = D("1e-8")
DRIFT_LIMIT = D("1.25")

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


def text(value):
    """Plain notation, never an exponent; keeps the value's own scale, as a
    stop on the tick or a quantity on the step should."""
    return format(value, "f")


def amount(value):
    """A derived amount, whose scale is an artifact of the arithmetic:
    13.6920 and 13.692 are the same risk, printed as the latter."""
    return format(value.normalize(), "f")


def coin(value):
    """A coin amount that involved a division, rounded for display."""
    return amount(value.quantize(COIN_PLACES, rounding=ROUND_HALF_EVEN))


def to_grid(value, grid, rounding):
    return (value / grid).to_integral_value(rounding=rounding) * grid


def size(case):
    rules = RULES[case["instrument"]]
    settle = SETTLE[case["instrument"]]
    family = case["instrument"].split()[1]
    side = case["side"]
    long = side == "long"
    tick, step = D(rules["tick"]), D(rules["step"])
    face = D(rules.get("face", "1"))
    e = D(case["entry"])
    out = {"settle": settle, "warnings": []}

    if case["capital"]["asset"] != settle:
        return dict(out, status="refused", reason="no_capital_in_settlement_asset")
    budget = D(case["capital"]["amount"]) * D(case["risk_percent"]) / 100
    out["budget"] = amount(budget)

    if "stop" in case:
        s = D(case["stop"])
        if (long and s >= e) or (not long and s <= e):
            return dict(out, status="refused", reason="stop_wrong_side")
    elif "stop_percent" in case:
        p = D(case["stop_percent"]) / 100
        s = e * (1 - p) if long else e * (1 + p)
    else:
        return dict(out, status="refused", reason="no_stop")
    # Away from the entry: never tighter than the stop asked for.
    stop = to_grid(s, tick, ROUND_FLOOR if long else ROUND_CEILING).quantize(tick)
    out["stop"] = text(stop)

    if family == "linear":
        unit_loss = abs(e - stop)
    else:
        unit_loss = face * abs(1 / e - 1 / stop)
    qty = to_grid(budget / unit_loss, step, ROUND_FLOOR).quantize(step)
    risk = qty * unit_loss
    out.update(qty=text(qty), unit_loss=amount(unit_loss) if family == "linear" else coin(unit_loss))
    if qty < D(rules["min_qty"]):
        return dict(out, status="refused", reason="below_min_qty")
    if qty > D(rules["max_mkt"]):
        return dict(out, status="refused", reason="above_market_max")

    if family == "linear":
        notional_quote = qty * e
        notional_settle = notional_quote
    else:
        notional_quote = qty * face
        notional_settle = notional_quote / e
    if "min_notional" in rules and notional_quote < D(rules["min_notional"]):
        return dict(out, status="refused", reason="below_min_notional")

    leverage = D(case["leverage"])
    initial_margin = notional_settle / leverage
    fee_allowance = notional_settle * D(case["taker_fee"]) * 2
    required = initial_margin + fee_allowance
    available = D(case["available_margin"]) - D(case.get("reserved_margin", "0"))
    show = amount if family == "linear" else coin
    out.update(
        risk=show(risk),
        notional=amount(notional_quote),
        notional_settle=show(notional_settle),
        initial_margin=show(initial_margin),
        fee_allowance=show(fee_allowance),
        required_margin=show(required),
    )
    if required > available:
        return dict(out, status="refused", reason="insufficient_margin")

    # A stop-out the settlement asset's wallet cannot pay would leave a debt
    # in that asset, whatever collateral funds the margin.
    wallet = D(case["settlement_wallet"])
    if wallet < budget:
        return dict(out, status="refused", reason="settlement_wallet_below_risk")
    if wallet < D(case["capital"]["amount"]):
        out["warnings"].append("wallet_below_capital")

    if "quote" in case:
        bid, ask, mark = (D(case["quote"][key]) for key in ("bid", "ask", "mark"))
        # The stop triggers on the mark price; the touch is where a market
        # entry would fill. Either one at or past the stop refuses the entry.
        if (long and (bid <= stop or mark <= stop)) or (not long and (ask >= stop or mark >= stop)):
            return dict(out, status="refused", reason="stop_crossed")
        fill = ask if long else bid
        if family == "linear":
            at_fill = qty * abs(fill - stop)
        else:
            at_fill = qty * face * abs(1 / fill - 1 / stop)
        out["risk_at_fill"] = show(at_fill)
        if at_fill > DRIFT_LIMIT * budget:
            return dict(out, status="refused", reason="risk_drift")
    return dict(out, status="sized")


def check_sizing(case, result):
    """The invariants the rules promise, then the hand-computed values."""
    if result["status"] == "sized":
        rules = RULES[case["instrument"]]
        family = case["instrument"].split()[1]
        long = case["side"] == "long"
        tick, step, e = D(rules["tick"]), D(rules["step"]), D(case["entry"])
        face = D(rules.get("face", "1"))
        stop, qty = D(result["stop"]), D(result["qty"])
        budget = D(result["budget"])
        assert stop % tick == 0, "stop off the tick"
        assert qty % step == 0, "quantity off the step"
        assert (stop < e) if long else (stop > e), "stop on the wrong side"
        # The PnL formula at the stop, written independently of size().
        if family == "linear":
            pnl_at_stop = qty * (stop - e) if long else qty * (e - stop)
            loss_per_unit = abs(e - stop)
        else:
            pnl_at_stop = qty * face * ((1 / e - 1 / stop) if long else (1 / stop - 1 / e))
            loss_per_unit = face * abs(1 / e - 1 / stop)
        assert pnl_at_stop < 0, "a stop-out must lose"
        assert -pnl_at_stop <= budget, "risk above the budget"
        assert (qty + step) * loss_per_unit > budget, "the floor is not tight"
    for key, value in case["expect"].items():
        assert result.get(key) == value, "{}: {} is {!r}, expected {!r}".format(case["id"], key, result.get(key), value)


def fr(value):
    """A decimal string as an exact fraction."""
    return Fraction(str(value))


def rounded(value, places):
    """A fraction rounded half to even at `places` decimals, still exact."""
    return Fraction(round(value * 10 ** places), 10 ** places)


def exact_sizing(case, result):
    """Every number of size() again, in fractions instead of Decimal."""
    if "qty" not in result:
        return
    rules = RULES[case["instrument"]]
    family = case["instrument"].split()[1]
    long = case["side"] == "long"
    e, tick, step = fr(case["entry"]), fr(rules["tick"]), fr(rules["step"])
    face = fr(rules.get("face", "1"))
    budget = fr(case["capital"]["amount"]) * fr(case["risk_percent"]) / 100
    if "stop" in case:
        s = fr(case["stop"])
    else:
        p = fr(case["stop_percent"]) / 100
        s = e * (1 - p) if long else e * (1 + p)
    stop = (math.floor(s / tick) if long else math.ceil(s / tick)) * tick
    unit = abs(e - stop) if family == "linear" else face * abs(1 / e - 1 / stop)
    qty = math.floor(budget / unit / step) * step
    assert fr(result["stop"]) == stop, "{}: stop".format(case["id"])
    assert fr(result["qty"]) == qty, "{}: qty".format(case["id"])

    def same(key, value):
        if key in result:
            expected = value if family == "linear" else rounded(value, 12)
            assert fr(result[key]) == expected, "{}: {}".format(case["id"], key)

    same("risk", qty * unit)
    notional = qty * e if family == "linear" else qty * face
    assert "notional" not in result or fr(result["notional"]) == notional, "{}: notional".format(case["id"])
    settled = notional if family == "linear" else notional / e
    margin = settled / fr(case["leverage"])
    fees = settled * fr(case["taker_fee"]) * 2
    same("notional_settle", settled)
    same("initial_margin", margin)
    same("fee_allowance", fees)
    same("required_margin", margin + fees)
    if "risk_at_fill" in result:
        fill = fr(case["quote"]["ask"] if long else case["quote"]["bid"])
        same("risk_at_fill", qty * abs(fill - stop) if family == "linear" else qty * face * abs(1 / fill - 1 / stop))


def exact_cycle(case, result):
    long = case["side"] == "long"
    qty, e, x = fr(case["qty"]), fr(case["entry"]), fr(case["exit"])
    if case["family"] == "linear":
        gross = qty * ((x - e) if long else (e - x))
        notional = qty * e
        places = None
    else:
        face = fr(case["face"])
        gross = qty * face * ((1 / e - 1 / x) if long else (1 / x - 1 / e))
        notional = qty * face / e
        places = 12
    net = gross - fr(case["fees"]) + fr(case["funding"])
    for key, value in (("gross", gross), ("net", net), ("entry_notional", notional)):
        expected = value if places is None else rounded(value, places)
        assert fr(result[key]) == expected, "{}: {}".format(case["id"], key)
    ratio = net / notional
    assert fr(result["return"]) == rounded(ratio, 8), "{}: return".format(case["id"])
    assert fr(result["return_percent"]) == rounded(ratio * 100, 2), "{}: return_percent".format(case["id"])


def settle_cycle(case):
    family, long = case["family"], case["side"] == "long"
    qty, e, x = D(case["qty"]), D(case["entry"]), D(case["exit"])
    fees, funding = D(case["fees"]), D(case["funding"])
    if family == "linear":
        gross = qty * ((x - e) if long else (e - x))
        entry_notional = qty * e
        show = amount
    else:
        face = D(case["face"])
        gross = qty * face * ((1 / e - 1 / x) if long else (1 / x - 1 / e))
        entry_notional = qty * face / e
        show = coin
    net = gross - fees + funding
    ratio = net / entry_notional
    return {
        "gross": show(gross),
        "fees": show(fees),
        "funding": show(funding),
        "net": show(net),
        "entry_notional": show(entry_notional),
        "return": text(ratio.quantize(RATIO_PLACES, rounding=ROUND_HALF_EVEN)),
        "return_percent": text((ratio * 100).quantize(D("0.01"), rounding=ROUND_HALF_EVEN)),
    }


def check_cycle(case, result):
    for key, value in case["expect"].items():
        assert result.get(key) == value, "{}: {} is {!r}, expected {!r}".format(case["id"], key, result.get(key), value)


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


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", help="write the cases and their results as JSON here")
    args = parser.parse_args()
    with localcontext(Context(prec=PRECISION)):
        prd_example_checks()
        sizing = []
        for case in SIZING_CASES:
            result = size(case)
            check_sizing(case, result)
            exact_sizing(case, result)
            sizing.append({"case": case, "result": result})
        cycles = []
        for case in CYCLE_CASES:
            result = settle_cycle(case)
            check_cycle(case, result)
            exact_cycle(case, result)
            cycles.append({"case": case, "result": result})
    for item in sizing:
        result = item["result"]
        detail = result.get("reason") or "qty {} at stop {}, risk {} {}".format(
            result["qty"], result["stop"], result["risk"], result["settle"])
        print("{:<28} {:<8} {}".format(item["case"]["id"], result["status"], detail))
    for item in cycles:
        result = item["result"]
        print("{:<28} {:<8} gross {} net {} return {} %".format(
            item["case"]["id"], "cycle", result["gross"], result["net"], result["return_percent"]))
    if args.write:
        document = {
            "format": 1,
            "generated_by": "spikes/futures-contract/sizing.py",
            "decimal_precision": PRECISION,
            "rounding": {
                "stop": "to the tick, away from the entry (LONG floor, SHORT ceiling)",
                "quantity": "floor to the market step",
                "coin_amounts": "12 decimal places, half to even, where a division is involved",
                "return": "8 decimal places, half to even",
            },
            "drift_limit": text(DRIFT_LIMIT),
            "rules": RULES,
            "sizing": sizing,
            "cycles": cycles,
        }
        with open(args.write, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(document, handle, indent=2)
            handle.write("\n")
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
