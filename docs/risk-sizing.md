# Risk sizing

**Sizing** turns an admitted entry into a quantity a venue takes: how much
to buy or sell so that being stopped out at the entry's stop costs about the
share of capital its profile's policy allows. This page is the contract of
it: what sizing is given, the rules in their order, how amounts are rounded,
which margin an entry draws on, what makes sizing refuse and what makes it
give no answer at all, and what is reported once an entry has filled.

It is implemented by `model::futures::sizing`, a pure function with no
store, no clock and no venue behind it, and held to the shared cases of
[`tests/fixtures/sizing_cases.json`](../tests/fixtures/sizing_cases.json),
which an independent model writes. `tests/sizing/` replays every case on the
model and under a policy read back from SQLite and from PostgreSQL, and
`tests/sizing/docs.rs` holds this page to the code and to those cases.

It implements ADR 0002 §6 and the price checks of §9 for the requirements
[automation-requirements.md](automation-requirements.md) lists as OBS-R018
to OBS-R028 with the decisions D04, D07, D10, D13, D14 and D15 (the
automation PRD (iteration 3), §2.1 on p.4 and §3 to §3.2 on pp.5–6). It is
the parity matrix's capability `futures.sizing`, on both venues, both
families and both sides. No command reaches it yet: the executor that sizes
an admitted entry and sends it is ALG-745's.

## Contents

