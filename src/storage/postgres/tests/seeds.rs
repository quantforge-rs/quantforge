//! Rows for the schema chain's tests: one in every table of each schema,
//! at the edges a step must keep, each list written in the SQL of the
//! schema that first holds its tables.

/// One row in every table of schema 5, at the edges a step must keep:
/// decimal text with its scale, the `i64` extremes, a trade count stored
/// wrapped, a legacy run, and a blob holding every byte value.
pub(super) const SEED: &str = r#"
INSERT INTO candles VALUES
  ('bybit_spot', 'BTCUSDT', '1m', -9223372036854775808, -9223372036854715809,
   '1.10', '1.1', '0.00000000', '79228162514264337593543950335', '-0.5', -1),
  ('bybit_inverse', 'BTCUSD', '1m', 9223372036854775807, 9223372036854775807,
   '1.10', '1.10', '1.10', '1.10', '0.0000000000000000000000000001', NULL);
INSERT INTO bot_runs VALUES
  ('run-recorded', 'bybit_spot', 'BTCUSDT', '1m', 'sma_cross', 'running', '{"run_id":"run-recorded"}',
   1792454400000, 1792454460000, NULL, NULL, 'testnet', 'key_fingerprint', '0123456789abcdef', -- gitleaks:allow
   'recorded', 1792454400000, NULL),
  ('run-legacy', 'bybit_spot', 'BTCUSDT', '1m', 'sma_cross', 'stopped', '{"run_id":"run-legacy"}',
   1700000000000, 1700000060000, 1700000060000, 'stopped by hand', NULL, NULL, NULL,
   'legacy', NULL, '{"run_id":"run-legacy","old":true}');
INSERT INTO order_events(run_id, symbol, side, order_type, status, order_id, client_order_id,
                         transact_time_ms, raw_json, legacy_raw_json) VALUES
  ('run-recorded', 'BTCUSDT', 'BUY', 'MARKET', 'FILLED', '12345678901234567890', 'qf-a',
   1792454400000, '{"orderId":"12345678901234567890"}', NULL),
  ('run-legacy', 'BTCUSDT', 'SELL', 'MARKET', 'FILLED', 'abc-XYZ_09', NULL, NULL, '{}', '{"orderId":7}');
INSERT INTO closed_trades(run_id, symbol, entry_time_ms, exit_time_ms, entry_price, exit_price,
                          qty, gross_quote_pnl, entry_order_id, exit_order_id) VALUES
  ('run-recorded', 'BTCUSDT', 1, 2, '100.10', '101.1', '0.00100000', '-0.00099', '7', NULL);
INSERT INTO segments
  SELECT 'sha256:' || repeat('ab', 32), 1, 256,
         decode(string_agg(lpad(to_hex(b), 2, '0'), '' ORDER BY b), 'hex')
  FROM generate_series(0, 255) AS b;
INSERT INTO segment_index VALUES
  ('bybit_spot', 'BTCUSDT', '1m', 1792454400000, 1792454400000, 1792540740000, 1440,
   'sha256:' || repeat('ab', 32));
INSERT INTO segment_dirty(exchange, symbol, interval, window_start_ms) VALUES
  ('bybit_spot', 'BTCUSDT', '1m', 1792540800000);
"#;

/// A bundle and a job, the rows schema 6 adds.
pub(super) const SEED_6: &str = r#"
INSERT INTO bundles VALUES ('sha256:' || repeat('cd', 32), 3, '\x000102', 1792454400000);
INSERT INTO jobs(job_id, state, bundle_hash, request_json, submitted_at_ms, started_at_ms) VALUES
  (repeat('a', 32), 'running', 'sha256:' || repeat('cd', 32), '{}', 1792454400000, 1792454401000);
"#;

/// A result, the row schema 7 adds.
pub(super) const SEED_7: &str = r#"
INSERT INTO job_results VALUES ('sha256:' || repeat('ef', 32), 3, '\x000102', 1792454400000);
"#;

/// A job's log, the row schema 8 adds.
pub(super) const SEED_8: &str = r#"
INSERT INTO job_logs VALUES (repeat('a', 32), '{"dropped_lines":0,"lines":[]}', 1792454400000);
"#;

/// A profile and the first revision of its policy, the rows schema 9 adds.
pub(super) const SEED_9: &str = r#"
INSERT INTO automation_profiles VALUES
  ('main', 'bybit', 'demo', 'venue', '12345', 'linear', 'k3x9p2', 1792454400000);
INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json) VALUES
  ('main', 1, 1792454400000, '{"format":1}');
"#;

