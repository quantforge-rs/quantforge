//! The two fixtures the suite reads, and the conversions between their text
//! and the model:
//! - `tests/fixtures/storage_conformance_cases.json`, the scenarios, written
//!   by `spikes/storage-contract/conformance_cases.py`;
//! - `tests/fixtures/storage_identity_cases.json`, the identity cases,
//!   written by `spikes/storage-contract/identity_cases.py`.
//!
//! Neither is written by the Rust code under test. Every decimal is read
//! from its exact text and compared as text, never through a float.

use quantforge::{
    BotRunState, BundleHash, Candle, ClosedTrade, Environment, ExchangeId, ExchangeOrder,
    ExecutionMode, Fill, Interval, JobId, JobOutcome, JobRecord, KeyFingerprint, MarketId, NewJob,
    OrderStatus, PositionState, RunIdentity, RunStatus, Side, StoredResult, Symbol, VenueAccountId,
    VenueOrderId, VenueTradeId,
};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub fn fixture(relative: &str) -> PathBuf {
    relative.split('/').fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        |path, part| path.join(part),
    )
}

pub fn read<T: for<'de> Deserialize<'de>>(relative: &str) -> T {
    let path = fixture(relative);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{relative}: {error}"))
}

// --- The scenarios -----------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cases {
    pub format: u32,
    pub note: String,
    pub markets: BTreeMap<String, MarketText>,
    pub series_rule: String,
    pub runs: BTreeMap<String, RunText>,
    pub orders: BTreeMap<String, OrderText>,
    pub trades: BTreeMap<String, TradeText>,
    pub candles: Vec<Scenario>,
    pub journal: Vec<Scenario>,
    pub segments: Vec<Scenario>,
    pub races: Vec<serde_json::Value>,
    pub control: Vec<Scenario>,
    pub jobs: Vec<Scenario>,
}

pub fn load() -> Cases {
    let cases: Cases = read("tests/fixtures/storage_conformance_cases.json");
    assert_eq!(cases.format, 1);
    assert!(cases.note.contains("not by the Rust code under test"));
    cases
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub doc: String,
    pub steps: Vec<Step>,
}

