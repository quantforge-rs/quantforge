//! The PostgreSQL schema: the DDL the chain in `migrate` runs, the
//! store's control row, and the lock that serializes every `init` on one
//! database. `docs/storage-schema.md` shows the same DDL, and a test holds
//! the two equal line for line.
//!
//! Every name is unqualified, so the tables live in the first schema of the
//! role's `search_path` that exists, normally `public`: one store per
//! database, as one store per SQLite file.

use tokio_postgres::Transaction;
use tokio_postgres::types::Type;

use crate::{StoreActivation, StoreId, now_utc_ms};

/// The advisory lock every `init` takes before it reads the version, so
/// processes starting together migrate one at a time. PostgreSQL scopes it
/// to the database; it is the bytes of `QFSCHEMA`.
pub(super) const LOCK_KEY: i64 = 0x5146_5343_4845_4D41;

/// The tables of the current schema. A database holding any of them but no
/// recorded version is not a QuantForge store, and `init` refuses it.
pub(super) const TABLES: [&str; 16] = [
    "meta",
    "candles",
    "bot_runs",
    "order_events",
    "closed_trades",
    "segments",
    "segment_index",
    "segment_dirty",
    "store_control",
    "schema_migrations",
    "bundles",
    "jobs",
    "job_results",
    "job_logs",
    "automation_profiles",
    "policy_revisions",
];

/// Schema 5, where PostgreSQL starts: the tables of SQLite's schema 5,
/// mapped as ADR 0001 says, with decimals as exact `TEXT`. Every later
/// version adds a step of its own and never edits this text.
pub(super) const SCHEMA_5: &str = r#"
CREATE TABLE meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE candles (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  open_time_ms BIGINT NOT NULL,
  close_time_ms BIGINT NOT NULL,
  open TEXT NOT NULL,
  high TEXT NOT NULL,
  low TEXT NOT NULL,
  close TEXT NOT NULL,
  volume TEXT NOT NULL,
  trades BIGINT,
  PRIMARY KEY (exchange, symbol, interval, open_time_ms)
);

CREATE TABLE bot_runs (
  run_id TEXT PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  strategy_name TEXT NOT NULL,
  status TEXT NOT NULL,
  state_json TEXT NOT NULL,
  started_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  stopped_at_ms BIGINT,
  last_error TEXT,
  environment TEXT,
  account_kind TEXT,
  account_id TEXT,
  identity_source TEXT NOT NULL DEFAULT 'legacy',
  identity_at_ms BIGINT,
  legacy_state_json TEXT
);
CREATE INDEX idx_bot_runs_market
  ON bot_runs(exchange, symbol, interval, strategy_name, updated_at_ms DESC);

CREATE TABLE order_events (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  order_type TEXT NOT NULL,
  status TEXT NOT NULL,
  order_id TEXT,
  client_order_id TEXT,
  transact_time_ms BIGINT,
  raw_json TEXT NOT NULL,
  legacy_raw_json TEXT
);
CREATE INDEX idx_order_events_run
  ON order_events(run_id, seq DESC);

CREATE TABLE closed_trades (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  run_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  entry_time_ms BIGINT NOT NULL,
  exit_time_ms BIGINT NOT NULL,
  entry_price TEXT NOT NULL,
  exit_price TEXT NOT NULL,
  qty TEXT NOT NULL,
  gross_quote_pnl TEXT NOT NULL,
  entry_order_id TEXT,
  exit_order_id TEXT
);
CREATE INDEX idx_closed_trades_run
  ON closed_trades(run_id, seq DESC);

CREATE TABLE segments (
  hash TEXT PRIMARY KEY,
  format INTEGER NOT NULL,
  payload_len INTEGER NOT NULL,
  data BYTEA NOT NULL
);