- [The three answers](#the-three-answers)
- [What sizing is given](#what-sizing-is-given)
- [The rules](#the-rules)
- [Units](#units)
- [Rounding](#rounding)
- [The margin an entry draws on](#the-margin-an-entry-draws-on)
- [Refusals](#refusals)
- [What sizing cannot trust](#what-sizing-cannot-trust)
- [Just before submission](#just-before-submission)
- [After the fills](#after-the-fills)
- [What the budget is not](#what-the-budget-is-not)
- [Worked examples](#worked-examples)
- [The shared cases](#the-shared-cases)
- [What this leaves to later stories](#what-this-leaves-to-later-stories)

## The three answers

`SizingRequest::size` answers one of three things, and they are kept apart:

- **Sized.** A `SizedEntry`: the quantity to send, with every figure it was
  worked out from.
- **Refused.** A `SizingRefusal`: one of the parity matrix's refusals, a
  message that says what stands in the way, and the figures reached before
  it. An entry is refused, never shrunk to fit: sizing does not lower a
  quantity to meet a margin, a bracket or a maximum.
- **A fault.** A `SizingFault`: sizing could not work the answer out from
  what it was given, because an input is for something else, too old,
  missing, ambiguous or invalid. A fault is not a refusal and is not in the
  matrix's vocabulary. Nothing is sent on one; what it halts, and how it is
  recorded, is the executor's to decide (ALG-745, ALG-748).

The same request gives the same answer every time. Sizing reads nothing but
its request and changes nothing.

## What sizing is given

Everything is handed in. The policy comes from the entry's own
[snapshot](automation-policy.md#what-an-entry-is-held-to), taken when the
entry was admitted, so a setting changed since then resizes nothing.

- **The entry:** its side, the alert's expected price E, and the alert's own
  stop if it brought one.
- **The policy snapshot:** the capital B in the instrument's settlement
  asset, the risk percentage r, the fallback stop percentage p if the policy
  gives one, and the leverage.

And the rest, which a fault names by these words. What comes from the venue
comes with the time it was read:

<!-- begin: sizing inputs -->
- `rules`: the instrument as the venue lists it now: its tick, its
  market-order step, minimum and maximum, its minimum notional where it has
  one, and an inverse contract's face value.
- `account`: an account snapshot: the wallets and the margin pools. It is
  immutable: sizing judges the entry against the account as it stood at one
  moment.
- `brackets`: the account's leverage brackets for the instrument, each a
  maximum leverage and the largest position it allows, in the settlement
  asset.
- `fees`: the account's fee rates for the instrument. Sizing uses the taker
  rate.
- `pool`: the margin pool the entry draws on, which sizing finds in the
  account snapshot.
- `reservation`: the margin already reserved against that pool for the same
  account, in the pool's unit. Its caller totals it; sizing keeps no record.
- `usd_index`: the venue's index price of the settlement asset in USD, with
  the index's name and time, where the pool is the account's USD pool.
- `wallet`: the settlement asset's wallet, which sizing finds in the account
  snapshot.
- `prices`: the mark price and the best bid and ask, read just before
  submission.
- `fills`: the entry's fills, once it has some.
<!-- end: sizing inputs -->

Reservations that several entries race for are not sizing's: it is handed a
total, and whoever persists reservations arbitrates them (ALG-738).

## The rules

With E the alert's expected price, in this order:

1. **The budget** is `R = B × r`: the capital times the risk percentage.
   Capital is configured per settlement asset and never converted: an
   instrument whose settlement asset the profile holds no capital in is
   refused before sizing, when its snapshot is asked for.
2. **The stop.** The alert's own stop is used if it brought one, and it must
   lie on the protective side: below E for a long, above E for a short. A
   stop at E is on the wrong side too. Otherwise the policy's fallback
   percentage gives `E × (1 − p)` for a long and `E × (1 + p)` for a short.
   With neither, the entry is refused. The fallback never replaces a stop
   the alert brought, even one that is refused.
3. **The stop on its tick.** The stop is rounded to the instrument's tick
   away from the entry, down for a long and up for a short, so it is never
   tighter than asked. That is S′, and everything after uses S′.
4. **The quantity.**
   - A linear quantity is `R / |E − S′|`, in the base asset.
   - An inverse contract count is `R / (F × |1/E − 1/S′|)`, with F the
     contract's face value in USD.
   - Either is floored to the market-order step. A quantity is never
     rounded up, so its risk, recomputed from the floored quantity, is at or
     below R.
5. **The quantity's limits.** It must reach the market-order minimum and
   stay within the market-order maximum; a quantity of nothing is below
   every minimum. An entry is a market order, so where a venue lists
   different rules for limit orders, those decide nothing here.
6. **The notional** is `quantity × E` for linear and `contracts × F` for
   inverse, in the quote asset, and must reach the instrument's minimum
   notional where it has one. In the settlement asset it is the same for
   linear and that `÷ E` for inverse.
7. **The margin** is the notional in the settlement asset over the leverage,
   plus the taker fee on that notional twice, for opening and for closing.
   Leverage changes the margin and never the quantity.
8. **The bracket.** The position, in the settlement asset, must fit the
   largest cap among the brackets whose maximum leverage is at least the
   policy's. A leverage no bracket allows fits none.
9. **The pool.** The margin must fit what the entry's pool has free: its
   available margin less the reservations.
10. **The wallet.** The settlement asset's wallet must hold at least R,
    whatever funds the margin: a stop-out is paid in that asset. A wallet
    that holds R but less than B is allowed, with the warning
    `wallet_below_capital`.

No allowance is reserved for slippage. What an entry may slip before it is
sent is bounded by the drift check [just before
submission](#just-before-submission), and what it did slip is reported
[after the fills](#after-the-fills).

Of the leverage, sizing holds one thing: the position must fit the
account's brackets at it. Whether the venue takes that leverage for the
instrument at all, its range and its step, is not sizing's to say, and a
leverage the venue does not take is still sized. It is held where a
leverage is set: for an asset with settings of its own, when the setting is
made and whenever the profile is checked
([automation-policy.md](automation-policy.md#the-venues-say)); and for any
instrument, one trading on the defaults included, when the leverage is set
on the venue and read back, before any entry is sent (ADR 0002 §4; ALG-740,
ALG-762). None of the matrix's refusals names a leverage off the venue's
step, so an entry is not refused for one before then.

A sized entry carries these figures, and a refusal the ones reached before
it. The names are the ones the shared cases record them under:

<!-- begin: sizing figures -->
- `budget`: R, in the settlement asset.
- `stop`: S′, in the tick's own scale, with what was asked: the alert's
  stop or the fallback percentage.
- `qty`: the quantity, in the step's own scale: base asset for linear,
  contracts for inverse.
- `unit_loss`: what one unit loses from E to S′, in the settlement asset.
- `risk`: what the quantity loses from E to S′.
- `notional`: in the quote asset.
- `notional_settle`: in the settlement asset.
- `initial_margin`: the notional in the settlement asset over the leverage.
- `fee_allowance`: taker fees for opening and for closing.
- `required_margin`: the two together, in the settlement asset.
- `bracket_cap`: the largest position the leverage's brackets allow.
- `pool`: the unit of the pool the entry draws on.
- `required_in_pool`: the margin in the pool's unit.
- `free_margin`: what the pool has free: available less reserved.
<!-- end: sizing figures -->

## Units

Money carries its asset and a quantity its unit; the two families differ in
both, and nothing here mixes them.

| | Linear | Inverse |
| --- | --- | --- |
| Settled in | USDT | the coin |
| Quantity | base asset | whole contracts |
| Capital, budget, risk, margin, fees | USDT | the coin |
| Notional, minimum notional | USDT | USD |
| One unit's loss at the stop | `\|E − S′\|` | `F × \|1/E − 1/S′\|` |

The face value F is the venue's own, read from the instrument as its adapter
lists it: on Binance the contract size in its metadata, 100 USD for BTC and
10 USD for every other coin, and on Bybit 1 USD by a documented rule, since
no metadata field carries it. The tick, the steps and the limits are the
venue's too. So the same budget, entry and stop size differently on each:
the two venues round on different grids and, for inverse, count in
contracts of different worth. Equal risk rules do not make equal fills, or
equal profits.

## Rounding

Every decision is made on exact values: which side a stop is on, where a
floor falls, whether a margin fits. Nothing is rounded before it is
compared, and no amount ever passes through a binary float.

What is reported is rounded only where it has to be:

- a **stop** keeps its tick's scale, and a **quantity** its step's: a tick
  of `0.0100` gives a stop of `200.0000`. Where no exact decimal holds a
  value at that scale, as a tick or a step written with more places than it
  needs can make it, its trailing zeros are dropped; the filled quantity of
  a fill report keeps the finest scale of its fills on the same terms;
- every other **amount** is exact where it has no more decimal places than
  the number below, and rounded half to even at that many where it has
  more, as a division's result has to be somewhere; trailing zeros are
  dropped:

<!-- begin: amount places -->
12
<!-- end: amount places -->

A figure too large for an exact decimal, more than 96 bits once its zeros
are dropped, is a fault and never a rounded or a wrapped number.

## The margin an entry draws on

An account's margin is in pools, and the pools are the venue's own:

| Venue | Pool |
| --- | --- |
| Binance USDⓈ-M | the USDT pool, or the account's USD pool in multi-assets mode |
| Binance COIN-M | each coin's own pool |
| Bybit, isolated margin | each coin's own pool |
| Bybit, cross margin | the account's USD pool |

A venue's adapter reports the pools the account's margin mode gives it, so
an entry finds exactly one: its settlement asset's own pool, or the
account's USD pool. `margin_pool` names it, for sizing and for whoever
totals the reservations against it. An account that reports both, or
neither, is a fault: sizing never chooses a pool.

**An asset's own pool** is in the settlement asset, as the margin is, and
nothing is converted.

**The USD pool** is in USD, and the margin is not. The margin is measured in
USD at the venue's own index price of the settlement asset: the instrument's
index price for a coin-settled contract, the venue's index of USDT in USD
for a USDT-settled one. That is the one conversion sizing makes, and it is
the venue's figure, never a rate from anywhere else. It is handed in as a
`UsdIndex` with the asset, the price, the venue's name for the index and
the time it was read, and a sized entry keeps all four with the pool it
measured. A USD pool with no index price, one for another asset, one that
does not name its index or one that is too old is a fault.

Capital is never converted at all. It is configured in each settlement
asset, and an entry that settles in an asset without capital is refused.

## Refusals

A refusal is one of the parity matrix's, in the matrix's order, which is the
order sizing gives them in: an entry that breaks several rules is refused
for the first.

<!-- begin: sizing refusals -->
- `no_capital_in_settlement_asset`: the profile holds no capital in the
  asset the instrument settles in. The policy refuses this when the entry's
  snapshot is asked for, before sizing.
- `stop_wrong_side`: the alert's stop is not on the protective side of E.
- `no_stop`: the alert brings no stop and the policy gives no fallback, or
  the stop rounds down to no price at all on the instrument's tick.
- `below_min_qty`: the floored quantity is below the market-order minimum.
- `above_market_max`: the quantity is above the market-order maximum.
- `below_min_notional`: the quantity is worth less than the instrument's
  minimum notional.
- `above_leverage_bracket`: the position is above the largest cap its
  leverage's brackets allow, or no bracket allows the leverage.
- `insufficient_margin`: the margin is more than the pool has free once the
  reservations are taken off.
- `settlement_wallet_below_risk`: the settlement asset's wallet holds less
  than R.
- `stop_crossed`: at submission, the mark price or the touch is at or past
  the stop.
- `risk_drift`: at submission, the risk at the expected fill is above the
  drift limit.
<!-- end: sizing refusals -->

Some things can never be refused, because they can never be handed to
sizing: a price that is zero or below, a risk percentage outside (0, 100], a
stop percentage outside (0, 100), a leverage below 1, a capital that is not
above zero, an inverse instrument without its face value. Each is refused
where it is built or read back.

## What sizing cannot trust

Before anything is worked out, sizing looks at what it was given, in one
fixed order, and the first thing it cannot trust is the fault it answers.
A fault has a kind and names the input:

<!-- begin: fault kinds -->
- `mismatched`: the input is for something else than the entry: another
  instrument, another account, another asset.
- `stale`: the input is older than its limit, or timed further ahead of
  the clock than that.
- `missing`: the input, or a part sizing needs of it, is not there.
- `ambiguous`: the input gives two answers where sizing needs one.
- `invalid`: the input says something that cannot be so.
- `out_of_range`: a figure came out larger than an exact decimal holds. It
  names no input.
<!-- end: fault kinds -->

The order, for a request:

<!-- begin: fault order -->
- `mismatched rules`: the rules are another instrument's than the one the
  entry was admitted on.
- `mismatched account`: the account snapshot is another account's than the
  one the entry's profile trades on.
- `stale rules`: the rules were read too long ago.
- `missing brackets`: the venue gave no leverage brackets.
- `invalid brackets`: a bracket's cap is in another asset than the
  settlement asset, or is not above zero.
- `stale brackets`: the brackets were read too long ago.
- `invalid fees`: the taker fee is below zero.
- `stale fees`: the fee rates were read too long ago.
- `stale account`: the account snapshot was taken too long ago.
- `missing pool`: the account reports no pool the entry could draw on.
- `ambiguous pool`: the account reports more than one.
- `invalid pool`: the pool's available margin is not stated in the pool's
  own unit.
- `invalid reservation`: the reservations are not stated in the pool's
  unit, or are below zero.
- `missing usd_index`: the pool is in USD and no index price was given.
- `mismatched usd_index`: the index price is another asset's.
- `invalid usd_index`: the index price does not name its index.
- `stale usd_index`: the index price was read too long ago.
- `ambiguous wallet`: the account reports more than one wallet of the
  settlement asset.
<!-- end: fault order -->

And for the prices handed in at submission:

<!-- begin: submission fault order -->
- `mismatched prices`: the prices are another instrument's.
- `missing prices`: the snapshot lacks a bid, an ask, or the time its book
  was read.
- `invalid prices`: the bid is above the ask.
- `stale prices`: the mark price or the book was read too long ago.
<!-- end: submission fault order -->

An account that lists no wallet for the settlement asset holds none of it:
that is a refusal, `settlement_wallet_below_risk`, and not a fault.

**Freshness.** An input is fresh while its time is within its limit of the
clock, on either side. One timed further ahead of the clock than its limit
says the clocks disagree, and is trusted no more than an old one. The
limits, in milliseconds, are these unless the caller passes its own:

<!-- begin: freshness limits -->
- `prices`: 5000, for the mark price, the book and a USD index price.
- `account`: 10000, for the account snapshot, with its wallets and pools.
- `rules`: 3600000, for the instrument's rules, the leverage brackets and
  the fee rates.
<!-- end: freshness limits -->

## Just before submission

A sized entry is held once more to prices read just before it is sent
(`SizedEntry::at_submission`), since the alert's expected price may be
seconds old:

- **The stop has been reached** when the mark price, which is what triggers
  the stop, or the touch, the bid for a long and the ask for a short, is at
  or past S′. The entry is refused as `stop_crossed`.
- **The risk has drifted** when the quantity's loss from the expected fill,
  the ask for a long and the bid for a short, to S′ is above this many
  times R. The entry is refused as `risk_drift`. A loss of exactly that is
  within the limit:

<!-- begin: drift limit -->
1.25
<!-- end: drift limit -->

The quantity is not resized at the new price. An entry is sized once, from
E, and then either sent as sized or refused.

## After the fills

Sizing works from the alert's expected price; a venue fills where the
market is. The two are kept apart. `SizedEntry::fill_risk` takes the
entry's fills, each a price and a quantity, and reports:

<!-- begin: fill report -->
- `filled`: the quantity filled: at most the quantity sized, and less after
  a partial fill.
- `expected`: what the filled quantity would have lost from E to S′.
- `actual`: what it loses from where it filled, each fill at its own price.
  A fill at or past the stop has nothing left to lose to it, and counts
  for nothing.
- `deviation`: `actual − expected`: above zero when the fills were worse
  than E, below when they were better.
- `budget`: R.
- `exceeds_budget`: whether `actual` is above R.
- `past_stop`: whether any fill was at or past the stop.
<!-- end: fill report -->

Fills that cannot be the entry's are a fault: none at all, or none that
filled anything, a quantity in another unit than the instrument trades in,
or more filled than was sized.

## What the budget is not

R is the loss an entry is sized to if its stop fills at its price. It is
not a limit on the loss:

- a stop is a market order once it triggers, and a gap fills it past its
  price;
- fees are charged on both legs, and funding accrues while the position is
  open;
- the entry itself may fill away from E, which is what the report above
  measures.

Sizing bounds none of these and claims none bounded. The venue's own
rejection also stays authoritative: its margin and its limits are checked
here as of a snapshot, and the venue checks them again when the order
arrives.

**The PRD's worked example.** The automation PRD's trade on p.9 fits a
budget of 150 USDT, 1 % of 15,000 USDT, in every figure but its risk line,
which says 15.00. The cases keep both readings and neither is the PRD's
table:

<!-- begin: prd example -->
- `prd-p9-binance-linear-long`: 4.38 SOL, risking 149.9274 USDT of a 150
  USDT budget.
- `prd-p9-literal-15-usdt`: 0.43 SOL, risking 14.7189 USDT of a 15 USDT
  budget.
<!-- end: prd example -->

## Worked examples

One sized entry for each venue, family and side, from the shared cases. The
risk is in the settlement asset, and so is the margin.

<!-- begin: worked examples -->
| Cell | Case | E | S′ | Quantity | Risk | Margin |
| --- | --- | --- | --- | --- | --- | --- |
| `futures.sizing/binance/linear/long` | `prd-p9-binance-linear-long` | 234.23 | 200.0000 | 4.38 | 149.9274 USDT | 206.2114074 USDT |
| `futures.sizing/binance/linear/short` | `linear-short-percent-stop` | 84123.45 | 86647.20 | 0.039 | 98.42625 USDT | 331.36226955 USDT |
| `futures.sizing/binance/inverse/long` | `binance-inverse-long` | 84123.4 | 81599.6 | 135 | 0.004963452356 BTC | 0.032256185556 BTC |
| `futures.sizing/binance/inverse/short` | `binance-inverse-short` | 3012.34 | 3105.56 | 100 | 0.009964721966 ETH | 0.06672553563 ETH |
| `futures.sizing/bybit/linear/long` | `prd-p9-bybit-linear-long` | 234.23 | 200.000 | 4.3 | 147.189 USDT | 202.5457079 USDT |
| `futures.sizing/bybit/linear/short` | `bybit-linear-short` | 234.23 | 243.600 | 16.0 | 149.92 USDT | 753.658448 USDT |
| `futures.sizing/bybit/inverse/long` | `bybit-inverse-long` | 84123.4 | 81599.60 | 13599 | 0.004999851007 BTC | 0.032508896454 BTC |
| `futures.sizing/bybit/inverse/short` | `bybit-inverse-short` | 187.45 | 191.20 | 1911 | 0.199948717147 SOL | 3.409453720992 SOL |
<!-- end: worked examples -->

## The shared cases

[`tests/fixtures/sizing_cases.json`](../tests/fixtures/sizing_cases.json) is
written by `python3 -I spikes/futures-contract/sizing.py --write
tests/fixtures/sizing_cases.json`, whose `--doc docs/risk-sizing.md` also
rewrites the lists below. It is an independent model that computes every
case twice, in `Decimal` at 60 digits and again in exact fractions with no
arithmetic in common, and holds each result to the rules' promises and to
figures worked out by hand. It shares no code with the Rust. The same
cases are
[`spikes/futures-contract/sizing-cases.json`](../spikes/futures-contract/sizing-cases.json),
the file ADR 0002 links; the crate's tests read the copy under `tests/`,
since `spikes/` is not packaged. The fixture's format is:

<!-- begin: fixture format -->
2
<!-- end: fixture format -->

Each case hands sizing its inputs in the shared model's stored form, what a
venue's adapter makes of its instrument metadata, its leverage tiers and an
account, and records the whole answer. Besides the cases the fixture holds
the instruments and tiers they name, the fill reports, and the exact
arithmetic a fraction helper must answer whatever it is built on.

<!-- begin: sizing cases -->
- `prd-p9-binance-linear-long`: The PRD's p.9 trade with a 150 USDT budget,
  1 % of 15,000: it reproduces every p.9 figure but the risk line
- `prd-p9-bybit-linear-long`: The same trade on Bybit, whose 0.1 step keeps
  less of the budget
- `prd-p9-literal-15-usdt`: The p.9 risk line read literally, 15 USDT: 0.43821
  SOL before the 0.01 step, 0.43 after
- `linear-short-percent-stop`: SHORT with the 3 % fallback stop, rounded up to
  the tick
- `binance-inverse-long`: COIN-M LONG, 100 USD contracts; the wallet holds
  less than the capital, which only warns
- `binance-inverse-short`: COIN-M SHORT, 10 USD contracts, with an absolute
  stop rounded up to the tick
- `bybit-inverse-long`: Bybit inverse LONG, 1 USD contracts: the trade of
  binance-inverse-long, about 100 times the contracts (13,599 against 135),
  since the finer grid keeps more of the budget
- `bybit-inverse-short`: Bybit inverse SHORT with the 2 % fallback stop
- `refuse-wrong-side-stop`: A LONG whose stop is above its entry
- `refuse-no-stop`: Neither a payload stop nor a fallback percentage
- `refuse-below-min-qty`: A budget too small for one step is refused, never
  rounded up
- `refuse-below-min-notional`: Enough for two steps, but 4.6846 USDT is below
  the 5 USDT minimum notional
- `refuse-above-market-max`: 198.117 BTC is above the 120 BTC market-order
  maximum
- `refuse-above-leverage-bracket`: At 100x Bybit's lowest risk tier caps
  SOLUSDT at 50,000 USDT, far below this 190,428.99 USDT position
- `leverage-bracket-fits-at-20x`: The same position at 20x, whose tiers reach
  10,000,000 USDT
- `refuse-inverse-above-leverage-bracket`: At 50x Bybit caps SOLUSD at 200
  SOL, and 75,135 contracts are 400.83 SOL
- `refuse-insufficient-margin`: 331.36226955 USDT needed; 400 available minus
  a 100 USDT reservation leaves 300
- `refuse-linear-wallet-below-risk`: Cross margin funded by other collateral:
  15,000 USD of margin, but 100 USDT cannot pay a 150 USDT stop-out
- `refuse-wallet-below-risk`: An inverse stop-out the BTC wallet could not pay
- `refuse-capital-currency`: An ETH-settled instrument with capital configured
  only in BTC
- `submit-drift-accepted`: At an ask of 243.00 the risk is 18.49, within 1.25
  x 15 = 18.75
- `submit-drift-refused`: At an ask of 244.00 the risk is 18.92, above 18.75
- `submit-book-crossed`: A SHORT whose stop the ask has already reached
- `submit-mark-crossed`: A LONG whose book is still above its stop, but whose
  mark price, which triggers the stop, is below it
- `bybit-linear-short`: Bybit linear SHORT with the 4 % fallback stop:
  243.5992 rounds up to 243.60
- `signal-stop-overrides-the-fallback`: The alert's own stop is used, though
  the asset has a 3 % fallback that would give 227.20
- `refuse-wrong-side-stop-despite-the-fallback`: A wrong-side stop in the
  alert refuses the entry; the fallback never replaces it
- `refuse-short-wrong-side-stop`: A SHORT whose stop is below its entry
- `refuse-stop-at-the-entry`: A stop at the entry price protects nothing: it
  is on the wrong side too
- `refuse-stop-rounds-to-nothing`: A stop below one tick rounds down to no
  price at all, so the entry has no stop the venue can hold
- `linear-long-stop-off-the-tick`: A LONG stop of 200.008 rounds down to
  200.00, away from the entry, never to the nearer 200.01
- `linear-exact-budget`: 150 / 37.50 is 4 exactly: the floor keeps all four,
  and the risk equals the budget
- `inverse-exact-budget`: 0.25 / (1/80 - 1/100) is 100 contracts exactly, and
  a third of a coin of margin at 3x
- `refuse-margin-short-by-less-than-a-reported-place`: The same entry needs a
  third of a coin and a little, reported as 0.334433333333: a pool with
  exactly that much free is short by the thirds the figure leaves off
- `linear-margin-at-3x`: A third of 1,007.189 USDT has no end: the margin is
  rounded half to even at 12 places
- `margin-halfway-rounds-to-the-even-digit-below`: An eighth of
  1,023.5851000437 USDT is 127.9481375054625, halfway at the 12th place: it
  rounds to the even digit, here the one below
- `margin-halfway-rounds-to-the-even-digit-above`: An eighth of
  1,023.5851001311 USDT is 127.9481375163875: the even digit is the one above
- `min-qty-exactly-met`: One step, 0.001 BTC, is the minimum quantity: allowed
- `market-max-exactly-met`: 302,850 / 2,523.75 is 120 BTC exactly, the
  market-order maximum: allowed
- `min-notional-exactly-met`: 0.02 SOL at 250.00 is 5 USDT, the minimum
  notional: allowed
- `bracket-cap-exactly-met`: 200 SOL at 250.00 is 50,000 USDT, all that
  Bybit's 100x tier holds: allowed
- `refuse-one-step-above-the-bracket`: One more step, 200.1 SOL, is 50,025
  USDT: above the 100x tier
- `refuse-bracket-short-by-less-than-a-reported-place`: 135 contracts are
  0.16047853510438... BTC, reported as 0.160478535104: a bracket capped at
  exactly that figure does not hold them
- `refuse-leverage-above-every-tier`: No tier allows 150x: there is no cap to
  fit under
- `margin-exactly-met`: 190.95 USDT needed, and 290.95 available less a 100
  USDT reservation leaves exactly that: allowed
- `refuse-margin-one-cent-short`: With 290.94 available the same entry is one
  cent short
- `refuse-reservations-above-the-pool`: Reservations of 700 USDT against 400
  available leave 300 less than nothing, which is not 300 free
- `refuse-pool-in-deficit`: A pool the venue reports 500 USDT short has
  nothing free, however large the shortfall
- `zero-taker-fee`: A taker fee of nothing is a fee rate like any other: no
  allowance, and the margin alone must fit
- `index-not-needed-is-not-looked-at`: An entry funded by its asset's own pool
  needs no index price: one handed in anyway, old and another asset's, changes
  nothing
- `wallet-exactly-the-risk`: A wallet of 150 USDT can pay a 150 USDT stop-out:
  allowed, with the warning that it is below the capital
- `refuse-wallet-one-cent-below-the-risk`: A wallet of 149.99 USDT cannot
- `wallet-in-use-still-holds-the-risk`: What counts is what the wallet holds,
  not what of it is free: 15,000 USDT held with 100 free pays a 150 USDT
  stop-out
- `refuse-wallet-in-debt`: A wallet 200 USDT in debt holds less than nothing,
  which is not 200
- `several-wallets-and-pools`: A COIN-M account with a pool and a wallet for
  each coin: the ETH ones fund an ETH-settled entry
- `refuse-inverse-below-one-contract`: 0.00003 BTC of budget is less than one
  100 USD contract risks: refused, never rounded up to one
- `inverse-small-budget-on-one-usd-contracts`: The same budget on Bybit's 1
  USD contracts sizes 81 of them: the face value is the venue's
- `refuse-inverse-insufficient-margin`: A COIN-M SHORT whose margin, in ETH,
  is more than the ETH pool has free
- `usd-pool-linear-fits`: Bybit cross margin: 202.5457079 USDT of margin is
  202.62672618316 USD at an index of 1.0004, within 202.63
- `refuse-usd-pool-short-by-the-index`: With 202.60 USD free the same entry is
  refused, though 202.5457079 would fit unconverted
- `usd-pool-below-par-fits`: Binance multi-assets mode: 331.36226955 USDT is
  331.29599709609 USD at 0.9998, within the 331.30 left
- `usd-pool-inverse-fits`: Bybit cross margin, coin-settled: the BTC the entry
  needs is measured in USD at the BTC index price
- `refuse-usd-pool-inverse-short`: The same entry with 2,700 USD free
- `refuse-no-settlement-wallet`: Margin funded entirely by other collateral:
  the account holds no USDT at all to pay a stop-out with
- `drift-exactly-at-the-limit`: At an ask of 250.00 the risk is 25, exactly
  1.25 x 20: not above it, so accepted
- `refuse-drift-one-tick-over`: One tick higher, 250.01, risks 25.005
- `refuse-mark-exactly-at-the-stop`: A mark price at the stop has reached it
- `refuse-bid-exactly-at-the-stop`: So has a bid at the stop
- `one-tick-clear-of-the-stop`: A bid and a mark one tick above the stop have
  not
- `locked-book-is-a-book`: A bid equal to the ask is still a book: only a bid
  above the ask is not
- `budget-with-more-places-than-reported`: A budget of 1,000.0000000000005
  USDT is reported as 1,000, and every comparison is made on what it is
- `quantity-from-the-budget-before-rounding`: A budget of 999.9999999999995
  USDT is reported as 1,000 too, and sizes 0.999 BTC at a loss of 1,000 a
  unit: the 1,000 reported would size a step more than the budget pays for
- `drift-limit-from-the-budget-before-rounding`: From an ask of
  85,250.000000000000625 the risk is exactly 1.25 times that budget, and not
  above it; it would be above 1.25 times the 1,000 reported
- `refuse-wallet-below-the-budget-before-rounding`: A wallet of 1,000 USDT is
  below that budget, though not below the 1,000 reported
- `short-submit-accepted`: A SHORT fills at the bid: 2.5 SOL from 233.90 to
  the 240.00 stop risk 15.25, within 18.75
- `refuse-ask-exactly-at-the-stop`: A SHORT whose stop the ask has just
  reached
- `refuse-short-mark-exactly-at-the-stop`: A SHORT whose stop the mark price
  has just reached
- `inverse-submit-accepted`: Inverse risk at the fill is in the coin: 13,599
  contracts from an ask of 84,150.5
- `refuse-inverse-drift`: From an ask of 84,800.0 the same contracts risk more
  than 1.25 x 0.005 BTC
- `inverse-short-submit-accepted`: An inverse SHORT fills at the bid
- `refuse-inverse-short-mark-at-the-stop`: An inverse SHORT whose stop the
  mark price has reached
- `order-01-stop-wrong-side`: A stop on the wrong side is refused before
  anything is sized
- `order-02-no-stop`: No stop at all, likewise
- `order-03-below-min-qty`: With a 10 % stop, a budget of 0.1 USDT sizes
  nothing
- `order-04-above-market-max`: A budget of 10,000,000 USDT sizes 426,803.24
  SOL, above the market-order maximum
- `order-05-below-min-notional`: A budget of 0.5 USDT sizes 0.02 SOL, worth
  less than the minimum notional
- `order-06-above-leverage-bracket`: A budget of 150 USDT sizes 6.40 SOL,
  which no bracket allows at 150x
- `order-07-insufficient-margin`: At 5x the bracket holds it, and the pool has
  nothing free
- `order-08-settlement-wallet-below-risk`: With margin free, the account still
  holds no USDT to pay a stop-out
- `order-09-stop-crossed`: Sized; at submission the mark price is below the
  stop, and the ask far above the entry
- `order-10-risk-drift`: With the mark price back above the stop, the ask is
  still far above the entry
- `fault-rules-of-another-instrument`: The rules handed in are BTCUSDT's, and
  the entry was admitted on SOLUSDT
- `fault-account-of-another-key`: The account snapshot is another account's
  than the profile's
- `fault-rules-too-old`: Instrument rules read an hour and a millisecond ago
- `fresh-rules-at-the-limit`: Instrument rules read exactly an hour ago are
  still used
- `fault-no-brackets`: The account's leverage brackets for the instrument are
  empty
- `fault-brackets-in-another-asset`: A bracket's cap is in USD, and the
  instrument settles in USDT
- `fault-bracket-without-a-cap`: A bracket whose cap is zero holds nothing
- `fault-brackets-too-old`: Leverage brackets read an hour and a millisecond
  ago
- `fresh-brackets-at-the-limit`: Leverage brackets read exactly an hour ago
  are still used
- `fault-taker-fee-below-zero`: A taker fee below zero would make the fee
  allowance a credit
- `fault-fees-too-old`: Fee rates read an hour and a millisecond ago
- `fresh-fees-at-the-limit`: Fee rates read exactly an hour ago are still used
- `fault-account-too-old`: An account snapshot ten seconds and a millisecond
  old
- `fresh-account-at-the-limit`: An account snapshot exactly ten seconds old is
  still used
- `fault-account-ahead-of-the-clock`: An account snapshot timed more than ten
  seconds after the clock: the clocks disagree
- `fresh-account-ahead-at-the-limit`: One timed exactly ten seconds after the
  clock is still used
- `fault-no-pool`: The account reports no margin pool the entry could draw on
- `fault-two-pools`: The account reports both a USDT pool and a USD pool:
  which one funds the entry is not sizing's to guess
- `fault-pool-in-another-unit`: A USD pool whose available margin is stated in
  USDT
- `fault-reservation-below-zero`: A reservation below zero would add margin
- `fault-reservation-in-another-unit`: Reservations stated in USD against a
  USDT pool
- `fault-two-settlement-wallets`: Two USDT wallets in one snapshot
- `fault-usd-pool-without-an-index`: A USD pool, and no index price to measure
  USDT against it
- `fault-index-of-another-asset`: The index price handed in is BTC's, and the
  entry settles in USDT
- `fault-index-without-a-source`: An index price that does not say where it
  came from
- `fault-index-too-old`: An index price five seconds and a millisecond old
- `fresh-index-at-the-limit`: An index price exactly five seconds old is still
  used
- `fault-prices-of-another-instrument`: The prices handed in at submission are
  BTCUSDT's
- `fault-no-bid`: A price snapshot without a bid
- `fault-no-quote-time`: A book that does not say when it was read
- `fault-book-crossed`: A bid above the ask is not a book
- `fault-mark-too-old`: A mark price five seconds and a millisecond old
- `fresh-mark-at-the-limit`: A mark price exactly five seconds old is still
  used
- `fault-quote-too-old`: A book five seconds and a millisecond old
- `fresh-quote-at-the-limit`: A book exactly five seconds old is still used
- `fault-mark-ahead-of-the-clock`: A mark price timed more than five seconds
  after the clock
- `chain-01-rules-of-another-instrument`: The rules are another instrument's,
  and everything looked at after it wrong as well
- `chain-02-account-of-another-key`: The account snapshot is another
  account's, and everything looked at after it wrong as well
- `chain-03-rules-too-old`: The rules are too old, and everything looked at
  after it wrong as well
- `chain-04-no-brackets`: There are no leverage brackets, and everything
  looked at after it wrong as well
- `chain-05-brackets-in-another-asset`: A bracket's cap is in another asset,
  and everything looked at after it wrong as well
- `chain-06-brackets-too-old`: The brackets are too old, and everything looked
  at after it wrong as well
- `chain-07-taker-fee-below-zero`: The taker fee is below zero, and everything
  looked at after it wrong as well
- `chain-08-fees-too-old`: The fee rates are too old, and everything looked at
  after it wrong as well
- `chain-09-account-too-old`: The account snapshot is too old, and everything
  looked at after it wrong as well
- `chain-10-no-pool`: The account has no pool for the entry, and everything
  looked at after it wrong as well
- `chain-11-two-pools`: The account has two pools for the entry, and
  everything looked at after it wrong as well
- `chain-12-pool-in-another-unit`: The USD pool is stated in USDT, and
  everything looked at after it wrong as well
- `chain-13-reservation-in-another-unit`: The reservations are stated in USDT,
  not USD, and everything looked at after it wrong as well
- `chain-14-no-index`: There is no index price for the USD pool, and
  everything looked at after it wrong as well
- `chain-15-index-of-another-asset`: The index price is another asset's, and
  everything looked at after it wrong as well
- `chain-16-index-without-a-source`: The index price names no source, and
  everything looked at after it wrong as well
- `chain-17-index-too-old`: The index price is too old, and everything looked
  at after it wrong as well
- `chain-18-two-settlement-wallets`: The account has two USDT wallets, and
  everything looked at after it wrong as well
- `chain-submit-01-prices-of-another-instrument`: The prices are another
  instrument's, and everything looked at after it wrong as well
- `chain-submit-02-no-bid`: The book has no bid, and everything looked at
  after it wrong as well
- `chain-submit-03-book-crossed`: The bid is above the ask, and everything
  looked at after it wrong as well
- `chain-submit-04-mark-too-old`: The mark price is too old, and everything
  looked at after it wrong as well
<!-- end: sizing cases -->

And the reports after the fills:

<!-- begin: fill cases -->
- `fill-at-the-expected-price`: Filled whole at E: the risk is the one sized
- `fill-exactly-the-budget`: A fill that risks the whole budget and no more
  does not exceed it
- `fill-worse-than-expected`: Filled 0.27 higher: 151.11 USDT at risk, above
  the 150 USDT budget
- `fill-better-than-expected`: Filled 0.23 lower: less at risk than sized
- `partial-fill-at-two-prices`: 3.38 of 4.38 SOL filled, at two prices: each
  fill counts at its own
- `fill-past-the-stop`: A fill at or below a LONG's stop has nothing left to
  lose to it, and says so
- `fill-exactly-at-the-stop`: A fill at the stop's own price has reached it:
  nothing left to lose, and past it
- `short-fill-exactly-at-the-stop`: The same for a SHORT, whose stop is above
- `fill-exactly-the-budget-before-rounding`: A fill that risks
  1,000.0000000000005 USDT of a budget of that much does not exceed it, though
  both are reported as 1,000
- `fills-written-with-more-places-than-their-sum-can-keep`: Two fills of 5
  written with 28 decimal places add up to 10, which no exact decimal holds at
  28 places: it is reported as 10, never as out of range
- `short-fill-lower-than-expected`: A SHORT filled below E has further to its
  stop
- `bybit-short-fill`: The Bybit SHORT, filled at two prices
- `inverse-long-two-fills`: Inverse fills are summed in the coin, each at its
  own price
- `binance-inverse-long-fill`: 100 USD contracts, filled a little below E
- `binance-inverse-short-fill`: 10 USD contracts sold above E: a SHORT filled
  better
- `inverse-short-fill`: 1 USD contracts sold below E: further to the stop
- `fault-no-fills`: No fill, no risk to report
- `fault-nothing-filled`: Fills that add up to nothing
- `fault-filled-more-than-sized`: More filled than the entry was sized for:
  not this entry's fills
- `fault-fill-in-another-unit`: A fill counted in contracts on a linear entry
<!-- end: fill cases -->

## What this leaves to later stories

- **The executor.** Sizing an admitted entry, holding it to fresh prices,
  sending it and recording the sizing with the intent are ALG-745's. So is
  what a fault halts and how it is recorded (ALG-745, ALG-748).
- **Reservations.** Sizing is handed the total already reserved against a
  pool. Keeping reservations in a store, and deciding between entries that
  race for the same margin, are ALG-738's.
- **The venues' own figures.** The rules, the brackets, the fee rates, the
  account's pools and the index prices come from each venue's adapter
  (ALG-739 and ALG-740 for Bybit, ALG-759, ALG-760 and ALG-762 for
  Binance). Until an adapter serves them, the cases are sized from values
  recorded from the venues' public metadata, with illustrative fees and,
  for Binance, illustrative brackets.
- **Evidence on live venues.** The parity matrix's cells for
  `futures.sizing` take their evidence from the conformance story
  (ALG-742).
- **Messages.** Telling the operator what was sized and what filled is
  ALG-751's.
