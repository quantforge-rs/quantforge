//! The execution store's statements (`src/storage/postgres/execution/`),
//! each with the index its plan must use, and the data they are planned
//! over. A read lists every column of its table, as the store does; the
//! values pick one profile, position and record of the data.

use super::Query;

/// The account most of the queries name.
macro_rules! scope {
    () => {
        "venue = 'bybit' AND category = 'linear' AND environment = 'demo' \
         AND account_kind = 'venue' AND account_id = '71002233'"
    };
}

/// The states of an intent that is not settled, as a read lists them.
macro_rules! unsettled {
    () => {
        "('admitted', 'submitting', 'outcome_unknown', 'partially_filled')"
    };
}

/// A read: its name, the table it must never scan whole, the indexes its
/// plan uses, and its text in parts.
macro_rules! read {
    ($id:literal, $table:literal, [$($index:expr),*], $($sql:tt)*) => {
        Query {
            id: $id,
            indexes: &[$($index),*],
            never_scanned: &[$table],
            write: false,
            sql: concat!($($sql)*),
        }
    };
}

const SIGNAL_KEY: &str = "signal_inbox_venue_category_environment_account_kind_accoun_key";
const OPERATION_KEY: &str = "execution_operations_venue_category_environment_account_kin_key";
const INTENT_KEY: &str = "execution_intents_venue_category_environment_account_kind_a_key";
const ENTRY_SLOT: &str = "execution_intents_venue_category_environment_account_kind__key1";
const ORDER_KEY: &str = "execution_orders_venue_category_environment_account_kind_ac_key";
const FILL_KEY: &str = "execution_fills_venue_category_environment_account_kind_acc_key";
const OPEN_SLOT: &str = "position_cycles_venue_category_environment_account_kind_acc_key";
const INCIDENT_KEY: &str = "execution_incidents_venue_category_environment_account_kind_key";
const EVENT_KEY: &str = "outbox_events_venue_category_environment_account_kind_accou_key";

