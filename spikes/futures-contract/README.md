# Futures contract evidence

Two scripts back the numbers in
[ADR 0002](../../docs/adr/0002-automation-architecture.md) and
[futures-venues.md](../../docs/futures-venues.md). Both use the Python
standard library only. Run them with `python3 -I`, which ignores the
environment and the current directory, from the repository root.

Neither script is built, tested or shipped by the crate: `Cargo.toml`
excludes `spikes/` from the package.

## `universe.py`: the product universe

```bash
python3 -I spikes/futures-contract/universe.py --out universe.json --list eligible.json
```

It sends unauthenticated GET requests to the public instrument endpoints of:

- **Binance:** USDⓈ-M and COIN-M, on mainnet and on demo. Binance's futures
  testnet is the same backend as demo.
- **Bybit:** linear and inverse, on mainnet, testnet and demo.

Then it applies ADR 0002's rule: trading-status crypto perpetuals, USDT-settled
for linear and coin-settled for inverse. It reports, for each source:

- how many instruments it keeps, and how many it refuses, broken down by
  contract type, status and margin asset;
- the unit fields of a few reference instruments.

Bybit answers 403 to some regions.

- **`--out`** writes that summary.
  [`universe-2026-10-22.json`](universe-2026-10-22.json) is the run the docs
  quote.
- **`--list`** writes the eligible symbols for each source, which ALG-756
  freezes.

The counts change as venues list and delist contracts. A later run is expected
to differ from the committed one; the rule is the contract, not the count.

## `sizing.py`: risk sizing and accounting

```bash
python3 -I spikes/futures-contract/sizing.py --write spikes/futures-contract/sizing-cases.json
```

It computes ADR 0002's sizing, refusal and accounting rules with `Decimal` at
60 digits, with every rounding mode explicit. The cases cover:

- linear and inverse;
- LONG and SHORT;
- Binance's 100 USD and 10 USD contracts, and Bybit's 1 USD contracts;
- minimums and maximums;
- the leverage brackets, in USDT and in the coin;
- the margin budget;
- the submission guards;
- the PRD's p.9 and p.10 figures. Every p.9 figure but its risk line fits a
  150 USDT budget.

Each result is checked three ways:

- against the invariants the rules promise:
  - on the tick and the step;
  - within budget;
  - one more step would exceed the budget;
  - the stop on the protective side;
  - the position within its leverage bracket;
- against the same numbers computed again in exact rational arithmetic
  (`fractions`), with code that shares nothing with the first computation;
- for the stops, quantities and linear amounts, against values computed by
  hand.

The script exits non-zero if any check fails. Planted defects in the inverse
formula, the stop rounding, the fee allowance, the crossed-stop check, the net
PnL and the choice of leverage bracket each made it fail.

[`sizing-cases.json`](sizing-cases.json) holds the cases and their results, for
ALG-736 to reuse.

The instrument rules in the cases are the 2026-10-22 values from `universe.py`.
The taker fees are illustrative. Bybit's leverage tiers are its public risk
limits on 2026-10-22. Binance serves its brackets per account, to signed
requests only, so the Binance tiers are illustrative too.
