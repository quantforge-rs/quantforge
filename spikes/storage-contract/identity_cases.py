#!/usr/bin/env python3
"""The storage identity cases of docs/storage-schema.md, written independently
of the Rust code under test: for every product line, the storage columns, the
wire spelling and the state_json token of one market, the decimal texts a
store returns unchanged, and the venue IDs that are text on every venue.

Run with ``python3 -I identity_cases.py`` from the repository root; it writes
tests/fixtures/storage_identity_cases.json. Stdlib only.
"""

import json
import pathlib
import sys

VENUES = ["binance", "bybit"]
CATEGORIES = ["spot", "linear", "inverse"]
SYMBOLS = {"spot": " btcusdt ", "linear": "btcusdt", "inverse": "BTCUSD"}
INTERVALS = {"spot": "1m", "linear": "1h", "inverse": "8h"}


def token(venue: str, category: str) -> str:
    """The serde token: CamelCase of venue and category."""
    return venue.capitalize() + category.capitalize()


def market(venue: str, category: str) -> dict:
    symbol = SYMBOLS[category].strip().upper()
    interval = INTERVALS[category]
    return {
        "venue": venue,
        "category": category,
        "symbol": SYMBOLS[category],
        "interval": interval,
        "storage": {"exchange": f"{venue}_{category}", "symbol": symbol, "interval": interval},
        "wire": {"exchange": venue, "category": category, "symbol": symbol, "interval": interval},
        "state_json_exchange": token(venue, category),
    }


CASES = {
    "format": 1,
    "note": (
        "The storage identity cases of docs/storage-schema.md: every product line's "
        "storage columns, wire spelling and state_json token, the decimal texts a store "
        "returns unchanged, scale included, and venue IDs as text. Written by "
        "spikes/storage-contract/identity_cases.py, not by the Rust code under test."
    ),
    "markets": [market(venue, category) for venue in VENUES for category in CATEGORIES],
    "decimals": [
        "0",
        "1.10",
        "1.1",
        "0.0000001",
        "67012.34000000",
        "12.04510000",
        "-1.5",
        "123456789.123456789012345678",
        "0.1234567890123456789012345678",
    ],
    "venue_ids": ["7", "1321003749386327552", "a1b2c3-d4", "00000042"],
}


def main() -> int:
    root = pathlib.Path(__file__).resolve().parents[2]
    out = root / "tests" / "fixtures" / "storage_identity_cases.json"
    out.write_text(json.dumps(CASES, indent=2) + "\n")
    print(f"wrote {out.relative_to(root)}: {len(CASES['markets'])} markets, {len(CASES['decimals'])} decimals")
    return 0


if __name__ == "__main__":
    sys.exit(main())
