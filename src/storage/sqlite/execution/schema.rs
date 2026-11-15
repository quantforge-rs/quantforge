//! Schema 10 on SQLite (QUA-206): the execution store's tables, as
//! `docs/storage-schema.md` shows them. The ten a transfer copies each
//! start with their number and their scope; the last three are the
//! store's own.

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(in crate::storage::sqlite) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS signal_inbox (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
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
                  revision INTEGER NOT NULL,
                  signal_json TEXT NOT NULL,
                  received_at_ms INTEGER NOT NULL,
                  judged_at_ms INTEGER,
                  UNIQUE (venue, category, environment, account_kind, account_id, profile_id, signal_id)
                );
                CREATE INDEX IF NOT EXISTS idx_signal_inbox_state
                  ON signal_inbox(state, seq);

                CREATE TABLE IF NOT EXISTS signal_deliveries (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
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
                  fire_time_ms INTEGER NOT NULL,
                  received_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_signal_deliveries_signal
                  ON signal_deliveries(venue, category, environment, account_kind, account_id, profile_id, signal_id, seq);

                CREATE TABLE IF NOT EXISTS execution_operations (
                  operation_id INTEGER PRIMARY KEY AUTOINCREMENT,
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
                  cycle_id INTEGER,
                  replace_state TEXT,
                  revision INTEGER NOT NULL,
                  operation_json TEXT NOT NULL,
                  created_at_ms INTEGER NOT NULL,
                  updated_at_ms INTEGER NOT NULL,
                  UNIQUE (venue, category, environment, account_kind, account_id, operation_key)
                );
                CREATE INDEX IF NOT EXISTS idx_execution_operations_replace
                  ON execution_operations(replace_state, operation_id);

                CREATE TABLE IF NOT EXISTS execution_intents (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
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
                  entry_slot INTEGER,
                  operation_id INTEGER,
                  cycle_id INTEGER,
                  revision INTEGER NOT NULL,
                  intent_json TEXT NOT NULL,
                  created_at_ms INTEGER NOT NULL,
                  updated_at_ms INTEGER NOT NULL,
                  UNIQUE (venue, category, environment, account_kind, account_id, profile_id, signal_id),
                  UNIQUE (venue, category, environment, account_kind, account_id, symbol, entry_slot)
                );
                CREATE INDEX IF NOT EXISTS idx_execution_intents_state
                  ON execution_intents(state, seq);
                CREATE INDEX IF NOT EXISTS idx_execution_intents_position
                  ON execution_intents(venue, category, environment, account_kind, account_id, symbol, state, seq);

                CREATE TABLE IF NOT EXISTS execution_orders (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  profile_id TEXT,
                  operation_id INTEGER NOT NULL,
                  client_order_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  venue_order_id TEXT,
                  phase TEXT NOT NULL,
                  revision INTEGER NOT NULL,
                  order_json TEXT NOT NULL,
                  recorded_at_ms INTEGER NOT NULL,
                  updated_at_ms INTEGER NOT NULL,
                  UNIQUE (venue, category, environment, account_kind, account_id, client_order_id)
                );
                CREATE INDEX IF NOT EXISTS idx_execution_orders_operation
                  ON execution_orders(operation_id, seq);

                CREATE TABLE IF NOT EXISTS execution_fills (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  profile_id TEXT,
                  symbol TEXT NOT NULL,
                  trade_id TEXT NOT NULL,
                  venue_order_id TEXT NOT NULL,
                  operation_id INTEGER,
                  cycle_id INTEGER,
                  fill_json TEXT NOT NULL,
                  time_ms INTEGER NOT NULL,
                  recorded_at_ms INTEGER NOT NULL,
                  UNIQUE (venue, category, environment, account_kind, account_id, symbol, trade_id)
                );
                CREATE INDEX IF NOT EXISTS idx_execution_fills_cycle
                  ON execution_fills(cycle_id, seq);

                CREATE TABLE IF NOT EXISTS position_cycles (
                  cycle_id INTEGER PRIMARY KEY,
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  profile_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  side TEXT NOT NULL,
                  protection TEXT NOT NULL,
                  open_slot INTEGER,
                  revision INTEGER NOT NULL,
                  cycle_json TEXT NOT NULL,
                  opened_at_ms INTEGER NOT NULL,
                  closed_at_ms INTEGER,
                  updated_at_ms INTEGER NOT NULL,
                  UNIQUE (venue, category, environment, account_kind, account_id, symbol, open_slot)
                );
                CREATE INDEX IF NOT EXISTS idx_position_cycles_open
                  ON position_cycles(open_slot, cycle_id);

                CREATE TABLE IF NOT EXISTS margin_reservations (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  profile_id TEXT NOT NULL,
                  operation_id INTEGER NOT NULL UNIQUE,
                  symbol TEXT NOT NULL,
                  pool TEXT NOT NULL,
                  state TEXT NOT NULL,
                  revision INTEGER NOT NULL,
                  reservation_json TEXT NOT NULL,
                  reserved_at_ms INTEGER NOT NULL,
                  released_at_ms INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_margin_reservations_pool
                  ON margin_reservations(venue, environment, account_kind, account_id, pool, state, seq);

                CREATE TABLE IF NOT EXISTS execution_incidents (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
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
                  revision INTEGER NOT NULL,
                  incident_json TEXT NOT NULL,
                  opened_at_ms INTEGER NOT NULL,
                  cleared_at_ms INTEGER,
                  UNIQUE (venue, category, environment, account_kind, account_id, incident_key)
                );
                CREATE INDEX IF NOT EXISTS idx_execution_incidents_state
                  ON execution_incidents(state, seq);

                CREATE TABLE IF NOT EXISTS outbox_events (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  profile_id TEXT,
                  event_key TEXT NOT NULL,
                  kind TEXT NOT NULL,
                  delivery TEXT NOT NULL,
                  revision INTEGER NOT NULL,
                  event_json TEXT NOT NULL,
                  created_at_ms INTEGER NOT NULL,
                  delivery_at_ms INTEGER,
                  UNIQUE (venue, category, environment, account_kind, account_id, event_key)
                );
                CREATE INDEX IF NOT EXISTS idx_outbox_events_delivery
                  ON outbox_events(delivery, seq);

                CREATE TABLE IF NOT EXISTS position_leases (
                  venue TEXT NOT NULL,
                  category TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  symbol TEXT NOT NULL,
                  fence INTEGER NOT NULL,
                  holder TEXT,
                  profile_id TEXT,
                  activation_epoch INTEGER,
                  acquired_at_ms INTEGER,
                  expires_at_ms INTEGER,
                  PRIMARY KEY (venue, category, environment, account_kind, account_id, symbol)
                );

                CREATE TABLE IF NOT EXISTS reservation_pools (
                  venue TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  pool TEXT NOT NULL,
                  revision INTEGER NOT NULL,
                  PRIMARY KEY (venue, environment, account_kind, account_id, pool)
                );

                CREATE TABLE IF NOT EXISTS store_control_events (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  from_activation TEXT NOT NULL,
                  to_activation TEXT NOT NULL,
                  activation_epoch INTEGER NOT NULL,
                  changed_at_ms INTEGER NOT NULL
                );
"#;