pub(super) const QUERIES: &[Query] = &[
    read!(
        "execution.signal",
        "signal_inbox",
        [SIGNAL_KEY],
        "SELECT * FROM signal_inbox WHERE ",
        scope!(),
        " AND profile_id = 'main' \
         AND signal_id = 'trend:entry:long:SYM0USDT.P:40000' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.received_signals",
        "signal_inbox",
        ["idx_signal_inbox_state"],
        "SELECT * FROM signal_inbox WHERE state = 'received' ORDER BY seq LIMIT 100"
    ),
    read!(
        "execution.deliveries",
        "signal_deliveries",
        ["idx_signal_deliveries_signal"],
        "SELECT * FROM signal_deliveries WHERE ",
        scope!(),
        " AND profile_id = 'main' \
         AND signal_id = 'trend:entry:long:SYM0USDT.P:40000' ORDER BY seq"
    ),
    read!(
        "execution.operation",
        "execution_operations",
        ["execution_operations_pkey"],
        "SELECT * FROM execution_operations WHERE operation_id = 40000 ORDER BY operation_id \
         FOR UPDATE"
    ),
    read!(
        "execution.operation_by_key",
        "execution_operations",
        [OPERATION_KEY],
        "SELECT * FROM execution_operations WHERE ",
        scope!(),
        " \
         AND operation_key = 'sig/main/1/trend:entry:long:SYM0USDT.P:40000' \
         ORDER BY operation_id FOR UPDATE"
    ),
    read!(
        "execution.unsettled_replacements",
        "execution_operations",
        ["idx_execution_operations_replace"],
        "SELECT * FROM execution_operations WHERE replace_state IN ('pending', \
         'outcome_unknown') ORDER BY operation_id"
    ),
    read!(
        "execution.intent",
        "execution_intents",
        [INTENT_KEY],
        "SELECT * FROM execution_intents WHERE ",
        scope!(),
        " AND profile_id = 'main' \
         AND signal_id = 'trend:entry:long:SYM0USDT.P:40000' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.entry_slot",
        "execution_intents",
        [ENTRY_SLOT],
        "SELECT * FROM execution_intents WHERE ",
        scope!(),
        " AND symbol = 'SYM196USDT' \
         AND entry_slot = 1 ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.position_intents",
        "execution_intents",
        ["idx_execution_intents_position"],
        "SELECT * FROM execution_intents WHERE ",
        scope!(),
        " AND symbol = 'SYM196USDT' \
         AND state IN ",
        unsettled!(),
        " ORDER BY seq"
    ),
    read!(
        "execution.unsettled_intents",
        "execution_intents",
        ["idx_execution_intents_state"],
        "SELECT * FROM execution_intents WHERE state IN ",
        unsettled!(),
        " ORDER BY seq"
    ),
    read!(
        "execution.order",
        "execution_orders",
        [ORDER_KEY],
        "SELECT * FROM execution_orders WHERE ",
        scope!(),
        " \
         AND client_order_id = 'qf1-k3x9p2-1-40000-e1' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.orders",
        "execution_orders",
        ["idx_execution_orders_operation"],
        "SELECT * FROM execution_orders WHERE operation_id = 40000 ORDER BY seq"
    ),
    read!(
        "execution.fill",
        "execution_fills",
        [FILL_KEY],
        "SELECT * FROM execution_fills WHERE ",
        scope!(),
        " AND symbol = 'SYM0USDT' \
         AND trade_id = 't-40000' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.fills",
        "execution_fills",
        ["idx_execution_fills_cycle"],
        "SELECT * FROM execution_fills WHERE cycle_id = 15000 ORDER BY seq"
    ),
    read!(
        "execution.cycle",
        "position_cycles",
        ["position_cycles_pkey"],
        "SELECT * FROM position_cycles WHERE cycle_id = 15000 ORDER BY cycle_id FOR UPDATE"
    ),
    read!(
        "execution.open_cycle",
        "position_cycles",
        [OPEN_SLOT],
        "SELECT * FROM position_cycles WHERE ",
        scope!(),
        " AND symbol = 'SYM196USDT' \
         AND open_slot = 1 ORDER BY cycle_id FOR UPDATE"
    ),
    read!(
        "execution.open_cycles",
        "position_cycles",
        ["idx_position_cycles_open"],
        "SELECT * FROM position_cycles WHERE open_slot = 1 ORDER BY cycle_id"
    ),
    read!(
        "execution.reservation",
        "margin_reservations",
        ["margin_reservations_operation_id_key"],
        "SELECT * FROM margin_reservations WHERE operation_id = 40000 ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.pool",
        "margin_reservations",
        ["idx_margin_reservations_pool"],
        "SELECT * FROM margin_reservations WHERE venue = 'bybit' AND environment = 'demo' \
         AND account_kind = 'venue' AND account_id = '71002233' AND pool = 'asset:USDT' \
         AND state = 'held' ORDER BY seq"
    ),
    read!(
        "execution.incident",
        "execution_incidents",
        [INCIDENT_KEY],
        "SELECT * FROM execution_incidents WHERE ",
        scope!(),
        " \
         AND incident_key = 'op/4000/unsettled_entry' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.open_incidents",
        "execution_incidents",
        ["idx_execution_incidents_state"],
        "SELECT * FROM execution_incidents WHERE state = 'open' ORDER BY seq"
    ),
    read!(
        "execution.event",
        "outbox_events",
        [EVENT_KEY],
        "SELECT * FROM outbox_events WHERE ",
        scope!(),
        " \
         AND event_key = 'op/40000/submitted:1' ORDER BY seq FOR UPDATE"
    ),
    read!(
        "execution.pending_events",
        "outbox_events",
        ["idx_outbox_events_delivery"],
        "SELECT * FROM outbox_events WHERE delivery = 'pending' ORDER BY seq LIMIT 100"
    ),
    read!(
        "execution.lease",
        "position_leases",
        ["position_leases_pkey"],
        "SELECT fence, holder, profile_id, activation_epoch, acquired_at_ms, expires_at_ms \
         FROM position_leases WHERE ",
        scope!(),
        " AND symbol = 'SYM196USDT' FOR UPDATE"
    ),
    Query {
        id: "execution.replace",
        indexes: &[INTENT_KEY],
        never_scanned: &["execution_intents"],
        write: true,
        sql: concat!(
            "UPDATE execution_intents SET state = 'filled', entry_slot = NULL, revision = 3, \
             intent_json = '{}', updated_at_ms = 2 WHERE ",
            scope!(),
            " AND profile_id = 'main' \
             AND signal_id = 'trend:entry:long:SYM0USDT.P:40000' AND revision = 2"
        ),
    },
    Query {
        id: "execution.record",
        indexes: &[EVENT_KEY],
        never_scanned: &[],
        write: true,
        sql: "INSERT INTO outbox_events (venue, category, environment, account_kind, \
              account_id, profile_id, event_key, kind, delivery, revision, event_json, \
              created_at_ms) VALUES ('bybit', 'linear', 'demo', 'venue', '71002233', 'main', \
              'op/40000/settled:filled', 'order_settled', 'pending', 1, '{}', 2) \
              ON CONFLICT (venue, category, environment, account_kind, account_id, event_key) \
              DO NOTHING",
    },
    read!(
        "transfer.intents",
        "execution_intents",
        ["execution_intents_pkey"],
        "SELECT * FROM execution_intents WHERE seq > 40000 ORDER BY seq LIMIT 256"
    ),
];

