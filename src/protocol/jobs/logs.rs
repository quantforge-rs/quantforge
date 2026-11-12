//! A job's log: the lines it wrote while it ran and how far it has come,
//! read a page at a time. `log_stream` holds the frames that follow it.

use serde::{Deserialize, Serialize};

use super::JobId;

/// The most lines one [`LogPage`] holds, and how many it holds when the
/// query names no `limit`.
pub const LOG_PAGE_LINES: u64 = 1_000;

/// How serious a line is.
///
/// ```
/// use quantforge::protocol::jobs::LogLevel;
///
/// let level: LogLevel = serde_json::from_str(r#""warning""#)?;
/// assert_eq!(level, LogLevel::Warning);
/// assert_eq!(serde_json::to_string(&level)?, r#""warning""#);
/// // A level from a later revision of the protocol still reads.
/// assert_eq!(serde_json::from_str::<LogLevel>(r#""trace""#)?, LogLevel::Unrecognized);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogLevel {
    /// Detail for whoever debugs the strategy.
    Debug,
    /// What the job is doing.
    Info,
    /// Something unexpected that the job went on from.
    Warning,
    /// Something that went wrong.
    Error,
    /// A level this build does not know, from a later revision of the
    /// protocol. Never sent: serializing it fails.
    #[serde(other, skip_serializing)]
    Unrecognized,
}

/// One line of a job's log.
///
/// ```
/// use quantforge::protocol::jobs::{LogLevel, LogLine};
///
/// let json = r#"{"seq":42,"at_ms":1792454401500,"level":"info","message":"fast crossed above slow at 81940.01"}"#;
/// let line: LogLine = serde_json::from_str(json)?;
/// assert_eq!((line.seq, line.level), (42, LogLevel::Info));
/// assert_eq!(line.dropped_bytes, None);
/// assert_eq!(serde_json::to_string(&line)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// The line's number in its job's log. The server numbers a job's lines
    /// from 1, in the order it took them, and never uses a number twice: a
    /// client that has line `seq` asks for what follows with `after=seq`.
    pub seq: u64,
    /// When the server took the line, by its own clock.
    pub at_ms: i64,
    /// How serious the line is.
    pub level: LogLevel,
    /// The line's text.
    pub message: String,
    /// How many bytes of the text the server left out, when the line was
    /// longer than it keeps of one: `message` is then the line's start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_bytes: Option<u64>,
}

/// How far a job has come: the bars it has run, of the bars it runs on.
///
/// ```
/// use quantforge::protocol::jobs::JobProgress;
///
/// let json = r#"{"bars_done":250000,"bars_total":527040}"#;
/// let progress: JobProgress = serde_json::from_str(json)?;
/// assert_eq!(progress.bars_done, 250_000);
/// assert_eq!(serde_json::to_string(&progress)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgress {
    /// The bars the strategy has been handed so far.
    pub bars_done: u64,
    /// The bars the job runs on: the candles its server read for it.
    pub bars_total: u64,
}

/// The query string of `GET /v1/backtests/{job_id}/logs`, for a page and
/// for a stream alike:
///
/// ```text
/// GET /v1/backtests/7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e/logs?after=42&limit=500
/// ```
///
/// A server ignores a parameter it does not know, and refuses one given
/// twice. A client in Rust can hand this struct to any HTTP client that
/// encodes a query string with serde.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogQuery {
    /// The number of the last line the client holds: the answer starts
    /// with the line after it. Left out, or 0, it starts with the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<u64>,
    /// The most lines a page may hold, from 1 to [`LOG_PAGE_LINES`], which
    /// it is when left out. A stream sends every line and ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