/// One row of each table schema 10 adds: a signal and its delivery, the
/// entry it admitted with its operation, order, fill, cycle, reservation,
/// incident and event, and the store's own lease, pool and activation log.
pub(super) const SEED_10: &str = r#"
INSERT INTO signal_inbox(venue, category, environment, account_kind, account_id, profile_id,
                         signal_id, payload_hash, state, revision, signal_json, received_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'trend:entry:long:SOLUSDT.P:t',
        'sha256:' || repeat('ab', 32), 'received', 1, '{"format":1}', 1792454400000);
INSERT INTO signal_deliveries(venue, category, environment, account_kind, account_id, profile_id,
                              signal_id, disposition, payload_hash, fire_time_ms, received_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'trend:entry:long:SOLUSDT.P:t',
        'new', 'sha256:' || repeat('ab', 32), 1792454399000, 1792454400000);
INSERT INTO execution_operations(venue, category, environment, account_kind, account_id,
                                 profile_id, kind, operation_key, symbol, signal_id, revision,
                                 operation_json, created_at_ms, updated_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'entry', 'sig/main/1/t', 'SOLUSDT',
        'trend:entry:long:SOLUSDT.P:t', 1, '{"format":1}', 1792454400000, 1792454400000);
INSERT INTO execution_intents(venue, category, environment, account_kind, account_id,
                              profile_id, signal_id, kind, symbol, side, state, entry_slot,
                              operation_id, revision, intent_json, created_at_ms, updated_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'trend:entry:long:SOLUSDT.P:t',
        'entry', 'SOLUSDT', 'long', 'submitting', 1, 1, 2, '{"format":1}', 1792454400000,
        1792454401000);
INSERT INTO execution_orders(venue, category, environment, account_kind, account_id, profile_id,
                             operation_id, client_order_id, symbol, phase, revision, order_json,
                             recorded_at_ms, updated_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 1, 'qf1-k3x9p2-1-1-e1', 'SOLUSDT',
        'recorded', 1, '{"format":1}', 1792454401000, 1792454401000);
INSERT INTO execution_fills(venue, category, environment, account_kind, account_id, profile_id,
                            symbol, trade_id, venue_order_id, operation_id, cycle_id, fill_json,
                            time_ms, recorded_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'SOLUSDT', 't-1', 'o-1', 1, 1,
        '{"format":1}', 1792454402000, 1792454402500);
INSERT INTO position_cycles(cycle_id, venue, category, environment, account_kind, account_id,
                            profile_id, symbol, side, protection, open_slot, revision,
                            cycle_json, opened_at_ms, updated_at_ms)
VALUES (1, 'bybit', 'linear', 'demo', 'venue', '12345', 'main', 'SOLUSDT', 'long', 'unprotected',
        1, 1, '{"format":1}', 1792454402000, 1792454402500);
INSERT INTO margin_reservations(venue, category, environment, account_kind, account_id,
                                profile_id, operation_id, symbol, pool, state, revision,
                                reservation_json, reserved_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 1, 'SOLUSDT', 'asset:USDT', 'held',
        1, '{"format":1}', 1792454401000);
INSERT INTO execution_incidents(venue, category, environment, account_kind, account_id,
                                profile_id, incident_key, kind, symbol, state, revision,
                                incident_json, opened_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'op/1/unsettled_entry',
        'unsettled_entry', 'SOLUSDT', 'open', 1, '{"format":1}', 1792454460000);
INSERT INTO outbox_events(venue, category, environment, account_kind, account_id, profile_id,
                          event_key, kind, delivery, revision, event_json, created_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'main', 'sig/main/signal_admitted/t',
        'signal_admitted', 'pending', 1, '{"format":1}', 1792454400000);
INSERT INTO position_leases(venue, category, environment, account_kind, account_id, symbol,
                            fence, holder, profile_id, activation_epoch, acquired_at_ms,
                            expires_at_ms)
VALUES ('bybit', 'linear', 'demo', 'venue', '12345', 'SOLUSDT', 3, 'executor-1', 'main', 1,
        1792454400000, 1792454430000);
INSERT INTO reservation_pools VALUES ('bybit', 'demo', 'venue', '12345', 'asset:USDT', 1);
INSERT INTO store_control_events(from_activation, to_activation, activation_epoch, changed_at_ms)
VALUES ('inactive', 'active', 1, 1792454300000);
"#;

/// A job's provenance, the row schema 11 adds, naming the segment the seed
/// holds.
pub(super) const SEED_11: &str = r#"
INSERT INTO job_provenance VALUES
  (repeat('a', 32), 'sha256:' || repeat('12', 32),
   '{"format":1,"inputs":{"data":{"segments":[{"hash":"sha256:abab"}]}}}', 'postgres', 11,
   1792454402000);
"#;
