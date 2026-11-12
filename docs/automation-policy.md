# Automation policies

An automation profile's **policy** is what an operator sets and a signal
never changes: the defaults assets trade on, the settings of the assets that
have their own, and the capital the profile risks in each settlement asset.
This page is the contract of it (**policy format 1**): what a policy holds,
how it changes, what an entry is held to, and what a store keeps. It is what
the CLI of ALG-750 is written against, and it stands without one: the
`PolicyStore` port, `automation::PolicyService` over it, and the shared
cases of
[`tests/fixtures/policy_cases.json`](../tests/fixtures/policy_cases.json).
`tests/policy/docs.rs` holds this page to the code and to those cases.

It implements ADR 0002 §5 and §6 for the requirements
[automation-requirements.md](automation-requirements.md) lists as OBS-R009,
OBS-R017, OBS-R025, OBS-R029, OBS-R037 and OBS-R038 with the decisions D07,
D08 and D13 (the automation PRD (iteration 3), §2.1 on p.4 and §3 to §3.3 on
pp.5–7). Sizing an entry from a policy is ALG-736's, arming a profile and
the commands are ALG-750's, and admitting a signal is ALG-743's.

## Contents

- [Profiles](#profiles)
- [What a policy holds](#what-a-policy-holds)
- [What an asset trades on](#what-an-asset-trades-on)
- [Changes and revisions](#changes-and-revisions)
- [What an entry is held to](#what-an-entry-is-held-to)
- [The venue's say](#the-venues-say)
- [The service](#the-service)
- [What a store keeps](#what-a-store-keeps)
- [The shared cases](#the-shared-cases)
- [What this leaves to later stories](#what-this-leaves-to-later-stories)

## Profiles

A profile is one account's automation on one product family. Its identity
is fixed when it is created and never changes:

- its **ID**, the operator's name for it: 1 to 32 of `a-z`, `0-9`, `-` and
  `_`, starting with a letter or digit;
- its **account**: the venue, the environment (`mainnet`, `testnet` or
  `demo`) and the account's ID there, or the fingerprint of its API key
  where the venue issues none (never on mainnet);
- its **product family**, `linear` or `inverse`;
- its **owner tag**, six lowercase letters and digits its client order IDs
  carry, generated when it is created and shared with no other profile of
  the store.

A profile names an account; it never holds a credential. A store takes
profiles of the one environment it is bound to
([storage-schema.md](storage-schema.md#the-stores-record)).

The same symbol on two venues, or in two families, is two instruments in
two profiles: `BTCUSDT` on one venue and `BTCUSDT` on the other share no
risk setting, no capital and no switch, and neither does `BTCUSDT` in a
linear profile with `BTCUSD` in an inverse one.

## What a policy holds

A policy is one document. Every revision of it is whole: nothing of a
policy lives anywhere else.

<!-- begin: policy format -->
1
<!-- end: policy format -->

<!-- begin: policy fields -->
- `format`: the version of this form; a document of another is refused,
  never guessed at
- `identity`: the profile, as above: its `id`, its `account_ref`, its
  `category` and its `owner_tag`
- `revision`: 1 for a new profile, and one more with every change
- `label`: a name shown beside the ID, 1 to 64 characters, or `null`;
  changing it changes nothing the profile trades
- `defaults`: the settings an asset trades on where it has none of its own
- `assets`: the assets with settings of their own, in the byte order of
  their symbols, each symbol once
- `capital`: the capital in each settlement asset, in the byte order of
  the assets' codes, each asset once
<!-- end: policy fields -->

The defaults:

<!-- begin: defaults fields -->
- `enabled`: whether an asset with no settings of its own takes entries on
  the defaults; `false` until an operator says otherwise
- `risk_percent`: the share of the capital one entry may lose at its stop,
  or `null`
- `stop_percent`: the fallback stop, or `null` for none
- `leverage`: the leverage an entry is opened at, or `null`
- `revision`: the revision that last changed the defaults
<!-- end: defaults fields -->

An asset's own settings:

<!-- begin: asset fields -->
- `instrument`: the product line (`exchange`), the venue's `symbol`, and the
  asset it settles in (`settle`), as the venue listed it when it was added
- `enabled`: whether the asset takes new entries
- `risk_percent`: its own, or `null` for the defaults'
- `stop`: `"Inherit"` for the defaults' fallback stop, `"Off"` for none
  whatever the defaults say, or `{"Percent": "…"}` for its own
- `leverage`: its own, or `null` for the defaults'
- `revision`: the revision that last changed these settings
<!-- end: asset fields -->

Capital in one settlement asset:

<!-- begin: capital fields -->
- `amount`: the `amount`, above zero, and its `asset`
- `revision`: the revision that set this amount
<!-- end: capital fields -->

The rules every document keeps, whether it is built by a change or read
back from a store:

- **Percentages are in percent, as an operator writes them.** A risk of
  `1` is 1 % of the capital, above 0 and at most 100. A stop of `3` sits
  3 % from the expected entry, below it for a LONG and above it for a
  SHORT, above 0 and below 100. A leverage is at least 1.
- **Decimals are exact text**, kept with the scale they were written with
  (`3.50` stays `3.50`), never a JSON number and never a float.
- **Capital is a setting, never the account's balance**, and is kept per
  settlement asset: USDT for a linear contract, the coin for an inverse one.
  Amounts in different assets are never converted or summed.
- **Whatever a document holds could trade.** Enabled defaults have a risk
  percentage and a leverage, and so has an enabled asset, its own or the
  defaults'. A change that would break this is refused whole.
- **Every asset is on the profile's own product line.**
- **No part is stamped later than the document's own revision.**

A policy of profile `main` at its seventh revision, as it is stored (one
line of compact JSON, in this order of fields):

<!-- begin: policy example -->
```json
{
  "format": 1,
  "identity": {
    "id": "main",
    "account_ref": {
      "venue": "Bybit",
      "environment": "Demo",
      "venue_account_id": {
        "KeyFingerprint": "00000000000000a1"
      }
    },
    "category": "Linear",
    "owner_tag": "k3x9p2"
  },
  "revision": 7,
  "label": null,
  "defaults": {
    "enabled": false,
    "risk_percent": "2",
    "stop_percent": null,
    "leverage": "10",
    "revision": 7
  },
  "assets": [
    {
      "instrument": {
        "exchange": "BybitLinear",
        "symbol": "SOLUSDT",
        "settle": "USDT"
      },
      "enabled": true,
      "risk_percent": "0.5",
      "stop": "Inherit",
      "leverage": null,
      "revision": 5
    }
  ],
  "capital": [
    {
      "amount": {
        "amount": "15000",
        "asset": "USDT"
      },
      "revision": 4
    }
  ]
}
```
<!-- end: policy example -->

## What an asset trades on

Each setting of an instrument is the asset's own where it has one, and the
defaults' otherwise, and a reader is told which
(`ProfilePolicy::effective`, each setting with its `source`, `Asset` or
`Defaults`):

- **Risk and leverage**: the asset's own, else the defaults'.
- **The fallback stop**: `Inherit` takes the defaults', a percentage is the
  asset's own, and `Off` gives none even where the defaults give one.
- **Whether it takes entries**: an asset with settings of its own has a
  switch of its own, and the defaults' switch means nothing to it. An asset
  without any trades only while the defaults are enabled. Defaults enable
  nothing by being set: a new profile's are off, and switching them on is an
  operator's change like any other.
- **Capital**: the profile's capital in the asset the instrument settles
  in, with the revision that set it.

## Changes and revisions

An operator changes a policy in one of these ways. Each is applied to one
revision and makes the next, a whole new document; nothing is changed in
place:

<!-- begin: policy changes -->
- `label`: names the profile, or removes its name
- `defaults`: sets or clears a default, or switches the defaults on or off
- `add_asset`: gives an asset settings of its own, enabled or not; a setting
  left out is the defaults'
- `edit_asset`: sets an asset's own setting, clears one back to the
  defaults', changes its fallback stop, or switches the asset on or off
- `reset_asset`: puts every setting of an asset back on the defaults; the
  asset stays, enabled or disabled as it was
- `set_capital`: sets the capital in one settlement asset
- `clear_capital`: removes the capital in one settlement asset; entries
  settled in it are refused from then on
<!-- end: policy changes -->

- **A change that changes nothing makes no revision.** Setting a value to
  the number it already is, even with another scale (`15000.00` over
  `15000`), leaves the policy, its revision and its stored text as they
  were.
- **A change stamps what it changed.** The new revision is stamped on the
  defaults, on each asset and on each capital amount that differs from the
  revision before, and on nothing else, so a reader sees which revision
  last touched what an entry is sized from.
- **A change is refused whole** when the policy it would make breaks a rule
  above, names an asset the profile does not hold or already holds, or
  names one on another product line. Nothing of a refused change is kept.
- **Of two changes made from one revision, one is stored.** A store appends
  a revision only after the one before it. A caller that read revision N
  and names it is told when the profile has moved on (`PolicyError::Moved`)
  and reads it again; a caller that names none has its change applied to
  whatever is newest. Either way no change is lost under another and none
  is half applied.
- **Every revision is kept.** A store never rewrites or deletes one, so a
  revision's number names exactly one policy for good.

## What an entry is held to

When an entry signal is admitted, two things answer for it, in this order.
First the profile's newest policy answers for the instrument, as its caller
read it from the venue's list at that moment (`PolicyService::entry`):
either the entry is refused, with one of the parity matrix's refusals, or
it gets a **policy snapshot**. Then the snapshot answers for the entry's stop
(`PolicySnapshot::stop_for`, asked with the alert's own stop, if it brought
one). Only an entry both have answered for is admitted, and it keeps its
snapshot for good: `ExternalIntent::admit` takes no entry that has neither
a stop of its own nor a fallback in its snapshot.

The refusals, in the order they are looked for; the first that applies is
the entry's. The first five are `PolicyService::entry`'s, and `no_stop` is
`PolicySnapshot::stop_for`'s:

<!-- begin: entry refusals -->
- `ambiguous_selection`: the instrument is on another product line than the
  profile's, or the venue lists it settled in another asset than the profile
  holds it under
- `not_armed`: the asset is disabled
- `unknown_ticker`: the asset has no settings of its own, and the defaults
  are not enabled
- `outside_universe`: universe rule 1 leaves the instrument out: a dated or
  TradFi contract, one not trading, or a linear one not settled in USDT
- `no_capital_in_settlement_asset`: the profile has no capital in the asset
  the instrument settles in
- `no_stop`: the alert brings no stop, and the policy gives the asset no
  fallback
<!-- end: entry refusals -->

These are the policy's own refusals, and they are all the policy decides.
An entry that passes them is admitted, not yet sized: the sizing rules of
ADR 0002 §6 (ALG-736) still apply before anything is sent, and the leverage
is theirs. The snapshot carries the leverage the operator set, and sizing
holds it to the venue's leverage brackets for the instrument and the
account: a position no bracket holds at that leverage is refused,
`above_leverage_bracket`, never shrunk and never sent.

<!-- begin: snapshot format -->
1
<!-- end: snapshot format -->

<!-- begin: snapshot fields -->
- `format`: the version of this form
- `profile`: the profile's ID
- `account`: the profile's account, the only one the entry may trade on
- `revision`: the revision of the policy the snapshot was taken from
- `instrument`: the instrument, as the venue listed it
- `risk_percent`: the value and its `source`
- `stop_percent`: the fallback stop or `null`, and its `source`
- `leverage`: the value and its `source`
- `capital`: the amount and its asset, the instrument's settlement asset
- `capital_revision`: the revision that set that capital
<!-- end: snapshot fields -->

The snapshot the policy above gives an entry on `SOLUSDT`:

<!-- begin: snapshot example -->
```json
{
  "format": 1,
  "profile": "main",
  "account": {
    "venue": "Bybit",
    "environment": "Demo",
    "venue_account_id": {
      "KeyFingerprint": "00000000000000a1"
    }
  },
  "revision": 7,
  "instrument": {
    "exchange": "BybitLinear",
    "symbol": "SOLUSDT",
    "settle": "USDT"
  },
  "risk_percent": {
    "value": "0.5",
    "source": "Asset"
  },
  "stop_percent": {
    "value": null,
    "source": "Defaults"
  },
  "leverage": {
    "value": "10",
    "source": "Defaults"
  },
  "capital": {
    "amount": "15000",
    "asset": "USDT"
  },
  "capital_revision": 4
}
```
<!-- end: snapshot example -->

- **The exact policy, and its revisions, on every admitted entry.** An
  entry is admitted only with a snapshot of its own profile's policy for
  its own account and instrument (`ExternalIntent::admit`), so one keyed to
  another account, or to another instrument, is never sized from this
  profile's settings; its stored record holds the snapshot from then on.
  `revision` and `capital_revision` name rows a store keeps for good, so
  `PolicyService::policy_at` reads back the policy the entry was admitted
  under, word for word, after any number of edits and restarts.
- **An edit applies to later entries only.** A change makes a new revision
  and leaves every snapshot as it was: a position already open, and the
  stop that protects it, are never resized, moved or removed because a
  setting changed.
- **Which stop an entry uses** (`PolicySnapshot::stop_for`): the alert's own
  stop price comes first and is never replaced by the fallback, even when
  sizing then refuses it; without one, the snapshot's fallback percentage
  applies; without either the entry is refused, `no_stop`, and never
  admitted.
- **A signal's only say is its own stop.** The account, the risk
  percentage, the leverage, the capital and each switch are read from the
  profile and its policy, and from nowhere else. A payload has no field for
  an account, a credential, a risk or a switch, and the profile it reaches
  is chosen by the route it arrived on (ADR 0002 §9), never by what it
  says.
- **Only entries are held to a policy.** Disabling an asset, switching the
  defaults off or removing capital stops new entries on what it covers and
  nothing else: an exit is admitted with no policy at all, and monitoring,
  stop repair and protective closes of a position that is open go on as
  before. A disabled asset keeps its settings, and enabling it takes them
  back.

## The venue's say

Symbols, settlement assets and leverage are the venue's to define, so they
are checked against the instrument list of the profile's own connector
(`InstrumentConnector` for its venue, family and environment), never
against a list kept here. Only the calls that are handed the connector
check: `PolicyService::change` asks no venue and holds a change to the
policy's own rules alone, so a change that adds an asset, enables one or
sets a leverage goes through `add_asset` or `change_checked`:

- **An asset is added as the venue lists it.** `PolicyService::add_asset`
  takes a symbol and reads the instrument, settlement asset included, from
  the venue; a symbol the venue does not list on the profile's product line
  is refused.
- **A change that would enable something the venue does not take is
  refused before it is stored** (`PolicyService::change_checked`). A problem
  the policy already had does not block an unrelated change, so an asset the
  venue delisted can still be disabled.
- **Before a profile may execute**, `PolicyService::check` holds its policy
  to the list. It checks what the policy names: each enabled asset, which
  must be a contract the venue lists, inside the universe, settled as the
  profile holds it, at a leverage the venue takes, with capital in the asset
  it settles in; and the defaults, while they are enabled, only for capital
  in some asset. An empty answer means every enabled asset would find its
  instrument, its leverage and its capital. It says nothing of an
  instrument the policy does not name: one that trades on enabled defaults
  is held to the venue's listing when an entry on it arrives, and refused
  then by name, `outside_universe` or `no_capital_in_settlement_asset`; its
  leverage, like every entry's, is held to the venue's brackets when the
  entry is sized.
- **An entry is held to the listing its caller hands in.**
  `PolicyService::entry` asks no venue. Whoever admits an entry reads the
  venue's instrument list as it stands, refuses the entry itself when the
  venue lists no such symbol, and hands `entry` the listing it found. A
  listing read earlier is not read again here: a stale one is answered for
  as it was handed in.

The problems, in the order an asset is looked at; the first that applies is
the asset's:

<!-- begin: policy problems -->
- `not_listed`: the venue lists no such contract on the profile's product
  line
- `settlement_changed`: the venue lists it settled in another asset than
  the profile holds it under
- `outside_universe`: universe rule 1 leaves it out
- `leverage_out_of_range`: the leverage is outside what the venue takes for
  the instrument
- `leverage_off_step`: the leverage is not on the venue's leverage step
- `no_capital`: no capital is set in the asset it settles in; enabled
  defaults with no capital in any asset are reported the same way
<!-- end: policy problems -->

Precision beyond that belongs to an order, not to a setting: quantities,
ticks, minimums and leverage brackets are applied when an entry is sized
(ALG-736), from the instrument and the account as they are then. The check
here reads an instrument's public leverage range, which catches a setting
no account could use; the brackets an account is really given, some of
them served only to the account itself, are sizing's to apply.

## The service

`automation::PolicyService` is the one way commands and admission reach a
policy. It is built over a `PolicyStore` and a `Clock`, and holds neither a
connection nor a credential of its own.

| Call | What it does |
| --- | --- |
| `create_profile(id, account, category)` | Records a profile with a fresh owner tag and an empty policy: revision 1, no settings, no capital, nothing enabled |
| `profiles()` | Every profile's identity, by ID, byte by byte |
| `policy(profile)` | The newest revision |
| `policy_at(profile, revision)` | The policy as it stood at a revision |
| `history(profile)` | Each revision with the time it was made, oldest first |
| `change(profile, expected, change)` | Makes one change as the next revision, held to the policy's own rules only: no venue is asked; `expected` is the revision the caller read, or none |
| `change_checked(…, venue)` | The same, refused when it would enable what the venue does not take |
| `add_asset(profile, expected, symbol, settings, venue)` | Gives a symbol settings of its own, as the venue lists it |
| `check(profile, venue)` | The problems that keep the profile from executing, and the revision checked |
| `entry(profile, instrument)` | The snapshot an entry is admitted under, or why the policy refuses the entry, for the listing its caller read from the venue: no venue is asked; the entry's stop is then the snapshot's to answer (`stop_for`) |

A change answers the policy it left and whether it made a revision. What a
call refuses is a `PolicyError`: a profile or a revision the store does not
hold, a profile that moved on (`Moved`, with the revision found), a
connector of another product line or environment than the profile's
(`WrongConnector`), a symbol the venue does not list (`NotListed`), a change
the venue does not take (`Rejected`, with each problem), an entry the policy
refuses (`Refused`), or a failure of the model, the store or the venue under
it, each with its own error.

## What a store keeps

A store keeps every profile and every revision through the `PolicyStore`
port, the same on SQLite and on PostgreSQL:
[storage-schema.md](storage-schema.md#keeping-policies) is what it promises,
and the `policies` group of the shared conformance suite holds both backends
to it ([storage-conformance.md](storage-conformance.md#the-policy-store)).
In short: a profile is recorded once with revision 1 of its policy; each
later revision is appended only after the one before it, by one guarded
write; nothing is rewritten or deleted; and a copy to another store carries
profiles and revisions as they are
([storage-migration.md](storage-migration.md)).

## The shared cases

[`tests/fixtures/policy_cases.json`](../tests/fixtures/policy_cases.json)
(format 1) holds the policy's behavior as cases, written by an independent
model, `spikes/futures-contract/policy_cases.py`, never by the Rust code
under test:

```bash
python3 -I spikes/futures-contract/policy_cases.py --write tests/fixtures/policy_cases.json
```

Every case runs once on each of the four product lines, two venues with a
linear and an inverse family each, every line with a profile of its own and
a small instrument list as its venue's adapter would normalize it, so the
same operations are held to the same answers on both venues and in both
families. A step is a change with the document it makes (or `unchanged`, or
the refusal), an instrument's effective settings, an entry with its
snapshot or refusal and the stop it uses, or a check with its problems.

<!-- begin: policy cases -->
- `inheritance`: an asset without settings of its own trades on the defaults,
  setting by setting
- `reset_to_default`: an asset's own settings go back to the defaults, one by
  one or all at once
- `stop_precedence`: the alert's stop comes first; the fallback is the
  asset's, the defaults', or off
- `disabled_asset`: a disabled asset takes no entry and keeps its settings;
  enabling it takes them back
- `defaults_switch`: defaults enable nothing by themselves; switched on, an
  asset without settings trades
- `capital`: capital is set per settlement asset, with the revision that set
  it
- `invariants`: what a document holds could trade: changes that would break
  that are refused whole
- `revision_stamps`: each change stamps its revision on what it changed, and
  on nothing else
- `venue_check`: before a profile may execute, its policy is held to the
  venue's own list
- `two_changes_from_one_revision`: of two changes made from one revision one
  is stored and the other is made again on top of it: either order, and each
  alone
<!-- end: policy cases -->

`tests/policy/` replays them three ways: through the model alone
(`model.rs`); through `PolicyService` on a SQLite store and, with
`QUANTFORGE_TEST_POSTGRES_URL`, on a PostgreSQL one, each store opened again
part-way and every revision read back (`service.rs`); and the races stepped
and run for real, with writers on stores of their own (`races.rs`).
`intent.rs` holds an admitted entry to its snapshot across later edits and a
restart, and `docs.rs` holds this page.

## What this leaves to later stories

- **Arming.** Nothing here arms a profile or lets it execute: a policy is a
  setting, and `check` is the gate an arming command asks before it
  confirms (ALG-750). Until then every profile is unarmed.
- **The commands.** `quantforge` has no policy command yet; ALG-750 puts
  one over the service above.
- **Sizing.** Turning a snapshot, a price and a stop into a quantity is
  ALG-736's, from `spikes/futures-contract/sizing-cases.json`.
- **Admission and the execution store.** Reading a payload, choosing the
  profile by its route, recording the intent with its snapshot and
  everything after it are ALG-737's, ALG-743's and ALG-738's.
- **A venue's own spelling of a ticker in an alert.** A policy holds the
  venue's symbol; mapping an alert's ticker to it belongs with the payload
  contract (ALG-737).
