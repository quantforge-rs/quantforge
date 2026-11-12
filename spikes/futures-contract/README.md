# Futures contract evidence

Two scripts back the numbers in
[ADR 0002](../../docs/adr/0002-automation-architecture.md) and
[futures-venues.md](../../docs/futures-venues.md), one of which also writes
the shared cases risk sizing is held to, and a third writes the shared
cases of a profile's policy. All use the Python standard library only. Run them with `python3 -I`, which ignores the environment and the
current directory, from the repository root.

No script here is built, tested or shipped by the crate: `Cargo.toml`
excludes `spikes/` from the package.

## `universe.py`: the product universe

```bash
python3 -I spikes/futures-contract/universe.py --out universe.json --list docs/parity/universe-v1-YYYY-MM-DD.json
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
- **`--list`** writes, for each source, the eligible symbols and the rule that
  selected them. ALG-756 froze one as
  [`universe-v1-2026-10-22.json`](../../docs/parity/universe-v1-2026-10-22.json)
  for the parity matrix.

The counts change as venues list and delist contracts. A later run is expected
to differ from the committed one; the rule is the contract, not the count.

## `sizing.py`: risk sizing and accounting

```bash
python3 -I spikes/futures-contract/sizing.py --write spikes/futures-contract/sizing-cases.json
python3 -I spikes/futures-contract/sizing.py --write tests/fixtures/sizing_cases.json
python3 -I spikes/futures-contract/sizing.py --doc docs/risk-sizing.md
```

It computes ADR 0002's sizing, refusal and accounting rules with `Decimal` at
60 digits, with every rounding mode explicit, and is the independent model
the Rust sizing is held to ([risk-sizing.md](../../docs/risk-sizing.md)). It
shares no code with the Rust. Each case hands sizing its inputs in the shared
model's stored form, what a venue's adapter makes of its instrument metadata,
its leverage tiers and an account, so the Rust reads the cases as they are.
The cases cover:

- linear and inverse, LONG and SHORT, on both venues: every cell of the
  parity matrix's `futures.sizing`, each sized and each refused;
- Binance's 100 USD and 10 USD contracts, and Bybit's 1 USD contracts;
- which stop an entry uses, and each stop rounded away from the entry;
- every limit met exactly and missed by one step: the minimum quantity and
  notional, the market-order maximum, the bracket's cap, the margin, the
  wallet and the drift limit;
- margin pools in the settlement asset and in USD, at the venue's index
  price;
- the submission guards, on linear and on inverse contracts;
- the order refusals are given in, and the order sizing looks for what it
  cannot trust: an input for something else, too old, missing, ambiguous or
  invalid;
- the risk an entry's fills took, beside the risk it was sized with;
- the PRD's p.9 and p.10 figures. Every p.9 figure but its risk line fits a
  150 USDT budget.

Each result is checked three ways:

- against the invariants the rules promise:
  - on the tick and the step;
  - within budget;
  - one more step would exceed the budget;
  - the stop on the protective side;
  - the position within its leverage bracket, and its margin within the pool;
- against the whole answer worked out again in exact rational arithmetic
  (`fractions`), with no arithmetic in common with the first computation: the
  two must agree on every refusal and every figure;
- for the stops, quantities and linear amounts, against values computed by
  hand.

The script exits non-zero if any check fails. Eighteen planted defects each
made it fail, among them a one-in-ten-million error in the inverse formula, a
stop rounded towards the entry, a fee allowance for one leg, a price exactly
at the stop not counted as crossed, a quantity rounded up, a USD pool left
unconverted, reservations ignored, an input at its freshness limit called
stale, the inputs looked at in another order, and a half rounded up instead
of to even.

It writes the same cases to two files. [`sizing-cases.json`](sizing-cases.json)
is the one ADR 0002 links;
[`tests/fixtures/sizing_cases.json`](../../tests/fixtures/sizing_cases.json) is
the one `tests/sizing/` replays, on the model and under a policy read back
from SQLite and PostgreSQL, since `spikes/` is not packaged with the crate. A
test holds the two to each other. Change a case here, never in a file, and
write both again; `--doc` then rewrites the lists of
[risk-sizing.md](../../docs/risk-sizing.md) that repeat the cases, which the
tests hold to the fixture as well. The file also holds the exact arithmetic a fraction helper
must answer, which the Rust helper's unit tests read.

The instrument rules in the cases are the 2026-10-22 values from `universe.py`.
The taker fees are illustrative. Bybit's leverage tiers are its public risk
limits on 2026-10-22. Binance serves its brackets per account, to signed
requests only, so the Binance tiers are illustrative too, and so are the names
of the indexes a USD pool is measured at, which each adapter records as it
reads them.

## `policy_cases.py`: a profile's policy

```bash
python3 -I spikes/futures-contract/policy_cases.py --write tests/fixtures/policy_cases.json
```

An independent model of an automation profile's policy, the contract of
[automation-policy.md](../../docs/automation-policy.md): the defaults and
what an asset inherits from them, each change with the revision it stamps,
the refusals, the snapshot an admitted entry carries and the stop it uses,
and what a venue's instrument list says about a policy. It shares no code
with the Rust model. Every case runs on each of the four futures product
lines, and two changes made from one revision are recorded in either order.

It writes [`tests/fixtures/policy_cases.json`](../../tests/fixtures/policy_cases.json),
which `tests/policy/` replays through the Rust model and through the policy
service on SQLite and PostgreSQL. Without `--write` it only runs the model
and prints the counts. Change a case here, never in the fixture, and write
the fixture again.
