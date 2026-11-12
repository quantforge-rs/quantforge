//! A job's log through the server, with a worker the test writes through:
//! a client attached while the job runs sees the lines written before it
//! came and then each as it comes, with the job's progress; one that comes
//! after the job ended gets the whole log, from that server and from the
//! next on its store; and a job that floods its log leaves the newest
//! lines and a count of the rest, never more than the log's limits.

use futures_util::StreamExt;
use quantforge::protocol::jobs::{JobProgress, LogFrame, LogLevel, LogLine, LogPage};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::*;

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Opens the job's log stream, from the line after `after`.
async fn follow(address: SocketAddr, job: &str, after: u64) -> Stream {
    let url = format!("ws://{address}/v1/backtests/{job}/logs?after={after}");
    let (stream, response) = tokio_tungstenite::connect_async(url)
        .await
        .expect("a log stream");
    assert_eq!(response.status(), 101);
    stream
}

/// What a stream said next: a frame, or the close it ended with.
#[derive(Debug, PartialEq)]
enum Said {
    Frame(LogFrame),
    Closed(Option<CloseCode>),
}

/// The next thing the stream says, within ten seconds.
async fn next(stream: &mut Stream) -> Said {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), stream.next())
            .await
            .expect("the stream says something within ten seconds");
        match message {
            None => return Said::Closed(None),
            Some(Ok(Message::Text(text))) => {
                return Said::Frame(serde_json::from_str(text.as_str()).expect("a log frame"));
            }
            Some(Ok(Message::Close(close))) => {
                return Said::Closed(close.map(|close| close.code));
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            Some(other) => panic!("a log stream sent {other:?}"),
        }
    }
}

/// Whether the stream stays silent for a while.
async fn silent(stream: &mut Stream) -> bool {
    tokio::time::timeout(Duration::from_millis(300), stream.next())
        .await
        .is_err()
}

/// A line as the tests compare them: its number, level and text.
fn shown(line: &LogLine) -> (u64, LogLevel, String) {
    (line.seq, line.level, line.message.clone())
}

/// The frames of a stream to its end: every line, the progress it last
/// said, how many lines it said were dropped, and how it closed.
struct Followed {
    lines: Vec<LogLine>,
    progress: Option<JobProgress>,
    dropped_lines: u64,
    ended: bool,
    closed: Option<CloseCode>,
}

/// Reads a stream to its end, from the line after `after`: the last line
/// the test has from it, or its cursor. Whatever a stream sends, no line is
/// passed over in silence: each line is numbered one past the line before
/// it, or past the lines a `truncated` frame counted.
async fn to_the_end(stream: &mut Stream, after: u64) -> Followed {
    let mut followed = Followed {
        lines: Vec::new(),
        progress: None,
        dropped_lines: 0,
        ended: false,
        closed: None,
    };
    let mut expected = after + 1;
    loop {
        match next(stream).await {
            Said::Frame(LogFrame::Line(line)) => {
                assert!(!followed.ended, "a line after the end");
                assert_eq!(line.seq, expected, "a line passed over in silence");
                expected += 1;
                followed.lines.push(line);
            }
            Said::Frame(LogFrame::Progress(progress)) => followed.progress = Some(progress),
            Said::Frame(LogFrame::Truncated { dropped_lines }) => {
                assert!(dropped_lines > 0, "a count of nothing");
                expected += dropped_lines;
                followed.dropped_lines += dropped_lines;
            }
            Said::Frame(LogFrame::End) => followed.ended = true,
            Said::Frame(other) => panic!("a log stream sent {other:?}"),
            Said::Closed(code) => {
                followed.closed = code;
                return followed;
            }
        }
    }
}

/// One page of the job's log.
async fn page(address: SocketAddr, job: &str, query: &str) -> LogPage {
    let fetched = fetch(address, &format!("/v1/backtests/{job}/logs{query}"), None).await;
    assert_eq!(fetched.status, 200, "{query}");
    assert_eq!(fetched.cache_control, "no-cache");
    serde_json::from_slice(&fetched.body).expect("a page of the log")
}

