# TradingView alert payloads

A **signal** is the message of one TradingView alert: an entry or an exit on
one futures instrument, for one automation profile. This page is the
contract of it (**signal version 1**): what a payload is, what it can never
do, how a delivery is identified, what it resolves to for the profile its
webhook URL selects, which position an exit closes, and what a delivery is
answered with. The pieces:

- the wire format, `protocol::signal`, whose rustdoc is the format's
  reference;
- its shape for other tools, [`signal-schema.json`](signal-schema.json);
- the rules, `model::futures`' signal types, and `automation::SignalService`
  over a store's policies;
- the alert templates, [below](#the-alert-templates);
- the shared cases of
  [`tests/fixtures/signal_cases.json`](../tests/fixtures/signal_cases.json),
  which an independent Python model writes.

`tests/signals/` holds this page, the schema and the code to those cases.

It implements ADR 0002 §5 and §9 (ALG-737 in the parity matrix) for the
requirements [automation-requirements.md](automation-requirements.md) lists
as OBS-R004 and OBS-R008, with its part of OBS-R005 and OBS-R024 and the
decisions D02 and D06 (the automation PRD (iteration 3), §2 to §2.2 on
pp.3–4, §3.2 on pp.5–6 and §7 on p.13). The webhook route that
authenticates a delivery, records it and answers it is ALG-743's; the store
that keeps a signal and its orders is ALG-738's.

## Contents

- [How an alert arrives](#how-an-alert-arrives)
- [The payload](#the-payload)
- [Strict by design](#strict-by-design)
- [What a payload cannot do](#what-a-payload-cannot-do)
- [Identity](#identity)
- [Deliveries](#deliveries)
- [Resolving a signal](#resolving-a-signal)
- [Entries, exits and positions](#entries-exits-and-positions)
- [The receipt](#the-receipt)
- [The alert templates](#the-alert-templates)
- [The schema](#the-schema)
- [SQLite and PostgreSQL](#sqlite-and-postgresql)
- [The shared cases](#the-shared-cases)
- [What TradingView leaves open](#what-tradingview-leaves-open)
- [What this leaves to later stories](#what-this-leaves-to-later-stories)

## How an alert arrives

TradingView delivers an alert by `POST`ing the alert's message, as the
whole body, to the alert's webhook URL
([futures-venues.md](futures-venues.md#tradingview-webhooks) records what
its documentation says, and when it was read). Three of its facts shape the
contract:

- **It signs nothing.** A webhook carries no signature and no header of the
  sender's choosing, and TradingView tells users to keep secrets out of the
  message. So a payload authenticates nothing: the URL does (ADR 0002 §15),
  and a payload holds no credential.
- **It sends again.** A `5xx` answer other than `504` is sent again after 5
  seconds, up to three times. So a delivery has an identity that survives a
  resend, built only from what TradingView renders the same way each time.
- **It waits 3 seconds.** A delivery is answered once it is recorded, not
  once it is executed.

## The payload

A payload is one JSON object, in UTF-8.

<!-- begin: signal version -->
1
<!-- end: signal version -->

<!-- begin: signal fields -->
- `version`: the number `1`, not text
- `kind`: `entry` or `exit`; an exit is an exit or a take-profit alert
- `profile`: the profile the alert is for, by its ID: 1 to 32 of `a-z`,
  `0-9`, `-` and `_`, starting with a letter or a digit
- `source`: the alert's own constant name, under the same rule, such as
  `trend`
- `ticker`: the instrument as TradingView renders `{{ticker}}`: 1 to 40
  characters, its letters uppercase, with no exchange before it and no
  space, seen or unseen
- `exchange`: optional; the chart's exchange as TradingView renders
  `{{exchange}}`, 1 to 32 of `A-Z`, `0-9` and `_`, such as `BYBIT`
- `category`: optional; `linear` or `inverse`
- `side`: `long` or `short`: the side an entry opens, or an exit closes
- `bar_time`: the time of the bar the alert fired on, as TradingView
  renders `{{time}}`: `yyyy-MM-ddTHH:mm:ssZ`, UTC, from 1970 on
- `fire_time`: when the alert fired, as TradingView renders `{{timenow}}`,
  in the same form; never earlier than `bar_time`
- `price`: an entry's expected price, a decimal string above zero
- `stop`: optional; an entry's own stop price, a decimal string above zero
<!-- end: signal fields -->

An entry needs every field but the optional ones. An exit carries neither
of the last two:

<!-- begin: entry only -->
- `price`
- `stop`
<!-- end: entry only -->

`kind` and `side` are **constants of the alert's template**. Nothing is
worked out from an order's `buy` or `sell`: an alert that opens a short and
one that closes a long both sell, and only the template knows which it is.

**Prices** are in the instrument's quote asset: USDT for a linear contract,
USD per coin for an inverse one. A price is a string of its digits, with no
sign, no exponent and no separators, at most 28 decimal places and 96 bits
of digits, and it keeps the digits it was sent with: `"234.230"` is not
`"234.23"` to the signal's hash. A price written as a JSON number is
refused, since a float may already have rounded it. The expected price need
not sit on the instrument's tick; [risk sizing](risk-sizing.md) rounds the
stop it sizes against, away from the entry.

**There is no quantity.** A position is sized from the profile's risk
settings ([automation-policy.md](automation-policy.md)), in the base asset
for a linear contract and in whole contracts for an inverse one. A payload
has no field to say otherwise.

<!-- begin: signal limits -->
- a body: 4096 bytes
- nesting: 8 levels, the payload's own object included
- a ticker: 40 characters
<!-- end: signal limits -->

## Strict by design

The rest of the protocol ignores what it does not know. A signal is exactly
its format, as a strategy bundle's manifest is: a body is a signal or it is
refused, with **every** problem it has, each by a code and the member it is
about. Nothing is corrected: no case is folded, no space trimmed, no
missing value filled in.

<!-- begin: problem codes -->
- `too_large`: the body is over the size limit, whatever it holds
- `not_json`: the body is not one JSON value in UTF-8, nests deeper than
  the limit, or holds a number no double holds
- `not_an_object`: the body is JSON, but not an object; a list of signals
  is not a signal
- `duplicate_field`: a member is written more than once
- `unknown_field`: a member the format does not define
- `misplaced_field`: an exit carries an entry's field
- `missing_field`: a field its kind of signal needs is not there
- `wrong_type`: a value is of another JSON type: a price as a number, a
  stop as `null`, a version as text
- `unsupported_version`: the version is a whole number, and not this one
- `invalid_value`: a value of the right type breaks its field's rule
- `bar_after_fire`: the bar time is after the fire time
<!-- end: problem codes -->

Problems come in one order on every implementation: one about the body as
a whole, then the format's fields in the order above, then every other
member in the order of their names.

Two consequences are worth knowing before an alert is written:

- **An unfilled placeholder refuses the alert.** A stop that an indicator
  renders as `NaN`, or a `{{ticker}}` that was never rendered, is an
  `invalid_value`. It is never read as "no stop", which would put the
  profile's fallback percentage where the indicator meant a price.
- **A kind that cannot be read says nothing.** With no usable `kind`, a
  body is asked only for what every signal carries, and a price or a stop
  is judged as an entry's would be.
- **A member written twice is only that.** Readers disagree on which of
  the two counts, so neither is judged, and the member is not called
  missing either.

The format has a version of its own, apart from the protocol's: a payload
is written once into an alert and then sent for months. Version 1 may still
change until QuantForge 0.3.0 is released; after that, a change is a new
version.

## What a payload cannot do

A payload names things; it decides nothing. The operator's settings live in
the profile, and no field of the format reaches them:

- **No credentials, no account, no environment.** The webhook URL selects
  the profile, and the profile names one venue, one account, one
  environment and one product family
  ([automation-policy.md](automation-policy.md#profiles)).
- **No risk, no leverage, no quantity, no capital.** They come from the
  policy the entry is admitted under, as it stood then.
- **No switch.** Nothing in a payload arms a profile, enables an asset or
  turns live trading on.

A body that tries is not a signal: each of those members is an
`unknown_field`, and the cases under
[invalid payloads](#the-shared-cases) show one for each.

What a payload may do is **assert**. `profile` is required, and `exchange`
and `category` are optional; each must agree with the profile the URL
selected, or the signal is refused. An assertion never selects anything.

## Identity

A delivery is known by the profile it is for and its **signal ID**: these
fields, joined by colons.

<!-- begin: signal id fields -->
- `source`
- `kind`
- `side`
- `ticker`
- `bar_time`
<!-- end: signal id fields -->

They are what TradingView renders the same way every time it sends an
alert: three constants of the template, the chart's ticker and the bar's
time. An ID is at most 106 characters, with no space and no control
character.

A signal's **canonical payload** is one JSON object on one line: every
field the signal has, in the format's order, with nothing between them,
but for what is left out:

<!-- begin: left out of the canonical payload -->
- `fire_time`
<!-- end: left out of the canonical payload -->

Its **hash** is the SHA-256 of those bytes, written `sha256:` and 64
lowercase hex digits. The fire time is left out of both because nothing
says a resend renders it as the first send did; spacing, the order of
members and JSON escapes are not part of a signal either.

<!-- begin: worked example -->
```json
{
  "version": 1,
  "kind": "entry",
  "profile": "by-linear",
  "source": "trend",
  "ticker": "SOLUSDT.P",
  "side": "long",
  "bar_time": "2026-10-20T00:00:00Z",
  "fire_time": "2026-10-20T00:15:00Z",
  "price": "234.23",
  "stop": "200.00"
}
```

```text
trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z
{"version":1,"kind":"entry","profile":"by-linear","source":"trend","ticker":"SOLUSDT.P","side":"long","bar_time":"2026-10-20T00:00:00Z","price":"234.23","stop":"200.00"}
sha256:f0745f29b39829e8f09f7c37237b6b96f015f9854799cf2fcc3a9d4f1d18e537
```
<!-- end: worked example -->

## Deliveries

A store holds, for each profile and signal ID, the hash of the payload it
first recorded. A delivery is then one of three things
(`model::futures::Delivery`):

- **new**: the profile has no signal with this ID;
- **duplicate**: it has this ID with this hash. The delivery is the first
  one again: it gets that one's outcome, and nothing is done twice;
- **conflict**: it has this ID with another hash: the same alert on the
  same bar with another price, another stop or another assertion. It is
  refused as `conflict`, and the first stays what it was.

The profile is part of the key. The same ID under two profiles is two
signals, and neither is ever a repeat of the other, or routed to the
other's account.

An ID names one bar. An alert that can fire twice on one bar sends the
first one's ID again, and its second delivery is a duplicate or a conflict,
never a second trade: set alerts to fire **once a bar**, and give two
alerts that may fire on the same bar two `source` names.

## Resolving a signal

`automation::SignalService::resolve` answers what a signal asks of the
profile its URL selected: the entry or the exit a store records, an entry
with the policy it is admitted under, or the refusal it is recorded with.
The rules are looked for in this order, and the first that refuses is the
signal's (`model::futures::ExternalSignal::resolve`):

<!-- begin: resolution order -->
- `profile`: the signal names another profile than its URL selects
  (`ambiguous_selection`)
- `exchange`: it asserts an exchange that is not the profile's venue, by
  its name in capitals (`ambiguous_selection`)
- `category`: it asserts another product family than the profile's
  (`ambiguous_selection`)
- `fire_time`: an entry fired further ahead of the clock than the
  allowance (`future_timestamp`), or longer ago than the profile's age
  limit (`stale`)
- `ticker`: the ticker names no instrument of the profile's product line
  (`unknown_ticker`), or more than one (`ambiguous_selection`)
- `policy`: for an entry, the profile's policy does not take the
  instrument: the refusals of
  [automation-policy.md](automation-policy.md#what-an-entry-is-held-to)
- `stop`: for an entry, neither the alert nor the policy gives a stop
  (`no_stop`)
<!-- end: resolution order -->

The first four need nothing but the profile and the clock, and are decided
before the venue is asked: a venue that is down delays a signal and never
changes what becomes of it.

### How old an entry may be

<!-- begin: age limits -->
- an entry's age limit: 60 seconds, unless the profile sets another
- a profile's own limit: from 5 to 300 seconds
- ahead of the clock: at most 5 seconds, for every profile
<!-- end: age limits -->

An entry exactly at a bound is taken. A limit above 60 seconds only admits
alerts that TradingView delivered late; the price checks before submission
still run ([risk-sizing.md](risk-sizing.md#just-before-submission)). ADR
0002 §9 holds an admitted entry to the same limit once more, just before it
is sent, and records one that has grown too old as `expired`: that check is
the executor's, with this rule (`ExternalSignal::timely`).

**An exit is held to no clock.** It never expires and is never refused for
its time: no clock holds back a close. Its fire time only orders it against
entries, [below](#entries-exits-and-positions).

### What a ticker names

There are two spellings, and no third:

<!-- begin: ticker spellings -->
- `venue_symbol`: the venue's own symbol, such as `BTCUSD_PERP`. It names
  an asset only where the profile has settings of its own for it
- `tradingview`: TradingView's name of a perpetual: its base asset, its
  quote asset and `.P`, such as `BTCUSD.P`, whatever the venue calls the
  contract
<!-- end: ticker spellings -->

The first is narrow on purpose. An alert on a spot chart renders the same
text as a perpetual's symbol on some venues, and an asset that trades only
on the profile's defaults is never traded on the strength of that (ADR 0002
§5): it is named by TradingView's own ticker for the perpetual, which a
spot chart never renders.

The second reads the venue's instrument list, never a table of names: the
perpetual whose base and quote assets the ticker spells. A list that holds
two of them is refused, not guessed at. A dated future has no such name.

The same ticker names the same instrument for an entry and for an exit.
Whether an entry may trade it is the policy's to say, next; an exit takes
no policy, so an asset that is disabled, or one the profile's defaults no
longer cover, can still be closed.

## Entries, exits and positions

What a store holds decides the rest, by two rules of the model.

**An entry on a position already held is refused** (`EntryIntent::against`),
for good, not for later:

<!-- begin: held side -->
- flat: the entry is taken
- the same side: `pyramiding`, since a second entry would add to the
  position
- the other side: `reversal`, since the indicator sends an exit first
<!-- end: held side -->

**An exit closes only a position its own profile entered before it fired**
(`ExitIntent::closes`): the same profile, the same account and instrument,
the side the exit names, and an entry whose fire time is strictly earlier
than the exit's. An exit that closes nothing is a no-op, recorded as one.

That last rule is what keeps a delayed exit from closing a newer position.
An exit that fired before the position's entry, or in its second, was meant
for an earlier position, however late it arrives. An exit sent twice is a
duplicate like any other delivery, and an exit alert on a later bar that
finds nothing open is a no-op: exits are idempotent both ways.

## The receipt

A delivery that was recorded is answered `2xx` with a receipt
(`protocol::signal::SignalReceipt`). It means that the delivery was
recorded and will not be lost. **It never means that an order exists**:
whether one follows is the executor's to decide and to report.

<!-- begin: receipt fields -->
- `version`: the number `1`
- `signal_id`: the signal the receipt is for, within its profile
- `state`: where the signal stands
- `refusal`: the refusal's name; present exactly when the state is
  `rejected`
- `message`: optional; for a person, and free to change
<!-- end: receipt fields -->

<!-- begin: receipt states -->
- `received`: recorded, and not yet judged
- `admitted`: recorded and admitted; nothing has been sized, sent or
  filled
- `rejected`: recorded and refused, for the reason `refusal` names
- `duplicate`: a delivery already recorded; nothing is done twice
- `noop`: recorded, with nothing to do: an exit with no position of its
  own to close
<!-- end: receipt states -->

Those are the state a signal is recorded in and the four it reaches without
an order. Every later state needs one, and no receipt gives it.

<!-- begin: receipt examples -->
```json
{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"received"}
{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"admitted"}
{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"rejected","refusal":"stale","message":"the alert fired 75 s ago, and profile by-linear takes entries up to 60 s old"}
{"version":1,"signal_id":"trend:entry:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"duplicate"}
{"version":1,"signal_id":"trend:exit:long:SOLUSDT.P:2026-10-20T00:00:00Z","state":"noop","message":"profile by-linear holds no long position on SOLUSDT that this exit is for"}
```
<!-- end: receipt examples -->

What is not a signal has no receipt:

| What arrived | Answer | TradingView then |
| --- | --- | --- |
| a signal, recorded | `2xx`, a receipt | is done |
| a body that is not a signal | `400`, the protocol's error envelope with `invalid_request`, naming every problem | does not send it again |
| a delivery that could not be recorded | `5xx`, the envelope with `unavailable` | sends it again, up to three times |

`server::read_signal` reads a body into the model's signal or that `400`,
and `server::refusal_receipt` writes the receipt of a refusal; the route
around them is ALG-743's.

## The alert templates

A template is the text an operator pastes as an alert's message. Before it
is used, two constants are changed: `profile`, to the profile's ID, and
`source`, to a name for the alert. `kind` and `side` are what the template
is for, and are left alone. Every placeholder is in quotes, so that
whatever TradingView renders arrives as a JSON string.

<!-- begin: alert templates -->
**`entry-long`.** A LONG entry at the bar's close, on the profile's fallback stop.

```json
{
  "version": 1,
  "kind": "entry",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "side": "long",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}",
  "price": "{{close}}"
}
```

**`entry-short`.** A SHORT entry at the bar's close, on the profile's fallback stop.

```json
{
  "version": 1,
  "kind": "entry",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "side": "short",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}",
  "price": "{{close}}"
}
```

**`entry-long-with-stop`.** A LONG entry whose stop is the indicator's first plot.

```json
{
  "version": 1,
  "kind": "entry",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "side": "long",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}",
  "price": "{{close}}",
  "stop": "{{plot_0}}"
}
```

**`entry-short-with-stop`.** A SHORT entry whose stop is the indicator's plot named Stop, asserting the chart's exchange.

```json
{
  "version": 1,
  "kind": "entry",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "exchange": "{{exchange}}",
  "side": "short",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}",
  "price": "{{close}}",
  "stop": "{{plot("Stop")}}"
}
```

**`exit-long`.** An exit of a LONG: an exit or a take-profit alert.

```json
{
  "version": 1,
  "kind": "exit",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "side": "long",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}"
}
```

**`exit-short`.** An exit of a SHORT.

```json
{
  "version": 1,
  "kind": "exit",
  "profile": "main",
  "source": "trend",
  "ticker": "{{ticker}}",
  "side": "short",
  "bar_time": "{{time}}",
  "fire_time": "{{timenow}}"
}
```
<!-- end: alert templates -->

To set one up:

1. Open the alert on the **perpetual's own chart**: its ticker ends in
   `.P`.
2. Have it fire **once a bar**. A second firing on the same bar carries the
   first one's ID.
3. Paste the template as the message, with its `profile` and `source`
   changed.
4. Use the profile's webhook URL. The URL is the profile's secret; the
   message never holds one.

A template with a stop takes it from a plot of the indicator. A template
without one leaves the stop to the profile's fallback percentage; an entry
with neither is refused as `no_stop`.

**A strategy script** sends the same payloads. Its alert's message is
`{{strategy.order.alert_message}}`, and the script passes one payload for
each order it places, with that order's `kind` and `side` written as
constants. The placeholders that describe an order (`buy`, `sell`, `flat`)
appear in no template: the receiver never reads a direction from them.

## The schema

[`signal-schema.json`](signal-schema.json) is the payload's shape as JSON
Schema (draft 2020-12), for editors and for tools that check a message
before it is pasted. It is a necessary condition, never a sufficient one:
the schema accepts every signal, and refuses most of what the format
refuses. What a schema cannot see, the format still refuses. Of the shared
cases, these pass the schema and are not signals:

<!-- begin: beyond the schema -->
- `too-large`: `too_large`
- `a-byte-too-large`: `too_large`
- `duplicate-side`: `duplicate_field` `side`
- `duplicate-kind`: `duplicate_field` `kind`
- `version-with-a-fraction`: `wrong_type` `version`
- `version-with-an-exponent`: `wrong_type` `version`
- `time-of-no-day`: `invalid_value` `bar_time`
- `time-of-no-leap-day`: `invalid_value` `bar_time`
- `time-of-no-century-leap-day`: `invalid_value` `bar_time`
- `time-of-no-april-31`: `invalid_value` `bar_time`
- `time-of-no-june-31`: `invalid_value` `bar_time`
- `time-of-no-september-31`: `invalid_value` `bar_time`
- `time-of-no-november-31`: `invalid_value` `bar_time`
- `price-too-large`: `invalid_value` `price`
- `bar-after-fire`: `bar_after_fire` `bar_time`
- `bar-a-second-after-fire`: `bar_after_fire` `bar_time`
<!-- end: beyond the schema -->

## SQLite and PostgreSQL

Nothing here is stored, so nothing here differs by backend: the wire
format, a signal's ID and hash, and every rule are the same code. What a
resolution reads from a store is the profile and its newest policy, through
the `PolicyStore` port, and `tests/signals/service.rs` resolves every
shared case from a policy read back from SQLite and from PostgreSQL to the
answer the pure model gives. A signal's identity is scoped by its profile
on both.

## The shared cases

`python3 -I spikes/futures-contract/signal_cases.py --write
tests/fixtures/signal_cases.json` writes the cases, and the same script
with `--doc docs/signal-payload.md` refills the lists below and the
templates above. The Python model shares no code with the Rust one, and
checks the schema against the cases with a validator of its own.

<!-- begin: fixture format -->
1
<!-- end: fixture format -->

Each product line of the cases has one profile, with a policy made to show
every outcome: an asset with settings of its own, one that is disabled, one
left to the defaults, one the venue no longer trades, a profile whose
defaults are off, and one without capital in a contract's coin.

### Valid payloads

<!-- begin: valid payloads -->
- `bn-linear-long-entry`: Binance USD-M LONG entry, the alert's own stop
- `bn-linear-short-entry`: Binance USD-M SHORT entry without a stop: the
  profile's fallback applies
- `bn-linear-long-exit`: Binance USD-M exit of a LONG
- `bn-linear-short-exit`: Binance USD-M exit of a SHORT, as a take-profit
  alert sends it
- `bn-inverse-long-entry`: Binance COIN-M LONG entry: TradingView's BTCUSD.P
  is the venue's BTCUSD_PERP
- `bn-inverse-short-entry`: Binance COIN-M SHORT entry, naming the venue's own
  symbol
- `bn-inverse-long-exit`: Binance COIN-M exit of a LONG
- `bn-inverse-short-exit`: Binance COIN-M exit of a SHORT
- `by-linear-long-entry`: Bybit linear LONG entry, the alert's own stop
- `by-linear-short-entry`: Bybit linear SHORT entry, asserting its exchange
  and its family
- `by-linear-long-exit`: Bybit linear exit of a LONG
- `by-linear-short-exit`: Bybit linear exit of a SHORT
- `by-inverse-long-entry`: Bybit inverse LONG entry without a stop: the
  profile's fallback applies
- `by-inverse-short-entry`: Bybit inverse SHORT entry, the alert's own stop
- `by-inverse-long-exit`: Bybit inverse exit of a LONG, naming the venue's own
  symbol
- `by-inverse-short-exit`: Bybit inverse exit of a SHORT
- `compact`: The same signal on one line: spacing is not part of a payload
- `as-large-as-a-body-may-be`: The same signal, spaced out to the size limit
  exactly
- `members-in-another-order`: The same signal with its members in another
  order
- `a-later-fire-time`: The same signal fired again 10 seconds later, as a
  resend may render it: the same ID and the same hash
- `another-price`: The same ID with another price: another hash, so a conflict
  if it follows the first
- `price-with-trailing-zeros`: A price keeps the digits it was sent with:
  234.230 is not 234.23 to the hash
- `another-stop`: The same ID with another stop: another hash
- `without-its-stop`: The same ID without its stop: another hash
- `another-bar`: The same alert on the next bar: another ID
- `another-source`: Another alert on the same bar: another ID
- `bar-time-is-the-fire-time`: An alert fired the second its bar opened
- `non-ascii-ticker`: A ticker beyond ASCII, as a venue lists some
- `smallest-price`: The smallest price an exact decimal holds
- `largest-price`: The largest price an exact decimal holds
- `leap-day`: A bar on a leap day
- `century-leap-day`: A leap day in 2000: a year divisible by 400 is a leap
  year
- `months-of-31-days`: The last day of January and of March
- `more-months-of-31-days`: Of May and of July
- `the-other-months-of-31-days`: Of August and of October
- `december-31`: And of December, after the last day of February
- `months-of-30-days`: The last day of April and of June
- `more-months-of-30-days`: Of September and of November
- `first-second`: The first second a time can name
- `last-second`: The last second a time can name
- `longest-names`: The longest profile, source and ticker: the ID still fits
- `widest-ticker`: Forty characters are forty characters, however many bytes
  each takes
- `escaped-text`: Text written with JSON escapes is the text it spells
- `by-linear-long-entry-as-bn`: The Bybit entry's own text but for the profile
  it names: the same signal ID under another profile
- `bn-linear-says-bybit`: A Binance profile's payload asserting BYBIT
- `by-linear-says-coinbase`: A payload asserting an exchange QuantForge has no
  venue for
- `by-linear-says-inverse`: A linear profile's payload asserting inverse
- `by-linear-exit-says-inverse`: A linear profile's exit asserting inverse
- `by-linear-inverse-ticker`: A linear profile's payload naming an inverse
  contract
- `by-linear-venue-symbol`: The venue's own symbol in place of TradingView's
  name
- `bn-linear-btc`: The BTCUSDT perpetual, which the profile has no settings
  for
- `bn-linear-btc-symbol`: The same contract by the venue's own symbol
- `bn-linear-btc-symbol-exit`: An exit naming it so
- `bn-linear-1000shib`: A thousand-coin contract the profile has disabled,
  without a stop
- `bn-linear-1000shib-exit`: An exit on it
- `bn-inverse-sol`: A COIN-M contract the profile has no settings for
- `bn-inverse-sol-exit`: An exit on it
- `bn-inverse-eth-without-stop`: A COIN-M contract the profile has disabled,
  without a stop
- `by-linear-says-neither`: A payload asserting another exchange and another
  family
- `bn-inverse-entry-without-stop`: A COIN-M entry without a stop
- `by-linear-btc-without-stop`: A Bybit BTCUSDT entry without a stop
- `by-linear-btc-with-stop`: A Bybit BTCUSDT entry with its stop
- `by-linear-shib1000`: Bybit's thousand-coin contract
- `bn-linear-dated`: A dated future's own symbol
- `by-linear-ethusdc`: A USDC perpetual by its TradingView name
- `bn-linear-xau`: A gold perpetual
- `bn-linear-luna`: A perpetual that is settling
- `bn-linear-luna-exit`: An exit on it, by its symbol
- `by-inverse-eth`: An inverse contract in a coin without capital
- `by-inverse-sol`: An inverse contract on the defaults
- `by-linear-unknown`: A ticker Bybit lists nothing for
- `bn-inverse-unknown`: A ticker Binance lists no COIN-M contract for, without
  a stop
- `twice-solusdt-p`: A .P name on a list that names its pair twice
- `twice-solusdt`: The venue's symbol on that list
- `twice-exit`: An exit under another profile of the same account
- `by-linear-btc-long-exit`: An exit of a LONG on BTCUSDT
- `by-linear-long-exit-by-symbol`: An exit naming the venue's symbol
- `by-linear-long-exit-next-bar`: The exit alert on the next bar
<!-- end: valid payloads -->

### Invalid payloads

Each with the problems it is refused for.

<!-- begin: invalid payloads -->
- `empty`: An empty body (`not_json`)
- `plain-text`: An alert message that is not JSON: TradingView sends it as
  text/plain (`not_json`)
- `truncated`: A payload cut short (`not_json`)
- `two-values`: Two JSON values (`not_json`)
- `trailing-comma`: A comma after the last member (`not_json`)
- `single-quotes`: Single quotes (`not_json`)
- `a-comment`: A comment (`not_json`)
- `byte-order-mark`: A byte order mark before the object (`not_json`)
- `not-utf8`: Bytes that are not UTF-8 (`not_json`)
- `lone-surrogate`: An escape that names half a character (`not_json`)
- `raw-control-character`: A control character inside a string, unescaped
  (`not_json`)
- `nan`: NaN is not JSON (`not_json`)
- `number-with-a-leading-zero`: A number with a leading zero is not JSON
  (`not_json`)
- `number-no-double-holds`: A number too large for a double is not read as one
  (`not_json`)
- `integer-no-double-holds`: Nor an integer of four hundred digits
  (`not_json`)
- `nested-too-deep`: Nesting past the limit is not read at all (`not_json`)
- `too-large`: A body over the size limit, whatever it holds (`too_large`)
- `a-byte-too-large`: By as little as a byte (`too_large`)
- `an-array`: An array of signals: one delivery is one signal
  (`not_an_object`)
- `a-string`: A JSON string (`not_an_object`)
- `a-number`: A JSON number (`not_an_object`)
- `null`: A JSON null (`not_an_object`)
- `duplicate-side`: A member written twice: readers disagree on which one
  counts (`duplicate_field` `side`)
- `duplicate-kind`: A kind written twice says nothing of what the payload is
  (`duplicate_field` `kind`)
- `duplicate-unknown`: An unknown member written twice is reported once, as
  written twice (`duplicate_field` `note`)
- `unknown-field`: A member the format does not define (`unknown_field`
  `comment`)
- `field-in-capitals`: A member's name is written as the format writes it
  (`missing_field` `side`, `unknown_field` `Side`)
- `asks-for-a-quantity`: A payload cannot size an entry: there is no quantity
  to send (`unknown_field` `qty`)
- `asks-for-more-risk`: A payload cannot raise the operator's risk
  (`unknown_field` `risk_percent`)
- `asks-for-leverage`: Nor choose the leverage (`unknown_field` `leverage`)
- `asks-for-live-trading`: Nor switch live trading on (`unknown_field` `mode`)
- `names-an-account`: Nor choose an account or its credentials
  (`unknown_field` `account`, `unknown_field` `api_key`)
- `names-an-environment`: Nor an environment (`unknown_field` `environment`)
- `exit-with-a-price`: An exit carries no price (`misplaced_field` `price`)
- `exit-with-a-stop`: Nor a stop (`misplaced_field` `stop`)
- `no-version`: No version (`missing_field` `version`)
- `no-profile`: No profile: every payload names the profile it is for
  (`missing_field` `profile`)
- `no-price`: An entry without its expected price (`missing_field` `price`)
- `no-kind`: No kind: a price is then not missed, since only an entry needs
  one (`missing_field` `kind`)
- `an-empty-object`: An object with nothing in it (`missing_field` `version`,
  `missing_field` `kind`, `missing_field` `profile`, `missing_field` `source`,
  `missing_field` `ticker`, `missing_field` `side`, `missing_field`
  `bar_time`, `missing_field` `fire_time`)
- `brackets-in-text`: Brackets inside a string open nothing: the body is read,
  and its member found unknown (`unknown_field` `note`)
- `brackets-after-an-escaped-quote`: Nor do brackets after a quote the string
  escapes (`unknown_field` `note`)
- `brackets-after-an-escaped-backslash`: A backslash the string escapes does
  not escape the quote that ends it (`unknown_field` `more`, `unknown_field`
  `note`)
- `nested-to-the-limit`: Nesting up to the limit is read, and found to be the
  wrong type (`wrong_type` `version`, `missing_field` `kind`, `missing_field`
  `profile`, `missing_field` `source`, `missing_field` `ticker`,
  `missing_field` `side`, `missing_field` `bar_time`, `missing_field`
  `fire_time`)
- `version-as-text`: The version is a number, not text (`wrong_type`
  `version`)
- `version-with-a-fraction`: And a whole one (`wrong_type` `version`)
- `version-with-an-exponent`: Written as one (`wrong_type` `version`)
- `version-with-a-sign`: Without a sign (`wrong_type` `version`)
- `version-below-zero`: And not below zero (`wrong_type` `version`)
- `version-true`: A boolean is not a number (`wrong_type` `version`)
- `version-beyond-exact-integers`: A version past 2^53 - 1 is not an integer
  every reader holds (`wrong_type` `version`)
- `price-as-a-number`: A price without its quotes is a JSON number, which a
  float may already have rounded: write "{{close}}", in quotes (`wrong_type`
  `price`)
- `stop-is-null`: A stop left out is left out, not null (`wrong_type` `stop`)
- `side-as-a-list`: A value that is a list (`wrong_type` `side`)
- `kind-as-a-number`: A kind that is no text says nothing of what the payload
  is (`wrong_type` `kind`)
- `profile-as-an-object`: A value that is an object (`wrong_type` `profile`)
- `source-as-a-number`: A source that is a number (`wrong_type` `source`)
- `ticker-as-a-number`: A ticker that is a number (`wrong_type` `ticker`)
- `exchange-is-null`: An assertion left out is left out, not null
  (`wrong_type` `exchange`)
- `category-as-a-boolean`: A family that is a boolean (`wrong_type`
  `category`)
- `bar-time-as-a-number`: A time as a number of milliseconds (`wrong_type`
  `bar_time`)
- `fire-time-as-a-number`: Or of seconds (`wrong_type` `fire_time`)
- `a-later-version`: A version this format is not (`unsupported_version`
  `version`)
- `version-zero`: Version zero (`unsupported_version` `version`)
- `largest-exact-version`: The largest version an exact integer holds is still
  not this one (`unsupported_version` `version`)
- `kind-buy`: A kind is entry or exit, never an order's buy or sell
  (`invalid_value` `kind`)
- `kind-in-capitals`: In lowercase (`invalid_value` `kind`)
- `side-buy`: A side is long or short (`invalid_value` `side`)
- `side-flat`: And never flat (`invalid_value` `side`)
- `side-in-capitals`: In lowercase (`invalid_value` `side`)
- `profile-in-capitals`: A profile is named as it was created (`invalid_value`
  `profile`)
- `profile-starting-with-a-hyphen`: Starting with a letter or a digit
  (`invalid_value` `profile`)
- `profile-too-long`: In at most 32 characters (`invalid_value` `profile`)
- `profile-empty`: And at least one (`invalid_value` `profile`)
- `source-with-a-space`: A source is one word (`invalid_value` `source`)
- `source-with-a-colon`: Without the colon an ID is joined with
  (`invalid_value` `source`)
- `source-too-long`: Of at most 32 characters (`invalid_value` `source`)
- `source-with-a-line-break`: On one line (`invalid_value` `source`)
- `ticker-in-lowercase`: A ticker is written as TradingView renders it
  (`invalid_value` `ticker`)
- `ticker-with-its-exchange`: Without the exchange before it (`invalid_value`
  `ticker`)
- `ticker-with-a-space`: And without spaces (`invalid_value` `ticker`)
- `ticker-with-a-hidden-space`: Seen or unseen (`invalid_value` `ticker`)
- `ticker-with-a-wide-space`: In any script (`invalid_value` `ticker`)
- `ticker-with-a-quote`: Nor quotes (`invalid_value` `ticker`)
- `ticker-empty`: An empty ticker (`invalid_value` `ticker`)
- `ticker-too-long`: A ticker of more than 40 characters (`invalid_value`
  `ticker`)
- `ticker-unrendered`: A placeholder TradingView did not fill in
  (`invalid_value` `ticker`)
- `exchange-in-lowercase`: An exchange is written as {{exchange}} renders it
  (`invalid_value` `exchange`)
- `exchange-empty`: And is not empty (`invalid_value` `exchange`)
- `category-spot`: A signal trades futures: its family is linear or inverse
  (`invalid_value` `category`)
- `category-in-capitals`: In lowercase (`invalid_value` `category`)
- `time-with-an-offset`: A time is UTC, written with Z (`invalid_value`
  `fire_time`)
- `time-with-fractions`: To the second (`invalid_value` `fire_time`)
- `time-with-a-space`: With T between the day and the hour (`invalid_value`
  `fire_time`)
- `time-in-lowercase`: In capitals (`invalid_value` `fire_time`)
- `time-ending-in-a-small-z`: The Z as well (`invalid_value` `fire_time`)
- `time-with-a-letter`: A letter where a digit belongs (`invalid_value`
  `bar_time`)
- `time-without-its-seconds`: A time to the minute (`invalid_value`
  `fire_time`)
- `time-as-milliseconds`: As TradingView renders it, not as a number of
  milliseconds (`invalid_value` `bar_time`)
- `time-of-no-day`: A day the calendar does not have (`invalid_value`
  `bar_time`)
- `time-of-no-leap-day`: A leap day in a year without one (`invalid_value`
  `bar_time`)
- `time-of-no-century-leap-day`: 2100 is not a leap year (`invalid_value`
  `bar_time`)
- `time-of-no-april-31`: April has 30 days (`invalid_value` `bar_time`)
- `time-of-no-june-31`: So has June (`invalid_value` `bar_time`)
- `time-of-no-september-31`: And September (`invalid_value` `bar_time`)
- `time-of-no-november-31`: And November (`invalid_value` `bar_time`)
- `time-of-day-32`: No month has 32 (`invalid_value` `bar_time`)
- `time-of-no-month`: A month the calendar does not have (`invalid_value`
  `bar_time`)
- `time-of-day-zero`: Day zero (`invalid_value` `bar_time`)
- `time-of-no-hour`: An hour the clock does not have (`invalid_value`
  `bar_time`)
- `time-of-no-minute`: A minute the clock does not have (`invalid_value`
  `bar_time`)
- `time-with-a-leap-second`: A leap second (`invalid_value` `fire_time`)
- `time-before-1970`: A time before 1970 (`invalid_value` `bar_time`)
- `time-in-other-digits`: Digits of another script (`invalid_value`
  `bar_time`)
- `time-unrendered`: A placeholder TradingView did not fill in
  (`invalid_value` `bar_time`)
- `price-zero`: A price of nothing (`invalid_value` `price`)
- `price-zero-with-places`: However it is written (`invalid_value` `price`)
- `price-below-zero`: A price below zero (`invalid_value` `price`)
- `price-with-a-plus`: A price with a plus sign (`invalid_value` `price`)
- `price-with-an-exponent`: A price with an exponent (`invalid_value` `price`)
- `price-with-a-separator`: A price with a thousands separator
  (`invalid_value` `price`)
- `price-with-a-leading-zero`: A price with a leading zero (`invalid_value`
  `price`)
- `price-with-no-integer-part`: A price that starts with its point
  (`invalid_value` `price`)
- `price-ending-in-its-point`: Or ends with it (`invalid_value` `price`)
- `price-with-spaces`: A price with spaces around it (`invalid_value` `price`)
- `price-below-the-smallest`: A price of 29 decimal places, however few its
  digits (`invalid_value` `price`)
- `price-too-precise`: More decimal places than an exact decimal holds
  (`invalid_value` `price`)
- `price-with-too-many-zeros`: Even when the places beyond the 28th are zeros
  (`invalid_value` `price`)
- `price-too-large`: More digits than an exact decimal holds (`invalid_value`
  `price`)
- `price-unrendered`: A placeholder TradingView did not fill in
  (`invalid_value` `price`)
- `stop-not-a-number`: A stop the indicator had no value for: the alert is
  refused, and nothing falls back to the profile's percentage (`invalid_value`
  `stop`)
- `stop-na`: However the missing value is written (`invalid_value` `stop`)
- `stop-empty`: An empty stop (`invalid_value` `stop`)
- `stop-zero`: A stop of nothing (`invalid_value` `stop`)
- `bar-after-fire`: A bar that opens after its alert fired: the two times are
  swapped (`bar_after_fire` `bar_time`)
- `bar-a-second-after-fire`: By as little as a second (`bar_after_fire`
  `bar_time`)
- `several-problems`: Everything wrong with a payload is said at once
  (`unsupported_version` `version`, `invalid_value` `side`, `misplaced_field`
  `price`, `misplaced_field` `stop`, `unknown_field` `note`)
- `unknown-fields-in-order`: Unknown members are listed after the known ones,
  in the order of their names (`unknown_field` `Alpha`, `unknown_field`
  `alpha`, `unknown_field` `zeta`)
<!-- end: invalid payloads -->

### Resolutions

Each with what the signal resolves to, or the refusal it is recorded with.

<!-- begin: resolution cases -->
- `bn-linear-long`: A Binance USD-M LONG entry on an asset with settings of
  its own (entry long on SOLUSDT)
- `bn-linear-short`: A Binance USD-M SHORT entry without a stop takes the
  defaults' fallback (entry short on SOLUSDT)
- `bn-linear-exit-long`: A Binance USD-M exit of a LONG (exit long on SOLUSDT)
- `bn-linear-exit-short`: A Binance USD-M exit of a SHORT (exit short on
  SOLUSDT)
- `bn-inverse-long`: TradingView's BTCUSD.P is the COIN-M BTCUSD_PERP: the
  perpetual of base BTC and quote USD (entry long on BTCUSD_PERP)
- `bn-inverse-short`: The venue's own symbol names an asset the profile has
  settings for (entry short on BTCUSD_PERP)
- `bn-inverse-exit-long`: A Binance COIN-M exit of a LONG (exit long on
  BTCUSD_PERP)
- `bn-inverse-exit-short`: A Binance COIN-M exit of a SHORT (exit short on
  BTCUSD_PERP)
- `by-linear-long`: A Bybit linear LONG entry (entry long on SOLUSDT)
- `by-linear-short`: A Bybit linear SHORT entry whose assertions agree with
  its profile (entry short on SOLUSDT)
- `by-linear-exit-long`: A Bybit linear exit of a LONG (exit long on SOLUSDT)
- `by-linear-exit-short`: A Bybit linear exit of a SHORT (exit short on
  SOLUSDT)
- `by-inverse-long`: A Bybit inverse LONG entry without a stop: BTCUSD.P is
  BTCUSD there, and its fallback is the defaults' (entry long on BTCUSD)
- `by-inverse-short`: A Bybit inverse SHORT entry (entry short on BTCUSD)
- `by-inverse-exit-long`: A Bybit inverse exit of a LONG, by the venue's
  symbol (exit long on BTCUSD)
- `by-inverse-exit-short`: A Bybit inverse exit of a SHORT (exit short on
  BTCUSD)
- `wrong-profile`: A Bybit payload delivered to the Binance profile's URL: the
  profile it names is not the one its URL selects (`ambiguous_selection`)
- `same-id-other-profile`: The same signal ID under another profile's own URL
  is that profile's own signal (entry long on SOLUSDT)
- `wrong-exchange`: A payload that says BYBIT on a Binance profile
  (`ambiguous_selection`)
- `exchange-of-no-venue`: An exchange QuantForge trades on nowhere cannot
  agree with any profile (`ambiguous_selection`)
- `wrong-category`: A payload that says inverse on a linear profile
  (`ambiguous_selection`)
- `wrong-category-on-an-exit`: An exit is held to what it asserts too
  (`ambiguous_selection`)
- `inverse-ticker-on-linear`: An inverse contract's ticker on a linear profile
  is no instrument of it (`unknown_ticker`)
- `venue-symbol-of-an-asset`: The venue's own symbol names an asset the
  profile has settings for (entry long on SOLUSDT)
- `on-the-defaults`: An asset with no settings of its own trades on enabled
  defaults, named by TradingView's ticker; a dated future that shares its base
  and quote is not it (entry long on BTCUSDT)
- `venue-symbol-without-settings`: The venue's own symbol, as an alert on a
  spot chart renders it, names nothing the profile was not given settings for
  (`unknown_ticker`)
- `venue-symbol-without-settings-exit`: Nor for an exit: the same ticker names
  the same thing whatever the alert asks (`unknown_ticker`)
- `disabled-asset`: An entry on a disabled asset (`not_armed`)
- `disabled-asset-exit`: An exit on a disabled asset still resolves: no policy
  holds back a close (exit long on 1000SHIBUSDT)
- `defaults-off`: An asset without settings, on a profile whose defaults are
  not enabled (`unknown_ticker`)
- `defaults-off-exit`: Its exit still resolves (exit long on SOLUSD_PERP)
- `no-fallback-stop`: An entry without a stop on an asset whose fallback is
  off (`no_stop`)
- `no-inherited-fallback-stop`: An entry without a stop on an asset that
  inherits a fallback the defaults do not give (`no_stop`)
- `own-stop-without-a-fallback`: The same entry with a stop of its own (entry
  long on BTCUSDT)
- `thousand-contract`: A contract of a thousand coins, by its TradingView
  name, on the defaults (entry short on SHIB1000USDT)
- `non-ascii-ticker`: A ticker beyond ASCII (entry long on 币安人生USDT)
- `dated-by-symbol`: A dated future's own symbol names nothing: the profile
  has no settings for it, and it has no TradingView perpetual name
  (`unknown_ticker`)
- `usdc-perpetual`: ETHUSDC.P names Bybit's ETHPERP, by its base and its
  quote; the universe keeps a USDC contract out (`outside_universe`)
- `tradfi-perpetual`: A perpetual on gold is named, and kept out by the
  universe (`outside_universe`)
- `not-trading`: A perpetual the venue no longer trades (`outside_universe`)
- `not-trading-exit`: Its exit still resolves, by its symbol (exit long on
  LUNAUSDT)
- `no-capital`: An inverse contract whose coin the profile holds no capital in
  (`no_capital_in_settlement_asset`)
- `inverse-on-the-defaults`: An inverse contract on the defaults, sized from
  the capital in its own coin (entry short on SOLUSD)
- `unknown-ticker`: A ticker the venue lists nothing for (`unknown_ticker`)
- `pair-named-twice`: A list that names a pair twice is not guessed at
  (`ambiguous_selection`)
- `pair-named-twice-by-symbol`: The venue's symbol of an asset with settings
  still names one of them (entry long on SOLUSDT)
- `entry-at-the-age-limit`: An entry exactly as old as the limit is taken
  (entry long on SOLUSDT)
- `entry-past-the-age-limit`: A millisecond older, and it is stale (`stale`)
- `exit-never-too-old`: An exit an hour old is still an exit (exit long on
  SOLUSDT)
- `entry-within-the-future-allowance`: An entry timed 5 seconds after the
  clock is taken (entry long on SOLUSDT)
- `entry-from-the-future`: A millisecond more, and it is from the future
  (`future_timestamp`)
- `exit-from-the-future`: An exit timed ahead of the clock is still an exit:
  no clock holds back a close (exit long on SOLUSDT)
- `a-longer-age-limit`: A profile that takes entries up to 300 seconds old
  (entry long on SOLUSDT)
- `past-a-longer-age-limit`: And none older (`stale`)
- `a-shorter-age-limit`: One that takes entries up to 5 seconds old (entry
  long on SOLUSDT)
- `past-a-shorter-age-limit`: And none older (`stale`)
- `wrong-profile-before-anything-else`: A payload for another profile is
  refused for that, however stale and whatever its ticker
  (`ambiguous_selection`)
- `profile-before-the-exchange`: The profile is held before what the payload
  asserts (`ambiguous_selection`)
- `exchange-before-the-category`: An exchange that disagrees is said before a
  family that does (`ambiguous_selection`)
- `category-before-its-age`: A family that disagrees is said before the age
  (`ambiguous_selection`)
- `wrong-exchange-before-its-age`: And so is an exchange
  (`ambiguous_selection`)
- `stale-before-the-ticker`: A stale entry is refused for its age before its
  ticker is looked up (`stale`)
- `ticker-before-the-policy`: A ticker that names nothing is said before the
  policy is asked (`unknown_ticker`)
- `policy-before-the-stop`: A disabled asset is said before the stop its entry
  lacks (`not_armed`)
<!-- end: resolution cases -->

### Exits

<!-- begin: exit cases -->
- `exit-after-its-entry`: An exit that fired after the entry closes its
  position (closes it)
- `delayed-exit`: An exit that fired before the entry, delivered late, closes
  nothing: the position is a newer one than the exit was for (a no-op)
- `exit-in-the-same-second`: Nor does one that fired in the entry's own
  second: strictly before means before (a no-op)
- `exit-a-second-later`: One second later is later (closes it)
- `exit-of-the-other-side`: An exit of a SHORT closes no LONG (a no-op)
- `exit-of-another-instrument`: An exit on another instrument closes nothing
  here (a no-op)
- `exit-by-the-venue-symbol`: The ticker's spelling does not matter, the
  instrument does (closes it)
- `exit-of-another-profile`: Another profile's exit closes nothing, though it
  names the same account and instrument: a position is its own profile's (a
  no-op)
- `exit-with-nothing-open`: An exit with no position to close does nothing,
  and says so (a no-op)
<!-- end: exit cases -->

### Deliveries

Each with its deliveries in order, and what each one is.

<!-- begin: delivery cases -->
- `repeated-delivery`: TradingView resends a delivery it got a 5xx for: the
  second is the first again, and nothing is done twice (by-linear-long-entry
  new, by-linear-long-entry duplicate)
- `resend-with-a-later-fire-time`: A resend whose fire time was rendered again
  is still the same delivery (by-linear-long-entry new, a-later-fire-time
  duplicate)
- `same-id-another-price`: The same ID with another price is not the same
  signal: a conflict, refused (by-linear-long-entry new, another-price
  conflict)
- `same-id-another-scale`: Nor is one whose price is written with another
  scale (by-linear-long-entry new, price-with-trailing-zeros conflict)
- `same-id-another-stop`: Nor one with another stop, or with none
  (by-linear-long-entry new, another-stop conflict, without-its-stop conflict)
- `conflict-then-the-original-again`: A conflict changes nothing: the first
  delivery is still the one known (by-linear-long-entry new, another-price
  conflict, by-linear-long-entry duplicate)
- `same-id-two-profiles`: The same ID under two profiles is two signals:
  neither is a repeat of the other (by-linear-long-entry new,
  by-linear-long-entry-as-bn new, by-linear-long-entry duplicate,
  by-linear-long-entry-as-bn duplicate)
- `entry-then-exit`: An entry and an exit on one bar are two signals, and a
  repeated exit is a duplicate like any other (by-linear-long-entry new,
  by-linear-long-exit new, by-linear-long-exit duplicate)
- `next-bar`: The same alerts on the next bar are new signals
  (by-linear-long-entry new, another-bar new, by-linear-long-exit new,
  by-linear-long-exit-next-bar new)
- `two-alerts-one-bar`: Two alerts with their own names on one bar are two
  signals (by-linear-long-entry new, another-source new)
- `spacing-and-order`: Spacing, the order of members and escapes are not part
  of a signal (by-linear-long-entry new, compact duplicate,
  members-in-another-order duplicate, escaped-text duplicate)
<!-- end: delivery cases -->

## What TradingView leaves open

TradingView's documentation does not say everything the templates rely on,
and none of it was tried against a live alert here. Each open point is an
item of [futures-venues.md](futures-venues.md#unverified-items), owned by
the alert-setup story (ALG-754), and the contract holds whichever way it
resolves:

<!-- begin: unverified items -->
- 16: whether a resend renders an alert's placeholders again
- 17: the `{{ticker}}` and `{{exchange}}` of each venue's perpetual charts
- 18: what a plot without a value renders
- 19: how a strategy script renders a payload's texts
- 20: that `{{time}}` is never later than `{{timenow}}`
<!-- end: unverified items -->

The first is the one with a consequence. If a resend renders only the fire
time again, it is the same delivery, as designed. If it renders the price
again, it arrives as a conflict and is refused, the first delivery
standing. If it renders the bar time again, at a bar's edge, it is a new
signal: an entry is then held to the age limit and to the position already
held like any other, and an exit is timed at its resend, so it can close a
position of its side that was entered since the first send, at most about
20 seconds earlier. That errs toward flat, never toward exposure.

## What this leaves to later stories

- **The webhook route.** Authenticating a delivery by its URL's token and
  TradingView's certificate, recording it before the answer, and the
  answers themselves are ALG-743's.
- **The store.** Keeping a signal with its ID, its hash and its state, and
  finding a duplicate or a conflict, are ALG-738's, which is also where a
  position's entry is kept for an exit to be held to.
- **Where a profile keeps its age limit.** The policy document has no field
  for one, so until a story adds it every profile runs at the default. The
  type and the rule are here (`SignalAgeLimit`).
- **Arming.** Whether a profile may enter at all is ALG-750's; a signal for
  a profile that is not armed is refused there as `not_armed`.
- **Sizing, sending and protecting.** [risk-sizing.md](risk-sizing.md)
  sizes an admitted entry, and the executor (ALG-745) sends it.
- **A live alert.** Setting alerts up on TradingView, and trying the open
  points above against one, are ALG-754's.