CREATE TABLE segment_index (
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms BIGINT NOT NULL,
  first_open_ms BIGINT NOT NULL,
  last_open_ms BIGINT NOT NULL,
  bar_count INTEGER NOT NULL,
  hash TEXT NOT NULL,
  PRIMARY KEY (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE segment_dirty (
  id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  exchange TEXT NOT NULL,
  symbol TEXT NOT NULL,
  interval TEXT NOT NULL,
  window_start_ms BIGINT NOT NULL,
  UNIQUE (exchange, symbol, interval, window_start_ms)
);

CREATE TABLE store_control (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  store_id TEXT NOT NULL,
  environment TEXT,
  activation TEXT NOT NULL,
  activation_epoch INTEGER NOT NULL,
  changed_at_ms BIGINT NOT NULL
);

CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  from_version INTEGER NOT NULL,
  applied_at_ms BIGINT NOT NULL,
  quantforge_version TEXT NOT NULL
);
"#;

/// Schema 6 (QUA-358): the bundles a server was sent and its jobs; and, as
/// the first step after schema 5, segment hashes and run IDs collated
/// `"C"`, so that their primary keys serve the byte order a transfer reads
/// them in. A bundle's name is collated so from the start.
pub(super) const SCHEMA_6: &str = r#"
ALTER TABLE segments ALTER COLUMN hash TYPE TEXT COLLATE "C";
ALTER TABLE bot_runs ALTER COLUMN run_id TYPE TEXT COLLATE "C";

CREATE TABLE bundles (
  code_hash TEXT COLLATE "C" PRIMARY KEY,
  size_bytes BIGINT NOT NULL,
  data BYTEA NOT NULL,
  stored_at_ms BIGINT NOT NULL
);

CREATE TABLE jobs (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  job_id TEXT NOT NULL UNIQUE,
  state TEXT NOT NULL,
  bundle_hash TEXT NOT NULL,
  request_json TEXT NOT NULL,
  submitted_at_ms BIGINT NOT NULL,
  started_at_ms BIGINT,
  finished_at_ms BIGINT,
  results_json TEXT,
  failure_json TEXT
);
CREATE INDEX idx_jobs_state
  ON jobs(state, seq);
"#;

/// Schema 7 (QUA-356): the results of the jobs that succeeded, each under
/// its name, which a transfer reads in byte order.
pub(super) const SCHEMA_7: &str = r#"
CREATE TABLE job_results (
  hash TEXT COLLATE "C" PRIMARY KEY,
  size_bytes BIGINT NOT NULL,
  data BYTEA NOT NULL,
  stored_at_ms BIGINT NOT NULL
);
"#;

/// Schema 8 (QUA-355): what is kept of each job's log, under the job's ID,
/// which a transfer reads in byte order.
pub(super) const SCHEMA_8: &str = r#"
CREATE TABLE job_logs (
  job_id TEXT COLLATE "C" PRIMARY KEY,
  log_json TEXT NOT NULL,
  stored_at_ms BIGINT NOT NULL
);
"#;

/// Schema 9 (QUA-36): automation profiles, each under its ID, and every
/// revision of their policies, numbered by the store as they are appended.
/// A profile's ID is collated `"C"` in both tables, so that both keys serve
/// the byte order the IDs are listed and transferred in.
pub(super) const SCHEMA_9: &str = r#"
CREATE TABLE automation_profiles (
  profile_id TEXT COLLATE "C" PRIMARY KEY,
  venue TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  category TEXT NOT NULL,
  owner_tag TEXT NOT NULL UNIQUE,
  created_at_ms BIGINT NOT NULL
);

CREATE TABLE policy_revisions (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  profile_id TEXT COLLATE "C" NOT NULL,
  revision BIGINT NOT NULL,
  changed_at_ms BIGINT NOT NULL,
  policy_json TEXT NOT NULL,
  UNIQUE (profile_id, revision)
);
"#;

/// The store's control record as schema 5 creates it: a fresh ID, no
/// environment, inactive, epoch 0, as SQLite's migration writes it.
pub(super) async fn create_control_row(
    transaction: &Transaction<'_>,
) -> Result<(), tokio_postgres::Error> {
    let store_id = StoreId::generate();
    transaction
        .execute_typed(
            r#"
            INSERT INTO store_control(id, store_id, environment, activation, activation_epoch, changed_at_ms)
            VALUES (1, $1, NULL, $2, 0, $3)
            "#,
            &[
                (&store_id.as_str(), Type::TEXT),
                (&StoreActivation::Inactive.as_str(), Type::TEXT),
                (&now_utc_ms(), Type::INT8),
            ],
        )
        .await?;
    Ok(())
}
