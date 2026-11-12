//! The log route: a job's log read a page at a time, or, asked as a
//! WebSocket, followed to its end. While a job runs both read the log the
//! server keeps for it; once it has ended they read what the store holds.
//! A stream takes what it sends from the log as it goes and keeps nothing
//! of its own, so a client that reads slowly, or not at all, costs the
//! server no more memory than one that keeps up.

use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::rejection::{PathRejection, QueryRejection};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::http::header::UPGRADE;
use axum::response::Response;
use tokio::sync::watch;
use tokio::time::{Interval, MissedTickBehavior};
use tracing::error;

use super::Jobs;
use super::log::{JobLog, Kept, LogRead};
use super::routes::{job_named, no_job};
use crate::model::{JobId, JobPhase};
use crate::protocol::jobs::{self, JobProgress, LOG_PAGE_LINES, LogFrame, LogPage, LogQuery};
use crate::protocol::{ErrorBody, ErrorCode};
use crate::server::history::fresh;
use crate::server::refusal::Refusal;

/// How many lines a stream takes from a log at a time.
const BATCH: usize = 256;

/// How long a stream stays silent before it pings its client, which shows
/// whether the client is still there.
const PING_EVERY: Duration = Duration::from_secs(30);

/// How long a client may take to accept one frame before the stream gives
/// it up.
const SEND_PATIENCE: Duration = Duration::from_secs(30);

/// How often a stream waiting for a queued job looks at the store without
/// being woken. A job that starts here, or is cancelled here, wakes it at
/// once; only a job whose end was written past this server shows this way.
const RECHECK: Duration = Duration::from_secs(30);

/// How soon a stream looks again at a store that was too busy to answer.
const RETRY: Duration = Duration::from_secs(1);

/// How long a closed stream waits for its client's close.
const CLOSE_PATIENCE: Duration = Duration::from_secs(2);

/// The most bytes a stream reads of one message from its client, which has
/// nothing to say.
const MOST_FROM_CLIENT: usize = 1 << 10;

