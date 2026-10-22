#!/usr/bin/env python3
"""Resolve the v0.3 futures product universe from public venue metadata.

ADR 0002 section 3 defines the universe as a rule, not a list: trading-status
crypto perpetuals, USDT-settled for the linear family and coin-settled for the
inverse family, on Binance and Bybit. This script applies that rule to each
venue's public instrument metadata, on mainnet and on each test environment,
and reports what it keeps and what it refuses.

It sends unauthenticated GET requests to public endpoints only. Run it with
the standard library alone:

    python3 -I spikes/futures-contract/universe.py --out summary.json
    python3 -I spikes/futures-contract/universe.py --list eligible.json

Bybit answers 403 to requests from some regions; run it from elsewhere then.
"""

import argparse
import datetime
import json
import sys
import urllib.parse
import urllib.request

RULE_VERSION = 1

RULE = {
    "binance linear": "contractType PERPETUAL, marginAsset USDT, status TRADING",
    "binance inverse": "contractType PERPETUAL, contractStatus TRADING",
    "bybit linear": "contractType LinearPerpetual, settleCoin USDT, status Trading",
    "bybit inverse": "contractType InversePerpetual, status Trading",
}

# (venue, category, environment, url). Binance's futures testnet and its demo
# trading are one backend (testnet.binancefuture.com serves the same data as
# demo-fapi and demo-dapi), so it is probed once, as demo.
SOURCES = [
    ("binance", "linear", "mainnet", "https://fapi.binance.com/fapi/v1/exchangeInfo"),
    ("binance", "inverse", "mainnet", "https://dapi.binance.com/dapi/v1/exchangeInfo"),
    ("binance", "linear", "demo", "https://demo-fapi.binance.com/fapi/v1/exchangeInfo"),
    ("binance", "inverse", "demo", "https://demo-dapi.binance.com/dapi/v1/exchangeInfo"),
    ("bybit", "linear", "mainnet", "https://api.bybit.com/v5/market/instruments-info?category=linear&limit=1000"),
    ("bybit", "inverse", "mainnet", "https://api.bybit.com/v5/market/instruments-info?category=inverse&limit=1000"),
    ("bybit", "linear", "testnet", "https://api-testnet.bybit.com/v5/market/instruments-info?category=linear&limit=1000"),
    ("bybit", "inverse", "testnet", "https://api-testnet.bybit.com/v5/market/instruments-info?category=inverse&limit=1000"),
    ("bybit", "linear", "demo", "https://api-demo.bybit.com/v5/market/instruments-info?category=linear&limit=1000"),
    ("bybit", "inverse", "demo", "https://api-demo.bybit.com/v5/market/instruments-info?category=inverse&limit=1000"),
]

# Instruments whose unit fields are recorded, by (venue, category).
REFERENCE = {
    ("binance", "linear"): ["BTCUSDT", "SOLUSDT"],
    ("binance", "inverse"): ["BTCUSD_PERP", "ETHUSD_PERP", "SOLUSD_PERP"],
    ("bybit", "linear"): ["BTCUSDT", "SOLUSDT"],
    ("bybit", "inverse"): ["BTCUSD", "ETHUSD", "SOLUSD"],
}

TIMEOUT_SECONDS = 30


def fetch(url):
    request = urllib.request.Request(url, headers={"User-Agent": "quantforge-universe-probe/1"})
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return response.status, json.loads(response.read().decode("utf-8"))


def binance_instruments(url):
    status, body = fetch(url)
    return status, body["symbols"]


def bybit_instruments(url):
    """Follows `nextPageCursor` until Bybit returns an empty one."""
    instruments = []
    cursor = ""
    status = None
    while True:
        page_url = url if not cursor else url + "&cursor=" + urllib.parse.quote(cursor, safe="")
        status, body = fetch(page_url)
        if body.get("retCode") != 0:
            raise RuntimeError("bybit refused {}: {} {}".format(page_url, body.get("retCode"), body.get("retMsg")))
        result = body["result"]
        instruments.extend(result["list"])
        # The cursor arrives percent-encoded; decode it once so that quoting
        # above encodes it exactly once.
        cursor = urllib.parse.unquote(result.get("nextPageCursor") or "")
        if not cursor:
            return status, instruments


def binance_filter(symbol, kind):
    for item in symbol.get("filters", []):
        if item.get("filterType") == kind:
            return item
    return {}


