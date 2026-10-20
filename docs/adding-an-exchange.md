# Adding an exchange

An exchange joins QuantForge as one adapter module behind the `ports`
connector traits, one registry entry, and a set of fixtures it passes the
conformance suite with. Nothing outside `src/exchange/` changes: the engines
and the CLI reach every exchange through the traits, and `--exchange` lists
whatever the registry holds.

The contract the adapter meets is written down once, in the module docs of
[`src/exchange/conformance/`](../src/exchange/conformance/mod.rs), and
checked by the suite. Where the exchange behaves differently, the adapter
normalizes it to the contract; the engines are never taught an exchange's
quirks.

## 1. Name the exchange

Add a variant to `ExchangeId` in `src/model/market.rs`, with the storage name
`ExchangeId::as_str` returns for it, such as `bybit_spot`. Candles and runs
are stored under that name, so it never changes once released. The compiler
then lists every exhaustive match that needs the new exchange.

## 2. Write the adapter

Create `src/exchange/<name>/`, mirroring `src/exchange/binance/`: a client
for the transport and signing, the wire DTOs and their conversion, and the
connector implementations. One type, or several, implements:

- `ExchangeConnector`: the `ExchangeId`, the capabilities, and whether the
  endpoint trades real funds (`is_production`, which the order checks and
  the live CLI's `(PRODUCTION)` marker rely on);
- `MarketDataConnector`: klines, symbol rules, and the exchange clock;
- `TradingConnector`: market orders, cancels, lookups, and open orders;
- `AccountConnector`: balances and the account's trades.

The contract asks a few things most exchanges need help with:

- candles ascending by open time, both window bounds inclusive, and a window
  that opens before the exchange's first bar answered with its oldest bars,
  whatever order and paging the exchange uses;
- `close_time_ms` as the bar's last millisecond, `open + step - 1`, even for
  a bar still forming;
- every decimal parsed from the exchange's text, never through a float;
- a market order answered as filled, with its executed quantity, quote
  amount, fills and their trade ids, both order ids, and the transaction
  time; fetch the fills after placing the order if the exchange does not
  return them;
- "no limit" in a trading rule reported as absent, never as zero.

Declare the capabilities as one full struct literal, and pin it in a test as
`declared_capabilities_are_pinned` does in `src/exchange/binance/venue.rs`,
so that a capability added later makes the adapter decide its value. Callers
branch on capabilities, never on the `ExchangeId`.

Map every error the exchange documents onto an `ExchangeErrorKind`, in a
table test like the one in `src/exchange/binance/errors.rs`. The kinds that
matter most to a caller are `Unavailable`, a request the exchange refused
before processing it and so safe to resend, and `OutcomeUnknown`, a request
that may have taken effect, so an order is looked up before it is sent
again.

## 3. Register it

In the adapter's `mod.rs`, declare its `REGISTRATION`: the name operators
select it by with `--exchange`, its production base URL, and its connector
factory. In `src/exchange/registry.rs`, add its arm to `registration()` and
its id to `EXCHANGES`; the registry's completeness test counts them.

`tests/exchange_boundary.rs` keeps every other module from naming an
adapter. Give the new exchange the same scan Binance has, so that nothing
outside its module names it either.

## 4. Provide fixtures

Add the exchange's profile to `tests/conformance/profiles.rs`: its testnet
URL, the market the checks trade (liquid on the testnet), the query
parameters that change on every request, the headers that carry credentials
or must be forwarded, and the response fields that identify the account. The
match there is exhaustive, so the harness does not compile without it.

Record the fixtures from a live testnet run, with that testnet's keys:

```bash
CONFORMANCE_MODE=record CONFORMANCE_EXCHANGE=<name> \
  QF_API_KEY=... QF_API_SECRET=... \
  cargo test --locked --test conformance live:: -- --nocapture
```

Record mode writes the fixture of every check that passes live and then
replays exactly as it ran. A check that fails, or depends on anything but
the exchange's responses, writes nothing. `CONFORMANCE_CHECK=<id>` re-records
a single check.

The checks that run against fixtures only (`errors.clock_skew`,
`errors.rate_limited`, `errors.unavailable`, `errors.outcome_unknown`,
`errors.malformed_response`) script faults no testnet produces on demand.
Write their fixtures by hand, from the error responses the exchange
documents, with `"source": "authored"` and a `note` saying where the
responses come from; the Binance ones in `tests/conformance/fixtures/binance/`
are the template. A hand-written request names only its method and path.

Before committing, make sure no fixture holds the keys, then refresh the
matrix and review both diffs:

```bash
UPDATE_SNAPSHOTS=1 cargo test --test conformance
```

The fixtures tests then hold the adapter to the contract on every pull
request, and the controls test proves each check can still fail against it.

## 5. Run it live every day

Add the exchange to the matrix in `.github/workflows/conformance.yml`, with
its own GitHub environment, `testnet-<name>`, holding the testnet keys as
`TESTNET_API_KEY` and `TESTNET_API_SECRET`. Each job sees only its own
environment's secrets, and the job never runs on pull requests.

Finally, add `CHANGELOG.md` entries for the new exchange and its name on
`--exchange`, and list its files in `docs/architecture.md`.
