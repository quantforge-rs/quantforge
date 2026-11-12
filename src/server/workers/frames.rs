//! What a server and its workers say to each other over a worker's standard
//! input and output: frames of a four-byte big-endian length, then that many
//! bytes of JSON, tagged `type`. The frames are no part of the protocol: a
//! server speaks only to workers of its own build, each side's first frame
//! names its build, and a worker refuses a server of another.
//!
//! A server sends `setup` as it starts a worker. The worker puts its limits
//! and sandbox on itself and answers `ready`, with what it enforced and the
//! runtime it offers. The job follows: `job`, then its candles in `candles`
//! frames, oldest first, until all the bars `job` announced have come; the
//! bundle's files are in `bundle/` in the worker's scratch directory by
//! then. While the job runs the worker sends a `log` frame for each line
//! the job writes, and `progress` frames saying how many bars it has run.
//! A backtest that ran to its end sends its closed trades in `trades`
//! frames and its equity curve in `equity` frames, each in order. The
//! worker then answers `finished` once, and the server kills it.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::config::WorkerMode;
use super::isolation::Posture;
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::{JobSubmission, LogLevel};
use crate::protocol::results::{EquityRow, Metrics, TradeRow};
use crate::protocol::stream::Bar;
use crate::protocol::{ErrorBody, ExactDecimal};

/// The frames' own version, which any change to a frame raises.
const FRAMES: u32 = 3;

/// The build a server and its workers name in their first frames.
pub(super) fn build() -> String {
    format!("quantforge {} frames {FRAMES}", env!("CARGO_PKG_VERSION"))
}

/// The most bytes one frame from a worker may hold: a worker runs a job's
/// code, so nothing it sends is trusted.
pub(super) const MOST_FROM_WORKER: usize = 16 << 20;

/// The most bytes one frame from the server may hold.
pub(super) const MOST_TO_WORKER: usize = 64 << 20;

/// How many bars one `candles` frame carries.
pub(super) const BARS_PER_FRAME: usize = 10_000;

/// How many rows one `trades` or `equity` frame carries.
pub(super) const ROWS_PER_FRAME: usize = 10_000;

/// The most bytes of a line's text one `log` frame carries: a worker cuts a
/// longer line to its start and says by how much, so that no line a job
/// writes is ever a frame its server refuses.
pub(super) const LINE_BYTES_PER_FRAME: usize = 64 << 10;

/// A frame from the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToWorker {
    /// The first frame: the server's build, what the worker runs, and the
    /// limits it puts on itself.
    Setup {
        build: String,
        mode: WorkerMode,
        cpu_secs: u64,
        memory_bytes: u64,
    },
    /// The job, once the worker is ready.
    Job(Box<JobFrame>),
    /// The next run of the job's candles.
    Candles { bars: Vec<Bar> },
}

/// What a job runs, but its candles.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct JobFrame {
    pub(super) job_id: String,
    pub(super) submission: JobSubmission,
    /// The bundle's entry, `path.py:Class`, as its manifest names it.
    pub(super) entry: String,
    /// The bundle's parameters, as its manifest gives them.
    pub(super) params: Map<String, Value>,
    /// How many bars the `candles` frames after it carry in all.
    pub(super) bars: u64,
}

/// A frame from a worker.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum FromWorker {
    /// The answer to `setup`: the worker's build and process, what it
    /// enforced, and the runtime it offers, if any.
    Ready {
        build: String,
        pid: u32,
        posture: Posture,
        runtime: Option<Runtime>,
    },
    /// A line the job wrote to its log.
    Log {
        level: LogLevel,
        message: String,
        /// How many bytes the worker cut from the line's end.
        #[serde(default, skip_serializing_if = "is_zero")]
        dropped_bytes: u64,
    },
    /// How many of the job's bars its strategy has been handed so far.
    Progress { bars_done: u64 },
    /// The next run of a backtest's closed trades, before `finished`.
    Trades { rows: Vec<TradeFrame> },
    /// The next run of a backtest's equity curve, before `finished`.
    Equity { rows: Vec<EquityFrame> },
    /// How the job ended.
    Finished { outcome: Finished },
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

/// One closed trade, as a `trades` frame carries it.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TradeFrame {
    symbol: String,
    entry_time_ms: i64,
    exit_time_ms: i64,
    entry_price: ExactDecimal,
    exit_price: ExactDecimal,
    qty: ExactDecimal,
    gross_quote_pnl: ExactDecimal,
}

impl From<&TradeRow> for TradeFrame {
    fn from(row: &TradeRow) -> Self {
        Self {
            symbol: row.symbol.clone(),
            entry_time_ms: row.entry_time_ms,
            exit_time_ms: row.exit_time_ms,
            entry_price: row.entry_price,
            exit_price: row.exit_price,
            qty: row.qty,
            gross_quote_pnl: row.gross_quote_pnl,
        }
    }
}

impl From<TradeFrame> for TradeRow {
    fn from(frame: TradeFrame) -> Self {
        Self {
            symbol: frame.symbol,
            entry_time_ms: frame.entry_time_ms,
            exit_time_ms: frame.exit_time_ms,
            entry_price: frame.entry_price,
            exit_price: frame.exit_price,
            qty: frame.qty,
            gross_quote_pnl: frame.gross_quote_pnl,
        }
    }
}

/// One bar's equity, as an `equity` frame carries it.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct EquityFrame {
    open_time_ms: i64,
    equity: ExactDecimal,
}

impl From<&EquityRow> for EquityFrame {
    fn from(row: &EquityRow) -> Self {
        Self {
            open_time_ms: row.open_time_ms,
            equity: row.equity,
        }
    }
}

impl From<EquityFrame> for EquityRow {
    fn from(frame: EquityFrame) -> Self {
        Self {
            open_time_ms: frame.open_time_ms,
            equity: frame.equity,
        }
    }
}

/// How a job ended, as its worker saw it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Finished {
    /// A backtest that ran to its end: its summary. Its trades and its
    /// equity curve came in the frames before.
    Backtest { metrics: Metrics },
    /// With what it produced, as text: a synthetic workload's report.
    Succeeded { results: Vec<ResultText> },
    /// Without results, with why.
    Failed { error: ErrorBody },
}

/// One result, as text.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ResultText {
    pub(super) name: String,
    pub(super) media_type: String,
    pub(super) text: String,
}

/// Writes `frame` and flushes it.
pub(super) fn write<W: Write, T: Serialize>(output: &mut W, frame: &T) -> io::Result<()> {
    let body = serde_json::to_vec(frame).map_err(io::Error::other)?;
    let length = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a frame cannot hold 4 GiB or more",
        )
    })?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(&body)?;
    output.flush()
}

/// Reads the next frame: `None` when the input ends before one begins. A
/// frame of more than `most` bytes, or one cut short, is an error.
pub(super) fn read<R: Read, T: DeserializeOwned>(
    input: &mut R,
    most: usize,
) -> io::Result<Option<T>> {
    let mut length = [0_u8; 4];
    let mut filled = 0;
    while filled < length.len() {
        match input.read(&mut length[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let length = usize::try_from(u32::from_be_bytes(length)).unwrap_or(usize::MAX);
    if length > most {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {length} bytes, more than the {most} one may hold"),
        ));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests;
