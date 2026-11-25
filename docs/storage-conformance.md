# The storage conformance suite

Every QuantForge storage backend passes one suite: the same scenarios, with
the same answers, whatever keeps the data. SQLite and PostgreSQL pass it,
each directly and through `AnyStore`, and so would any later backend, such
as TimescaleDB. This page lists what the suite
holds a backend to, where its answers come from, and how a backend joins.
[storage-schema.md](storage-schema.md) is the schema the backends share; the
suite is the behavior behind the ports of `src/ports/storage.rs`. The
parity matrix's `storage.conformance` capability names it (ALG-402 there).

The answers are never another backend's. They are written in
[`tests/fixtures/storage_conformance_cases.json`](../tests/fixtures/storage_conformance_cases.json)
by an independent reference model, so a backend that agrees with SQLite on a
wrong answer still fails. `tests/storage/docs.rs` holds the lists below to
the fixture and to the checks coded in `tests/storage/conformance/`.

## Contents

- [What the suite holds](#what-the-suite-holds)
- [The fixtures](#the-fixtures)
- [Registering a backend](#registering-a-backend)
- [PostgreSQL](#postgresql)
- [The races](#the-races)
- [What the suite leaves open](#what-the-suite-leaves-open)

## What the suite holds

Each scenario runs on a fresh, empty store and names the answer of every
step; a group fails listing every scenario that failed. Most are written in
the fixture; the last of each list are coded in `tests/storage/conformance/`
because they read the shared cases of other documents: the identity cases of
[storage-schema.md](storage-schema.md#the-shared-cases) and the segment
cases of [segment-spec.md](segment-spec.md#the-shared-cases).

### The candle store

<!-- begin: candles -->
- `candles.empty_market`: a market without candles reads nothing
- `candles.written_counts_statements`: an upsert counts the candles it was
  given, not the rows it added
- `candles.duplicate_overwrites`: a candle written again under its open time
  replaces every other column
- `candles.every_column_counts`: a candle written again with any one column
  changed is stored with that change
- `candles.later_duplicate_wins`: of two candles with one open time in a
  batch, the later is kept
- `candles.bounds_order_and_limit`: a load is ascending with both bounds
  included and keeps the oldest; recent keeps the newest, ascending
- `candles.empty_ranges`: a range holding no candle, a zero limit and a
  reversed range read nothing
- `candles.exact_bounds`: a bound on a candle's open time includes it, and
  one past it does not
- `candles.markets_isolated`: the same open time on every product line,
  symbol and interval reads back only its own candle
- `candles.decimal_scale`: every decimal column keeps the text it was
  written with, scale included
- `candles.unknown_trades`: an unknown trade count stays unknown and is not
  zero
- `candles.trade_count_range`: every trade count from 0 to the largest
  unsigned 64-bit one reads back unchanged
- `candles.unwritable_batch`: a batch holding a candle no segment window can
  hold is refused whole
- `candles.reopened`: a store opened again on the same database resumes from
  its newest candle
- `candles.inexact_text_refused`: stored text a decimal cannot hold exactly is
  refused by every read that returns it, never rounded
- `candles.coverage`: every market's first and last open time, under its own
  key, and nothing of a key this build cannot read back
- `candles.identity_product_lines`: every product line of the identity cases
  stores its market under the storage spelling the cases give, and reads
  back only its own candle
- `candles.identity_decimals`: every decimal text of the identity cases is
  stored and read back unchanged
- `candles.segment_cases`: the candles of every segment spec case, stored and
  read back, pack to the case's name
<!-- end: candles -->

### The run journal

<!-- begin: journal -->
- `journal.round_trip`: a run reads back in full: position, cursor, error
  and configuration
- `journal.unknown_run`: a run never created reads nothing
- `journal.created_once`: a run is created once, and its first identity
  stays
- `journal.saved_after_created`: a run is saved only after it was created
- `journal.save_keeps_identity`: saving replaces the state and never the
  identity
- `journal.last_save_wins`: the last save replaces the state whatever its
  times, and a refused create changes nothing: one process drives a run at a
  time
- `journal.latest_per_market`: the latest run is the last updated of its
  market and strategy; another strategy, interval, venue or family never
  wins
- `journal.latest_tie`: of runs updated in the same millisecond the one
  started later is the latest, then the greater run ID
- `journal.latest_tie_by_bytes`: on a tie of both times, run IDs compare
  byte by byte, so run-a outranks run-B whatever the database's collation
- `journal.order_events`: order events read back newest first, up to the
  limit, exactly as appended
- `journal.duplicate_event`: an order event appended twice is listed twice
- `journal.closed_trades`: closed trades read back newest first with their
  scale and their missing order IDs
- `journal.inexact_text_refused`: a closed trade whose stored text a decimal
  cannot hold exactly is refused when listed, never rounded
- `journal.runs_apart`: two runs' journals never mix
- `journal.reopened`: a store opened again on the same database loads a run
  for resume
- `journal.wire_forms`: columns hold the wire forms and the JSON the serde
  tokens
- `journal.legacy_unresolved`: a legacy run loads with no environment and no
  account, is found as the latest, and is never driven
- `journal.legacy_without_mode`: a legacy run that recorded no execution
  mode is refused on load, never given one
- `journal.identity_venue_ids`: venue IDs of every shape in the identity
  cases round-trip as text through both journals
- `journal.environments_and_accounts`: runs recorded under two environments
  or two accounts are never one another's, and binding a store later adopts
  no run
<!-- end: journal -->

### The segment store

Its scenarios write candles too, so a backend runs them once its candle and
segment stores both exist.

<!-- begin: segments -->
- `segments.no_candles`: a market without candles is all tip
- `segments.packs_closed_windows`: a refresh packs every closed window and
  leaves the tip
- `segments.fill_order`: candles written in any order pack to the same
  segments
- `segments.nothing_changed`: a second refresh, and an identical resync,
  change nothing
- `segments.changed_back`: a window changed and changed back keeps its
  segment
- `segments.tip_until_next_window`: the window of the newest candle stays in
  the tip until a later window starts
- `segments.backfill_keeps_old_segment`: a backfill reopens its window at
  once, the next refresh packs only that window, and the segment it replaced
  still loads
- `segments.correction_keeps_old_segment`: a corrected window is indexed
  under its new segment, and the segment it replaced stays stored and opens
  under its own name
- `segments.emptied_window`: a marked window with no candles left loses its
  index entry, and its segment stays stored
- `segments.scale_is_a_change`: a candle written again with another scale
  packs to a new segment
- `segments.plan_ranges`: a plan lists the segments a range overlaps, oldest
  first, then the tip
- `segments.long_windows`: three-day and weekly candles stay in the tip of
  their long windows
- `segments.crowded_window`: a window holding more candles than a segment
  keeps itself and every later window in the tip
- `segments.inexact_text`: stored text not in its exact form is left
  unpacked rather than corrected
- `segments.opens_to_its_candles`: a stored segment opens under its name to
  the candles it packs; an unknown name loads nothing
- `segments.shared_name`: the same candles under two product lines share one
  segment, listed by two index entries
- `segments.product_lines_apart`: the same symbol on two product lines keeps
  its own windows: a refresh of one packs none of the other's, and a change
  to one leaves the other's plan as it was
- `segments.spec_cases`: every case of the segment spec, stored and closed
  by a later candle, packs to the case's name
- `segments.altered_bytes_refused`: bytes altered behind the store's back
  load as stored and are refused when opened: another segment's bytes, bytes
  that are no segment, and a payload of a format this build does not read
- `segments.failed_write_back_changes_nothing`: a write-back that fails
  stores no segment and no index entry and leaves every mark, so the next
  refresh packs the window
- `segments.refreshes_racing_writers`: with refreshes racing writers, every
  plan that lists a window packs each correction committed before it, and
  every window ends indexed under exactly its final candles
<!-- end: segments -->

### Durable commits

What a backend's acknowledgement means and how long it may take
([storage-durability.md](storage-durability.md)). Its checks are all coded,
since they hold locks and end connections. A failure only one backend can
show is classified in that backend's own tests:
`src/storage/sqlite/errors.rs` and `src/storage/postgres/tests/durability.rs`.
`tests/storage/crash.rs` kills a writer on each backend.

<!-- begin: durability -->
- `durability.commit_visible`: a write the store answered is there for
  another connection and for a store opened afterwards
- `durability.rollback_leaves_nothing`: a batch that fails partway leaves no
  candle and no mark, and goes through once it can
- `durability.contention_is_bounded`: a write held up by another writer fails
  as contention within its budget and changes nothing, and goes through once
  the writer is done
- `durability.reads_pass_a_held_writer`: a read answers at once while another
  writer holds the write lock
- `durability.reconnects`: a store keeps answering, reads and writes, after
  its connections are gone
- `durability.settings_read_back`: a store reads back every setting its
  commits rely on, and all of them are met
<!-- end: durability -->

### The store's record

`StoreControl`: the store's own record, its migration log, the environment
it is bound to, and the legacy runs an operator adopts. Its scenarios write
runs too, so a backend runs them once its journal and store record both
exist.

<!-- begin: control -->
- `control.new_store`: a new store is inactive at epoch 0, bound to no
  environment, and logs one step to the current schema from 0
- `control.bound_for_life`: a store is bound to one environment for life;
  binding it again to the same one changes nothing
- `control.activation_is_a_new_epoch`: an activation takes a bound store to
  active at the next epoch, a deactivation keeps the epoch, and the log keeps
  every change in order, for a store opened again too
- `control.activation_refusals`: a store bound to no environment is not
  activated, an active one is not activated again, and only an active store is
  deactivated; a refusal changes nothing and logs nothing
- `control.legacy_runs_listed`: legacy runs are listed oldest first, then by
  run ID, with the mode only a run that recorded one has; recorded runs are
  not listed
- `control.adopt_a_legacy_run`: an adopted legacy run takes the verified
  identity, loads, matches only that identity, and keeps its original state
  beside it
- `control.adopt_states_the_missing_mode`: a legacy run that recorded no
  execution mode is adopted only with the mode the operator states, then loads
  with it
- `control.adopt_refusals`: only a legacy run is adopted, only under an
  adopted identity, and a run that recorded its mode refuses another; a
  refusal changes nothing
- `control.store_id_kept`: a store's ID is 32 lowercase hex digits and stays
  the same when the store is opened again
- `control.activations_racing`: of eight activations of one store at once, one
  is made and seven find it done: the store is active at epoch 1, and its log
  holds one row
<!-- end: control -->

### The job store

`JobStore`: the bundles a server was sent, its jobs, their results, their
logs and their provenance. A bundle is stored once under its name; a job
moves from queued to running to one final phase, or straight to cancelled,
each move one guarded change, so a claim never takes a job twice and nothing
changes an ended job; a succeeded job's results are stored in the change that
ends it, each once under its name; what a server kept of a job's log is
stored once, under the job; and so is a job's provenance, with the segments
packed for the job, in one change that names no segment the store lacks. The
scenarios use any bytes, names and text: checking a bundle, a result or a
segment against its name, and reading a log or a provenance, is the
server's.

<!-- begin: jobs -->
- `jobs.bundle_stored_once`: a bundle is stored once under its name, every
  byte kept, and loaded back; an unknown name loads nothing
- `jobs.submitted_queued`: a job is recorded queued with nothing else yet; a
  job ID twice and a bundle the store does not hold are refused
- `jobs.listed_newest_first`: jobs are listed the last submitted first, at
  most as many as asked for
- `jobs.claimed_in_submission_order`: a claim takes the job submitted first of
  those queued and starts it; with none queued it takes nothing
- `jobs.finished_only_from_running`: a running job ends once, succeeded with
  its results or failed with why; a queued or ended job is left as it is
- `jobs.cancelled_queued_or_running`: a queued or running job is cancelled,
  keeping when it started; an ended job stays as it is and an unknown one is
  not found
- `jobs.running_failed_on_restart`: every running job, and nothing else, is
  failed with the reason given, as a server does as it starts
- `jobs.json_kept_verbatim`: a job's request, results and failure are kept as
  the text given, whatever it holds
- `jobs.results_stored_with_the_end`: a succeeded job's results are stored as
  it ends, every byte kept, and loaded back by name; an unknown name loads
  nothing, and a store opened again still holds them
- `jobs.results_of_no_end_not_stored`: an end that changes nothing stores
  nothing: not the results of a job cancelled as it ran, of one that had
  ended, of a queued one or of an unknown one; a failed job stores none
- `jobs.result_stored_once`: a result two jobs produce is stored once under
  its name: the second job ends with its results listed, and the bytes stored
  first stay
- `jobs.log_stored_once`: a job's log is stored once under the job, kept as
  the text given, and read back; a job with none reads nothing, a log stored
  again changes nothing, and a store opened again still holds it
- `jobs.log_of_any_job_held`: a log is stored for a job wherever it stands,
  queued, running, ended or cancelled, and changes nothing else of it; one for
  a job the store does not hold is refused
- `jobs.provenance_recorded_once`: a job's provenance is recorded once under
  the job, its text kept as given with its identity, and read back with the
  time and the store's own backend and schema version; one recorded again
  changes nothing, its segments included, and a job with none reads nothing
- `jobs.provenance_stores_its_segments`: the segments packed for a job are
  stored with its provenance, each once under its name, the bytes stored first
  staying; a later provenance names a segment the store holds without handing
  it again
- `jobs.provenance_names_held_segments`: a provenance naming a segment the
  store neither holds nor is handed is refused, storing nothing, the segments
  handed with it included; one for a job the store does not hold is refused
- `jobs.reopened`: a store opened again keeps every job as it stood, and
  claims on in order
- `jobs.claims_racing`: stores claiming at once from many queued jobs claim
  each exactly once, none twice and none lost
- `jobs.failed_end_changes_nothing`: an end whose results cannot all be
  stored changes nothing: the job still runs and holds no result, and ends
  once they can
- `jobs.failed_provenance_changes_nothing`: a provenance whose record cannot be
  stored changes nothing: none of the segments handed with it is stored, and
  it is recorded once it can
<!-- end: jobs -->

### The policy store

`PolicyStore`: automation profiles, each recorded once with the identity it
was created with, and every revision of their policies, appended one after
another and never changed ([storage-schema.md](storage-schema.md#keeping-policies)).
A step gives a policy as its stored document, policy format 1, and an
answer is held to the stored text, byte for byte. What a policy means, and
what a change makes of it, is not the store's:
[automation-policy.md](automation-policy.md) and the cases of
`tests/fixtures/policy_cases.json` hold that. The scenarios bind the store
to an environment first, so a backend runs them once its store record
exists too.

<!-- begin: policies -->
- `policies.profile_created_once`: a profile is recorded once, with the first
  revision of its policy; recorded again it is refused, and nothing of the
  second is kept
- `policies.owner_tag_of_its_own`: a profile whose owner tag another profile
  of the store holds is refused, and nothing of it is kept
- `policies.environment_of_the_store`: a store bound to no environment takes
  no profile, and a bound one takes only profiles of its own environment
- `policies.first_revision_comes_with_the_profile`: a profile is created with
  revision 1 of its policy and no other, and revision 1 is never written again
- `policies.revisions_appended`: each revision is appended after the one
  before it, read back by its number word for word, and listed with the time
  it was given; the newest is the policy in force
- `policies.one_of_two_from_one_revision`: of two revisions made from the same
  one the first is stored, and the second changes nothing, however often it is
  offered
- `policies.no_revision_skipped`: a revision that does not follow the newest
  one is not stored
- `policies.only_its_own_profile`: a policy is stored only for the profile it
  names: one for a profile the store does not hold is refused, and so is one
  naming another account, product family or owner tag than the profile was
  created with
- `policies.profiles_in_byte_order`: profiles are listed by ID, byte by byte
- `policies.profiles_apart`: each profile has revisions of its own: a revision
  of one changes nothing of another
- `policies.text_kept`: a policy reads back as the text that was stored, every
  decimal with its scale
- `policies.reopened`: a store opened again holds every profile and every
  revision as they stood, and appends on
- `policies.foreign_document_refused`: a row whose document is another
  profile's, another revision's or of another format is refused when read, and
  the rows beside it still read
- `policies.saves_racing`: stores saving at once from one revision store one
  policy each round, whole, and the others change nothing
- `policies.failed_create_changes_nothing`: a profile whose first revision
  cannot be stored is not recorded, and is created once the store takes the
  row
<!-- end: policies -->

### The execution store

`ExecutionStore`: the signal inbox, the operations, the ledger, the outbox
and the leases, whose contract is [execution-store.md](execution-store.md).
Its scenarios are a fixture of their own,
[`tests/fixtures/execution_cases.json`](../tests/fixtures/execution_cases.json),
because every record in them is named once, by the exact text a store keeps
it as: a step hands the store a record read from that text, and a record
the store hands back is held to it byte for byte, and to its revision. What
a record means is the model's; the store's part is what this list holds:
what a unit comes to, in which order its checks answer, who holds a lease,
what an operation is numbered, and what each read lists. The `_racing`
entries are the fixture's races: it sets a store up and names what each of
several writers asks at once, each on a store of its own opened on the same
database, and the suite checks what holds whoever wins. The last entries
are coded, since they refuse writes, hold locks, end connections and change
rows behind the store's back.

<!-- begin: execution -->
- `execution.delivery_recorded_once`: the first delivery of a signal is
  recorded with its admission, its intent and its event in one write; the same
  payload again changes nothing but the note of it, and another payload under
  the signal's ID is a conflict, noted with its payload and told once for each
  payload
- `execution.received_then_judged`: a delivery may be recorded as received and
  judged later, once: admitted with its intent in the same write, rejected
  with its refusal, or asking for nothing; received signals are listed oldest
  first until they are judged
- `execution.unit_shape_refusals`: a unit that is not one a store applies is
  refused before anything is read: one that changes nothing, admits a signal
  without its intent or records an intent without its admission, or with the
  admission of another account's signal of the same name, first records a
  signal or an operation by a change, replaces a fill, changes a record twice,
  by its name or under the key its account knows it by, changes a position
  without its lease or two positions at once, reserves without the revision of
  its pool, or gives its position two unresolved entries or two open cycles
- `execution.lease_fences_writers`: a position's lease goes to one holder at a
  time under the next token; a holder asking again keeps its token; a lease
  that ran out still fences until another holder takes it, and from then on
  nothing its former holder writes is applied, renewed or released
- `execution.activation_gates_execution`: a store that is not active records
  deliveries and numbers operations, and grants no lease; a deactivation stops
  every unit written under a lease at once, and the activation after it voids
  every lease, for its own holder too, so the next grant of each position
  takes the token after its last
- `execution.compare_and_swap_all_or_nothing`: a change names what it expects
  to find, no record or the revision it replaces, and a unit one of whose
  changes finds anything else writes nothing and answers what was found
- `execution.successor_rules`: a record is replaced only by the same record
  moved as its kind allows: an intent along its state machine with its policy,
  its attempts and its cycle kept, an order with nothing it had learned taken
  back, a cycle's protection along its machine, a reservation released once,
  an incident cleared once, an event's delivery ended once; anything else is
  refused and changes nothing
- `execution.one_unresolved_entry_and_one_open_cycle`: a position has one
  unresolved entry and one open cycle at a time, whoever's profile acts on it:
  an entry that was sent holds the slot until it is settled, a cycle until it
  is closed, and another position of the same account has slots of its own
- `execution.reservations_and_the_pool`: a unit that reserves or releases
  margin names the revision it read the pool at, and is applied only while the
  pool still stands there; every reservation and every release moves the pool
  on, each pool of each venue account has a revision of its own, and what is
  held against a pool is added up exactly
- `execution.operations_numbered_once`: the store numbers every operation, and
  an operation asked for again under its key is the one already numbered, as
  it stands now; a key names one operation of its account for good; a Spot
  replacement is an operation with no signal, no profile and no position,
  whose state moves only as the matrix allows and whose orders are never
  renamed, by their client ID or by the venue's once it is known
- `execution.two_venues_one_alert`: the same alert, byte for byte, sent to
  profiles of one name that trade one symbol on two venues is two signals:
  each has its own inbox entry, intent, operation, lease, pool, orders, cycle
  and events, what happens to one changes nothing of the other, and one unit
  may change both
- `execution.orders_fills_and_cycles`: an order's identity is recorded before
  it is sent and then only learns: its venue ID, the venue's report, each fill
  once; an entry's first fill opens the position's cycle, numbered as the
  operation that opened it, an exit sent again is another order of its
  operation, and a fill is known by its symbol and its trade ID, each for
  itself
- `execution.incidents_and_the_outbox`: an incident is raised once under its
  key and cleared once, with the event that tells of it written in the same
  unit; pending events are listed oldest first up to a limit, and an event
  leaves the list when its delivery ended, delivered or not known to be
- `execution.environment_of_the_store`: a store keeps the executions of the
  environment it is bound to and of no other, and of none before it is bound:
  a unit, an operation and a lease of another environment are refused, and
  nothing of them is written
- `execution.unsettled_work_listed_for_recovery`: every intent that is not
  settled is listed, oldest first, for whoever starts on the store, and a
  position's own are listed for whoever holds its lease; an intent leaves both
  lists when it is settled, and a store opened again lists what the one before
  it left
- `execution.inverse_contracts_keep_their_units`: a coin-margined line keeps
  its own units exactly: whole contracts for quantities, the coin for margin,
  risk and fees, each decimal with the scale it was written with, and a signal
  that names its exchange and family keeps both
- `execution.first_deliveries_racing`: of eight deliveries of one signal at
  once, one is recorded with its intent and its event, and seven are
  duplicates that name it: the inbox holds one signal, one intent and one
  event, and a note of all eight
- `execution.conflicting_deliveries_racing`: of eight deliveries at once under
  one signal ID, four with one payload and four with another, one is recorded;
  the three that brought its payload are duplicates and the four that brought
  the other are conflicts, told once
- `execution.same_change_racing`: of four writers that hold one lease and
  write the same change at once, one is applied and three find the intent
  already moved: the order is recorded once
- `execution.leases_racing`: of eight holders asking for one position's lease
  at once, one is granted it under token 1 and seven are told who holds it
- `execution.reservations_racing`: four writers, each on a position of its
  own, reserve from one pool at once, each sizing again when the pool has
  moved: every reservation is made once, and the pool ends four revisions on,
  holding all four
- `execution.operations_racing`: of eight askers for an operation under one
  key at once, all are answered with the same number, and of four askers under
  keys of their own each gets a number no other has
- `execution.records_keep_their_text`: every record of the fixture is read
  from the text a store keeps it as and written again byte for byte
- `execution.failed_unit_changes_nothing`: a unit one of whose writes fails
  leaves nothing of itself, whichever write it was: no signal, no note of its
  delivery, no intent and no event; once the write goes through, the same unit
  is applied whole
- `execution.waits_are_bounded`: a unit and a lease call held up by another
  writer fail as contention within their budget and change nothing, and go
  through once the writer is done
- `execution.lost_answer_asked_again`: a writer that never got its answer asks
  again and learns what became of its write: a delivery sent again is a
  duplicate that names the signal recorded, and a change made again finds its
  own record at the revision it left, by a store whose connections are gone
  too
- `execution.row_changed_behind_the_store_is_refused`: a row whose columns no
  longer say what its document says is refused where it is read and where a
  unit would replace it, and is read again once it is put right
- `execution.lease_row_gives_no_authority`: a lease row written behind the
  back of a store that was never activated fences nothing, and the store's
  activation voids it: a unit written under it is applied neither before nor
  after, and its position's next grant takes the token after the row's
<!-- end: execution -->

## The fixtures

`tests/fixtures/storage_conformance_cases.json` (format 1) holds:

- `markets`: the markets the scenarios name, by storage spelling: the same
  symbol on both venues and every family, beside another symbol and other
  intervals;
- `series_rule`: how a `series` generates its candles, so that thousands of
  candles take one line;
- `runs`, `orders` and `trades`: the records the journal scenarios write,
  every decimal as its exact text;
- `candles`, `journal`, `segments`, `control`, `jobs` and `policies`: the
  scenarios, each an `id`, the statement listed above, and its `steps`;
- `races`: the interleavings each backend steps through itself.

`tests/fixtures/execution_cases.json` (format 1) holds the execution
store's:

- `scopes`, `accounts` and `positions`: whose records the scenarios write,
  each under a name the steps use: one venue account with two profiles on
  its linear line and one on its coin-margined line, a profile of the same
  name that trades the same symbol on the other venue, the first account
  again on testnet, and its Spot line, which has no profile;
- `records`: every record a step names, with its kind and its stored text;
- `scenarios`: each an `id`, the statement listed above, and its `steps`;
- `races`: each an `id`, its statement, the `setup` steps and the `writers`.

A step is one operation and its answer: a result, or a refusal of a kind
(`invalid_argument`) whose message contains a given text.

| Steps | What they check |
| --- | --- |
| `upsert`, `load`, `recent`, `max_open_time` | the candle store, every candle compared as text |
| `coverage` | the markets the candle store holds, by their stored keys, each with its first and last open time, in any order |
| `create_run`, `save_run`, `load_run`, `run_identity`, `latest_run`, `identity_matches` | runs and their identities, every state compared as its JSON |
| `append_order_event`, `list_order_events`, `append_closed_trade`, `list_closed_trades` | the two journals |
| `refresh`, `resolve`, `load_segment` | the segment store; every listed segment must also load, open under its name, and be as long as listed |
| `store_bundle`, `load_bundle`, `submit_job`, `job`, `jobs`, `claim_job`, `finish_job`, `cancel_job`, `fail_running_jobs` | the job store, every record compared as text, its JSON byte for byte |
| `store_record`, `migrations`, `bind_environment`, `activate`, `deactivate`, `control_events`, `legacy_runs`, `adopt_run` | the store's record, its activation and the log of it |
| `commit` | a unit of the execution store: its answer, `applied`, a `duplicate` or a `conflict` naming the signal recorded, `stale` with what was found, `fenced` with why, or a refusal |
| `begin_operation`, `acquire_lease`, `renew_lease`, `release_lease`, `lease` | an operation's number, and a position's lease |
| `signal`, `received_signals`, `deliveries`, `intent`, `position_intents`, `unsettled_intents`, `operation`, `unsettled_replacements`, `order`, `orders`, `fill`, `fills`, `cycle`, `open_cycle`, `open_cycles`, `pool`, `reservation`, `incident`, `open_incidents`, `event`, `pending_events` | the execution store's reads, every record compared with the fixture's and written again to its text |
| `raw_select`, `raw_contains`, `raw_execute`, `value_types` | the backend's own tables, for what the ports do not show: the text a column holds, a row written behind the store's back, the type a value is stored as |
| `reopen` | the same database opened again, as a restarted process opens it |

The reference model is
[`spikes/storage-contract/conformance_cases.py`](../spikes/storage-contract/conformance_cases.py),
written from this page and [segment-spec.md](segment-spec.md), stdlib Python
only. It runs every scenario on its model and records each answer. Its
segment packer is the one the spec prints, and before it writes anything it
reproduces every case of `tests/fixtures/segment_cases.json`, whose payloads
and names an earlier independent implementation computed. To change a
scenario, change the script and run it from the repository root:

```bash
python3 -I spikes/storage-contract/conformance_cases.py
```

The rules it models, beyond what the statements above say:

- a candle is keyed by its market and open time; a batch is refused whole
  when one of its candles lies too close to either end of the timestamp
  range for a segment window to hold it;
- a candle counts as changed when any column's text differs, a trade count
  known or unknown included, and a change marks its window dirty;
- a refresh packs each dirty window below the tip, the window of the newest
  candle: a window that cannot be packed (more than 1,440 candles, or a
  decimal not in its exact form) is counted blocked and stays dirty; one
  that packs to the name it is indexed under is unchanged; any other is
  built, and the segment it had stays stored; one with no candles left
  loses its index entry and counts as built;
- a plan lists the indexed windows below the lower of the tip window and
  the lowest dirty window that overlap the range, then the rest of the range
  as the tip;
- the latest run of a market and strategy is the one updated last, then the
  one started last, then the one with the greatest run ID;
- order events and closed trades are listed newest first, in the order they
  were appended, duplicates kept;
- a read refuses stored decimal text that a `Decimal` cannot hold exactly
  (more than 28 decimal places, or a mantissa of 2^96 or more), naming the
  text, and never rounds it; QuantForge writes only text it can read back,
  so such text was written behind the store's back.

The execution store's reference model is
[`spikes/storage-contract/execution_model.py`](../spikes/storage-contract/execution_model.py),
written from [execution-store.md](execution-store.md): how each record is
spelled where a store keeps it, the columns its document decides, and the
store itself, deciding every unit in the order that page fixes. It takes
the state machines from the [parity matrix](parity/matrix.toml), not from
the code.
[`execution_cases.py`](../spikes/storage-contract/execution_cases.py) runs
the scenarios on it. Each scenario also says what it means its steps to
come to, so the model and the scenario have to agree before anything is
written:

```bash
python3 -I spikes/storage-contract/execution_cases.py
```

`spikes/storage-contract/transfer_cases.py` takes the execution tables of
[the transfer's fixture](storage-migration.md#the-canonical-text) from the
same model.

## Registering a backend

A backend joins the suite in its own file under `tests/storage/`, the way
`sqlite.rs`, `any_store.rs` and `postgres/mod.rs` do:

1. Implement `conformance::Backend`. Its `fresh` returns a `Fixture`: a store
   that is initialized, empty, and isolated from every other store the suite
   opens; a `reopen` that opens the same database again, initialized, as a
   restarted process would; the `Raw` view below; and a guard, dropped last,
   that keeps the database alive and removes it afterwards.
2. Implement `conformance::Raw` over the backend's own client. The
   scenarios' SQL is portable: plain `SELECT`, `INSERT` and `UPDATE` on the
   tables of [storage-schema.md](storage-schema.md), every selected column
   text or `CAST(… AS TEXT)`, no function of one dialect. A few things are
   written per backend: `value_types` (SQLite's `typeof`, PostgreSQL's
   `pg_typeof`), `blob` (a literal of bytes), `write_refusal` (how a table is
   made to refuse new rows: a trigger, a constraint), `hold_writes` (how
   another writer holds a table up) and `end_connections`.
3. Add one test per port the backend implements, calling its group:
   `conformance::candles::<B>()` once it implements `CandleStore`,
   `conformance::journal::<B>()` with `RunJournalStore`,
   `conformance::control::<B>()` with `RunJournalStore` and `StoreControl`,
   `conformance::segments::<B>()` with `CandleStore` and `SegmentStore`,
   `conformance::jobs::<B>()` with `JobStore`,
   `conformance::policies::<B>()` with `PolicyStore` and `StoreControl`,
   `conformance::execution::run::<B>()` with `ExecutionStore`, `StoreControl`
   and a bounded view of the store (`conformance::Bounded`), and
   `conformance::durability::<B>()` with the candle store, the journal, the
   store's record and the bounded view. The groups are generic over the
   ports, so a backend switches each on as its store lands.
4. Implement `DirtyWindows` (`src/storage/refresh.rs`) for the store, and
   step through the races in its own unit tests, as
   `src/storage/sqlite/segments/tests.rs` does; see [the races](#the-races).

A later backend, such as TimescaleDB, registers the same way and reuses the
fixture as it is. A behavior a new backend adds is a new scenario, written
in the reference model under an `id` of its own; an existing answer never
changes for one backend.

## PostgreSQL

`tests/storage/postgres/mod.rs` registers PostgreSQL. Every fixture creates a
database of its own, `qf_conformance_<pid>_<n>`, on the server
`QUANTFORGE_TEST_POSTGRES_URL` names, opens it through `PostgresStore`, and
drops it with `DROP DATABASE … WITH (FORCE)` (PostgreSQL 13 or later) when
the fixture goes, so scenarios never share rows and a run leaves nothing
behind. Without the variable its test skips, unless
`QUANTFORGE_TEST_POSTGRES_REQUIRED` is `1`, as in CI: then every test that
would skip fails instead ([ci.md](ci.md#postgresql)).

The URL connects as a user allowed to create databases, over plain TCP; the
store connects to each isolated database through the checks of
[configuration.md](configuration.md#storage-backends), which allow plain
text only to a loopback host. CI's service containers and a local cluster
both serve that. `scripts/postgres-test-servers.sh start` runs CI's two
servers with Docker ([ci.md](ci.md#running-them-locally)); a local cluster,
with Homebrew:

```bash
PATH="$(brew --prefix postgresql@16)/bin:$PATH"
initdb -D pg -U quant --auth=scram-sha-256 --pwfile=<(printf 'local-pw\n')
printf "port = 54329\nlisten_addresses = 'localhost'\nunix_socket_directories = ''\n" >> pg/postgresql.conf
pg_ctl -D pg -l pg.log -w start
QUANTFORGE_TEST_POSTGRES_URL='postgres://quant:local-pw@127.0.0.1:54329/postgres?sslmode=disable' \
  cargo test --test storage postgres
pg_ctl -D pg -w stop
```

The same variable takes the commands there too: `cargo test --test e2e`
runs every command that reads or writes the store on SQLite and on a database
of its own on the server, and proves that none of them leaves a SQLite file.
One of those tests creates a role of its own, so its user must be allowed that
as well.

Each fixture's store, and each one `reopen` opens, has run
`PostgresStore::init`, so every scenario starts on a new store's schema
(QUA-125). Every group runs on PostgreSQL: candles (QUA-124), the journal
and the store's record (QUA-123), segments (QUA-112) and durable commits
(QUA-111), then all five again behind `AnyStore`. The journal and
store-record groups run once more
on a database whose collation is `en_US.UTF-8`, where only byte order keeps
a tie on run IDs SQLite's, but only when the server can create one: a server
without that locale skips the run, saying so, so byte order is not proven on
every setup. CI's servers have the locale, and CI runs every group on
PostgreSQL 16, 17 and 18 ([ci.md](ci.md#postgresql)).
The same command also runs the schema's own tests, the candle store's
writes raced against a refresh's write-back, writers appending to one run
at once, and the query plans of
[postgres-query-plans.md](postgres-query-plans.md); `tests/live_engine.rs`
runs the engine's dry runs and resume checks on PostgreSQL with the same
variable. With it, `cargo test --lib postgres` steps [the races](#the-races)
through the store's own read and write-back, and proves what the ports
cannot show: a write-back that waits on a correction's lock finds its
revision, a window and a plan stay one snapshot while a commit lands, and
each failure the server reports falls in its category.

Two checks need more than one server. `QUANTFORGE_TEST_POSTGRES_WEAK_URL`
names a server with `fsync = off` in its configuration file, whose refusal
`cargo test --lib postgres` and `cargo test --test e2e` then show; CI and
`scripts/postgres-test-servers.sh` give one, and without it those tests
skip. The other runs on demand: `QUANTFORGE_TEST_POSTGRES_DATA_DIR` names the
data directory of the server
`QUANTFORGE_TEST_POSTGRES_URL` reaches, for the test that crashes it, with
`pg_ctl` on the `PATH`:

```bash
cargo test --test storage crash::postgres::acknowledged_writes_survive_the_server_crashing \
  -- --ignored --exact
```

Every group, race and check passed on PostgreSQL 16.15 on 2026-11-04, and
the whole suite on 16.15, 17.11 and 18.6 on 2026-11-07. The `policies`
group, added later, passed on 16.15 and 18.6 on 2026-11-12, under a
linguistic collation too.

## The races

A refresh reads a dirty window, packs it with no transaction open, then
writes it back only if no writer marked the window again meanwhile. The
ports cannot pause a refresh between those steps, so the fixture lists the
interleavings, and each backend steps through them with its own read, pack
and write-back:

<!-- begin: races -->
- `races.write_between_read_and_write_back`: a write between a refresh's
  read and its write-back keeps the window dirty, and the next refresh packs
  the new candles
- `races.two_refreshes`: of two refreshes packing one window, the later
  finds it done elsewhere
- `races.identical_write_between_read_and_write_back`: a write that changes
  nothing between a refresh's read and its write-back leaves the mark as it
  was, and the write-back publishes
- `races.stale_refresh_after_a_newer_one`: a refresh that read before a
  correction finds the window done elsewhere once a later refresh published
  it, and publishes nothing over it
<!-- end: races -->

A write-back answers `done`, `elsewhere` when another refresh unmarked the
window first, or `dirty` when a writer marked it again; the plan and refresh
counts after each race are in the fixture too. Both backends step through
them with one stepper, `src/storage/races.rs`, called from
`src/storage/sqlite/segments/tests.rs` and
`src/storage/postgres/tests/segments.rs`.

## What the suite leaves open

- Appending an order event or a closed trade to a run that was never
  created: both backends' schemas allow it, and no command does it.
- A segment's stored bytes: their length depends on the compression level,
  which is not part of the [segment format](segment-spec.md#compression).
  The suite checks only that a plan lists the length its bytes have.
- Concurrency beyond the races above, `segments.refreshes_racing_writers`
  and the `_racing` entries of the other groups, and how fast a backend
  answers. Which of several writers at once is the one applied is never
  held, only what holds whoever it is.
- What an execution record means: whether a signal may be admitted, an
  order sent or a cycle closed is the model's and the executor's to say
  ([execution-store.md](execution-store.md)), and the model's own tests
  hold it.
- The rest of an error's message, beyond the text a refusal names.