/// Every line of the job's log, read `limit` at a time with the cursor
/// each page hands on, and the last page.
async fn every_page(address: SocketAddr, job: &str, limit: u64) -> (Vec<LogLine>, LogPage) {
    let mut lines = Vec::new();
    let mut after = 0;
    loop {
        let read = page(address, job, &format!("?after={after}&limit={limit}")).await;
        assert!(read.lines.len() <= usize::try_from(limit).expect("a limit"));
        lines.extend(read.lines.iter().cloned());
        assert!(read.next_after >= after, "a cursor never goes back");
        after = read.next_after;
        if read.complete {
            return (lines, read);
        }
        assert!(
            !read.lines.is_empty(),
            "a page that is not the last holds lines"
        );
    }
}

fn progress(bars_done: u64, bars_total: u64) -> JobProgress {
    JobProgress {
        bars_done,
        bars_total,
    }
}

/// A client attached while the job runs gets the lines written before it
/// came, then each line and each step of the job's progress as it comes,
/// and the end with the job's; one that asks page by page sees the same.
async fn a_client_attached_mid_run_follows_the_log_and_the_progress<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, queue) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, queue).await;
    let hash = upload(address).await;
    let job = submit(address, &hash, 9).await;
    worker.until_begun(1).await;
    let log = worker.log(&job);

    // Nothing written yet: an empty page that is not the last.
    let empty = page(address, &job, "").await;
    assert_eq!((empty.lines.len(), empty.next_after), (0, 0));
    assert_eq!((empty.progress, empty.complete), (None, false));
    assert_eq!(empty.job_id.as_str(), job);

    log.line(LogLevel::Info, "started");
    log.progress(2, 10);
    log.line(LogLevel::Warning, "no trade in 2 bars");
    let first = page(address, &job, "").await;
    assert_eq!(
        first.lines.iter().map(shown).collect::<Vec<_>>(),
        [
            (1, LogLevel::Info, "started".to_string()),
            (2, LogLevel::Warning, "no trade in 2 bars".to_string()),
        ]
    );
    assert_eq!((first.dropped_lines, first.next_after), (0, 2));
    assert_eq!(
        (first.progress, first.complete),
        (Some(progress(2, 10)), false)
    );

    // Attached mid-run: what was written, then what is.
    let mut stream = follow(address, &job, 0).await;
    for expected in [first.lines[0].clone(), first.lines[1].clone()] {
        assert_eq!(
            next(&mut stream).await,
            Said::Frame(LogFrame::Line(expected))
        );
    }
    assert_eq!(
        next(&mut stream).await,
        Said::Frame(LogFrame::Progress(progress(2, 10)))
    );
    assert!(silent(&mut stream).await, "nothing new, nothing said");

    log.line(LogLevel::Error, "the strategy raised ValueError");
    let Said::Frame(LogFrame::Line(third)) = next(&mut stream).await else {
        panic!("the line as it was written");
    };
    assert_eq!(
        shown(&third),
        (
            3,
            LogLevel::Error,
            "the strategy raised ValueError".to_string()
        )
    );
    assert_eq!(state(address, &job).await["kind"], "running", "seen live");
    log.progress(7, 10);
    assert_eq!(
        next(&mut stream).await,
        Said::Frame(LogFrame::Progress(progress(7, 10)))
    );

    // A client that lost its connection asks from the last line it has.
    let mut resumed = follow(address, &job, 2).await;
    assert_eq!(
        next(&mut resumed).await,
        Said::Frame(LogFrame::Line(third.clone()))
    );
    assert_eq!(
        next(&mut resumed).await,
        Said::Frame(LogFrame::Progress(progress(7, 10)))
    );
    assert_eq!(page(address, &job, "?after=2").await.lines, [third]);

    // The job ends: both are told, after everything it wrote, and by then
    // the job's status says how it did.
    log.line(LogLevel::Info, "done");
    log.progress(10, 10);
    worker.tell(&job, WorkerOutcome::Succeeded(Vec::new()));
    for stream in [&mut stream, &mut resumed] {
        let rest = to_the_end(stream, 3).await;
        assert_eq!(
            rest.lines.iter().map(shown).collect::<Vec<_>>(),
            [(4, LogLevel::Info, "done".to_string())]
        );
        assert_eq!(rest.progress, Some(progress(10, 10)));
        assert!(rest.ended, "the end is said before the close");
        assert_eq!(rest.closed, Some(CloseCode::Normal));
    }
    // By the time a stream says the end, the store says how the job did
    // and holds its log: whoever asks later, here or elsewhere, gets it.
    assert_eq!(state(address, &job).await["kind"], "succeeded");
    let kept = backend
        .store()
        .job_log(&JobId::new(job.as_str()).expect("a job id"))
        .expect("a read")
        .expect("the log, stored before its end was said");
    let kept: Value = serde_json::from_str(&kept).expect("json");
    assert_eq!(kept["lines"].as_array().map(Vec::len), Some(4), "{kept}");
    assert_eq!(kept["progress"], json!({"bars_done": 10, "bars_total": 10}));
    let last = page(address, &job, "?after=4").await;
    assert_eq!((last.lines.len(), last.complete), (0, true));
    assert_eq!(last.progress, Some(progress(10, 10)));
}

