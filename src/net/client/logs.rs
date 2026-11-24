//! A job's log followed to its end: over the WebSocket stream the log route
//! offers, and a page at a time where the stream cannot be had or breaks
//! off, from the last line received, so that no line is shown twice or
//! missed.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::debug;

use super::{CONNECT_TIMEOUT, ClientError, RemoteClient};
use crate::protocol::jobs::{JobId, JobProgress, LogFrame, LogLine};

/// How long a stream may stay silent before it is given up: the server
/// pings one it has had nothing to send on for 30 seconds.
const STREAM_SILENCE: Duration = Duration::from_secs(90);

/// How long to wait before asking for the next page, when the last one held
/// no line.
const PAGE_WAIT: Duration = Duration::from_millis(500);

/// What a followed log says, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LogEvent {
    /// The next line.
    Line(LogLine),
    /// How far the job has come, when that changed.
    Progress(JobProgress),
    /// Lines the server no longer holds, before the next line.
    Dropped(u64),
}

/// Where a follow stands: the last line shown, and the progress last shown.
struct Cursor {
    after: u64,
    progress: Option<JobProgress>,
}

impl Cursor {
    fn line(&mut self, line: LogLine, on: &mut impl FnMut(LogEvent)) {
        if line.seq > self.after {
            self.after = line.seq;
            on(LogEvent::Line(line));
        }
    }

    /// Lines the server no longer holds, right after the cursor: the server
    /// numbers its next line past them, so the cursor passes them too, and
    /// a page read after a break does not report them again.
    fn dropped(&mut self, count: u64, on: &mut impl FnMut(LogEvent)) {
        if count > 0 {
            self.after = self.after.saturating_add(count);
            on(LogEvent::Dropped(count));
        }
    }

    fn progress(&mut self, progress: JobProgress, on: &mut impl FnMut(LogEvent)) {
        if self.progress != Some(progress) {
            self.progress = Some(progress);
            on(LogEvent::Progress(progress));
        }
    }
}

/// How a stream ended.
enum Streamed {
    /// With `end`: the log is whole.
    Ended,
    /// Some other way, for the pages to take over.
    Broken(String),
}

impl RemoteClient {
    /// Follows `job`'s log from the line after `after` until the job has
    /// ended and its last line was handed to `on`. Over `http` it takes the
    /// WebSocket stream; over `https`, which this build's stream does not
    /// speak, and wherever the stream fails, it reads pages.
    pub async fn follow(
        &self,
        job: &JobId,
        after: u64,
        mut on: impl FnMut(LogEvent),
    ) -> Result<(), ClientError> {
        let mut cursor = Cursor {
            after,
            progress: None,
        };
        if self.base.scheme() == "http" {
            match self.stream(job, &mut cursor, &mut on).await {
                Streamed::Ended => return Ok(()),
                Streamed::Broken(why) => {
                    debug!(%why, after = cursor.after, "reading the job's log a page at a time");
                }
            }
        }
        self.pages(job, &mut cursor, &mut on).await
    }

    async fn stream(
        &self,
        job: &JobId,
        cursor: &mut Cursor,
        on: &mut impl FnMut(LogEvent),
    ) -> Streamed {
        let mut address = self.route(&format!("backtests/{job}/logs?after={}", cursor.after));
        if address.set_scheme("ws").is_err() {
            return Streamed::Broken("the URL has no WebSocket form".to_string());
        }
        let mut request = match address.as_str().into_client_request() {
            Ok(request) => request,
            Err(error) => return Streamed::Broken(error.to_string()),
        };
        if let Some(bearer) = &self.bearer {
            request.headers_mut().insert(AUTHORIZATION, bearer.clone());
        }
        let connected =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(request)).await;
        let mut stream = match connected {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(error)) => return Streamed::Broken(error.to_string()),
            Err(_) => return Streamed::Broken("the upgrade timed out".to_string()),
        };
        loop {
            let message = match tokio::time::timeout(STREAM_SILENCE, stream.next()).await {
                Err(_) => return Streamed::Broken("the stream fell silent".to_string()),
                Ok(None) => return Streamed::Broken("the stream closed".to_string()),
                Ok(Some(Err(error))) => return Streamed::Broken(error.to_string()),
                Ok(Some(Ok(message))) => message,
            };
            let Message::Text(text) = message else {
                if matches!(message, Message::Close(_)) {
                    return Streamed::Broken("the server closed the stream".to_string());
                }
                continue;
            };
            match serde_json::from_str::<LogFrame>(text.as_str()) {
                Ok(LogFrame::Line(line)) => cursor.line(line, on),
                Ok(LogFrame::Progress(progress)) => cursor.progress(progress, on),
                Ok(LogFrame::Truncated { dropped_lines }) => cursor.dropped(dropped_lines, on),
                Ok(LogFrame::End) => return Streamed::Ended,
                Ok(LogFrame::Error(body)) => return Streamed::Broken(body.message),
                Ok(_) => {}
                Err(error) => return Streamed::Broken(format!("an unreadable frame: {error}")),
            }
        }
    }

    async fn pages(
        &self,
        job: &JobId,
        cursor: &mut Cursor,
        on: &mut impl FnMut(LogEvent),
    ) -> Result<(), ClientError> {
        loop {
            let page = self.log_page(job, cursor.after).await?;
            let had_lines = !page.lines.is_empty();
            cursor.dropped(page.dropped_lines, on);
            for line in page.lines {
                cursor.line(line, on);
            }
            if let Some(progress) = page.progress {
                cursor.progress(progress, on);
            }
            cursor.after = cursor.after.max(page.next_after);
            if page.complete {
                return Ok(());
            }
            if !had_lines {
                tokio::time::sleep(PAGE_WAIT).await;
            }
        }
    }
}

#[cfg(test)]
mod tests;
