//! A job's log as the server keeps it: the newest lines the job wrote, how
//! many older ones it dropped to stay within its limits, and how far the
//! job has come. The job's worker writes into it, the log route reads pages
//! of it and follows it while the job runs, and once the job has ended the
//! same content is stored with the job. Whatever a job writes, a log never
//! holds more than its [`LogLimits`].

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::model::now_utc_ms;
use crate::protocol::jobs::{JobProgress, LogLevel, LogLine};

/// The lines of one job's log a server keeps unless told otherwise.
pub const DEFAULT_LOG_LINES: usize = 10_000;

/// The bytes of text those lines may hold together: 1 MiB.
pub const DEFAULT_LOG_BYTES: usize = 1 << 20;

/// The bytes kept of one line: 8 KiB.
pub const DEFAULT_LOG_LINE_BYTES: usize = 8 << 10;

/// How much of one job's log the server keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogLimits {
    /// The most lines kept, the newest; at least 1.
    pub max_lines: usize,
    /// The most bytes of text the kept lines hold together.
    pub max_bytes: usize,
    /// The most bytes kept of one line: its start.
    pub max_line_bytes: usize,
}

impl Default for LogLimits {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_LOG_LINES,
            max_bytes: DEFAULT_LOG_BYTES,
            max_line_bytes: DEFAULT_LOG_LINE_BYTES,
        }
    }
}

/// What is kept of a log, and the form it is stored in once its job has
/// ended: the kept lines are numbered from one past `dropped_lines`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Kept {
    dropped_lines: u64,
    lines: VecDeque<LogLine>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<JobProgress>,
}

/// One read of a log: the lines after a cursor, and where the log stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LogRead {
    /// The lines after the cursor, oldest first.
    pub(super) lines: Vec<LogLine>,
    /// How many lines between the cursor and the first of `lines` are no
    /// longer kept.
    pub(super) dropped_lines: u64,
    /// The cursor after this read.
    pub(super) next_after: u64,
    pub(super) progress: Option<JobProgress>,
    /// Whether the job has ended and nothing follows `next_after`.
    pub(super) complete: bool,
}

impl Kept {
    /// A stored log, or why it is not one this build can answer: its lines
    /// must be numbered one after another from one past the dropped ones,
    /// each at a level this build knows, since no other can be sent on.
    pub(super) fn from_stored(json: &str) -> Result<Self, String> {
        let kept: Self = serde_json::from_str(json).map_err(|failure| failure.to_string())?;
        for (seq, line) in (kept.dropped_lines + 1..).zip(&kept.lines) {
            if line.seq != seq {
                return Err(format!("line {seq} is numbered {}", line.seq));
            }
            if line.level == LogLevel::Unrecognized {
                return Err(format!("line {seq} has a level this build does not know"));
            }
        }
        Ok(kept)
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.dropped_lines == 0 && self.progress.is_none()
    }

    /// The number of the newest line ever taken.
    fn last(&self) -> u64 {
        self.dropped_lines + self.lines.len() as u64
    }

    /// Up to `limit` lines after the line numbered `after`, in a log whose
    /// job has `ended` or not.
    pub(super) fn read(&self, after: u64, limit: usize, ended: bool) -> LogRead {
        let last = self.last();
        let (lines, dropped_lines, next_after) = if after >= last {
            (Vec::new(), 0, after)
        } else {
            // Lines up to `from` are behind the cursor, or gone.
            let from = after.max(self.dropped_lines);
            let index = usize::try_from(from - self.dropped_lines).unwrap_or(usize::MAX);
            let lines: Vec<LogLine> = self
                .lines
                .range(index.min(self.lines.len())..)
                .take(limit.max(1))
                .cloned()
                .collect();
            let next = lines.last().map_or(from, |line| line.seq);
            (lines, from - after, next)
        };
        LogRead {
            lines,
            dropped_lines,
            next_after,
            progress: self.progress,
            complete: ended && next_after >= last,
        }
    }
}

/// The log of one job. Cloned, it is the same log.
#[derive(Clone)]
pub struct JobLog {
    inner: Arc<Inner>,
}

