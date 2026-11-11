//! A job's results as the server makes, keeps and serves them: an engine's
//! result written as the protocol's three files, each named by the hash of
//! its bytes; the form the store keeps of a file, which is the file as one
//! zstd frame; and the check that turns what the store holds back into the
//! file, so that nothing damaged is ever sent under a result's name.

use sha2::{Digest, Sha256};

use crate::engine::backtest::BacktestResult;
use crate::model::{ResultHash, StoredResult};
use crate::protocol::jobs::ResultRef;
use crate::protocol::results::{
    EquityRow, Metrics, ResultError, ResultKind, TradeRow, encode_equity, encode_trades,
};
use crate::protocol::{ContentHash, ExactDecimal};

/// The zstd level this build packs results at. It is no part of the result
/// format: a file's hash covers the file, not what the store keeps of it.
const LEVEL: i32 = 3;

/// One result: its reference and the file a client downloads.
#[derive(Clone, PartialEq, Eq)]
pub struct ResultFile {
    /// Its name, media type, hash and size.
    pub reference: ResultRef,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

impl ResultFile {
    /// The file `bytes`, named `name`, of `media_type`.
    pub fn new(name: impl Into<String>, media_type: impl Into<String>, bytes: Vec<u8>) -> Self {
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Self {
            reference: ResultRef {
                name: name.into(),
                media_type: media_type.into(),
                hash: ContentHash::sha256(digest),
                size_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            },
            bytes,
        }
    }

    fn of(kind: ResultKind, bytes: Vec<u8>) -> Self {
        Self::new(kind.name(), kind.media_type(), bytes)
    }

    /// What a store keeps of the file, to hand it with the job's end: its
    /// bytes as one zstd frame, under the file's name.
    pub fn stored(&self) -> std::io::Result<StoredResult> {
        Ok(StoredResult {
            hash: ResultHash::from_digest(*self.reference.hash.digest()),
            size_bytes: self.reference.size_bytes,
            data: zstd::bulk::compress(&self.bytes, LEVEL)?,
        })
    }
}

impl std::fmt::Debug for ResultFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResultFile")
            .field("reference", &self.reference)
            .finish_non_exhaustive()
    }
}

/// A backtest's result as the protocol's rows: what a worker hands the
/// server, and what the three files hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacktestOutput {
    /// The summary.
    pub metrics: Metrics,
    /// Every closed trade, in the order they closed.
    pub trades: Vec<TradeRow>,
    /// The equity curve, one row per bar.
    pub equity: Vec<EquityRow>,
}

impl BacktestOutput {
    /// The rows of an engine's result, every decimal with the digits the
    /// engine computed. A backtest's trades name no orders, so the files
    /// have no column for them.
    pub fn of(result: &BacktestResult) -> Self {
        Self {
            metrics: Metrics {
                initial_cash: ExactDecimal::new(result.initial_cash),
                final_equity: ExactDecimal::new(result.final_equity),
                total_return_pct: ExactDecimal::new(result.total_return_pct),
                max_drawdown_pct: ExactDecimal::new(result.max_drawdown_pct),
                trade_count: u64::try_from(result.trade_count).unwrap_or(u64::MAX),
            },
            trades: result
                .trades
                .iter()
                .map(|trade| TradeRow {
                    symbol: trade.symbol.to_string(),
                    entry_time_ms: trade.entry_time_ms,
                    exit_time_ms: trade.exit_time_ms,
                    entry_price: ExactDecimal::new(trade.entry_price),
                    exit_price: ExactDecimal::new(trade.exit_price),
                    qty: ExactDecimal::new(trade.qty),
                    gross_quote_pnl: ExactDecimal::new(trade.gross_quote_pnl),
                })
                .collect(),
            equity: result
                .equity_curve
                .iter()
                .map(|point| EquityRow {
                    open_time_ms: point.open_time_ms,
                    equity: ExactDecimal::new(point.equity),
                })
                .collect(),
        }
    }

    /// The three files, in the order a job lists them: `metrics`, `trades`
    /// and `equity`.
    pub fn files(&self) -> Result<Vec<ResultFile>, ResultError> {
        Ok(vec![
            ResultFile::of(ResultKind::Metrics, self.metrics.encode()?),
            ResultFile::of(ResultKind::Trades, encode_trades(&self.trades)?),
            ResultFile::of(ResultKind::Equity, encode_equity(&self.equity)?),
        ])
    }
}