/// What `GET /v1/backtests/{job_id}/logs` answers when it is not asked as
/// a WebSocket: the lines of the job's log after the query's cursor.
///
/// ```
/// use quantforge::protocol::jobs::LogPage;
///
/// let json = r#"{"job_id":"7f3a0c1e9b2d4f6a8c0e1b3d5f7a9c2e","lines":[{"seq":43,"at_ms":1792454401600,"level":"warning","message":"no trade in 500 bars"}],"dropped_lines":0,"next_after":43,"progress":{"bars_done":600,"bars_total":1440},"complete":false}"#;
/// let page: LogPage = serde_json::from_str(json)?;
/// assert_eq!(page.lines[0].seq, 43);
/// assert_eq!((page.next_after, page.complete), (43, false));
/// assert_eq!(serde_json::to_string(&page)?, json);
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    /// The job.
    pub job_id: JobId,
    /// The lines after the cursor, oldest first, at most the query's
    /// `limit` of them.
    pub lines: Vec<LogLine>,
    /// How many lines after the cursor, and before the first of `lines`,
    /// the server no longer holds: it keeps only the newest of a job that
    /// writes more than it keeps. 0 when none is missing.
    pub dropped_lines: u64,
    /// The cursor to ask with next: the number of the last line this page
    /// accounts for.
    pub next_after: u64,
    /// How far the job has come, once it has said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<JobProgress>,
    /// Whether the log ends here: the job has ended, and no line follows
    /// `next_after`. Until then a client asks again.
    pub complete: bool,
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn every_level_has_its_name_and_an_unknown_one_is_unrecognized_and_never_sent() {
        for (level, name) in [
            (LogLevel::Debug, "debug"),
            (LogLevel::Info, "info"),
            (LogLevel::Warning, "warning"),
            (LogLevel::Error, "error"),
        ] {
            // Exhaustive on purpose: a new level stops this module from
            // compiling until it is listed above.
            match level {
                LogLevel::Debug
                | LogLevel::Info
                | LogLevel::Warning
                | LogLevel::Error
                | LogLevel::Unrecognized => {}
            }
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(&level).expect("serialize"), json);
            assert_eq!(
                serde_json::from_str::<LogLevel>(&json).expect("a level"),
                level
            );
        }
        for unknown in [r#""critical""#, r#""INFO""#, r#""""#] {
            assert_eq!(
                serde_json::from_str::<LogLevel>(unknown).expect("a level"),
                LogLevel::Unrecognized,
                "{unknown}"
            );
        }
        assert!(serde_json::from_str::<LogLevel>("1").is_err());
        assert!(serde_json::to_string(&LogLevel::Unrecognized).is_err());
    }

    #[test]
    fn a_line_has_its_exact_json_and_says_what_was_cut_from_it() {
        let whole = line(7, "a line\twith \"quotes\" and ünïcode");
        let json = r#"{"seq":7,"at_ms":1792454400007,"level":"info","message":"a line\twith \"quotes\" and ünïcode"}"#;
        assert_eq!(serde_json::to_string(&whole).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<LogLine>(json).expect("a line"),
            whole
        );

        let cut = LogLine {
            dropped_bytes: Some(1_040_384),
            level: LogLevel::Error,
            ..line(8, "the start of a very long line")
        };
        let json = r#"{"seq":8,"at_ms":1792454400008,"level":"error","message":"the start of a very long line","dropped_bytes":1040384}"#;
        assert_eq!(serde_json::to_string(&cut).expect("serialize"), json);
        assert_eq!(serde_json::from_str::<LogLine>(json).expect("a line"), cut);
        // A missing count and a null one read alike.
        let null = r#"{"seq":7,"at_ms":1792454400007,"level":"info","message":"a line\twith \"quotes\" and ünïcode","dropped_bytes":null}"#;
        assert_eq!(
            serde_json::from_str::<LogLine>(null).expect("a line"),
            whole
        );
    }

    #[test]
    fn every_field_of_a_line_but_the_cut_is_required() {
        let json = r#"{"seq":7,"at_ms":1,"level":"info","message":"m"}"#;
        for field in [
            r#""seq":7,"#,
            r#""at_ms":1,"#,
            r#""level":"info","#,
            r#","message":"m""#,
        ] {
            let without = json.replace(field, "");
            assert_ne!(without, json, "{field} is in the fixture");
            assert!(
                serde_json::from_str::<LogLine>(&without).is_err(),
                "a line without {field} must be refused"
            );
        }
        assert!(serde_json::from_str::<LogLine>(&json.replace("7", "-7")).is_err());
    }

    #[test]
    fn a_page_has_its_exact_json_with_and_without_progress() {
        let page = LogPage {
            job_id: JobId::new("j-1").expect("a job id"),
            lines: vec![line(3, "three"), line(4, "four")],
            dropped_lines: 2,
            next_after: 4,
            progress: None,
            complete: true,
        };
        let json = r#"{"job_id":"j-1","lines":[{"seq":3,"at_ms":1792454400003,"level":"info","message":"three"},{"seq":4,"at_ms":1792454400004,"level":"info","message":"four"}],"dropped_lines":2,"next_after":4,"complete":true}"#;
        assert_eq!(serde_json::to_string(&page).expect("serialize"), json);
        assert_eq!(serde_json::from_str::<LogPage>(json).expect("a page"), page);

        let empty = LogPage {
            lines: Vec::new(),
            dropped_lines: 0,
            next_after: 0,
            progress: Some(JobProgress {
                bars_done: 0,
                bars_total: 10,
            }),
            complete: false,
            ..page
        };
        let json = r#"{"job_id":"j-1","lines":[],"dropped_lines":0,"next_after":0,"progress":{"bars_done":0,"bars_total":10},"complete":false}"#;
        assert_eq!(serde_json::to_string(&empty).expect("serialize"), json);
        assert_eq!(
            serde_json::from_str::<LogPage>(json).expect("a page"),
            empty
        );
        for field in [
            r#""dropped_lines":0,"#,
            r#""next_after":0,"#,
            r#","complete":false"#,
        ] {
            assert!(
                serde_json::from_str::<LogPage>(&json.replace(field, "")).is_err(),
                "a page without {field} must be refused"
            );
        }
    }

    #[test]
    fn a_query_is_written_and_read_as_a_query_string_names_it() {
        let query = LogQuery {
            after: Some(42),
            limit: None,
        };
        assert_eq!(
            serde_json::to_string(&query).expect("serialize"),
            r#"{"after":42}"#
        );
        assert_eq!(
            serde_json::from_str::<LogQuery>("{}").expect("a query"),
            LogQuery::default()
        );
        assert_eq!(LOG_PAGE_LINES, 1_000);
    }
}