/// Once a job has ended, however it ended, its whole log is replayed to a
/// client that comes later, as a stream and page by page, by the server
/// that ran it and by the next one on its store.
async fn a_client_that_comes_after_the_end_gets_the_whole_log<B: Backend>(backend: &B) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, queue) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, queue.clone()).await;
    let hash = upload(address).await;

    // Three jobs that wrote to their logs: one succeeds, one fails, one is
    // cancelled while it runs. And one cancelled before it starts.
    let written = [
        (LogLevel::Debug, "fast=2 slow=3"),
        (LogLevel::Info, "币安人生USDT: tab\tand \"quotes\""),
        (LogLevel::Warning, ""),
        (LogLevel::Error, "the last line"),
    ];
    let mut ran = Vec::new();
    for (index, end) in ["succeeded", "failed", "cancelled"].into_iter().enumerate() {
        let job = submit(address, &hash, 9).await;
        worker.until_begun(index + 1).await;
        let log = worker.log(&job);
        for (level, message) in written {
            log.line(level, message);
        }
        log.progress(6, 10);
        match end {
            "succeeded" => worker.tell(&job, WorkerOutcome::Succeeded(Vec::new())),
            "failed" => worker.tell(
                &job,
                failure(ErrorCode::InvalidRequest, "the strategy raised ValueError"),
            ),
            _ => assert_eq!(cancel(address, &job).await.status(), 200),
        }
        until_state(address, &job, end).await;
        ran.push(job);
    }
    let never = {
        let blocker = submit(address, &hash, 9).await;
        worker.until_begun(4).await;
        let never = submit(address, &hash, 9).await;
        assert_eq!(cancel(address, &never).await.status(), 200);
        worker.tell(&blocker, WorkerOutcome::Succeeded(Vec::new()));
        until_state(address, &blocker, "succeeded").await;
        never
    };

    let expected: Vec<(u64, LogLevel, String)> = written
        .iter()
        .zip(1..)
        .map(|((level, message), seq)| (seq, *level, message.to_string()))
        .collect();
    let mut first_read: Vec<Vec<LogLine>> = Vec::new();
    for (round, address) in [address, {
        // The server stops; the next one on the store answers the same.
        queue.stop().await;
        let (next_worker, _next_release) = Scripted::new();
        let (store, next) = jobs(backend, next_worker, 1).await;
        serve(store, next).await
    }]
    .into_iter()
    .enumerate()
    {
        for (index, job) in ran.iter().enumerate() {
            let mut stream = follow(address, job, 0).await;
            let replay = to_the_end(&mut stream, 0).await;
            assert_eq!(
                replay.lines.iter().map(shown).collect::<Vec<_>>(),
                expected,
                "job {index}, server {round}"
            );
            assert_eq!(replay.progress, Some(progress(6, 10)));
            assert_eq!(replay.dropped_lines, 0);
            assert!(replay.ended);
            assert_eq!(replay.closed, Some(CloseCode::Normal));

            // Page by page, with the cursor each page hands on.
            let (lines, last) = every_page(address, job, 3).await;
            assert_eq!(lines, replay.lines);
            assert_eq!((last.next_after, last.progress), (4, Some(progress(6, 10))));
            // From a cursor, a stream sends what follows it and no more.
            let mut later = follow(address, job, 3).await;
            let tail = to_the_end(&mut later, 3).await;
            assert_eq!(tail.lines, replay.lines[3..]);

            // The same lines, with the times the first server gave them.
            if round == 0 {
                first_read.push(replay.lines);
            } else {
                assert_eq!(replay.lines, first_read[index], "job {index}");
            }
        }
        // A job that never started has an empty log, which ends at once.
        let mut stream = follow(address, &never, 0).await;
        let nothing = to_the_end(&mut stream, 0).await;
        assert!(nothing.lines.is_empty() && nothing.ended);
        assert_eq!(nothing.progress, None);
        let empty = page(address, &never, "").await;
        assert_eq!((empty.lines.len(), empty.complete), (0, true));
    }
}

