//! The shared cases in `tests/fixtures/result_cases.json`, which every
//! implementation of the spec must pass. Their files and hashes come from
//! `spikes/result-format/result_cases.py`, never from the code under test.

use quantforge::protocol::ContentHash;
use quantforge::protocol::results::{
    EquityRow, Metrics, RESULT_FORMAT, ResultKind, TradeRow, decode_equity, decode_trades,
    encode_equity, encode_trades,
};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const CASES: &str = include_str!("../fixtures/result_cases.json");

#[derive(Deserialize)]
pub(crate) struct Fixture {
    pub(crate) format: u32,
    pub(crate) cases: Vec<Case>,
}

#[derive(Deserialize)]
pub(crate) struct Case {
    pub(crate) name: String,
    pub(crate) result: String,
    /// A table's rows, when the case lists them.
    #[serde(default)]
    pub(crate) rows: Vec<Value>,
    /// How many rows the case's rule makes, when it lists none.
    pub(crate) series: Option<i64>,
    pub(crate) metrics: Option<Value>,
    pub(crate) file_hex: Option<String>,
    pub(crate) hash: String,
    pub(crate) size_bytes: u64,
}

pub(crate) fn fixture() -> Fixture {
    serde_json::from_str(CASES).expect("result cases")
}

fn text(row: &Value, column: &str) -> String {
    row[column]
        .as_str()
        .unwrap_or_else(|| panic!("{column} is text in {row}"))
        .to_string()
}

fn time(row: &Value, column: &str) -> i64 {
    row[column]
        .as_i64()
        .unwrap_or_else(|| panic!("{column} is an integer in {row}"))
}

fn trade(row: &Value) -> TradeRow {
    let decimal = |column: &str| text(row, column).parse().expect("an exact decimal");
    TradeRow {
        symbol: text(row, "symbol"),
        entry_time_ms: time(row, "entry_time_ms"),
        exit_time_ms: time(row, "exit_time_ms"),
        entry_price: decimal("entry_price"),
        exit_price: decimal("exit_price"),
        qty: decimal("qty"),
        gross_quote_pnl: decimal("gross_quote_pnl"),
    }
}

fn equity(row: &Value) -> EquityRow {
    EquityRow {
        open_time_ms: time(row, "open_time_ms"),
        equity: text(row, "equity").parse().expect("an exact decimal"),
    }
}

/// The rule of a case that lists no rows: bar `i` of `count` one-minute
/// bars from 2026-10-20T00:00:00Z holds `10000 + i`, with `i mod 100` as
/// its two decimal places.
fn series(count: i64) -> Vec<EquityRow> {
    (0..count)
        .map(|bar| EquityRow {
            open_time_ms: 1_792_454_400_000 + 60_000 * bar,
            equity: format!("{}.{:02}", 10_000 + bar, bar % 100)
                .parse()
                .expect("an exact decimal"),
        })
        .collect()
}

/// The file this build writes for the case, and the rows it reads back
/// from that file, as the fixture writes rows.
fn written(case: &Case) -> (Vec<u8>, Vec<Value>) {
    match case.result.as_str() {
        "metrics" => {
            let value = case.metrics.clone().expect("a metrics case");
            let metrics: Metrics = serde_json::from_value(value).expect("metrics");
            let file = metrics.encode().expect("a file");
            let read = Metrics::decode(&file).expect("the metrics");
            let read = serde_json::to_value(read).expect("a value");
            (file, vec![read])
        }
        "trades" => {
            let trades: Vec<TradeRow> = case.rows.iter().map(trade).collect();
            let file = encode_trades(&trades).expect("a file");
            let read = decode_trades(&file).expect("the trades");
            let rows = read
                .iter()
                .map(|trade| {
                    serde_json::json!({
                        "symbol": trade.symbol,
                        "entry_time_ms": trade.entry_time_ms,
                        "exit_time_ms": trade.exit_time_ms,
                        "entry_price": trade.entry_price.to_string(),
                        "exit_price": trade.exit_price.to_string(),
                        "qty": trade.qty.to_string(),
                        "gross_quote_pnl": trade.gross_quote_pnl.to_string(),
                    })
                })
                .collect();
            (file, rows)
        }
        "equity" => {
            let curve: Vec<EquityRow> = match case.series {
                Some(count) => series(count),
                None => case.rows.iter().map(equity).collect(),
            };
            let file = encode_equity(&curve).expect("a file");
            let read = decode_equity(&file).expect("the curve");
            let rows = read
                .iter()
                .map(|row| {
                    serde_json::json!({
                        "open_time_ms": row.open_time_ms,
                        "equity": row.equity.to_string(),
                    })
                })
                .collect();
            (file, rows)
        }
        other => panic!("{}: no result is named {other}", case.name),
    }
}

#[test]
fn every_shared_case_is_written_as_its_file_and_reads_back_digit_for_digit() {
    let fixture = fixture();
    assert_eq!(fixture.format, RESULT_FORMAT);
    let mut seen = Vec::new();
    for case in &fixture.cases {
        let (file, read) = written(case);
        let digest: [u8; 32] = Sha256::digest(&file).into();
        assert_eq!(
            ContentHash::sha256(digest).to_string(),
            case.hash,
            "{}: the hash",
            case.name
        );
        assert_eq!(
            u64::try_from(file.len()).expect("a size"),
            case.size_bytes,
            "{}: the size",
            case.name
        );
        match (&case.file_hex, case.series) {
            (Some(file_hex), None) => {
                assert_eq!(hex::encode(&file), *file_hex, "{}: the file", case.name);
                let listed = match &case.metrics {
                    Some(metrics) => vec![metrics.clone()],
                    None => case.rows.clone(),
                };
                assert_eq!(read, listed, "{}: read back", case.name);
            }
            (None, Some(count)) => {
                assert_eq!(read.len(), usize::try_from(count).expect("a count"));
            }
            other => panic!(
                "{}: a case lists its file or a series, not {other:?}",
                case.name
            ),
        }
        seen.push(case.result.as_str());
    }
    for kind in ResultKind::ALL {
        assert!(
            seen.iter().filter(|result| **result == kind.name()).count() >= 2,
            "the fixture holds cases of {}",
            kind.name()
        );
    }
    assert!(fixture.cases.iter().any(|case| case.series.is_some()));
}

#[test]
fn every_listed_file_decodes_to_its_rows_and_no_other_table_reads_it() {
    for case in &fixture().cases {
        let Some(file_hex) = &case.file_hex else {
            continue;
        };
        let file = hex::decode(file_hex).expect("hex");
        let reads = [
            Metrics::decode(&file).is_ok(),
            decode_trades(&file).is_ok(),
            decode_equity(&file).is_ok(),
        ];
        let expected = ["metrics", "trades", "equity"].map(|result| result == case.result);
        assert_eq!(reads, expected, "{}", case.name);
    }
}