/// One operation and what a conforming store answers.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    Upsert {
        market: String,
        #[serde(default)]
        candles: Option<Vec<CandleText>>,
        #[serde(default)]
        series: Option<Series>,
        #[serde(default)]
        written: Option<usize>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    Load {
        market: String,
        start: Option<i64>,
        end: Option<i64>,
        limit: Option<usize>,
        #[serde(default)]
        expect: Option<Vec<CandleText>>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    Recent {
        market: String,
        limit: usize,
        #[serde(default)]
        expect: Option<Vec<CandleText>>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    MaxOpenTime {
        market: String,
        expect: Option<i64>,
    },
    Coverage {
        expect: Vec<CoverageText>,
    },
    Refresh {
        market: String,
        expect: RefreshText,
    },
    Resolve {
        market: String,
        first: i64,
        last: i64,
        #[serde(default)]
        expect: Option<PlanText>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    LoadSegment {
        hash: String,
        expect: Option<StoredText>,
    },
    RawSelect {
        sql: String,
        expect: Vec<Vec<Option<String>>>,
    },
    RawContains {
        sql: String,
        contains: Vec<String>,
    },
    RawExecute {
        sql: String,
    },
    ValueTypes {
        table: String,
        column: String,
        expect: Vec<String>,
    },
    Reopen,
    StoreBundle {
        hash: String,
        data: String,
        expect: bool,
    },
    LoadBundle {
        hash: String,
        expect: Option<String>,
    },
    LoadResult {
        hash: String,
        expect: Option<String>,
    },
    SubmitJob {
        job: NewJobText,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    Job {
        job_id: String,
        expect: Option<JobText>,
    },
    Jobs {
        limit: usize,
        expect: Vec<JobText>,
    },
    ClaimJob {
        at: i64,
        expect: Option<JobText>,
    },
    FinishJob {
        job_id: String,
        outcome: OutcomeText,
        at: i64,
        expect: bool,
    },
    CancelJob {
        job_id: String,
        at: i64,
        expect: Option<JobText>,
    },
    FailRunningJobs {
        failure: String,
        at: i64,
        expect: usize,
    },
    CreateRun {
        run: String,
        identity: IdentityText,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    SaveRun {
        run: String,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    LoadRun {
        run_id: String,
        #[serde(default)]
        expect: Option<String>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    RunIdentity {
        run_id: String,
        expect: Option<IdentityText>,
    },
    LatestRun {
        market: String,
        strategy: String,
        #[serde(default)]
        expect: Option<String>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    IdentityMatches {
        run_id: String,
        invocation: IdentityText,
        expect: String,
    },
    AppendOrderEvent {
        run_id: String,
        order: String,
    },
    ListOrderEvents {
        run_id: String,
        limit: usize,
        expect: Vec<String>,
    },
    AppendClosedTrade {
        run_id: String,
        trade: String,
    },
    ListClosedTrades {
        run_id: String,
        limit: usize,
        #[serde(default)]
        expect: Option<Vec<String>>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    StoreRecord {
        expect: StoreRecordText,
    },
    Migrations {
        expect: Vec<MigrationText>,
    },
    BindEnvironment {
        environment: String,
        #[serde(default)]
        refused: Option<Refusal>,
    },
    LegacyRuns {
        expect: Vec<LegacyRunText>,
    },
    AdoptRun {
        run_id: String,
        identity: IdentityText,
        execution_mode: Option<String>,
        #[serde(default)]
        refused: Option<Refusal>,
    },
}

/// The store's record, but its ID, which is random.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StoreRecordText {
    pub schema_version: u32,
    pub environment: Option<String>,
    pub activation: String,
    pub activation_epoch: u32,
}

/// One step of the migration log, but when and by which build.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MigrationText {
    pub to_version: u32,
    pub from_version: u32,
}

/// A legacy run as `StoreControl::legacy_runs` lists it, its market by
/// name.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LegacyRunText {
    pub run_id: String,
    pub market: String,
    pub status: String,
    pub execution_mode: Option<String>,
    pub started_at_ms: i64,
}

/// An execution mode from its wire form.
pub fn execution_mode(text: &str) -> ExecutionMode {
    match text {
        "dry_run" => ExecutionMode::DryRun,
        "live" => ExecutionMode::Live,
        other => panic!("no execution mode {other}"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Refusal {
    pub kind: String,
    pub contains: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketText {
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
}

impl Cases {
    pub fn market(&self, name: &str) -> MarketId {
        let text = self
            .markets
            .get(name)
            .unwrap_or_else(|| panic!("no market named {name}"));
        MarketId::new(
            ExchangeId::from_storage_name(&text.exchange)
                .unwrap_or_else(|| panic!("no product line {}", text.exchange)),
            Symbol::new(&text.symbol).expect("symbol"),
            Interval::from_str(&text.interval).expect("interval"),
        )
    }
}

// --- Candles -----------------------------------------------------------------

/// A job as the cases submit it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewJobText {
    pub job_id: String,
    pub bundle: String,
    pub request_json: String,
    pub submitted_at_ms: i64,
}

impl NewJobText {
    pub fn new_job(&self) -> NewJob {
        NewJob {
            job_id: JobId::new(self.job_id.as_str()).expect("a job id"),
            bundle: BundleHash::from_str(&self.bundle).expect("a bundle hash"),
            request_json: self.request_json.clone(),
            submitted_at_ms: self.submitted_at_ms,
        }
    }
}

/// A job's record as text: what a store must answer.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JobText {
    pub job_id: String,
    pub phase: String,
    pub bundle: String,
    pub request_json: String,
    pub submitted_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
    pub results_json: Option<String>,
    pub failure_json: Option<String>,
}

impl JobText {
    pub fn of(record: &JobRecord) -> Self {
        Self {
            job_id: record.job_id.as_str().to_string(),
            phase: record.phase.as_str().to_string(),
            bundle: record.bundle.to_string(),
            request_json: record.request_json.clone(),
            submitted_at_ms: record.submitted_at_ms,
            started_at_ms: record.started_at_ms,
            finished_at_ms: record.finished_at_ms,
            results_json: record.results_json.clone(),
            failure_json: record.failure_json.clone(),
        }
    }
}

/// How the cases end a run: succeeded with its results' JSON, and the
/// results a store keeps with it, or failed with why.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeText {
    succeeded: Option<String>,
    failed: Option<String>,
    #[serde(default)]
    results: Vec<StoredResultText>,
}

/// One result as the cases hand it to a store: its bytes as hex.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredResultText {
    hash: String,
    size_bytes: u64,
    data: String,
}

impl OutcomeText {
    pub fn outcome(&self) -> JobOutcome {
        match (&self.succeeded, &self.failed) {
            (Some(results_json), None) => JobOutcome::Succeeded {
                results_json: results_json.clone(),
                results: self
                    .results
                    .iter()
                    .map(|result| StoredResult {
                        hash: result.hash.parse().expect("a result hash"),
                        size_bytes: result.size_bytes,
                        data: hex::decode(&result.data).expect("hex"),
                    })
                    .collect(),
            },
            (None, Some(failure_json)) if self.results.is_empty() => JobOutcome::Failed {
                failure_json: failure_json.clone(),
            },
            other => panic!("an outcome succeeded or failed, not {other:?}"),
        }
    }
}

/// A candle as text: what was written, and what must come back.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandleText {
    pub open_time_ms: i64,
    pub close_time_ms: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub trades: Option<u64>,
}

pub fn decimal(text: &str) -> Decimal {
    let value = Decimal::from_str_exact(text).unwrap_or_else(|error| panic!("{text}: {error}"));
    assert_eq!(value.to_string(), text, "a decimal not in its exact form");
    value
}

impl CandleText {
    pub fn candle(&self) -> Candle {
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

    pub fn of(candle: &Candle) -> Self {
        Self {
            open_time_ms: candle.open_time_ms,
            close_time_ms: candle.close_time_ms,
            open: candle.open.to_string(),
            high: candle.high.to_string(),
            low: candle.low.to_string(),
            close: candle.close.to_string(),
            volume: candle.volume.to_string(),
            trades: candle.trades,
        }
    }
}

/// Candles generated by the fixture's `series_rule`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub from: i64,
    pub count: usize,
    #[serde(default)]
    pub spacing_ms: Option<i64>,
    #[serde(default)]
    pub range: Option<[usize; 2]>,
    #[serde(default)]
    pub skip: Vec<usize>,
    #[serde(default)]
    pub close: Option<String>,
}

impl Series {
    pub fn candles(&self, interval: Interval) -> Vec<CandleText> {
        let step = interval.step_ms();
        let spacing = self.spacing_ms.unwrap_or(step);
        let [first, end] = self.range.unwrap_or([0, self.count]);
        (first..end.min(self.count))
            .filter(|index| !self.skip.contains(index))
            .map(|index| {
                let open_time_ms = self.from + index as i64 * spacing;
                let price = self
                    .close
                    .clone()
                    .unwrap_or_else(|| format!("{}.{:02}", 100 + index % 50, index % 100));
                CandleText {
                    open_time_ms,
                    close_time_ms: open_time_ms + step - 1,
                    open: price.clone(),
                    high: price.clone(),
                    low: price.clone(),
                    close: price,
                    volume: "0.50000000".to_string(),
                    trades: Some(open_time_ms.rem_euclid(97) as u64),
                }
            })
            .collect()
    }
}

// --- Segments ----------------------------------------------------------------

/// A market a store holds candles of, by its stored key, and its span.
#[derive(Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct CoverageText {
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
    pub first_open_ms: i64,
    pub last_open_ms: i64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RefreshText {
    pub built: usize,
    pub unchanged: usize,
    pub blocked: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanText {
    pub segments: Vec<SegmentText>,
    pub tip: Option<TipText>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SegmentText {
    pub window_start_ms: i64,
    pub first_open_ms: i64,
    pub last_open_ms: i64,
    pub bar_count: u32,
    pub hash: String,
    pub payload_len: u64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TipText {
    pub first_open_ms: i64,
    pub last_open_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredText {
    pub bar_count: usize,
}

// --- Journal -----------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdentityText {
    pub environment: Option<String>,
    pub account: Option<AccountText>,
    pub source: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountText {
    pub key: String,
    pub fingerprint: String,
}

pub fn environment(text: &str) -> Environment {
    Environment::ALL
        .into_iter()
        .find(|environment| environment.as_str() == text)
        .unwrap_or_else(|| panic!("no environment {text}"))
}

impl IdentityText {
    pub fn identity(&self) -> RunIdentity {
        let environment = self.environment.as_deref().map(environment);
        let account = self.account.as_ref().map(|account| {
            // The oracle hashed the key itself: the two must agree.
            let fingerprint = KeyFingerprint::of(&account.key);
            assert_eq!(
                fingerprint.as_str(),
                account.fingerprint,
                "account fingerprint mismatch"
            );
            VenueAccountId::KeyFingerprint(fingerprint)
        });
        match self.source.as_str() {
            "recorded" => RunIdentity::recorded(environment, account),
            "adopted" => RunIdentity::adopted(environment, account),
            "legacy" => {
                assert!(environment.is_none() && account.is_none(), "{self:?}");
                RunIdentity::legacy()
            }
            other => panic!("no identity source {other}"),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunText {
    pub run_id: String,
    pub market: String,
    pub strategy_name: String,
    pub strategy_config: serde_json::Value,
    pub execution_mode: String,
    pub status: String,
    pub last_processed_open_time_ms: Option<i64>,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    pub stopped_at_ms: Option<i64>,
    pub last_error: Option<String>,
    pub position: PositionText,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionText {
    pub qty: String,
    pub entry_price: Option<String>,
    pub entry_time_ms: Option<i64>,
    pub entry_order_id: Option<String>,
}

fn order_id(text: &str) -> VenueOrderId {
    VenueOrderId::new(text).expect("order id")
}

impl Cases {
    pub fn run(&self, name: &str) -> BotRunState {
        let text = self
            .runs
            .get(name)
            .unwrap_or_else(|| panic!("no run named {name}"));
        BotRunState {
            run_id: text.run_id.clone(),
            market: self.market(&text.market),
            strategy_name: text.strategy_name.clone(),
            strategy_config: text.strategy_config.clone(),
            execution_mode: execution_mode(&text.execution_mode),
            status: [
                RunStatus::Starting,
                RunStatus::Running,
                RunStatus::Stopped,
                RunStatus::Failed,
            ]
            .into_iter()
            .find(|status| status.as_str() == text.status)
            .unwrap_or_else(|| panic!("no run status {}", text.status)),
            last_processed_open_time_ms: text.last_processed_open_time_ms,
            started_at_ms: text.started_at_ms,
            updated_at_ms: text.updated_at_ms,
            stopped_at_ms: text.stopped_at_ms,
            last_error: text.last_error.clone(),
            position: PositionState {
                qty: decimal(&text.position.qty),
                entry_price: text.position.entry_price.as_deref().map(decimal),
                entry_time_ms: text.position.entry_time_ms,
                entry_order_id: text.position.entry_order_id.as_deref().map(order_id),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderText {
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    pub status: String,
    pub order_id: Option<String>,
    pub client_order_id: Option<String>,
    pub requested_qty: Option<String>,
    pub requested_quote_qty: Option<String>,
    pub executed_qty: Option<String>,
    pub cumulative_quote_qty: Option<String>,
    pub avg_price: Option<String>,
    pub transact_time_ms: Option<i64>,
    pub fills: Option<Vec<FillText>>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillText {
    pub price: String,
    pub qty: String,
    pub commission: String,
    pub commission_asset: Option<String>,
    pub trade_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradeText {
    pub symbol: String,
    pub entry_time_ms: i64,
    pub exit_time_ms: i64,
    pub entry_price: String,
    pub exit_price: String,
    pub qty: String,
    pub gross_quote_pnl: String,
    pub entry_order_id: Option<String>,
    pub exit_order_id: Option<String>,
}

impl Cases {
    pub fn order(&self, name: &str) -> ExchangeOrder {
        let text = self
            .orders
            .get(name)
            .unwrap_or_else(|| panic!("no order named {name}"));
        let status = OrderStatus::from_exchange(&text.status);
        assert_eq!(status.as_str(), text.status, "a known order status");
        ExchangeOrder {
            symbol: Symbol::new(&text.symbol).expect("symbol"),
            side: Side::from_str(&text.side).expect("side"),
            order_type: text.order_type.clone(),
            status,
            order_id: text.order_id.as_deref().map(order_id),
            client_order_id: text.client_order_id.clone(),
            requested_qty: text.requested_qty.as_deref().map(decimal),
            requested_quote_qty: text.requested_quote_qty.as_deref().map(decimal),
            executed_qty: text.executed_qty.as_deref().map(decimal),
            cumulative_quote_qty: text.cumulative_quote_qty.as_deref().map(decimal),
            avg_price: text.avg_price.as_deref().map(decimal),
            transact_time_ms: text.transact_time_ms,
            fills: text.fills.as_ref().map(|fills| {
                fills
                    .iter()
                    .map(|fill| Fill {
                        price: decimal(&fill.price),
                        qty: decimal(&fill.qty),
                        commission: decimal(&fill.commission),
                        commission_asset: fill.commission_asset.clone(),
                        trade_id: fill
                            .trade_id
                            .as_deref()
                            .map(|id| VenueTradeId::new(id).expect("trade id")),
                    })
                    .collect()
            }),
            raw: text.raw.clone(),
        }
    }

    pub fn trade(&self, name: &str) -> ClosedTrade {
        let text = self
            .trades
            .get(name)
            .unwrap_or_else(|| panic!("no trade named {name}"));
        ClosedTrade {
            symbol: Symbol::new(&text.symbol).expect("symbol"),
            entry_time_ms: text.entry_time_ms,
            exit_time_ms: text.exit_time_ms,
            entry_price: decimal(&text.entry_price),
            exit_price: decimal(&text.exit_price),
            qty: decimal(&text.qty),
            gross_quote_pnl: decimal(&text.gross_quote_pnl),
            entry_order_id: text.entry_order_id.as_deref().map(order_id),
            exit_order_id: text.exit_order_id.as_deref().map(order_id),
        }
    }
}

// --- The identity cases ------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityCases {
    pub format: u32,
    pub note: String,
    pub markets: Vec<MarketCase>,
    pub decimals: Vec<String>,
    pub venue_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketCase {
    pub venue: String,
    pub category: String,
    /// As a user might type it: the store normalizes it.
    pub symbol: String,
    pub interval: String,
    pub storage: StorageSpelling,
    pub wire: WireSpelling,
    pub state_json_exchange: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageSpelling {
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireSpelling {
    pub exchange: String,
    pub category: String,
    pub symbol: String,
    pub interval: String,
}

pub fn identity_cases() -> IdentityCases {
    let cases: IdentityCases = read("tests/fixtures/storage_identity_cases.json");
    assert_eq!(cases.format, 1);
    assert!(cases.note.contains("not by the Rust code under test"));
    cases
}

/// Every interval's wire form.
const INTERVALS: [&str; 15] = [
    "1s", "1m", "3m", "5m", "15m", "30m", "1h", "2h", "4h", "6h", "8h", "12h", "1d", "3d", "1w",
];

#[derive(Debug, Deserialize)]
struct SpecCases {
    cases: Vec<SpecCase>,
}

/// One shared case of the segment spec: a window's candles and the name
/// their payload has.
#[derive(Debug, Deserialize)]
pub struct SpecCase {
    pub name: String,
    pub candles: Vec<CandleText>,
    pub hash: String,
}

impl SpecCase {
    /// The interval whose step the case's candles span.
    pub fn interval(&self) -> Interval {
        let first = &self.candles[0];
        let step = first.close_time_ms - first.open_time_ms + 1;
        INTERVALS
            .into_iter()
            .map(|token| token.parse::<Interval>().expect("interval"))
            .find(|interval| interval.step_ms() == step)
            .unwrap_or_else(|| panic!("{}: no interval of {step} ms", self.name))
    }
}

/// The cases of `tests/fixtures/segment_cases.json`.
pub fn segment_spec_cases() -> Vec<SpecCase> {
    read::<SpecCases>("tests/fixtures/segment_cases.json").cases
}