/// `GET /v1/backtests/{job_id}/logs`.
pub(in crate::server) async fn logs(
    State(jobs): State<Jobs>,
    id: Result<Path<String>, PathRejection>,
    query: Result<Query<LogQuery>, QueryRejection>,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Result<Response, Refusal> {
    let Path(text) = id.map_err(|rejection| Refusal::invalid(rejection.body_text()))?;
    let job_id = job_named(text)?;
    let Query(query) = query.map_err(|rejection| {
        Refusal::invalid(format!(
            "the query string is not a log query: {}",
            rejection.body_text()
        ))
    })?;
    let after = query.after.unwrap_or(0);
    if !headers.contains_key(UPGRADE) {
        let limit = query.limit.unwrap_or(LOG_PAGE_LINES);
        if !(1..=LOG_PAGE_LINES).contains(&limit) {
            return Err(Refusal::invalid(format!(
                "limit {limit} is not between 1 and {LOG_PAGE_LINES}"
            )));
        }
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        return Ok(fresh(Json(page(&jobs, &job_id, after, limit).await?)));
    }
    let upgrade = upgrade.map_err(|rejection| {
        Refusal::invalid(format!(
            "the request is not a WebSocket upgrade: {}",
            rejection.body_text()
        ))
    })?;
    // While the answer is still an HTTP one: a job no one holds is refused
    // as on every job route.
    if jobs.live_log(&job_id).is_none() {
        stored(&jobs, &job_id).await?;
    }
    let open = jobs.stream_opened();
    Ok(upgrade
        .max_message_size(MOST_FROM_CLIENT)
        .max_frame_size(MOST_FROM_CLIENT)
        .on_upgrade(move |socket| async move {
            let _open = open;
            follow(socket, jobs, job_id, after, PING_EVERY).await;
        }))
}

/// One page of the job's log after `after`.
async fn page(jobs: &Jobs, job_id: &JobId, after: u64, limit: usize) -> Result<LogPage, Refusal> {
    let read = match jobs.live_log(job_id) {
        Some(log) => log.read(after, limit),
        None => {
            let (kept, phase) = stored(jobs, job_id).await?;
            kept.read(after, limit, phase.is_final())
        }
    };
    let job_id = jobs::JobId::new(job_id.as_str())
        .map_err(|_| Refusal::internal(format!("the server cannot read job {job_id}")))?;
    Ok(LogPage {
        job_id,
        lines: read.lines,
        dropped_lines: read.dropped_lines,
        next_after: read.next_after,
        progress: read.progress,
        complete: read.complete,
    })
}

/// What the store holds of the job's log, empty when it holds none, and
/// where the job stands.
async fn stored(jobs: &Jobs, job_id: &JobId) -> Result<(Kept, JobPhase), Refusal> {
    let id = job_id.clone();
    jobs.inner
        .store
        .call(move |store| {
            let record = store
                .job(&id)
                .map_err(|failure| Refusal::storage("reading a job", failure))?
                .ok_or_else(|| no_job(id.as_str()))?;
            let log_json = store
                .job_log(&id)
                .map_err(|failure| Refusal::storage("reading a job's log", failure))?;
            let kept = match log_json {
                None => Kept::default(),
                Some(log_json) => Kept::from_stored(&log_json).map_err(|why| {
                    error!(job = %id, %why, "a stored job's log cannot be read");
                    Refusal::internal(format!("the server cannot read the log of job {id}"))
                })?,
            };
            Ok((kept, record.phase))
        })
        .await
}

/// How a stream ends.
enum Close {
    /// The job has ended and its log was sent: `end`, then a normal close.
    Ended,
    /// The server is stopping, and the job has not ended.
    Away,
    /// The stream cannot go on: an `error` frame, then a close that says
    /// the server failed.
    Failed(Refusal),
    /// The client has gone: nothing more is sent.
    Gone,
}

/// Follows the job's log over `socket` from the line after `after`, to the
/// job's end or the client's, pinging a client it has not said anything to
/// for `ping_every`.
async fn follow(
    mut socket: WebSocket,
    jobs: Jobs,
    job_id: JobId,
    mut after: u64,
    ping_every: Duration,
) {
    let mut stopping = jobs.inner.stopping.subscribe();
    let mut moved = jobs.inner.moved.subscribe();
    let mut idle = tokio::time::interval(ping_every);
    idle.set_missed_tick_behavior(MissedTickBehavior::Delay);
    idle.reset();
    let mut said = None;
    let close = loop {
        // Marked before the job is looked for: a job that starts, or is
        // cancelled, after the look wakes the wait below.
        moved.borrow_and_update();
        if let Some(log) = jobs.live_log(&job_id) {
            break tail(&mut socket, &log, &mut after, &mut said, &mut idle).await;
        }
        let next_look = match stored(&jobs, &job_id).await {
            Ok((kept, phase)) if phase.is_final() => {
                break replay(&mut socket, &kept, &mut after, &mut said).await;
            }
            // Queued, or claimed a moment ago: its log begins soon.
            Ok(_) => RECHECK,
            // A store too busy to answer is no reason to end a stream that
            // has nothing to say yet: it is asked again soon.
            Err(refusal) if refusal.code == ErrorCode::Unavailable => RETRY,
            Err(refusal) => break Close::Failed(refusal),
        };
        // Until the job moves, and no longer than to the next look.
        let look = tokio::time::sleep(next_look);
        tokio::pin!(look);
        let waited = loop {
            tokio::select! {
                _ = moved.changed() => break None,
                () = &mut look => break None,
                () = stopped(&mut stopping) => break Some(Close::Away),
                message = socket.recv() => if gone(message.as_ref()) {
                    break Some(Close::Gone);
                },
                _ = idle.tick() => if !send(&mut socket, Message::Ping(Bytes::new())).await {
                    break Some(Close::Gone);
                },
            }
        };
        if let Some(close) = waited {
            break close;
        }
    };
    let sent = match close {
        Close::Ended => close_with(&mut socket, Some(LogFrame::End), close_code::NORMAL, "").await,
        Close::Away => {
            let reason = "the server is stopping; ask again once it is back";
            close_with(&mut socket, None, close_code::AWAY, reason).await
        }
        Close::Failed(refusal) => {
            let frame = LogFrame::Error(ErrorBody {
                code: refusal.code,
                message: refusal.message,
                supported: None,
            });
            close_with(&mut socket, Some(frame), close_code::ERROR, "").await
        }
        Close::Gone => false,
    };
    if sent {
        // The client's own close says that it has every frame.
        let closed = async { while let Some(Ok(_)) = socket.recv().await {} };
        let _ = tokio::time::timeout(CLOSE_PATIENCE, closed).await;
    }
}

/// Returns once the server is stopping.
async fn stopped(stopping: &mut watch::Receiver<bool>) {
    // What the wait answers borrows the channel, and is dropped here.
    let _ = stopping.wait_for(|stopping| *stopping).await;
}

/// Sends a running job's log as it grows, until the job has ended and its
/// last line was sent.
async fn tail(
    socket: &mut WebSocket,
    log: &JobLog,
    after: &mut u64,
    said: &mut Option<JobProgress>,
    idle: &mut Interval,
) -> Close {
    let mut changes = log.changes();
    loop {
        // Marked before the log is read: a line written after the read
        // wakes the wait below.
        changes.borrow_and_update();
        let read = log.read(*after, BATCH);
        let sent = send_read(socket, &read, after, said).await;
        match sent {
            None => return Close::Gone,
            Some(0) => {}
            // The client heard from the stream just now.
            Some(_) => idle.reset(),
        }
        if read.complete {
            return Close::Ended;
        }
        if read.lines.len() == BATCH {
            // More waits to be sent. A look at what the client said first,
            // without waiting for it: however long a job keeps its stream
            // busy, the client's pings are answered and its close is seen.
            if let Ok(message) = tokio::time::timeout(Duration::ZERO, socket.recv()).await {
                if gone(message.as_ref()) {
                    return Close::Gone;
                }
            }
            continue;
        }
        // Until the log changes; what a client says in between is read and
        // dropped, and a silent stream pings it.
        loop {
            tokio::select! {
                _ = changes.changed() => break,
                message = socket.recv() => if gone(message.as_ref()) {
                    return Close::Gone;
                },
                _ = idle.tick() => if !send(socket, Message::Ping(Bytes::new())).await {
                    return Close::Gone;
                },
            }
        }
    }
}

/// Sends an ended job's stored log, a batch at a time.
async fn replay(
    socket: &mut WebSocket,
    kept: &Kept,
    after: &mut u64,
    said: &mut Option<JobProgress>,
) -> Close {
    loop {
        let read = kept.read(*after, BATCH, true);
        if send_read(socket, &read, after, said).await.is_none() {
            return Close::Gone;
        }
        if read.complete {
            return Close::Ended;
        }
    }
}

/// Sends what one read holds: what was dropped before its lines, the
/// lines, and the job's progress when it is not what was last sent. Moves
/// the cursor past it. Answers how many frames it sent, or `None` once the
/// client has gone.
async fn send_read(
    socket: &mut WebSocket,
    read: &LogRead,
    after: &mut u64,
    said: &mut Option<JobProgress>,
) -> Option<usize> {
    let mut sent = 0;
    if read.dropped_lines > 0 {
        let dropped_lines = read.dropped_lines;
        if !send_frame(socket, &LogFrame::Truncated { dropped_lines }).await {
            return None;
        }
        sent += 1;
    }
    for line in &read.lines {
        if !send_frame(socket, &LogFrame::Line(line.clone())).await {
            return None;
        }
        sent += 1;
    }
    *after = read.next_after;
    if let Some(progress) = read.progress.filter(|progress| Some(*progress) != *said) {
        if !send_frame(socket, &LogFrame::Progress(progress)).await {
            return None;
        }
        *said = Some(progress);
        sent += 1;
    }
    Some(sent)
}

async fn send_frame(socket: &mut WebSocket, frame: &LogFrame) -> bool {
    match serde_json::to_string(frame) {
        Ok(json) => send(socket, Message::Text(json.into())).await,
        Err(failure) => {
            error!(%failure, "a log frame cannot be written");
            false
        }
    }
}

/// Sends one message, or gives the client up when it does not take it in
/// time.
async fn send(socket: &mut WebSocket, message: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_PATIENCE, socket.send(message)).await,
        Ok(Ok(()))
    )
}

/// Sends a last frame, if any, and WebSocket's close.
async fn close_with(
    socket: &mut WebSocket,
    last: Option<LogFrame>,
    code: u16,
    reason: &'static str,
) -> bool {
    if let Some(frame) = last {
        if !send_frame(socket, &frame).await {
            return false;
        }
    }
    let close = CloseFrame {
        code,
        reason: reason.into(),
    };
    send(socket, Message::Close(Some(close))).await
}

/// Whether what a read of the socket answered says that the client has
/// gone. Anything else a client sends is ignored.
fn gone(message: Option<&Result<Message, axum::Error>>) -> bool {
    matches!(message, None | Some(Err(_) | Ok(Message::Close(_))))
}

#[cfg(test)]
mod tests;
