# The execution store

The **execution store** is where automated trading keeps what it was asked,
what it decided and what it did: the signal inbox, the operations, the
ledger of intents, orders, fills, position cycles and margin reservations,
the incidents, the event outbox, and the leases that say who may act on a
position. This page is its contract: what a store keeps, the one kind of
write it takes, the order that write is checked in, who holds a lease, and
what is never deleted. It is what the webhook route (ALG-743) and the entry
executor (ALG-745) are written against, and it stands without them: the
`ExecutionStore` port, `automation::ExecutionLedger` over it, and the shared
cases of
[`tests/fixtures/execution_cases.json`](../tests/fixtures/execution_cases.json).
`tests/execution/docs.rs` holds this page to the code and to those cases.

It implements ADR 0002 §2 (leases and activation), §5 (one unresolved
entry, no pyramiding, no reversal), §9 (signals and unknown outcomes) and
§15 (the store) for the requirement
[automation-requirements.md](automation-requirements.md) lists as OBS-R007
with the decisions D06, D09, D14, D16 and D23 (the automation PRD
(iteration 3), §7 on p.13). The tables are
[storage-schema.md](storage-schema.md)'s, schema 10; what every backend is
held to is [storage-conformance.md](storage-conformance.md#the-execution-store)'s;
and a store copied into another is
[storage-migration.md](storage-migration.md)'s.

## Contents

- [Whose record it is](#whose-record-it-is)
- [Three identities](#three-identities)
- [The records](#the-records)
- [The unit](#the-unit)
- [The inbox](#the-inbox)
- [Operations](#operations)
- [Leases](#leases)
- [Activation](#activation)
- [Slots](#slots)
- [Reservations](#reservations)
- [Incidents and the outbox](#incidents-and-the-outbox)
- [The ledger service](#the-ledger-service)
- [What a store keeps for good](#what-a-store-keeps-for-good)
- [Waits, commits and failures](#waits-commits-and-failures)
- [The shared cases](#the-shared-cases)
- [What this leaves to later stories](#what-this-leaves-to-later-stories)

## Whose record it is

Every record carries its **scope**: the venue, the product family
(`spot`, `linear` or `inverse`), the environment, the account, and the
profile where one acts. A profile acts on futures only, so a Spot record
names none. A store keeps the records of the one environment it is bound
to ([storage-schema.md](storage-schema.md#the-stores-record)), and of none
before it is bound: a unit, an operation or a lease of another environment
is refused, and nothing of it is written.

The scope is part of what each record is known by. The same signal ID, the
same symbol, the same event key and the same client order ID under two
scopes are two records that never meet: a profile named `main` that trades
`SOLUSDT` on each venue has two inboxes, two positions, two leases and two
margin pools, and an alert sent to both, byte for byte, is two signals.

## Three identities

Three things are identified, each by its own key, and none stands in for
another:

- **A signal** is known by its profile and its ID, with the hash of its
  canonical payload ([signal-payload.md](signal-payload.md)). That alone
  decides whether a delivery is new, the same one again, or another payload
  under the same ID, whatever became of any position since.
- **An operation** is one action that changes something on an exchange: an
  entry, a close, a stop, a change of leverage, a Spot replacement. The
  store numbers it, and its number is in every client order ID it sends.
  It may come from a signal or from a command, and needs neither a signal
  nor a futures position.
- **A position cycle** is one owned position from flat back to flat. It is
  numbered as the operation that opened it. An intent and an operation
  name the cycle they belong to beside their own identity, never as part
  of it, so a retry, a late delivery or a restart never makes a second
  admission out of one signal.

## The records

A record is one JSON document that names its format, kept in the table of
its kind beside the columns the document decides. Every decimal in it is
exact text, scale included. The kinds, the tables that keep them, and the
format this build writes and reads:

<!-- begin: record kinds -->
- `signal`: `signal_inbox`, format `1`. A signal in its profile's inbox:
  the signal, its canonical payload, when it arrived, and what it was
  judged to be.
- `operation`: `execution_operations`, format `1`. One numbered action:
  its kind, the key it was asked for under, its instrument, the signal and
  the cycle it belongs to, and, for a Spot replacement, the orders.
- `intent`: `execution_intents`, format `1`. An admitted signal's intent:
  the entry or the exit, its state, the policy it was admitted under, each
  order sent for it, the operation that carries it out and its cycle.
- `order`: `execution_orders`, format `1`. One order of an operation: its
  client order ID, recorded before its request is sent, and what was
  learned of it since.
- `fill`: `execution_fills`, format `1`. One fill as its venue reported it,
  recorded once.
- `cycle`: `position_cycles`, format `1`. One position cycle: its side, its
  protection, and how it closed.
- `reservation`: `margin_reservations`, format `1`. The margin and the risk
  an entry holds until its order has ended.
- `incident`: `execution_incidents`, format `1`. What halts entries until
  an operator has looked.
- `event`: `outbox_events`, format `1`. One thing to tell, and whether it
  was told.
<!-- end: record kinds -->

`signal_deliveries` holds no record of its own: it is the note of every
delivery, the first of a signal and each one after it.

A document of another format is refused where it is read, never guessed
at. So is a row whose columns no longer say what its document says: the
columns are there to find and to list rows by, and the document is the
record. A row QuantForge wrote always agrees; one that does not was
changed behind the store's back.

The stored form of a reservation, as an example: the fixture's
`reservation-1`, written here over several lines:

<!-- begin: record example -->
```json
{
  "format": 1,
  "scope": {
    "venue": "Bybit",
    "category": "Linear",
    "environment": "Demo",
    "account": {
      "Venue": "71002233"
    },
    "profile": "main"
  },
  "operation": 1,
  "instrument": {
    "exchange": "BybitLinear",
    "symbol": "SOLUSDT",
    "settle": "USDT"
  },
  "pool": {
    "Asset": "USDT"
  },
  "margin": {
    "amount": "205.18548",
    "asset": "USDT"
  },
  "risk": {
    "amount": "149.9274",
    "asset": "USDT"
  },
  "reserved_at_ms": 1792454400000,
  "released": null
}
```
<!-- end: record example -->

A record changes only into the same record, moved as its kind allows:

<!-- begin: successor rules -->
- `signal`: judged once, from `received` to `admitted`, `rejected` or
  `noop`; the signal, its payload and when it arrived never change.
- `operation`: its number, scope, kind, key, instrument and signal never
  change; a cycle, once named, is never replaced; a replacement's replaced
  order never changes, its replacing order keeps its client order ID, and
  its state moves only as the parity matrix's `spot_replace` machine allows.
- `intent`: its state moves along the matrix's `signal_intent` machine or
  stays; the policy it was admitted under never changes, its attempts are
  only added to, and a cycle, once named, is never replaced.
- `order`: its identity never changes, and nothing it had learned is taken
  back: a venue ID, an unknown outcome, a refusal, the venue's report.
- `fill`: never replaced.
- `cycle`: its number, position, profile, side and opening never change;
  its protection moves along the matrix's `futures_position` machine or
  stays.
- `reservation`: released once, and otherwise unchanged.
- `incident`: cleared once, and otherwise unchanged.
- `event`: its delivery ends once, `delivered` or `outcome_unknown`, and
  what it tells never changes.
<!-- end: successor rules -->

The words a column spells a state with:

<!-- begin: state words -->
- signal states: `received`, `admitted`, `rejected`, `noop`
- order phases: `recorded`, `outcome_unknown`, `acknowledged`, `refused`,
  `reported`
- reservation states: `held`, `released`
- release reasons: `opened`, `not_opened`
- incident states: `open`, `cleared`
- incident kinds: `unsettled_entry`, `protection_failure`, `second_writer`
- delivery states: `pending`, `delivered`, `outcome_unknown`
- operation kinds: `entry`, `close`, `stop`, `leverage`, `spot_replace`
- event kinds: `signal_admitted`, `signal_rejected`, `signal_noop`,
  `signal_conflict`, `order_submitted`, `outcome_unknown`, `order_settled`,
  `incident_opened`, `incident_cleared`
<!-- end: state words -->

An order's phase is what is known of it, the most telling first: `reported`
once the venue has reported the order itself, else `refused`, else
`acknowledged` once it has a venue ID, else `outcome_unknown` once its
request ended with no answer, else `recorded`.

## The unit

A store takes one kind of write, the **unit**: everything one step
changes, applied whole or not at all, in one transaction that begins and
ends inside the call. No transaction is ever open while an exchange is
asked anything: a step that needs the exchange is two units with the
request between them, and what the first one recorded is what a process
that dies in between leaves for the next to find.

A unit holds:

- **a delivery**, where it records one: the signal as the delivery brought
  it, and the event to tell if it conflicts;
- **changes**, each a record and what it expects to find where it writes:
  no record, or the revision of the record it replaces. A record's first
  write stands at revision 1, and each replacement at the next;
- **a fence**, where it changes a position: the position, the holder and
  the token of the [lease](#leases) it is written under;
- **a pool's revision**, where it reserves or releases margin: the revision
  its writer read the [pool](#reservations) at;
- **the time** of its writer's clock, which the store stamps its rows with
  and decides nothing by.

A unit is refused, before the store reads anything, when it is not one a
store applies:

- it changes nothing;
- it first records a signal or an operation by a change, or replaces a
  fill: a signal is first recorded by its delivery, an operation by being
  numbered, and a fill once;
- it changes one record twice;
- it changes a record that is part of a futures position, an intent, an
  order, a fill, a cycle or a reservation, without that position's lease,
  under another position's lease, or it changes two positions. The one
  exception is an intent's first write, which admission makes and which
  holds no lease, since nothing is sent for an intent until it is changed;
- it reserves from two pools, changes a reservation without naming its
  pool's revision, or names one and changes none;
- it admits a signal without recording its intent, or first records an
  intent anywhere but in the unit that admits its signal, as `admitted`;
- its delivery's conflict event is not a signal conflict of the same
  profile.

Then the store decides it, in this order, and the first check that fails
is the one answered:

<!-- begin: decision order -->
1. `refused`: a record of another environment than the store's, or any
   record on a store bound to none.
2. `fenced`: the unit names a lease that is not its writer's any more, or
   the store is not active.
3. `duplicate` or `conflict`: the unit's delivery is of a signal the inbox
   already holds. The delivery is noted, a conflict is told to the outbox
   once for each payload, and nothing else of the unit is applied.
4. `stale`: the pool does not stand at the revision the unit names.
5. `stale` or `refused`: each change, in the unit's order. It is stale
   where it does not find what it expects, and refused where it is no
   successor of the record it found.
6. `refused`: a change would take a slot of its position that another
   record holds.
7. `applied`: everything is written.
<!-- end: decision order -->

The answers:

<!-- begin: answers -->
- `applied`: every change was written, with the delivery if there was one.
- `duplicate`: the delivery is one the inbox already holds, with the same
  payload. The answer carries the signal as it was recorded and judged.
- `conflict`: the delivery brought another payload under a recorded
  signal's ID. The answer carries the signal recorded, unchanged.
- `stale`: nothing was written. The answer names the record and the
  revision it stands at, or that there is none, or the revision the pool
  stands at. The writer reads again and decides again.
- `fenced`: nothing was written. Nobody holds the position's lease; or it
  is another holder's, named with its token; or the store is not active at
  the epoch the lease was granted under.
<!-- end: answers -->

A refusal is an error, `invalid_argument`, and writes nothing either. The
difference is in who is at fault: `stale` and `fenced` are what two honest
writers come to when one gets in first, and a refusal is a unit no writer
should have built.

Compare-and-swap on the revision is what makes two workers safe: of two
that read an intent at revision 1 and each write its successor, one is
applied and the other is told the intent stands at revision 2. It is also
what makes a lost answer safe: a writer that never learned what became of
its unit sends it again, and is told `duplicate` for a delivery, or `stale`
with the revision its own write left, where it finds its own record.

## The inbox

Every authenticated delivery is recorded. The first of a signal writes its
record; the unit may also judge it in the same write, which is how an
admission is one transaction: the signal as `admitted`, its intent, and the
event that tells of it, all there or none.

- The same ID with the same payload hash again is a **duplicate**. Nothing
  is done twice: the answer is the first delivery's record, whatever it
  came to, admitted, rejected or still waiting.
- The same ID with another payload hash is a **conflict**
  (the refusal `conflict` of [signal-payload.md](signal-payload.md)). The
  delivery's note keeps the payload it brought, the outbox is told once for
  each such payload, and the signal recorded is unchanged.
- A delivery may be recorded as `received` and judged later, once, by a
  change that names its revision. A signal is `admitted` in the unit that
  records its intent, `rejected` with its refusal and its message, or
  `noop` with a note of why it asks for nothing. Received signals are
  listed oldest first.

The inbox records the profile the route selected, never the route's token
(ADR 0002 §15). A signal ID is deduplicated for as long as the store
lives, since nothing in it is deleted, and in a copy of the store too.

## Operations

`begin_operation` numbers an operation. Asked again under a key the account
already recorded, it answers the operation that key names, as it stands
now, and numbers nothing: a key names one operation of its account for
good, and the same key asked for another kind, instrument, signal or
replaced order is refused. An operation asked for without a key is always
a new one. The executor's keys for a signal's operations are
`sig/<profile>/<ordinal>/<signal ID>`.

The number is the store's, across every venue and account in it, and
client order IDs carry it
([shared-model.md](shared-model.md)): layout 1 is
`qf1-<owner tag>-<epoch>-<operation>-<leg><attempt>`. An order's identity
is recorded, under the position's lease, **before** its request is sent,
and a client order ID is recorded once in an account, so no order is ever
sent under an ID another carries.

A **Spot replacement** (amend, or cancel and replace; ALG-766) is an
operation with no signal, no profile and no position. It names the order
it replaces and, once chosen, the order that replaces it, by client order
ID and by the venue's ID once learned, and it has the parity matrix's
`spot_replace` states. Replacements whose outcome is open, `pending` or
`outcome_unknown`, are listed for whoever resumes them.

## Leases

A futures position is one account's one instrument, whoever's profile acts
on it. Whoever changes it holds its **lease**: one holder at a time, for a
time, under a **fence token**.

- A lease is granted only by an [active](#activation) store. Every grant of
  a position's lease takes the next token, and a released or voided lease
  leaves its token behind, so tokens only rise.
- While a lease lives, another holder is told who has it. Its own holder
  asking again keeps the token and moves the end.
- A lease ends at its end, to the millisecond: from then on it is anyone's
  to take.
- A unit under a lease is applied only while the position's lease row shows
  exactly the unit's holder and token, on a store active at the epoch the
  lease was granted under. **Time is no part of it.** A lease that ran out
  and was taken by no one still fences: its holder's writes are applied,
  and its holder may renew it. One that was taken does not, whatever the
  former holder's clock says: nothing it writes is applied, and it can
  neither renew nor release what is not its own. That is what makes a
  paused or partitioned process harmless once another has taken over.

A lease is the promise of the store that granted it, to one executor. It
is kept in a table of the store's own and is never copied
([storage-migration.md](storage-migration.md)).

## Activation

A store executes only while an operator has it **active**
([storage-schema.md](storage-schema.md#the-stores-record)). The port makes
the transitions, each one transaction with the row of its log:

- `activate` takes a bound store to `active` at the next epoch. An active
  store is not activated again, since each activation is a new epoch.
- `deactivate` takes an active store to `deactivated`, at the epoch it has.
- An activation **voids every lease** in the same transaction, for its own
  holder too. No lease outlives the epoch that granted it, and no lease row
  written behind the store's back is ever held to be the store's own. Each
  position keeps its last token, so its next grant takes the one after.

What activation does not gate: a store that is not active still records a
delivery, judges it, and numbers an operation. It grants no lease, renews
none, and applies no unit written under one. A deactivation therefore
stops every executor of the store at its next write.

`StoreIdentity::authority` answers the epoch a store may execute under, or
why it may not, for arming to be held to (ALG-767). A copy of a store
brings none of it: the target is `inactive` at epoch 0, with no lease and
an activation log of its own that starts empty.

## Slots

Two rules of ADR 0002 §5 are held by the store itself, under whatever
profile acts:

- a position has **one unresolved entry** at a time: an entry that was
  sent, `submitting` or `outcome_unknown`, holds its position's entry slot
  until it is settled;
- a position has **one open cycle** at a time: a cycle holds its position's
  open slot until it is closed.

A change that would take a slot another record holds is refused. An
admitted entry holds no slot: it takes one when its first order is
recorded, under the lease, which is where the rule is decided for good.
The other position of the same account, and the same symbol on the other
venue, have slots of their own.

## Reservations

An entry's margin is reserved when its order is recorded, in the same
unit, and released when the order has ended: `opened` once it filled, and
the venue's own figures show the margin, or `not_opened` when it opened
nothing. Sizing counts what is held against the pool it draws from
([risk-sizing.md](risk-sizing.md)).

A **pool** is one margin pool of one venue account, across its product
lines: an asset's, or the account's in USD. Each pool has a **revision**.
A unit that reserves or releases names the revision its writer read the
pool at, and is applied only while the pool still stands there; every such
unit moves the pool one revision on. So of two entries sized at once
against the same free margin, on two positions under two leases, one is
recorded and the other is told the pool has moved, and is sized again
against what is left. A pool's revision is the store's own: a copy starts
again at 0, with every reservation the source held still counted.

## Incidents and the outbox

An **incident** halts entries until an operator has looked: on one
instrument of its account, or every entry of its profile. It is raised
once under its key, cleared once with a note, and never deleted; exits,
stops and the protective close go on while it is open.

An **event** is one thing to tell, written in the unit that makes it true:
a signal admitted, an order recorded, an outcome unknown, an incident
raised. An event that exists therefore tells of something that happened,
and nothing happens that the outbox was not told. An event is written once
under its key, so one cause is told once however often it is reached.
Pending events are listed oldest first, and an event leaves the list when
its delivery ended: `delivered`, or `outcome_unknown` where the transport
cannot say, which is never sent again on its own. Delivering them is
ALG-749's.

## The ledger service

`automation::ExecutionLedger` is the store's first user: what an executor
records, each step one unit.

- **`admit`** records a delivery for the profile its route selected, with
  what became of it, in one write. What the signal asks is
  `SignalService::resolve`'s to say; what the store holds decides the
  rest. An entry is refused while an incident halts it (`incident_open`),
  on a position already held (`pyramiding`, `reversal`), and while an
  earlier entry on its position is unresolved (`unresolved_entry`). An exit
  is admitted for the open cycle its own profile entered before it fired,
  and bound to it, or for such an entry still on its way, which it then
  waits for; any other exit has nothing to do and is recorded as `noop`.
  Another profile's position on the same account is never an exit's to
  close, whatever its alerts are named. `receive` records a delivery
  without judging it.
- **`record_attempt`** records that an intent's next order is about to be
  sent: the intent `submitting` with the attempt, the order's record, the
  entry's reservation, and the event, under the position's lease. It is
  called **before** the request, never after.
- **`settle`** applies what the venue said of an attempt: the intent moved
  by the evidence, the order's record, each fill not recorded yet, the
  cycle an entry's first fill opens, the reservation released once the
  entry is settled, and the event. Evidence that changes nothing writes
  nothing.
- **`resume`** is what a process does for every unsettled intent it finds
  when it starts, and before any retry: it asks the venue for the client
  order ID that was recorded before the attempt was sent, and **never sends
  an order**. An order the venue holds is settled from its report. One it
  does not hold was recorded and perhaps never sent, which is recorded as
  an unknown outcome first; then an entry is never placed only on ADR 0002
  §9's proof (the propagation delay passed, a verified lookup inside the
  venue's retention, and a position snapshot from after it that is still
  flat), and short of that stays unknown, blocking its position. An exit
  may be sent again, as a new attempt recorded first, since a close can
  only reduce. An entry is never sent twice.

An intent's operation is the one its latest attempt was sent under, which
its client order ID names.

## What a store keeps for good

**Nothing in the execution store is deleted**, by any call of the port or
any command of this release: not a signal, a delivery's note, an intent,
an order, a fill, a cycle, a reservation, an incident or an event. A
signal ID is therefore deduplicated for the store's life, and every
decision can be read back with what it was decided on.

<!-- begin: retention -->
- kept for good: `signal_inbox`, `signal_deliveries`,
  `execution_operations`, `execution_intents`, `execution_orders`,
  `execution_fills`, `position_cycles`, `margin_reservations`,
  `execution_incidents`, `outbox_events`, `store_control_events`
- the store's own, rewritten in place: `position_leases`,
  `reservation_pools`
<!-- end: retention -->

A later release may add a command that reclaims old rows. The rule it has
to keep is written here so that it is not invented then: it never removes
an intent that is not settled, an open cycle, a reservation still held, an
open incident, a pending event, a received signal not yet judged, a
replacement whose outcome is open, or anything one of them refers to: the
signal of an intent, the operation of an order, the fills of a cycle. And
it never removes a signal while a delivery of it could still arrive, which
only an operator can say.

## Waits, commits and failures

The execution store commits and waits as every store does
([storage-durability.md](storage-durability.md)): a unit answered is
durable, and a call held up by another writer fails as `contention`
within its budget and has changed nothing. An executor opens its store
with durability enforced, as live trading does, so a store whose commits
were weakened is refused before anything is recorded in it.

A unit whose write fails partway leaves nothing: no signal without its
intent, no order without its event. A call whose answer is lost
(`outcome_unknown`, a connection gone) is made again, and learns what
became of the first: see [the unit](#the-unit).

On SQLite a unit is one immediate transaction, so it reads and writes as
the database's one writer. On PostgreSQL a unit is one `READ COMMITTED`
transaction that locks what it decides on, in this order: its position's
lease row, one advisory lock for each key it may write a first row under,
its pool's row, and every row it finds. Two units on one position, two
that reserve from one pool, and two that may record the same signal, the
same event or the same order therefore wait for each other, and the second
decides on what the first left, whole. A change of activation locks the
table of leases first, so it waits for every unit under a lease and is
seen by the next. A unit never holds the store's own record locked, so a
copy that begins never waits for a unit that waits for it.

The order a unit lists its changes in decides its answer, and never its
locks. Every unit locks the rows it finds table by table, in the order a
copy takes its tables
([storage-migration.md](storage-migration.md#what-is-copied)), and within
a table by the values that name the row; it then writes its tables in
that same order. So two units never hold a row each that the other waits
for, and a unit that waits for a table a beginning copy holds has written
to none the copy still waits for: neither two units nor a unit and a copy
that begins can be left waiting for each other, for the server to end one
of them as a `conflict`.

## The shared cases

[`tests/fixtures/execution_cases.json`](../tests/fixtures/execution_cases.json)
holds the scenarios every backend passes, each step with its answer, and
every record in them by the exact text a store keeps it as. An independent
reference model wrote it, never the Rust code under test
([storage-conformance.md](storage-conformance.md#the-execution-store) lists
them, with the races and the coded checks). `tests/execution/` runs the
ledger service on both backends against a venue that only answers
queries, with a writer that dies right after each step of a signal's life
in turn, and one killed at no step in particular. The
[transfer's fixture](storage-migration.md#the-canonical-text) holds an
execution store's tables too, and a copy is read back record by record.

## What this leaves to later stories

- The webhook route that authenticates a delivery and answers it with a
  receipt (ALG-743), and the notification transport that empties the outbox
  (ALG-749).
- Sizing an entry, sending it and confirming its fill (ALG-745), the stop
  (ALG-746), exits and their races (ALG-747), and recovery drills against
  real venues (ALG-748). They record what they do through this store.
- Arming a profile, and the commands that activate and deactivate a store
  (ALG-750, ALG-767, ALG-768). Until then `store status` shows a store's
  activation, and no command changes it.
- The second-writer check that reads the activation epoch in client order
  IDs (ALG-767): the store records the incident; nothing raises it yet.
- A copy from PostgreSQL back to SQLite, and the cutover of an active
  store (ALG-768).
- A command that reclaims old rows, under the rule
  [above](#what-a-store-keeps-for-good).