/// A job that writes without end fills no more of the server than its
/// log's limits: the newest lines stay, the rest are counted, a long line
/// is cut and says so, and what is stored at the job's end is the same.
async fn a_job_that_floods_its_log_leaves_the_newest_lines_and_a_count_of_the_rest<B: Backend>(
    backend: &B,
) {
    const KEPT: u64 = 40;
    const WRITTEN: u64 = 5_000;
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let limits = LogLimits {
        max_lines: usize::try_from(KEPT).expect("a count"),
        max_bytes: 64 << 10,
        max_line_bytes: 100,
    };
    let (worker, _release) = Scripted::new();
    let (store, queue) = jobs_keeping(backend, worker.clone(), 1, limits).await;
    let address = serve(store, queue.clone()).await;
    let hash = upload(address).await;
    let job = submit(address, &hash, 9).await;
    worker.until_begun(1).await;
    let log = worker.log(&job);

    // A client that follows from the start gets the first line as it
    // comes, and then reads nothing while the job floods its log.
    let mut slow = follow(address, &job, 0).await;
    log.line(LogLevel::Info, "line 1");
    let Said::Frame(LogFrame::Line(first)) = next(&mut slow).await else {
        panic!("the first line");
    };
    assert_eq!(first.seq, 1);
    for n in 2..WRITTEN {
        log.line(LogLevel::Info, &format!("line {n}"));
    }
    log.line(LogLevel::Warning, &"long ".repeat(1_000));

    // The log as it stands: the newest lines, and how many came before.
    let kept = usize::try_from(KEPT).expect("a count");
    let now = page(address, &job, "").await;
    let lines = now.lines.clone();
    assert_eq!(lines.len(), kept);
    assert_eq!(lines[0].seq, WRITTEN - KEPT + 1);
    assert_eq!(lines[0].message, format!("line {}", WRITTEN - KEPT + 1));
    assert_eq!(
        (now.dropped_lines, now.next_after),
        (WRITTEN - KEPT, WRITTEN)
    );
    assert!(!now.complete, "the job still runs");
    let long = lines.last().expect("the long line");
    assert_eq!((long.seq, long.level), (WRITTEN, LogLevel::Warning));
    assert_eq!(long.message.len(), 100);
    assert_eq!(long.dropped_bytes, Some(4_900));
    // A cursor inside what was dropped is told how much it missed; one
    // within what is kept misses nothing.
    let part = page(address, &job, "?after=4000").await;
    assert_eq!(part.dropped_lines, WRITTEN - KEPT - 4_000);
    assert_eq!(part.lines, lines);
    let within = page(address, &job, &format!("?after={}", WRITTEN - 2)).await;
    assert_eq!((within.dropped_lines, within.lines.len()), (0, 2));

    // A client that comes now is told at once what it cannot have.
    let mut late = follow(address, &job, 0).await;
    assert_eq!(
        next(&mut late).await,
        Said::Frame(LogFrame::Truncated {
            dropped_lines: WRITTEN - KEPT
        })
    );

    worker.tell(&job, WorkerOutcome::Succeeded(Vec::new()));
    until_state(address, &job, "succeeded").await;
    let rest = to_the_end(&mut late, WRITTEN - KEPT).await;
    assert_eq!(rest.lines, lines);
    assert!(rest.ended);
    // The client that stopped reading: however far it got before the lines
    // it had not read were dropped, each one it missed was counted for it,
    // and it ends on the job's last line.
    let caught_up = to_the_end(&mut slow, 1).await;
    assert!(caught_up.ended);
    assert_eq!(caught_up.lines.last().map(|line| line.seq), Some(WRITTEN));
    assert_eq!(
        u64::try_from(caught_up.lines.len()).expect("a count") + caught_up.dropped_lines,
        WRITTEN - 1,
        "every line after the first was sent or counted"
    );

    // Stored with the job: the same lines and the same count, from the
    // next server too.
    queue.stop().await;
    let (next_worker, _next_release) = Scripted::new();
    let (store, next_queue) = jobs(backend, next_worker, 1).await;
    let restarted = serve(store, next_queue).await;
    let (stored, stored_last) = every_page(restarted, &job, 7).await;
    assert_eq!(stored, lines);
    assert_eq!(stored_last.next_after, WRITTEN);
    let first_page = page(restarted, &job, "?limit=7").await;
    assert_eq!(first_page.dropped_lines, WRITTEN - KEPT);
    assert_eq!(first_page.lines, lines[..7]);
    let mut replayed = follow(restarted, &job, 0).await;
    let replay = to_the_end(&mut replayed, 0).await;
    assert_eq!(replay.dropped_lines, WRITTEN - KEPT);
    assert_eq!(replay.lines, lines);
}

