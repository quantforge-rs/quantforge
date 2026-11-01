//! A refresh racing writers and other refreshes, step by step: the races of
//! `tests/fixtures/storage_conformance_cases.json`, which every backend
//! steps through its own read, pack and write-back. Here a read is
//! `read_window` on a connection of its own, and a write-back is `pack`
//! then `write_window`, which answers `done`, `elsewhere` (another refresh
//! unmarked the window) or `dirty` (a writer marked it again).

use super::build::{Written, pack, read_window, write_window};
use crate::storage::segments as format;
use crate::{
    Candle, CandleStore, ExchangeId, Interval, MarketId, SegmentStore, SqliteStore, Symbol,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

const CASES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/storage_conformance_cases.json"
));

#[derive(Deserialize)]
struct Cases {
    markets: BTreeMap<String, MarketText>,
    races: Vec<Race>,
}

#[derive(Deserialize)]
struct MarketText {
    exchange: String,
    symbol: String,
    interval: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Race {
    id: String,
    #[expect(dead_code, reason = "the statement is for the reader of the fixture")]
    doc: String,
    setup: Vec<RaceStep>,
    window_start_ms: i64,
    steps: Vec<RaceStep>,
    after: Vec<After>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum RaceStep {
    Upsert {
        market: String,
        candles: Vec<CandleText>,
    },
    Read {
        #[serde(rename = "as")]
        name: String,
    },
    WriteBack {
        of: String,
        expect: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum After {
    Resolve {
        market: String,
        first: i64,
        last: i64,
        expect: Plan,
    },
    Refresh {
        market: String,
        expect: Counts,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CandleText {
    open_time_ms: i64,
    close_time_ms: i64,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
    trades: Option<u64>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Plan {
    segments: Vec<Segment>,
    tip: Option<Tip>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Segment {
    window_start_ms: i64,
    first_open_ms: i64,
    last_open_ms: i64,
    bar_count: u32,
    hash: String,
    payload_len: u64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Tip {
    first_open_ms: i64,
    last_open_ms: i64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Counts {
    built: usize,
    unchanged: usize,
    blocked: usize,
}

fn decimal(text: &str) -> Decimal {
    Decimal::from_str_exact(text).expect("an exact decimal")
}

impl CandleText {
    fn candle(&self) -> Candle {
        Candle {
            open_time_ms: self.open_time_ms,
            close_time_ms: self.close_time_ms,
            open: decimal(&self.open),
            high: decimal(&self.high),
            low: decimal(&self.low),
            close: decimal(&self.close),
            volume: decimal(&self.volume),
            trades: self.trades,
        }
    }
}

impl Cases {
    fn market(&self, name: &str) -> MarketId {
        let text = &self.markets[name];
        MarketId::new(
            ExchangeId::from_storage_name(&text.exchange).expect("product line"),
            Symbol::new(&text.symbol).expect("symbol"),
            Interval::from_str(&text.interval).expect("interval"),
        )
    }
}

fn plan(store: &SqliteStore, market: &MarketId, first: i64, last: i64) -> Plan {
    let plan = store
        .resolve_segments(market, first, last)
        .expect("resolve");
    Plan {
        segments: plan
            .segments
            .iter()
            .map(|segment| Segment {
                window_start_ms: segment.window_start_ms,
                first_open_ms: segment.first_open_ms,
                last_open_ms: segment.last_open_ms,
                bar_count: segment.bar_count,
                hash: segment.hash.to_string(),
                payload_len: segment.payload_len,
            })
            .collect(),
        tip: plan.tip.map(|tip| Tip {
            first_open_ms: tip.first_open_ms,
            last_open_ms: tip.last_open_ms,
        }),
    }
}

/// Steps one race on a fresh store and checks what follows it.
fn step_through(cases: &Cases, race: &Race) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
    CandleStore::init(&store).expect("init");
    let mut market = None;
    let mut reads = HashMap::new();
    for step in race.setup.iter().chain(&race.steps) {
        match step {
            RaceStep::Upsert {
                market: name,
                candles,
            } => {
                let candles: Vec<Candle> = candles.iter().map(CandleText::candle).collect();
                let id = cases.market(name);
                store.upsert_candles(&id, &candles).expect("upsert");
                market = Some(id);
            }
            RaceStep::Read { name } => {
                let market = market.as_ref().expect("a market written first");
                let window = format::window(market.interval, race.window_start_ms).expect("window");
                let mut connection = store.open().expect("connection");
                let snapshot = read_window(&mut connection, market, &window)
                    .expect("read")
                    .expect("a dirty window");
                reads.insert(name.clone(), (connection, snapshot));
            }
            RaceStep::WriteBack { of, expect } => {
                let market = market.as_ref().expect("a market written first");
                let (mut connection, snapshot) = reads.remove(of).expect("read before");
                let packed = pack(&snapshot).expect("pack");
                let written = write_window(
                    &mut connection,
                    market,
                    race.window_start_ms,
                    snapshot.id,
                    &packed,
                )
                .expect("write back");
                let answer = match written {
                    Written::Done => "done",
                    Written::Elsewhere => "elsewhere",
                    Written::Marked => "dirty",
                };
                assert_eq!(answer, expect, "{}: write-back of {of}", race.id);
            }
        }
    }
    for after in &race.after {
        match after {
            After::Resolve {
                market,
                first,
                last,
                expect,
            } => assert_eq!(
                &plan(&store, &cases.market(market), *first, *last),
                expect,
                "{}: plan after the race",
                race.id
            ),
            After::Refresh { market, expect } => {
                let done = store
                    .refresh_segments(&cases.market(market))
                    .expect("refresh");
                let found = Counts {
                    built: done.built,
                    unchanged: done.unchanged,
                    blocked: done.blocked,
                };
                assert_eq!(&found, expect, "{}: refresh after the race", race.id);
            }
        }
    }
}

#[test]
fn every_race_of_the_shared_cases_ends_as_the_cases_say() {
    let cases: Cases = serde_json::from_str(CASES).expect("the conformance cases");
    assert!(cases.races.len() >= 2, "the races are listed");
    for race in &cases.races {
        step_through(&cases, race);
    }
}
