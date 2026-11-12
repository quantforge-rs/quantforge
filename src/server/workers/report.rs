//! What a job says to its server while it runs: the lines it writes to its
//! log, and how many of its bars it has run, each a frame on the worker's
//! standard output. Saying it is best effort: once a frame cannot be
//! written the server has gone, nothing more is said, and the worker's
//! watch on its input ends the process.

use std::io::Write;

use super::frames::{self, FromWorker, LINE_BYTES_PER_FRAME};
use crate::protocol::jobs::LogLevel;
use crate::server::jobs::start_of;

/// A job's voice: the worker's output, for as long as its server reads it.
pub(super) struct Reporter<'a> {
    output: &'a mut (dyn Write + Send),
    /// Set once a frame could not be written.
    lost: bool,
}

impl<'a> Reporter<'a> {
    pub(super) fn new(output: &'a mut (dyn Write + Send)) -> Self {
        Self {
            output,
            lost: false,
        }
    }

    /// Writes a line to the job's log. A line longer than a frame carries
    /// is cut to its start, between two characters, and says by how much.
    pub(super) fn line(&mut self, level: LogLevel, message: &str) {
        let sent = start_of(message, LINE_BYTES_PER_FRAME);
        self.send(&FromWorker::Log {
            level,
            message: sent.to_string(),
            dropped_bytes: u64::try_from(message.len() - sent.len()).unwrap_or(u64::MAX),
        });
    }

    /// Says that the strategy has been handed `bars_done` of the job's bars.
    pub(super) fn progress(&mut self, bars_done: u64) {
        self.send(&FromWorker::Progress { bars_done });
    }

    fn send(&mut self, frame: &FromWorker) {
        if !self.lost && frames::write(&mut self.output, frame).is_err() {
            self.lost = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frames written to `output`, as their JSON.
    fn frames_of(mut output: &[u8]) -> Vec<String> {
        let mut found = Vec::new();
        while let Some(frame) =
            frames::read::<_, serde_json::Value>(&mut output, 1 << 20).expect("a frame")
        {
            found.push(frame.to_string());
        }
        found
    }

    #[test]
    fn a_line_and_progress_are_each_a_frame_in_the_order_said() {
        let mut output = Vec::new();
        let mut reporter = Reporter::new(&mut output);
        reporter.line(LogLevel::Info, "started");
        reporter.progress(5);
        reporter.line(LogLevel::Error, "");
        assert_eq!(
            frames_of(&output),
            [
                r#"{"level":"info","message":"started","type":"log"}"#,
                r#"{"bars_done":5,"type":"progress"}"#,
                r#"{"level":"error","message":"","type":"log"}"#,
            ]
        );
    }

    #[test]
    fn a_line_longer_than_a_frame_carries_is_cut_between_two_characters() {
        let mut output = Vec::new();
        // One byte short of the limit, then a two-byte character across it.
        let long = format!("{}é and more", "x".repeat(LINE_BYTES_PER_FRAME - 1));
        Reporter::new(&mut output).line(LogLevel::Debug, &long);
        let frame: serde_json::Value =
            serde_json::from_str(&frames_of(&output)[0]).expect("a frame");
        let message = frame["message"].as_str().expect("text");
        assert_eq!(message.len(), LINE_BYTES_PER_FRAME - 1);
        assert_eq!(
            frame["dropped_bytes"],
            u64::try_from(long.len() - message.len()).expect("a count")
        );
    }

    /// Takes `room` bytes, then fails every write.
    struct Closing {
        room: usize,
        written: usize,
    }

    impl Write for Closing {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.written + bytes.len() > self.room {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.written += bytes.len();
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn once_the_server_has_gone_nothing_more_is_written() {
        let mut output = Closing {
            room: 60,
            written: 0,
        };
        let mut reporter = Reporter::new(&mut output);
        reporter.line(LogLevel::Info, "fits");
        assert!(!reporter.lost);
        reporter.line(LogLevel::Info, "does not fit in what is left of the pipe");
        assert!(reporter.lost);
        reporter.progress(1);
        reporter.line(LogLevel::Info, "x");
        let written = output.written;
        assert!(written > 0 && written <= 60, "{written}");
    }
}