/// Four profiles' lines: two profiles of one account's linear line, the
/// same profile name on the other venue, and the first account's
/// coin-margined line.
const LINES: &str = "(VALUES (0, 'bybit', 'linear', 'venue', '71002233', 'main'), \
     (1, 'bybit', 'linear', 'venue', '71002233', 'second'), \
     (2, 'binance', 'linear', 'key_fingerprint', '00000000000000a1', 'main'), \
     (3, 'bybit', 'inverse', 'venue', '71002233', 'coin')) \
     AS l(n, venue, category, account_kind, account_id, profile_id)";

/// An execution store some years in, over four profiles' lines and 200
/// symbols: 60,000 signals with 80,000 deliveries and as many operations
/// and orders, 54,000 intents of which 40 are entries still on their way,
/// 100,000 fills over 20,000 cycles of which 30 are open, 54,000
/// reservations of which 40 are held, 5,000 incidents of which 5 are open,
/// 200,000 events of which 100 are pending, and a lease row for each of
/// 800 positions.
pub(super) fn seed() -> String {
    format!(
        "INSERT INTO signal_inbox(venue, category, environment, account_kind, account_id, \
                                  profile_id, signal_id, payload_hash, state, refusal, revision, \
                                  signal_json, received_at_ms, judged_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'trend:entry:long:SYM' || (i % 200) || 'USDT.P:' || i, \
                'sha256:' || lpad(to_hex(i), 64, '0'), \
                CASE WHEN i > 59950 THEN 'received' WHEN i % 10 = 9 THEN 'rejected' \
                     ELSE 'admitted' END, \
                CASE WHEN i <= 59950 AND i % 10 = 9 THEN 'stale' END, 1, \
                '{{\"format\":1,\"signal\":' || i || '}}', 1792454400000 + i * 1000, \
                CASE WHEN i <= 59950 THEN 1792454400000 + i * 1000 END \
         FROM generate_series(1, 60000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO signal_deliveries(venue, category, environment, account_kind, account_id, \
                                       profile_id, signal_id, disposition, payload_hash, \
                                       fire_time_ms, received_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'trend:entry:long:SYM' || (j % 200) || 'USDT.P:' || j, \
                CASE WHEN i <= 60000 THEN 'new' ELSE 'duplicate' END, \
                'sha256:' || lpad(to_hex(j), 64, '0'), 1792454400000 + j * 1000, \
                1792454400000 + i * 1000 \
         FROM generate_series(1, 80000) AS i, LATERAL (SELECT 1 + (i - 1) % 60000 AS j) AS x \
              JOIN {LINES} ON l.n = j % 4; \
         INSERT INTO execution_operations(venue, category, environment, account_kind, \
                                          account_id, profile_id, kind, operation_key, symbol, \
                                          signal_id, cycle_id, replace_state, revision, \
                                          operation_json, created_at_ms, updated_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'entry', \
                'sig/' || l.profile_id || '/1/trend:entry:long:SYM' || (i % 200) || 'USDT.P:' || i, \
                'SYM' || (i % 200) || 'USDT', \
                'trend:entry:long:SYM' || (i % 200) || 'USDT.P:' || i, \
                CASE WHEN i <= 20000 THEN i END, NULL, 1, '{{\"format\":1,\"id\":' || i || '}}', \
                1792454400000 + i * 1000, 1792454400000 + i * 1000 \
         FROM generate_series(1, 60000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO execution_operations(venue, category, environment, account_kind, \
                                          account_id, kind, operation_key, symbol, \
                                          replace_state, revision, operation_json, \
                                          created_at_ms, updated_at_ms) \
         SELECT 'bybit', 'spot', 'demo', 'venue', '71002233', 'spot_replace', 'cmd/' || i, \
                'BTCUSDT', CASE WHEN i > 190 THEN 'pending' ELSE 'accepted' END, 2, \
                '{{\"format\":1}}', 1792454400000 + i, 1792454400000 + i \
         FROM generate_series(1, 200) AS i; \
         INSERT INTO execution_intents(venue, category, environment, account_kind, account_id, \
                                       profile_id, signal_id, kind, symbol, side, state, \
                                       entry_slot, operation_id, cycle_id, revision, \
                                       intent_json, created_at_ms, updated_at_ms) \
         SELECT venue, category, environment, account_kind, account_id, profile_id, signal_id, \
                'entry', 'SYM' || (seq % 200) || 'USDT', 'long', \
                CASE WHEN seq > 59900 THEN 'submitting' ELSE 'filled' END, \
                CASE WHEN seq > 59900 THEN 1 END, seq, CASE WHEN seq <= 20000 THEN seq END, 2, \
                '{{\"format\":1,\"intent\":' || seq || '}}', received_at_ms, received_at_ms + 500 \
         FROM signal_inbox WHERE state = 'admitted' ORDER BY seq; \
         INSERT INTO execution_orders(venue, category, environment, account_kind, account_id, \
                                      profile_id, operation_id, client_order_id, symbol, \
                                      venue_order_id, phase, revision, order_json, \
                                      recorded_at_ms, updated_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, i, \
                'qf1-k3x9p2-1-' || i || '-e1', 'SYM' || (i % 200) || 'USDT', \
                CAST(8000000000 + i AS TEXT), 'reported', 3, \
                '{{\"format\":1,\"operation\":' || i || '}}', 1792454400000 + i * 1000, \
                1792454400000 + i * 1000 + 900 \
         FROM generate_series(1, 60000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO execution_fills(venue, category, environment, account_kind, account_id, \
                                     profile_id, symbol, trade_id, venue_order_id, \
                                     operation_id, cycle_id, fill_json, time_ms, recorded_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'SYM' || (i % 200) || 'USDT', 't-' || i, CAST(8000000000 + i AS TEXT), \
                1 + i % 60000, 1 + i % 20000, '{{\"format\":1,\"fill\":' || i || '}}', \
                1792454400000 + i * 600, 1792454400000 + i * 600 + 50 \
         FROM generate_series(1, 100000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO position_cycles(cycle_id, venue, category, environment, account_kind, \
                                     account_id, profile_id, symbol, side, protection, \
                                     open_slot, revision, cycle_json, opened_at_ms, \
                                     closed_at_ms, updated_at_ms) \
         SELECT i, l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'SYM' || (i % 200) || 'USDT', 'long', \
                CASE WHEN i > 19970 THEN 'protected' ELSE 'closed' END, \
                CASE WHEN i > 19970 THEN 1 END, 3, '{{\"format\":1,\"cycle\":' || i || '}}', \
                1792454400000 + i * 3000, \
                CASE WHEN i <= 19970 THEN 1792454400000 + i * 3000 + 2000 END, \
                1792454400000 + i * 3000 + 2000 \
         FROM generate_series(1, 20000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO margin_reservations(venue, category, environment, account_kind, \
                                         account_id, profile_id, operation_id, symbol, pool, \
                                         state, revision, reservation_json, reserved_at_ms, \
                                         released_at_ms) \
         SELECT venue, category, environment, account_kind, account_id, profile_id, \
                operation_id, symbol, \
                CASE WHEN category = 'inverse' THEN 'asset:BTC' ELSE 'asset:USDT' END, \
                CASE WHEN entry_slot = 1 THEN 'held' ELSE 'released' END, \
                CASE WHEN entry_slot = 1 THEN 1 ELSE 2 END, \
                '{{\"format\":1,\"operation\":' || operation_id || '}}', created_at_ms, \
                CASE WHEN entry_slot IS NULL THEN updated_at_ms END \
         FROM execution_intents ORDER BY seq; \
         INSERT INTO execution_incidents(venue, category, environment, account_kind, \
                                         account_id, profile_id, incident_key, kind, symbol, \
                                         state, revision, incident_json, opened_at_ms, \
                                         cleared_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'op/' || i || '/unsettled_entry', 'unsettled_entry', \
                'SYM' || (i % 200) || 'USDT', CASE WHEN i > 4995 THEN 'open' ELSE 'cleared' END, \
                CASE WHEN i > 4995 THEN 1 ELSE 2 END, '{{\"format\":1}}', \
                1792454400000 + i * 10000, \
                CASE WHEN i <= 4995 THEN 1792454400000 + i * 10000 + 5000 END \
         FROM generate_series(1, 5000) AS i JOIN {LINES} ON l.n = i % 4; \
         INSERT INTO outbox_events(venue, category, environment, account_kind, account_id, \
                                   profile_id, event_key, kind, delivery, revision, \
                                   event_json, created_at_ms, delivery_at_ms) \
         SELECT l.venue, l.category, 'demo', l.account_kind, l.account_id, l.profile_id, \
                'op/' || (1 + i / 4) || '/' || \
                    (ARRAY['submitted:1', 'settled:partially_filled', 'settled:filled', \
                           'unknown:1'])[1 + i % 4], \
                'order_settled', CASE WHEN i > 199900 THEN 'pending' ELSE 'delivered' END, \
                CASE WHEN i > 199900 THEN 1 ELSE 2 END, \
                '{{\"format\":1,\"event\":' || i || '}}', 1792454400000 + i * 300, \
                CASE WHEN i <= 199900 THEN 1792454400000 + i * 300 + 100 END \
         FROM generate_series(1, 200000) AS i JOIN {LINES} ON l.n = (1 + i / 4) % 4; \
         INSERT INTO position_leases(venue, category, environment, account_kind, account_id, \
                                     symbol, fence, holder, profile_id, activation_epoch, \
                                     acquired_at_ms, expires_at_ms) \
         SELECT DISTINCT l.venue, l.category, 'demo', l.account_kind, l.account_id, \
                'SYM' || s || 'USDT', 7, 'executor-a', l.profile_id, 1, 1792454400000, \
                1792454430000 \
         FROM generate_series(0, 199) AS s JOIN {LINES} ON l.n = s % 4; \
         ANALYZE;"
    )
}