def classify(venue, category, instrument):
    """Returns (kind, eligible): the counting key and whether the rule keeps it."""
    if venue == "binance":
        contract = instrument.get("contractType") or "-"
        status = instrument.get("status") or instrument.get("contractStatus") or "-"
        margin = instrument.get("marginAsset") or "-"
        kind = "{} | {} | {}".format(contract, status, margin)
        if category == "linear":
            eligible = contract == "PERPETUAL" and margin == "USDT" and status == "TRADING"
        else:
            eligible = contract == "PERPETUAL" and status == "TRADING"
        return kind, eligible
    contract = instrument.get("contractType") or "-"
    status = instrument.get("status") or "-"
    settle = instrument.get("settleCoin") or "-"
    kind = "{} | {} | {}".format(contract, status, settle)
    if category == "linear":
        eligible = contract == "LinearPerpetual" and settle == "USDT" and status == "Trading"
    else:
        eligible = contract == "InversePerpetual" and status == "Trading"
    return kind, eligible


def symbol_of(instrument):
    return instrument["symbol"]


def unit_fields(venue, instrument):
    if venue == "binance":
        price = binance_filter(instrument, "PRICE_FILTER")
        lot = binance_filter(instrument, "LOT_SIZE")
        market_lot = binance_filter(instrument, "MARKET_LOT_SIZE")
        notional = binance_filter(instrument, "MIN_NOTIONAL")
        return {
            "contractType": instrument.get("contractType"),
            "status": instrument.get("status") or instrument.get("contractStatus"),
            "baseAsset": instrument.get("baseAsset"),
            "quoteAsset": instrument.get("quoteAsset"),
            "marginAsset": instrument.get("marginAsset"),
            "contractSize": instrument.get("contractSize"),
            "tickSize": price.get("tickSize"),
            "stepSize": lot.get("stepSize"),
            "minQty": lot.get("minQty"),
            "marketStepSize": market_lot.get("stepSize"),
            "marketMaxQty": market_lot.get("maxQty"),
            "minNotional": notional.get("notional"),
            "triggerProtect": instrument.get("triggerProtect"),
        }
    lot = instrument.get("lotSizeFilter", {})
    return {
        "contractType": instrument.get("contractType"),
        "status": instrument.get("status"),
        "baseCoin": instrument.get("baseCoin"),
        "quoteCoin": instrument.get("quoteCoin"),
        "settleCoin": instrument.get("settleCoin"),
        "tickSize": instrument.get("priceFilter", {}).get("tickSize"),
        "qtyStep": lot.get("qtyStep"),
        "minOrderQty": lot.get("minOrderQty"),
        "maxMktOrderQty": lot.get("maxMktOrderQty"),
        "minNotionalValue": lot.get("minNotionalValue"),
        "maxLeverage": instrument.get("leverageFilter", {}).get("maxLeverage"),
    }


def probe():
    fetched_at = datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0)
    sources = []
    eligible_lists = {}
    reference = {}
    for venue, category, environment, url in SOURCES:
        load = binance_instruments if venue == "binance" else bybit_instruments
        status, instruments = load(url)
        kinds = {}
        eligible = []
        for instrument in instruments:
            kind, keep = classify(venue, category, instrument)
            kinds[kind] = kinds.get(kind, 0) + 1
            if keep:
                eligible.append(symbol_of(instrument))
        eligible.sort()
        key = "{} {} {}".format(venue, category, environment)
        sources.append({
            "venue": venue,
            "category": category,
            "environment": environment,
            "url": url,
            "http_status": status,
            "instruments": len(instruments),
            "eligible": len(eligible),
            "refused": len(instruments) - len(eligible),
            "by_kind": dict(sorted(kinds.items())),
        })
        eligible_lists[key] = eligible
        by_symbol = {symbol_of(instrument): instrument for instrument in instruments}
        for symbol in REFERENCE[(venue, category)]:
            if symbol in by_symbol:
                reference["{} {}".format(key, symbol)] = unit_fields(venue, by_symbol[symbol])
    summary = {
        "rule_version": RULE_VERSION,
        "rule": RULE,
        "fetched_at": fetched_at.isoformat().replace("+00:00", "Z"),
        "sources": sources,
        "reference_instruments": reference,
    }
    return summary, eligible_lists


def table(summary):
    lines = [
        "| Venue | Family | Environment | Instruments | Eligible | Refused |",
        "| --- | --- | --- | --- | --- | --- |",
    ]
    for source in summary["sources"]:
        lines.append("| {venue} | {category} | {environment} | {instruments} | {eligible} | {refused} |".format(**source))
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", help="write the summary JSON here")
    parser.add_argument("--list", dest="list_path", help="write the eligible symbols, per source, here")
    args = parser.parse_args()
    summary, eligible_lists = probe()
    if args.out:
        with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(summary, handle, indent=2)
            handle.write("\n")
    if args.list_path:
        listing = {
            "rule_version": RULE_VERSION,
            "fetched_at": summary["fetched_at"],
            "eligible": eligible_lists,
        }
        with open(args.list_path, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(listing, handle, indent=2)
            handle.write("\n")
    print("fetched_at: {}".format(summary["fetched_at"]))
    print(table(summary))
    return 0


if __name__ == "__main__":
    sys.exit(main())
