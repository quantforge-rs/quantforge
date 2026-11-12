//! A job's log as a stream: the frames a server sends over the WebSocket
//! that follows a log to its end.

use serde::{Deserialize, Serialize};

use super::super::ErrorBody;
use super::logs::{JobProgress, LogLine};

/// A frame of a job's log stream, which the server sends when
/// `GET /v1/backtests/{job_id}/logs` is asked as a WebSocket. Each is one
/// text message holding a JSON object, and its `type` field names it.
///
/// ```
/// use quantforge::protocol::jobs::{LogFrame, LogLevel};
///
/// let json = r#"{"type":"line","seq":1,"at_ms":1792454401500,"level":"info","message":"started"}"#;
/// let frame: LogFrame = serde_json::from_str(json)?;
/// assert!(matches!(frame, LogFrame::Line(ref line) if line.level == LogLevel::Info));
/// assert_eq!(serde_json::to_string(&frame)?, json);
///
/// let end: LogFrame = serde_json::from_str(r#"{"type":"end"}"#)?;
/// assert_eq!(end, LogFrame::End);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogFrame {
    /// The next line of the log.
    Line(LogLine),
    /// How far the job has come, sent when it has changed since the server
    /// last said.
    Progress(JobProgress),
    /// Lines the server no longer holds, between the last line it sent, or
    /// the cursor, and the next.
    Truncated {
        /// How many lines are missing there.
        dropped_lines: u64,
    },
    /// The job has ended, and every line of its log the server holds has
    /// been sent. The server closes the connection after it.
    End,
    /// The stream cannot go on, and the server closes the connection. Its
    /// fields are an [`ErrorBody`]'s.
    Error(ErrorBody),
    /// A frame type this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

#[cfg(test)]
mod tests {
    use super::super::super::ErrorCode;
    use super::super::logs::LogLevel;
    use super::*;

    fn line(seq: u64, message: &str) -> LogLine {
        LogLine {
            seq,
            at_ms: 1_792_454_400_000 + i64::try_from(seq).expect("a small number"),
            level: LogLevel::Info,
            message: message.to_string(),
            dropped_bytes: None,
        }
    }

    /// Every frame a server sends, with its exact JSON.
    fn frames() -> Vec<(LogFrame, &'static str)> {
        vec![
            (
                LogFrame::Line(line(1, "started")),
                r#"{"type":"line","seq":1,"at_ms":1792454400001,"level":"info","message":"started"}"#,
            ),
            (
                LogFrame::Progress(JobProgress {
                    bars_done: 720,
                    bars_total: 1440,
                }),
                r#"{"type":"progress","bars_done":720,"bars_total":1440}"#,
            ),
            (
                LogFrame::Truncated {
                    dropped_lines: 9000,
                },
                r#"{"type":"truncated","dropped_lines":9000}"#,
            ),
            (LogFrame::End, r#"{"type":"end"}"#),
            (
                LogFrame::Error(ErrorBody {
                    code: ErrorCode::Unavailable,
                    message: "the server's store could not answer in time; retry later".to_string(),
                    supported: None,
                }),
                r#"{"type":"error","code":"unavailable","message":"the server's store could not answer in time; retry later"}"#,
            ),
        ]
    }

    #[test]
    fn every_frame_has_its_exact_json() {
        for (frame, json) in frames() {
            // Exhaustive on purpose: a new frame stops this module from
            // compiling until it is added to `frames`.
            match &frame {
                LogFrame::Line(_)
                | LogFrame::Progress(_)
                | LogFrame::Truncated { .. }
                | LogFrame::End
                | LogFrame::Error(_)
                | LogFrame::Unrecognized => {}
            }
            assert_eq!(serde_json::to_string(&frame).expect("serialize"), json);
            assert_eq!(
                serde_json::from_str::<LogFrame>(json).expect("a frame"),
                frame
            );
        }
    }

    #[test]
    fn a_frame_this_build_does_not_know_is_unrecognized_and_never_sent() {
        let later = r#"{"type":"checkpoint","bars_done":5,"note":"from a later revision"}"#;
        assert_eq!(
            serde_json::from_str::<LogFrame>(later).expect("a frame"),
            LogFrame::Unrecognized
        );
        assert!(serde_json::to_string(&LogFrame::Unrecognized).is_err());
        // A field a later revision adds to a known frame is ignored.
        let wider = r#"{"type":"end","state":"succeeded"}"#;
        assert_eq!(
            serde_json::from_str::<LogFrame>(wider).expect("a frame"),
            LogFrame::End
        );
    }

    // A tag read from buffered content would also take a variant's index.
    #[test]
    fn a_frame_named_by_a_number_or_by_nothing_is_refused() {
        for json in [
            r#"{"type":0}"#,
            r#"{"type":3}"#,
            r#"{"type":null}"#,
            "{}",
            "[]",
        ] {
            assert!(
                serde_json::from_str::<LogFrame>(json).is_err(),
                "{json} must be refused"
            );
        }
    }
}
