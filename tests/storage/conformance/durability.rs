//! What every backend's acknowledgement means, and how long it may take
//! (`docs/storage-durability.md`): a write answered is there for everyone, a
//! write refused leaves nothing, a writer held up past its budget fails in
//! time as contention, and a store outlives its connections. These checks
//! are coded rather than written in the fixture, since they hold locks and
//! end connections; the failures only one backend can show are classified
//! in that backend's own tests.

use std::time::{Duration, Instant};

use quantforge::{
    Candle, CandleQuery, CandleStore, ExchangeId, Interval, MarketId, RunIdentity, RunJournalStore,
    StorageErrorKind, StoreControl, Symbol,
};
use rust_decimal::Decimal;

use super::cases::{self, CandleText};
use super::{Bounded, Coded, Fixture, Raw};

/// The checks coded here: id and statement.
pub const CHECKS: &[(&str, &str)] = &[
    (
        "durability.commit_visible",
        "a write the store answered is there for another connection and for a store opened afterwards",
    ),
    (
        "durability.rollback_leaves_nothing",
        "a batch that fails partway leaves no candle and no mark, and goes through once it can",
    ),
    (
        "durability.contention_is_bounded",
        "a write held up by another writer fails as contention within its budget and changes nothing, and goes through once the writer is done",
    ),
    (
        "durability.reads_pass_a_held_writer",
        "a read answers at once while another writer holds the write lock",
    ),
    (
        "durability.reconnects",
        "a store keeps answering, reads and writes, after its connections are gone",
    ),
    (
        "durability.settings_read_back",
        "a store reads back every setting its commits rely on, and all of them are met",
    ),
];

pub fn check<S>(id: &str) -> Coded<S>
where
    S: CandleStore + RunJournalStore + StoreControl + Bounded,
{
    match id {
        "durability.commit_visible" => commit_visible,
        "durability.rollback_leaves_nothing" => rollback_leaves_nothing,
        "durability.contention_is_bounded" => contention_is_bounded,
        "durability.reads_pass_a_held_writer" => reads_pass_a_held_writer,
        "durability.reconnects" => reconnects,
        "durability.settings_read_back" => settings_read_back,
        other => panic!("no durability check {other}"),
    }
}

/// 2026-10-20T00:00:00Z.
const OPEN: i64 = 1_792_454_400_000;

const BUDGET: Duration = Duration::from_secs(2);

/// What a loaded machine may add to a budget before a call is seen to end.
/// A write that ignored the budget would wait the store's own 5 s.
const SCHEDULING: Duration = Duration::from_secs(1);

fn market() -> MarketId {
    MarketId::new(
        ExchangeId::BybitSpot,
        Symbol::new("BTCUSDT").expect("symbol"),
        Interval::M1,
    )
}

fn candle(open_time_ms: i64) -> Candle {
    let close = Decimal::from_str_exact("100.10").expect("a decimal");
    Candle {
        open_time_ms,
        close_time_ms: open_time_ms + 59_999,
        open: close,
        high: close,
        low: close,
        close,
        volume: Decimal::ONE,
        trades: Some(3),
    }
}

/// The rows of `table`, as `raw` counts them.
fn rows(raw: &dyn Raw, table: &str) -> String {
    raw.select(&format!("SELECT CAST(COUNT(*) AS TEXT) FROM {table}"))[0][0]
        .clone()
        .expect("a count")
}

fn stored<S: CandleStore>(store: &S) -> Vec<CandleText> {
    store
        .load_candles(&market(), CandleQuery::default())
        .expect("the candles")
        .iter()
        .map(CandleText::of)
        .collect()
}

fn commit_visible<S: CandleStore + RunJournalStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &*fixture.raw);
    let cases = cases::load();
    store
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect("a candle");
    assert_eq!(rows(raw, "candles"), "1", "another connection sees it");
    let run = cases.run("first");
    store
        .create_run(&run, &RunIdentity::recorded(None, None))
        .expect("a run");
    let order = cases.order("o1");
    store
        .append_order_event(&run.run_id, &order)
        .expect("an order event");
    assert_eq!(rows(raw, "bot_runs"), "1", "another connection sees it");
    assert_eq!(rows(raw, "order_events"), "1", "another connection sees it");

    let reopened = (fixture.reopen)();
    assert_eq!(stored(&reopened), [CandleText::of(&candle(OPEN))]);
    assert_eq!(
        reopened
            .load_run_state(&run.run_id)
            .expect("the run")
            .map(|state| state.run_id),
        Some(run.run_id.clone())
    );
    let events = reopened
        .list_order_events(&run.run_id, 10)
        .expect("the events");
    assert_eq!(events.len(), 1);
}

/// The marks table refuses its row after the candles went in: they go too.
fn rollback_leaves_nothing<S: CandleStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &*fixture.raw);
    let [refuse, allow] = raw.write_refusal("segment_dirty");
    raw.execute(&refuse);
    store
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect_err("the mark is refused");
    assert_eq!(rows(raw, "candles"), "0", "no candle without its mark");
    assert_eq!(rows(raw, "segment_dirty"), "0");

    raw.execute(&allow);
    store
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect("the same batch");
    assert_eq!(rows(raw, "candles"), "1");
    assert_eq!(rows(raw, "segment_dirty"), "1");
}

fn contention_is_bounded<S: CandleStore + Bounded>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &*fixture.raw);
    let bounded = store.within(BUDGET);
    let held = raw.hold_writes("candles");

    let started = Instant::now();
    let refused = bounded
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect_err("another writer holds the lock");
    let took = started.elapsed();
    assert_eq!(refused.kind(), StorageErrorKind::Contention, "{refused}");
    assert!(took < BUDGET + SCHEDULING, "it took {took:?} of {BUDGET:?}");
    assert!(
        took > BUDGET / 2,
        "it gave up after {took:?}, without waiting"
    );
    assert_eq!(rows(raw, "candles"), "0", "no acknowledgement, no candle");

    drop(held);
    bounded
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect("the writer is done");
    assert_eq!(rows(raw, "candles"), "1");
}

fn reads_pass_a_held_writer<S: CandleStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &*fixture.raw);
    store
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect("a candle");
    let held = raw.hold_writes("candles");
    let started = Instant::now();
    assert_eq!(stored(store).len(), 1);
    let took = started.elapsed();
    assert!(
        took < BUDGET,
        "it took {took:?}, as if it waited for the writer"
    );
    drop(held);
}

fn reconnects<S: CandleStore>(fixture: &Fixture<S>) {
    let (store, raw) = (&fixture.store, &*fixture.raw);
    store
        .upsert_candles(&market(), &[candle(OPEN)])
        .expect("a candle");
    raw.end_connections();
    assert_eq!(stored(store).len(), 1, "a read after they ended");
    raw.end_connections();
    store
        .upsert_candles(&market(), &[candle(OPEN + 60_000)])
        .expect("a write after they ended");
    assert_eq!(stored(store).len(), 2);
}

fn settings_read_back<S: StoreControl>(fixture: &Fixture<S>) {
    let report = fixture.store.durability().expect("the report");
    assert!(report.is_met(), "{:?}", report.problem());
    assert!(!report.settings().is_empty());
    assert_eq!(report.problem(), None);
}