/// A stream of a queued job waits for the job and then follows it, or ends
/// when the job is cancelled before it starts; a server that stops ends
/// the stream of a job it stopped with the job's end, and closes the
/// stream of one still queued without one.
async fn a_stream_of_a_queued_job_waits_for_it_and_a_stopping_server_closes_it<B: Backend>(
    backend: &B,
) {
    backend
        .store()
        .upsert_candles(&market(), &candles(10))
        .expect("the candles");
    let (worker, _release) = Scripted::new();
    let (store, queue) = jobs(backend, worker.clone(), 1).await;
    let address = serve(store, queue.clone()).await;
    let hash = upload(address).await;
    let running = submit(address, &hash, 9).await;
    worker.until_begun(1).await;
    let queued = submit(address, &hash, 9).await;
    let waiting = submit(address, &hash, 9).await;
    let dropped = submit(address, &hash, 9).await;

    let mut stream = follow(address, &queued, 0).await;
    let mut never = follow(address, &waiting, 0).await;
    let mut cancelled = follow(address, &dropped, 0).await;
    assert!(silent(&mut stream).await, "its job has not started");
    let early = page(address, &queued, "").await;
    assert_eq!((early.lines.len(), early.complete), (0, false));

    // A queued job that is cancelled never starts: its stream is told at
    // once that its empty log has ended.
    assert_eq!(cancel(address, &dropped).await.status(), 200);
    let nothing = to_the_end(&mut cancelled, 0).await;
    assert!(nothing.ended && nothing.lines.is_empty());
    assert_eq!(nothing.closed, Some(CloseCode::Normal));

    // The job before it ends, it starts, and the stream follows it.
    worker.tell(&running, WorkerOutcome::Succeeded(Vec::new()));
    worker.until_begun(2).await;
    worker.log(&queued).line(LogLevel::Info, "my turn");
    let Said::Frame(LogFrame::Line(line)) = next(&mut stream).await else {
        panic!("the line of a job that was queued when the stream opened");
    };
    assert_eq!(shown(&line), (1, LogLevel::Info, "my turn".to_string()));

    // The server stops: the job it ran is failed, and its stream ends as
    // any job's does; the stream of the job still queued is closed as the
    // server goes, with no end, since the job has not ended.
    assert_eq!(queue.stop().await, 1);
    let stopped = to_the_end(&mut stream, 1).await;
    assert!(stopped.ended);
    assert_eq!(stopped.closed, Some(CloseCode::Normal));
    let closed = to_the_end(&mut never, 0).await;
    assert!(!closed.ended && closed.lines.is_empty());
    assert_eq!(closed.closed, Some(CloseCode::Away));
}

on_each_backend!(
    a_client_attached_mid_run_follows_the_log_and_the_progress,
    a_client_that_comes_after_the_end_gets_the_whole_log,
    a_job_that_floods_its_log_leaves_the_newest_lines_and_a_count_of_the_rest,
    a_stream_of_a_queued_job_waits_for_it_and_a_stopping_server_closes_it,
);