/// The file `reference` names, from what the store keeps of it. Anything
/// but exactly one zstd frame of exactly the file is refused, saying why,
/// before more than the file's size is allocated.
pub(super) fn unpack(stored: &[u8], reference: &ResultRef) -> Result<Vec<u8>, String> {
    let frame_len = zstd::zstd_safe::find_frame_compressed_size(stored)
        .map_err(|_| "it is not a zstd frame".to_string())?;
    if frame_len != stored.len() {
        return Err("it is more than one zstd frame".to_string());
    }
    let size = usize::try_from(reference.size_bytes)
        .map_err(|_| "the file is larger than this server can hold".to_string())?;
    if let Ok(Some(declared)) = zstd::zstd_safe::get_frame_content_size(stored) {
        if declared != reference.size_bytes {
            return Err(format!(
                "it holds {declared} bytes, not the file's {}",
                reference.size_bytes
            ));
        }
    }
    let bytes = zstd::bulk::decompress(stored, size)
        .map_err(|failure| format!("it cannot be unpacked: {failure}"))?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let found = ContentHash::sha256(digest);
    if bytes.len() != size || found != reference.hash {
        return Err(format!("it unpacks to {} bytes named {found}", bytes.len()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::backtest::EquityPoint;
    use crate::model::{ClosedTrade, Symbol};
    use crate::protocol::results::{decode_equity, decode_trades};
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str(text).expect("a decimal")
    }

    fn result() -> BacktestResult {
        BacktestResult {
            initial_cash: dec("10000.00"),
            final_equity: dec("10250.5"),
            total_return_pct: dec("2.5050"),
            trade_count: 1,
            max_drawdown_pct: dec("0"),
            trades: vec![ClosedTrade {
                symbol: Symbol::new("btcusdt").expect("a symbol"),
                entry_time_ms: 60_000,
                exit_time_ms: 179_999,
                entry_price: dec("81940.01000000"),
                exit_price: dec("82010.5"),
                qty: dec("0.12203"),
                gross_quote_pnl: dec("-11.40"),
                entry_order_id: None,
                exit_order_id: None,
            }],
            equity_curve: vec![
                EquityPoint {
                    open_time_ms: 0,
                    equity: dec("10000.00"),
                },
                EquityPoint {
                    open_time_ms: 60_000,
                    equity: dec("10262.0"),
                },
            ],
        }
    }

    #[test]
    fn an_engines_result_is_three_files_named_by_their_bytes() {
        let output = BacktestOutput::of(&result());
        let files = output.files().expect("the files");
        let listed: Vec<(&str, &str)> = files
            .iter()
            .map(|file| {
                (
                    file.reference.name.as_str(),
                    file.reference.media_type.as_str(),
                )
            })
            .collect();
        assert_eq!(
            listed,
            [
                ("metrics", "application/json"),
                ("trades", "application/vnd.apache.arrow.file"),
                ("equity", "application/vnd.apache.arrow.file"),
            ]
        );
        for file in &files {
            let digest: [u8; 32] = Sha256::digest(&file.bytes).into();
            assert_eq!(file.reference.hash, ContentHash::sha256(digest));
            assert_eq!(
                file.reference.size_bytes,
                u64::try_from(file.bytes.len()).expect("a size")
            );
        }
        assert_eq!(
            String::from_utf8(files[0].bytes.clone()).expect("JSON"),
            r#"{"initial_cash":"10000.00","final_equity":"10250.5","total_return_pct":"2.5050","max_drawdown_pct":"0","trade_count":1}"#
        );
        let trades = decode_trades(&files[1].bytes).expect("the trades");
        assert_eq!(trades, output.trades);
        assert_eq!(trades[0].symbol, "BTCUSDT");
        assert_eq!(trades[0].entry_price.to_string(), "81940.01000000");
        assert_eq!(trades[0].gross_quote_pnl.to_string(), "-11.40");
        let curve = decode_equity(&files[2].bytes).expect("the curve");
        assert_eq!(curve, output.equity);
        assert_eq!(curve[1].equity.to_string(), "10262.0");
        // The same result is the same three files.
        assert_eq!(BacktestOutput::of(&result()).files().expect("files"), files);
    }

    #[test]
    fn a_stored_result_unpacks_to_its_file_and_nothing_else_does() {
        let files = BacktestOutput::of(&result()).files().expect("the files");
        for file in &files {
            let stored = file.stored().expect("packed");
            assert_eq!(stored.hash.to_string(), file.reference.hash.to_string());
            assert_eq!(stored.size_bytes, file.reference.size_bytes);
            assert_eq!(
                unpack(&stored.data, &file.reference),
                Ok(file.bytes.clone())
            );
        }
        let (trades, equity) = (&files[1], &files[2]);
        let stored = trades.stored().expect("packed").data;

        assert_eq!(
            unpack(&trades.bytes, &trades.reference),
            Err("it is not a zstd frame".to_string()),
            "the file itself, not packed"
        );
        let mut twice = stored.clone();
        twice.extend(&stored);
        assert_eq!(
            unpack(&twice, &trades.reference),
            Err("it is more than one zstd frame".to_string())
        );
        assert!(unpack(&stored[..stored.len() - 1], &trades.reference).is_err());

        // Another file's bytes under this name: the size, then the hash.
        let other = equity.stored().expect("packed").data;
        let refused = unpack(&other, &trades.reference).expect_err("another file");
        assert!(refused.starts_with("it holds "), "{refused}");
        let mut renamed = trades.reference.clone();
        renamed.hash = equity.reference.hash;
        let refused = unpack(&stored, &renamed).expect_err("another name");
        assert!(refused.starts_with("it unpacks to "), "{refused}");
    }

    #[test]
    fn a_result_file_shows_its_reference_and_never_its_bytes() {
        let file = ResultFile::new("report", "application/json", b"{}".to_vec());
        let shown = format!("{file:?}");
        assert!(
            shown.contains("report") && shown.contains("size_bytes: 2"),
            "{shown}"
        );
        assert!(!shown.contains("bytes: ["), "{shown}");
    }
}
