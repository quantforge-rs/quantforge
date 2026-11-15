//! Schema 10 on PostgreSQL (QUA-206): the execution store's tables, as
//! `docs/storage-schema.md` shows them and SQLite declares them, column
//! for column. Every number is a `BIGINT`, so one parameter type serves
//! them all; every table a transfer copies is keyed by a number, so none
//! needs a collation of its own.

pub(in crate::storage::postgres) const SCHEMA_10: &str = r#"
CREATE TABLE signal_inbox (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  signal_id TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  state TEXT NOT NULL,
  refusal TEXT,
  revision BIGINT NOT NULL,
  signal_json TEXT NOT NULL,
  received_at_ms BIGINT NOT NULL,
  judged_at_ms BIGINT,
  UNIQUE (venue, category, environment, account_kind, account_id, profile_id, signal_id)
);
CREATE INDEX idx_signal_inbox_state
  ON signal_inbox(state, seq);

CREATE TABLE signal_deliveries (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  signal_id TEXT NOT NULL,
  disposition TEXT NOT NULL,
  payload_hash TEXT NOT NULL,
  payload TEXT,
  fire_time_ms BIGINT NOT NULL,
  received_at_ms BIGINT NOT NULL
);
CREATE INDEX idx_signal_deliveries_signal
  ON signal_deliveries(venue, category, environment, account_kind, account_id, profile_id, signal_id, seq);

CREATE TABLE execution_operations (
  operation_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT,
  kind TEXT NOT NULL,
  operation_key TEXT,
  symbol TEXT NOT NULL,
  signal_id TEXT,
  cycle_id BIGINT,
  replace_state TEXT,
  revision BIGINT NOT NULL,
  operation_json TEXT NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  UNIQUE (venue, category, environment, account_kind, account_id, operation_key)
);
CREATE INDEX idx_execution_operations_replace
  ON execution_operations(replace_state, operation_id);

CREATE TABLE execution_intents (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  signal_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  state TEXT NOT NULL,
  entry_slot BIGINT,
  operation_id BIGINT,
  cycle_id BIGINT,
  revision BIGINT NOT NULL,
  intent_json TEXT NOT NULL,
  created_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  UNIQUE (venue, category, environment, account_kind, account_id, profile_id, signal_id),
  UNIQUE (venue, category, environment, account_kind, account_id, symbol, entry_slot)
);
CREATE INDEX idx_execution_intents_state
  ON execution_intents(state, seq);
CREATE INDEX idx_execution_intents_position
  ON execution_intents(venue, category, environment, account_kind, account_id, symbol, state, seq);

CREATE TABLE execution_orders (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT,
  operation_id BIGINT NOT NULL,
  client_order_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  venue_order_id TEXT,
  phase TEXT NOT NULL,
  revision BIGINT NOT NULL,
  order_json TEXT NOT NULL,
  recorded_at_ms BIGINT NOT NULL,
  updated_at_ms BIGINT NOT NULL,
  UNIQUE (venue, category, environment, account_kind, account_id, client_order_id)
);
CREATE INDEX idx_execution_orders_operation
  ON execution_orders(operation_id, seq);

CREATE TABLE execution_fills (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT,
  symbol TEXT NOT NULL,
  trade_id TEXT NOT NULL,
  venue_order_id TEXT NOT NULL,
  operation_id BIGINT,
  cycle_id BIGINT,
  fill_json TEXT NOT NULL,
  time_ms BIGINT NOT NULL,
  recorded_at_ms BIGINT NOT NULL,
  UNIQUE (venue, category, environment, account_kind, account_id, symbol, trade_id)
);
CREATE INDEX idx_execution_fills_cycle
  ON execution_fills(cycle_id, seq);

CREATE TABLE position_cycles (
  cycle_id BIGINT PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  side TEXT NOT NULL,
  protection TEXT NOT NULL,
  open_slot BIGINT,
  revision BIGINT NOT NULL,
  cycle_json TEXT NOT NULL,
  opened_at_ms BIGINT NOT NULL,
  closed_at_ms BIGINT,
  updated_at_ms BIGINT NOT NULL,
  UNIQUE (venue, category, environment, account_kind, account_id, symbol, open_slot)
);
CREATE INDEX idx_position_cycles_open
  ON position_cycles(open_slot, cycle_id);

CREATE TABLE margin_reservations (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  operation_id BIGINT NOT NULL UNIQUE,
  symbol TEXT NOT NULL,
  pool TEXT NOT NULL,
  state TEXT NOT NULL,
  revision BIGINT NOT NULL,
  reservation_json TEXT NOT NULL,
  reserved_at_ms BIGINT NOT NULL,
  released_at_ms BIGINT
);
CREATE INDEX idx_margin_reservations_pool
  ON margin_reservations(venue, environment, account_kind, account_id, pool, state, seq);

CREATE TABLE execution_incidents (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT NOT NULL,
  incident_key TEXT NOT NULL,
  kind TEXT NOT NULL,
  symbol TEXT,
  state TEXT NOT NULL,
  revision BIGINT NOT NULL,
  incident_json TEXT NOT NULL,
  opened_at_ms BIGINT NOT NULL,
  cleared_at_ms BIGINT,
  UNIQUE (venue, category, environment, account_kind, account_id, incident_key)
);
CREATE INDEX idx_execution_incidents_state
  ON execution_incidents(state, seq);

CREATE TABLE outbox_events (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  profile_id TEXT,
  event_key TEXT NOT NULL,
  kind TEXT NOT NULL,
  delivery TEXT NOT NULL,
  revision BIGINT NOT NULL,
  event_json TEXT NOT NULL,
  created_at_ms BIGINT NOT NULL,
  delivery_at_ms BIGINT,
  UNIQUE (venue, category, environment, account_kind, account_id, event_key)
);
CREATE INDEX idx_outbox_events_delivery
  ON outbox_events(delivery, seq);

CREATE TABLE position_leases (
  venue TEXT NOT NULL,
  category TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  symbol TEXT NOT NULL,
  fence BIGINT NOT NULL,
  holder TEXT,
  profile_id TEXT,
  activation_epoch BIGINT,
  acquired_at_ms BIGINT,
  expires_at_ms BIGINT,
  PRIMARY KEY (venue, category, environment, account_kind, account_id, symbol)
);

CREATE TABLE reservation_pools (
  venue TEXT NOT NULL,
  environment TEXT NOT NULL,
  account_kind TEXT NOT NULL,
  account_id TEXT NOT NULL,
  pool TEXT NOT NULL,
  revision BIGINT NOT NULL,
  PRIMARY KEY (venue, environment, account_kind, account_id, pool)
);

CREATE TABLE store_control_events (
  seq BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  from_activation TEXT NOT NULL,
  to_activation TEXT NOT NULL,
  activation_epoch BIGINT NOT NULL,
  changed_at_ms BIGINT NOT NULL
);
"#;