struct Inner {
    limits: LogLimits,
    state: Mutex<State>,
    /// Counts every change, so that whoever follows the log wakes for it.
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct State {
    kept: Kept,
    /// The bytes of text the kept lines hold.
    bytes: usize,
    /// Set once the log takes no more.
    closed: bool,
    /// Set once whoever reads the log is told that it has ended.
    ended: bool,
}

impl JobLog {
    /// An empty log that keeps no more than `limits`.
    pub fn new(limits: LogLimits) -> Self {
        Self {
            inner: Arc::new(Inner {
                limits,
                state: Mutex::new(State::default()),
                changed: watch::channel(0).0,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn woke(&self) {
        self.inner.changed.send_modify(|changes| *changes += 1);
    }

    /// Adds a line, stamped now and numbered after the last. A line longer
    /// than the log keeps of one is cut to its start, and says by how much;
    /// the oldest lines go when the log would hold more than it may, and
    /// are counted. A log that is closed takes no more.
    /// [`LogLevel::Unrecognized`], which names no level and is never sent,
    /// is kept as `info`.
    pub fn line(&self, level: LogLevel, message: &str) {
        self.line_cut(level, message, 0);
    }

    /// [`JobLog::line`] for a line whose writer already cut `cut_before`
    /// bytes from its end.
    pub(in crate::server) fn line_cut(&self, level: LogLevel, message: &str, cut_before: u64) {
        let limits = self.inner.limits;
        // A lone line always fits the log.
        let kept = start_of(message, limits.max_line_bytes.min(limits.max_bytes));
        let cut_here = u64::try_from(message.len() - kept.len()).unwrap_or(u64::MAX);
        let dropped_bytes = Some(cut_here.saturating_add(cut_before)).filter(|cut| *cut > 0);
        // Whatever a log holds can be written, for a client and for a store.
        let level = match level {
            LogLevel::Unrecognized => LogLevel::Info,
            known => known,
        };
        let at_ms = now_utc_ms();
        {
            let mut state = self.state();
            if state.closed {
                return;
            }
            let seq = state.kept.last() + 1;
            state.bytes += kept.len();
            state.kept.lines.push_back(LogLine {
                seq,
                at_ms,
                level,
                message: kept.to_string(),
                dropped_bytes,
            });
            while state.kept.lines.len() > 1
                && (state.kept.lines.len() > limits.max_lines || state.bytes > limits.max_bytes)
            {
                if let Some(oldest) = state.kept.lines.pop_front() {
                    state.bytes -= oldest.message.len();
                    state.kept.dropped_lines += 1;
                }
            }
        }
        self.woke();
    }

    /// Says how far the job has come: `bars_done` of `bars_total`, never
    /// more than the total, and never less than it said before.
    pub fn progress(&self, bars_done: u64, bars_total: u64) {
        let progress = JobProgress {
            bars_done: bars_done.min(bars_total),
            bars_total,
        };
        {
            let mut state = self.state();
            let behind = state.kept.progress.is_some_and(|said| {
                said.bars_total == bars_total && said.bars_done >= progress.bars_done
            });
            if state.closed || behind {
                return;
            }
            state.kept.progress = Some(progress);
        }
        self.woke();
    }

    /// Closes the log to whoever writes it: nothing more is added. Whoever
    /// reads it is not told that it has ended before [`JobLog::end`], so
    /// that what the log holds at its end can be stored first.
    pub fn close(&self) {
        self.state().closed = true;
    }

    /// Ends the log: its job has ended, nothing more is added, and whoever
    /// reads it is told so.
    pub fn end(&self) {
        {
            let mut state = self.state();
            state.closed = true;
            state.ended = true;
        }
        self.woke();
    }

    /// Whether the log has ended.
    pub fn has_ended(&self) -> bool {
        self.state().ended
    }

    /// Up to `limit` lines after the line numbered `after`.
    pub(super) fn read(&self, after: u64, limit: usize) -> LogRead {
        let state = self.state();
        state.kept.read(after, limit, state.ended)
    }

    /// Changes whenever the log does: a line, the progress, or its end.
    pub(super) fn changes(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    /// The log as a store keeps it, the server's JSON, or `None` for a log
    /// nothing was ever written to.
    pub fn stored_json(&self) -> Option<String> {
        let state = self.state();
        if state.kept.is_empty() {
            return None;
        }
        serde_json::to_string(&state.kept).ok()
    }
}

impl fmt::Debug for JobLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state();
        f.debug_struct("JobLog")
            .field("lines", &state.kept.lines.len())
            .field("dropped_lines", &state.kept.dropped_lines)
            .field("progress", &state.kept.progress)
            .field("ended", &state.ended)
            .finish()
    }
}

/// The longest start of `text` of at most `most` bytes that ends between
/// two characters.
pub(in crate::server) fn start_of(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text;
    }
    let mut end = most;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests;
