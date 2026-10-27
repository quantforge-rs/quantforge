# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Added

- a SQLite database from any earlier release is migrated in place when it is opened: schema 1 (0.1.0), 2 (0.2.0) and the development schemas 3 and 4 move forward to schema 5 step by step, each step in one transaction, and `docs/upgrading.md` is the operator's guide; nothing is deleted, and the original JSON of every migrated run and order event stays beside its migrated form
- `quantforge store status`, `store bind` and `store adopt-run`: the database's own record — its identity, the environment it is bound to for life (`mainnet`, `testnet` or `demo`, declared by the operator and checked against the base URL, never inferred from it), the migrations it went through, and the runs an earlier release wrote without a verified identity, which an operator adopts before they can be resumed or closed
- every run records the identity it started under (the store's environment and, for a live run, the fingerprint of the API key), and `trade run --run-id` and `trade close` drive a run only under the identity that recorded it; `monitor status` prints `run_identity`
- `docs/storage-schema.md`: the storage contract — identities, the store activation contract that ALG-738, ALG-767 and ALG-768 implement, enforce and verify, schema ownership, the SQLite DDL and the reviewed PostgreSQL DDL — and `tests/storage/` with the shared identity cases of `tests/fixtures/storage_identity_cases.json`
- `ExchangeId` names product lines: beside `binance_spot` come `binance_linear`, `binance_inverse`, `bybit_spot`, `bybit_linear` and `bybit_inverse`, each a `Venue` (`binance`, `bybit`) with a `Category` (`spot`, `linear`, `inverse`), and `MarketId::category()` gives a market's family; only Binance Spot has an adapter so far, and the CLI is unchanged
- `model` gains execution identity and units: `Environment`, `InstrumentId`, `AccountRef`, venue order, algo-order and trade IDs as text, and `OperationId`; `Price`, `BaseQty`, whole `Contracts`, `Quantity`, `FaceValue`, `Leverage` and `PositionSide`; `Money`, whose arithmetic refuses to mix assets; and `Refusal`, the parity matrix's refusal vocabulary
- `model::futures` types the futures contract of ADR 0002: instruments with their trading rules and universe rule 1, accounts, settings and positions, entry and reduce-only orders, acknowledgements that carry no status or fills, fills, stops and the read-back that alone makes a position protected, ledger records, private events, and QuantForge's 36-character client order IDs
- `ExternalIntent` and `PositionCycle` are the records a store will keep, and their `IntentState` and `ProtectionState` move only on the venue's evidence: an acknowledgement never fills an intent, a "not found" settles an entry only past the 30-second propagation delay with the position still flat, and nothing is protected or closed without a read-back or snapshot that shows it
- `RealizedPnl` keeps gross, fees, funding and net PnL apart in the settlement asset, each with its source and provisional until final; a figure the venue has not reported yet stays pending, never zero, and Spot's `ClosedTrade.gross_quote_pnl` is not reused
- `ports::futures` splits the futures connector ports by what each may do: only `EntryConnector` opens exposure, `ProtectionConnector` can only reduce it, and `FuturesCapabilities` describes how a venue carries the contract without ever marking an operation unsupported; `ports::Clock` is the injected time source
- `cargo test --test parity` checks the intent, position and accounting state machines, the refusal vocabulary and the frozen policy values against the code; `tests/exchange_boundary.rs` keeps each venue's own position fields inside its adapter; `docs/shared-model.md` lists what changes for Spot, the Python bindings and the wire, and what does not

- `docs/parity/` freezes the v0.3 capability parity matrix: `matrix.toml` lists every function QuantForge 0.3 promises on Binance and Bybit, with SQLite and PostgreSQL, as 458 required cells over venue, store, product family, side, surface and artifact, each with an objective pass condition, implementation and evidence stories, the evidence it needs and the contract its area follows; `cells.txt` ledgers the cells so that coverage only shrinks through a recorded scope change, `evidence.jsonl` takes pass or fail records only, the dated universe list records what rule 1 selected, and `cargo test --test parity` checks all of it against the code's own states and error kinds and renders `matrix.md`

- ADR 0002 decides the v0.3 automation architecture: TradingView alerts reach a Rust executor beside, not inside, the candle strategies, which stay unchanged; it fixes instrument, account, environment and position identity, the product universe (USDT-settled linear and coin-settled inverse perpetuals on Binance and Bybit), one-way mode with explicit arming, risk sizing in each instrument's settlement asset, mark-price stops held by the exchange, the response to a failed stop, stale-alert and ownership rules, and what parity across venues and stores means; `docs/automation-requirements.md` maps the automation PRD's requirements to the stories that deliver them, `docs/futures-venues.md` records each venue's products, routes, recent API changes and open questions, and `spikes/futures-contract/` resolves the product universe from public metadata and checks the sizing examples independently

- `docs/adr/` records architecture decisions; ADR 0001 decides how the synchronous storage ports will reach PostgreSQL: tokio-postgres with a deadpool pool on a small runtime the store owns, so no context that calls the ports can hit a nested-runtime panic, rustls with `ring` for TLS, a default-on `postgres` cargo feature, and PostgreSQL 16 to 18; compiling spikes in `spikes/postgres-bridge/` back each rule, and SQLite-only builds are unaffected

- candle segments: a market's closed candles are packed window by window, 1,440 bars of the interval counted from the Unix epoch (one UTC day of `1m` candles), into immutable segments named `sha256:` and the hash of their payload; `docs/segment-spec.md` specifies segment format 1 and records why it is not Arrow IPC
- `storage::segments` implements the format: `encode` writes a window's candles as columns, with every decimal as its exact text so that `1.10` and `1.1` pack differently, `decode` refuses any payload `encode` would not write, `compress` and `decompress` wrap the payload in exactly one zstd frame, and `open` reads a fetched segment back, checking it against its name
- `SegmentStore` is the port for packed segments, and `SqliteStore` implements it: `refresh_segments` packs every closed window whose candles changed since it was last packed, `resolve_segments` resolves a range of open times to the segments that hold it and the tip after them, whose candles are read from the candle store, and `load_segment` returns a segment's stored bytes by name
- writing candles marks the windows whose candles changed, in the same transaction, so a refresh repacks exactly those, and a plan never serves a segment older than its candles; a window that cannot be packed, such as one holding more than 1,440 candles, stays unpacked, keeps every later window in the tip, and is logged as a warning naming `data validate`
- `data sync` packs the market's segments after its first pass and after every pass that writes candles, and prints `segments_built: N`; `DataSyncEngine::with_segments` turns this on for library callers
- `tests/fixtures/segment_cases.json` holds the segment spec's shared cases for every implementation, with payloads and names computed by an independent Python implementation

- `serve run` starts the server other QuantForge instances connect to: it listens on `--listen` (`QF_LISTEN`, default `127.0.0.1:9600`, an IP address and a port; port 0 picks a free one) and prints the address it listens on as `listening: http://…`
- the server answers `GET /healthz` with `ok` while it runs, and `GET /version` with a `protocol::ServerVersion`: its release and the protocol versions it speaks, in a shape that is the same in every protocol version, so that a client can choose its version before its first versioned request
- `serve run` stops on SIGINT or SIGTERM (Ctrl-C on Windows): it stops accepting connections, finishes the requests in progress, prints `shutdown:` and `stopped:` lines, and exits 0; a second signal stops it without waiting for them, and it exits 1
- `--segment-cache-dir` (`QF_SEGMENT_CACHE_DIR`, default `data/segments`) names the directory `serve run` caches candle segments in, and is created at startup; like `--listen`, it reads its variable only when the flag is absent, and every other per-command option is still a flag only

- `protocol` defines the wire format between QuantForge instances as serde types: a backtest job's submission (a Python bundle named by its content hash, the candles it runs on, and the backtest settings), its status and result references, the error envelope, candle segment index entries, and the live stream's frames; its rustdoc is the wire specification, and every JSON example in it is a doctest
- the protocol is versioned: `protocol::PROTO_VERSION` and `protocol::MIN_SUPPORTED` bound the versions a build speaks, an HTTP request names its version in its path (`/v1/…`), and `VersionRange::negotiate` picks the newest version two peers share or says which side is too old; `stream::HelloVersions` reads the versions out of a hello of any version, so that a server can refuse a client whose version it does not speak with its own range
- protocol decimals travel as JSON strings and are read exactly: a JSON number, an exponent, or a value a decimal cannot hold without rounding is refused
- a frame type, error code, gap reason, or job state a receiver does not know reads as `Unrecognized` instead of failing, so that a version can grow without breaking older peers
- `tests/protocol_boundary.rs` compiles `src/protocol/` on its own and scans its sources, so that the module uses nothing else from the crate and depends only on `serde`, `serde_json`, `rust_decimal`, `sha2`, `thiserror`, and `toml`
- `docs/bundle-spec.md` specifies the strategy bundle a remote backtest runs: explicit files and a `quantforge.toml` manifest naming the entry module and class, the engine versions and Python version it runs on, its parameters, and its pinned packages — never a pickled object
- `protocol::bundle` implements the spec: `Bundle::from_files` and `Bundle::from_dir` check a bundle against the path rules, the allowed extensions (`.py`, `.toml`, `.json`, `.txt`), the size limits of `BundleLimits`, and the manifest, and report every problem with a shared code and a message that says what to change
- a bundle's code hash is the SHA-256 of its canonical tar, which sorts the files and fixes every timestamp, owner, and mode, so the same files hash the same on every machine; `Bundle::code_hash` gives it, and `Bundle::canonical_tar` the bytes a client uploads
- `Bundle::from_canonical_tar` reads an upload back and refuses any archive that is not exactly the canonical tar of its files, and `Bundle::admit` refuses a bundle whose engine, Python version, or pinned packages the server's `Runtime` does not offer
- `tests/fixtures/bundle_cases.json` holds the spec's shared cases for every implementation, the Python SDK's included, with code hashes computed by an independent Python implementation

- `exchange::conformance` writes down what every exchange connector must do for the engines as checks: candles on the UTC-millisecond grid with exact decimals and closed-bar semantics, kline window bounds, paging, and gaps, the symbol rules and the trading rules they impose, the market-order lifecycle, the error kinds, and the exchange clock; `exchange::conformance::run` runs a check against any connector factory and refuses a production endpoint before sending anything
- `tests/conformance/` runs every check against every registered exchange, replayed by `cargo test` from fixtures recorded on the testnet or, for the fault checks no testnet produces on demand, written by hand; `CONFORMANCE_MODE=testnet` runs every other check live against the testnet, and `CONFORMANCE_MODE=record` re-records the fixtures through a proxy that strips the credentials, signatures, and account id, and writes only checks that replay exactly as they ran
- each check is proven able to fail: a control corrupts the connector the way that check exists to catch, and the check must fail naming it
- `ExchangeErrorKind` classifies every `ExchangeError` in terms every exchange shares (`ExchangeError::kind`), and tells a request the exchange refused before processing it (`unavailable`, safe to resend) from one that may have taken effect (`outcome_unknown`, so an order is looked up before it is sent again), and a server error is always `outcome_unknown`, whatever its code; Binance maps its documented error codes and HTTP statuses onto the kinds
- `ExchangeError::api` builds the error an exchange reported
- `docs/exchange-feature-matrix.md`, generated from the fixtures, lists each exchange's capabilities beside its conformance results, and `docs/adding-an-exchange.md` describes adding an exchange
- a scheduled `conformance` workflow runs the suite's live checks against Binance Spot testnet every day, outside the pull-request checks

- `--exchange` selects the exchange by its registry name, with `QF_EXCHANGE` as its fallback and `binance` as the default; any other value fails before anything runs, listing the registered exchanges
- the registry names its exchanges: `exchange::lookup` and `exchange::name` map between an `ExchangeId` and the name operators select it by, `exchange::registered` lists every exchange, `exchange::DEFAULT_EXCHANGE` is the default, and `exchange::default_base_url` gives each one's production endpoint
- `tests/exchange_boundary.rs` also fails when code or `--help` outside `src/exchange/binance/` names Binance beyond the registry entry, the `ExchangeId` variant, and the refused old base-URL names

- `ExchangeCapabilities` describes what an exchange supports — market orders sized by base or quote quantity, client order ids, the candle page size, symbol rules, server time, and streams — so callers branch on capabilities instead of on `ExchangeId`; Binance Spot declares its own
- `exchange::connect` builds an exchange's connectors from an `ExchangeId` and a `ConnectorConfig` through a registry that maps every exchange id to a factory; `ExchangeConnectors::signed` returns the trading and account connectors only when credentials were supplied
- `MarketDataConnector::server_time` reads the exchange clock, `GET /api/v3/time` on Binance
- `tests/exchange_boundary.rs` fails when any code outside `src/exchange/` names a concrete exchange adapter

- `BacktestEngine` documents its execution model — bar-close signals, next-open fills, the basis-point fee on every fill, close-out at the last close, close-marked equity — and its determinism guarantee, with a runnable example that backtests a strategy defined outside the crate

- `CandleBatch` holds candles as parallel columns — `open_time_ms`, `close_time_ms`, `open`, `high`, `low`, `close`, `volume`, and `trades` — for the planned Python bindings, and converts to and from `Vec<Candle>` without loss; `CandleBatch::from_columns` builds one from a `CandleColumns` and rejects columns of different lengths
- `decimal_to_f64` and `decimal_from_f64` implement the decimal ↔ float64 policy in `docs/decimal-float-policy.md`: the nearest float64 one way, the shortest round-trip decimal the other, and an error instead of rounding for NaN, infinities, and values a decimal cannot hold; `tests/fixtures/decimal_float_cases.csv` pins the cases for Rust and Python alike

- the crate documentation describes the module tree innermost first, which library entry point each CLI command wraps, and which names are public API
- `docs/architecture.md` maps every CLI command to the file that implements it and gives each source file a one-line note; the `README.md` module overview links to it
- `CONTRIBUTING.md` documents how to add a CLI command: the file to create under `src/cli/<group>/`, the shared flattened arguments, and the snapshots, docs, and tests that change with it
- `scripts/file-size-check.sh` enforces a line budget on every `src/**/*.rs` file, counting raw lines including in-file tests: more than 400 lines warns and more than 600 fails; ci runs it on pull requests targeting `main`, and `scripts/release-check.sh` runs it before building
- `scripts/file-size-allowlist.txt` grants a file a justified ceiling above 600 lines; the file still fails above that ceiling, and its entry fails once the file is back within 600 lines or no longer exists
- `docs/configuration.md` lists every setting with its flag, environment variable, default, and scope, plus the environment variables read by dependencies; the `README.md` configuration section links to it
- `--api-key` and `--api-secret` global flags supply the exchange's API credentials on the command line, with `QF_API_KEY` and `QF_API_SECRET` as fallbacks; a flag wins over its environment variable
- `--help` names the credential environment variables but never prints their values
- a credential passed as a flag on an interactive terminal logs a one-line warning that it may be saved in shell history and is visible in process listings, recommending the environment variables instead
- codeql static analysis workflow running on pull requests targeting `main`, pushes to `main`, and a weekly schedule
- gitleaks secret scanning workflow covering the full git history on the same triggers plus manual dispatch
- ci, codeql, and gitleaks status badges in `README.md`

### Changed

- the venue's order and trade IDs are text everywhere, Spot included: `ExchangeOrder.order_id`, `Fill.trade_id`, `AccountTrade.trade_id` and `order_id`, `ClosedTrade.entry_order_id` and `exit_order_id`, `PositionState.entry_order_id`, and the `order_id` of a cancel or query request are `VenueOrderId`/`VenueTradeId`; `monitor cancel-order --order-id` takes the ID as the exchange spells it, and `order: id=` lines print it without `Some(..)`
- the SQLite schema version is 5; `RunJournalStore::save_run_state` updates a run that `create_run` recorded with its identity and refuses an unknown one; `StorageError` is `#[non_exhaustive]` and `SchemaVersionMismatch` became `SchemaUnsupported`, raised only for a database from a newer build
- the protocol's markets carry a required `category` beside `exchange` (`DataSelection`, `SegmentEntry`, `Subscribe`, `BarClosed`, `GapNotice`): `spot`, `linear` or `inverse`, always `spot` for a candle strategy
- `ExchangeId` is `#[non_exhaustive]`, so a `match` on it outside the crate needs a wildcard arm
- `exchange::default_base_url` and `exchange::factory` return `Option`, `None` for a product line without an adapter, and `exchange::connect` refuses such a line; `exchange::name` returns the venue's name, `binance` as before

- a candle is stored only once it has settled, a second after its close, so `trade run` acts on a just-closed bar about a second later on average, and at most one poll later; its bootstrap now replays `--bootstrap-bars` closed bars, where it used to replay one fewer when the newest stored candle was still forming, which can change what `--bootstrap-enter` decides
- `data sync --follow` with an `--end` at or after the current time keeps polling until the candle at the end has settled
- re-syncing candles that are already stored no longer rewrites their rows; `written` still counts every candle sent to the database
- `upsert_candles` refuses a candle whose open time lies too close to either end of the timestamp range for its segment window to be written

- `ExchangeError::Api` carries the error's `kind` and is `#[non_exhaustive]`: build it with `ExchangeError::api`, and match it with `..`

- `--binance-base-url` and `QF_BINANCE_BASE_URL` are renamed `--base-url` and `QF_BASE_URL`, and `QF_BINANCE_API_KEY` and `QF_BINANCE_API_SECRET` are renamed `QF_API_KEY` and `QF_API_SECRET`; rename exported variables — the old base-URL flag and variable now fail with a rename hint, so a testnet URL exported under the old name can never quietly become production, while the old credential variables are no longer read
- an omitted `--base-url` is the selected exchange's production endpoint, `https://api.binance.com/` for `binance` as before
- `--base-url` is checked before the database is created, so a URL that does not parse no longer leaves an empty database behind
- help, credential errors, and logs describe the exchange instead of naming Binance: the credentials are listed under "Exchange credentials", a missing pair fails with "requires exchange credentials", and a confirmed live run against production warns "live trading against a PRODUCTION endpoint"

- `MarketDataSource` is now `MarketDataConnector`, and `TradingVenue` is split into `TradingConnector` (market orders, cancel, query, open orders) and `AccountConnector` (balances, account trades); all three extend the new `ExchangeConnector`, which reports the exchange id, its capabilities, and whether the endpoint is production
- `KlineRequest`, `MarketOrderRequest`, `CancelOrderRequest`, and `OrderQueryRequest` moved from `ports` to `model`; their crate-root names are unchanged, but `quantforge::ports::KlineRequest` becomes `quantforge::model::KlineRequest`
- `BinanceCredentials` is now the exchange-agnostic `ApiCredentials`, and the crate root no longer re-exports it or `BinanceSpotClient`; build connectors through `exchange::connect`, or reach the adapter at `quantforge::exchange::binance::BinanceSpotClient`
- `DataSyncEngine::new` takes a `&dyn MarketDataConnector`, and `LiveTradeEngine::new` takes a `&dyn MarketDataConnector` and an `Option<SignedConnectors>` instead of a `TradingVenue`
- the CLI reaches the exchange only through the connector traits; every command sends the same requests as before, and the `(PRODUCTION)` marker asks the connector whether its endpoint is production
- `README.md` documents passing credentials as flags or environment variables, their precedence, and how to keep secrets out of shell history and process listings, with a fully flag-driven `monitor status` example
- missing-credential errors from `monitor`, `trade close`, and `trade run --mode live` name both the flags and the environment variables
- an empty or whitespace-only credential counts as missing instead of being sent to the exchange; an empty flag never falls back to the environment variable it overrides
- the `sdk` module is split into `sdk::strategy` (the `Strategy` trait and its `StrategyContext` snapshot), `sdk::indicators`, and `sdk::builtin`, with `StrategyError` staying on `sdk` itself; crate-root re-exports such as `quantforge::Strategy` and `quantforge::SmaCrossStrategy` are unchanged, but the path import `quantforge::sdk::strategies::SmaCrossStrategy` becomes `quantforge::sdk::builtin::SmaCrossStrategy`
- the `model`, `exchange`, and `storage` modules are split into public submodules — `model::candle`, `model::decimal`, `model::interval`, `model::market`, `model::order`, `model::rules`, `model::run`, `model::timestamp`, `exchange::binance`, and `storage::sqlite`; existing paths such as `quantforge::model::Candle` and `quantforge::storage::SqliteStore`, and every crate-root re-export, are unchanged
- gitleaks scanning runs the MIT-licensed gitleaks cli directly instead of `gitleaks-action`, which refuses to run for organization-owned repositories without a license key, and uploads its findings to the Security tab as sarif
- the `repository` and `homepage` links point at `quantforge-rs/quantforge` after the repository moved out of a personal account
- the `backtest`, `data_sync`, and `live` modules moved under `engine`; crate-root re-exports such as `quantforge::LiveTradeEngine` are unchanged, but path imports like `quantforge::live::LiveTradeConfig` become `quantforge::engine::live::LiveTradeConfig`
- `Strategy::on_bar` returns the desired target position as plain data instead of mutating a context callback, making the strategy boundary drivable across an FFI boundary
- `StrategyContext` is now a plain-data snapshot struct with owned `market`, `now_ms`, `cash`, and `position_qty` fields instead of a trait
- `Strategy::name` returns `&str` instead of `&'static str` so foreign strategies can report dynamically owned names
- ci runs on pull requests targeting `main` instead of on every push and pull request, removing duplicate workflow runs
- superseded ci runs for the same pull request are cancelled automatically
- `data validate` exits non-zero when validation issues are found
- the CLI rejects non-positive `--cash` and `--quote-order-qty`, negative `--fee-bps`, and `--poll-secs 0` before touching the database or network
- exchange order responses no longer coerce missing fill data to zero: unreported executed quantity, quote quantity, and fills are `None` on `ExchangeOrder`, and the trading engine refuses to update position state from an order without a reported executed quantity
- closed trades are never recorded with fabricated prices: `trade close` and the live exit update position state, then fail with a clear error instead of writing a trade row when entry or exit fill data is missing
- dry-run applies the same pre-trade checks as live (min-notional on entries); live sells are validated against the exchange minimum (dust is skipped with a warning) and maximum quantity rules
- resuming a run by `--run-id` now verifies market, strategy name and parameters, and execution mode and refuses mismatches; `BotRunState` records its execution mode, and `trade close` refuses runs recorded in dry-run mode
- an executed order is always reflected in position state even when journaling the order event fails; the journaling failure is surfaced afterwards with reconciliation guidance
- an unsellable dust remnant left after an exit or manual close is written off with a warning instead of wedging the run in a permanently open position
- opening a database with a mismatched schema version fails with actionable guidance instead of proceeding silently
- `BotRunState` gained a required `execution_mode` field, and the database holds the candle segment tables (schemas 3 and 4 of the chain above); a run QuantForge 0.2.0 wrote recorded no mode, so it stays as it is until `store adopt-run --execution-mode` states one, and `docs/upgrading.md` says which periods 0.2.0 stored forming candles in (every period a following sync covered, and the tail of a bounded sync whose end had not passed) and how to re-sync them
- data sync summaries report the first synced candle, and bounded syncs warn when the exchange returned no candles for part of the requested window
- startup logs the exchange and its effective base URL, and warns when the database file did not exist and was created empty
- `ms_to_rfc3339` renders out-of-range timestamps as an explicit `invalid-ms(...)` marker instead of a plausible epoch date
- backtests reject non-positive initial cash and negative fees at the engine level; the live bootstrap window uses checked arithmetic
- `trade run --mode live` requires explicit confirmation: an interactive `yes` prompt on a terminal, or `--yes` for unattended runs; non-interactive runs without `--yes` print a preview of the would-be live run and exit without trading
- live-mode confirmations and logs mark production exchange endpoints with `(PRODUCTION)` and a confirmed live run against one logs a warning, replacing the previous default-URL-only startup warning; printed base URLs have userinfo removed
- `data validate` flags non-positive `open`/`high`/`low`/`close` prices and negative volume; zero volume remains valid
- candle gap detection no longer overflows on open times near the representable maximum; an unrepresentable expected open time is reported as a gap

### Removed

- `BinanceCredentials::from_env` and `BinanceCredentials::from_required_env`; the library no longer reads credentials from the process environment, so pass them to `ApiCredentials::resolve` instead
- `BacktestConfig` no longer implements `Default`; construct its fields explicitly instead of relying on fabricated cash and fee values

### Fixed

- `data sync` stored the candle still forming at the end of each pass and never fetched it again, so a following sync kept a snapshot of nearly every candle taken seconds after it opened; it now stores a candle only once it has settled, and fetches the forming one again on a later pass

- `trade close --run-id` refuses a run recorded for a different exchange or symbol instead of selling this command's symbol against that run's position, and settles every refusal before contacting the exchange

- a backtest whose fill or equity arithmetic leaves the decimal range (for example an entry at a price so small that the quantity cannot be represented) returns the new `EngineError::Overflow` instead of panicking

- `Debug` output of `ApiCredentials`, and of any `BinanceSpotClient`, `ConnectorConfig`, or `ExchangeConnectors` holding them, no longer prints the API key or secret
- `Interval::H8` now formats as `8h` instead of `1m`
- re-sync any 8h candles written before this fix; they were stored under the `1m` interval key
- `Symbol` deserialization now validates and normalizes (trim, uppercase, reject empty) exactly like `Symbol::new`, closing the bypass on `state_json`/`raw_json` reloads
- zero-valued exchange filter limits (Binance renders "no constraint" as `0.00000000`, as its testnet does for `MARKET_LOT_SIZE`) are treated as absent rules, so quantity rounding falls back to `LOT_SIZE` instead of receiving a zero step

## [0.2.0] - 2026-03-21

### Changed

- kept the repository as a single Cargo package with internal modules
- replaced the `download` command with the `data` command group
- expanded SQLite from candle storage into candle plus bot journal storage

### Added

- incremental and follow-mode candle synchronization
- live-trading runtime with dry-run and live execution modes
- manual trade close command
- monitor command group for balances, open orders, recent trades, cancel, and manual close
- signed Binance Spot client support for account and order endpoints

## [0.1.0] - 2026-03-18

### Added

- single-package QuantForge CLI project layout
- Binance Spot OHLCV ingestion
- SQLite candle storage
- candle validation tooling
- deterministic event-driven backtest engine
- strategy SDK with SMA crossover example
- CI, packaging checks, and contributor documentation
