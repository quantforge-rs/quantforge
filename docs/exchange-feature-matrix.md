# Exchange feature matrix

What each registered exchange supports, and how its connector fares
against the conformance suite in
[`src/exchange/conformance/`](../src/exchange/conformance/mod.rs), whose
module docs state the contract. The results below replay each exchange's
committed fixtures; the scheduled `conformance` workflow runs the same
checks live against each testnet every day.

This page is generated: refresh it with
`UPDATE_SNAPSHOTS=1 cargo test --test conformance`, never by hand. To add
an exchange, see [adding-an-exchange.md](adding-an-exchange.md).

## Capabilities

| Capability | binance |
| --- | --- |
| market orders sized by base quantity | yes |
| market orders sized by quote quantity | yes |
| client order ids | yes |
| candles per request, at most | 1000 |
| symbol rules | yes |
| server time | yes |
| candle stream | no |
| account and order stream | no |

## Conformance

| Check | Verified live | binance |
| --- | --- | --- |
| `market_data.server_time`: the exchange clock reads UTC milliseconds and never runs backwards | testnet | pass |
| `market_data.candle_normalization`: candles sit on the interval grid with `close = open + step - 1`, ascending and unique, with consistent prices on the tick grid | testnet | pass |
| `market_data.closed_bars`: only the newest candle may still be forming, and it keeps its full-interval close time | testnet | pass |
| `market_data.kline_bounds`: both window bounds are inclusive on open time, `limit` is honored, and a limit above the maximum is capped | testnet | pass |
| `market_data.kline_paging`: paging by last open + step reproduces a window exactly, decimals included | testnet | pass |
| `market_data.kline_gaps`: a window without bars is empty, and one opening before the first bar skips to the oldest | testnet | pass |
| `market_data.symbol_rules`: rule limits are positive, minimums stay within maximums, and a quantity step and a price tick are present | testnet | pass |
| `orders.lifecycle`: a market buy fills with its fills reported, reads back by both ids, is not open, refuses a cancel, shows in trades and balances, and sells back rounded to the step | testnet | pass |
| `orders.rule_violations`: an order below the minimum notional or off the quantity step fails as `rule_violation` | testnet | pass |
| `errors.insufficient_balance`: an order beyond the free balance fails as `insufficient_balance` | testnet | pass |
| `errors.unknown_symbol`: an unlisted symbol fails as `unknown_symbol` | testnet | pass |
| `errors.unknown_order`: looking up or cancelling an order that never existed fails as `unknown_order` | testnet | pass |
| `errors.unauthorized`: credentials the exchange never issued fail as `unauthorized` | testnet | pass |
| `errors.clock_skew`: a request outside the exchange's time window fails as `clock_skew` | fixtures only | pass |
| `errors.rate_limited`: rate-limit and ban responses fail as `rate_limited` | fixtures only | pass |
| `errors.unavailable`: an exchange refusing work it did not start fails as `unavailable`, safe to retry | fixtures only | pass |
| `errors.outcome_unknown`: an exchange failing mid-request fails as `outcome_unknown`, so an order is looked up before it is sent again | fixtures only | pass |
| `errors.malformed_response`: a response that cannot be read fails as `invalid_response` | fixtures only | pass |

## Fixtures

| Exchange | Recorded | Hand-written | Recorded from | Last recorded |
| --- | --- | --- | --- | --- |
| binance | 13 | 5 | `https://testnet.binance.vision/` | 2026-10-20 |

A recorded fixture is the transcript of a passing live run against the
testnet, replayed request for request. A hand-written one scripts a fault
no testnet produces on demand — clock skew, rate limits, outages, garbled
responses — from the exchange's documented error responses, so those
checks run against fixtures only.

## Not covered yet

- Applying the price tick to orders: the connectors place market orders
  only, so no order carries a price. The tick is checked against candle
  prices instead.
- Compensating clock drift: a request outside the exchange's time window
  fails as `clock_skew`, and resynchronizing is the caller's job.
- Streams: no connector pushes candles or account updates yet.
