//! How a worker's stop is read, and how far ahead of its server a worker's
//! frames are read: no further than a few, however much the worker writes.

use std::os::unix::process::ExitStatusExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::protocol::jobs::LogLevel;

fn signalled(signal: rustix::process::Signal) -> ExitStatus {
    ExitStatus::from_raw(signal.as_raw())
}

#[test]
fn how_a_worker_stopped_names_the_limit_it_ran_into() {
    use rustix::process::Signal;
    assert_eq!(why(signalled(Signal::XCPU), ""), Stop::Cpu);
    assert_eq!(
        why(
            signalled(Signal::ABORT),
            "memory allocation of 16777216 bytes failed\n"
        ),
        Stop::Memory
    );
    assert_eq!(
        why(signalled(Signal::ABORT), "thread 'main' panicked"),
        Stop::Crash("it was stopped by signal 6 (SIGABRT)".to_string())
    );
    assert_eq!(
        why(signalled(Signal::KILL), ""),
        Stop::Crash("it was stopped by signal 9 (SIGKILL)".to_string())
    );
    assert_eq!(
        why(ExitStatus::from_raw(3 << 8), ""),
        Stop::Crash("it exited with status 3".to_string())
    );
}

/// A worker's output that never ends: one frame over and over, counting
/// the bytes read of it.
struct Endless {
    frame: Vec<u8>,
    at: usize,
    read: Arc<AtomicUsize>,
}

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let rest = &self.frame[self.at..];
        let taken = rest.len().min(buffer.len());
        buffer[..taken].copy_from_slice(&rest[..taken]);
        self.at = (self.at + taken) % self.frame.len();
        self.read.fetch_add(taken, Ordering::SeqCst);
        Ok(taken)
    }
}

/// Waits until `count` is at least `least` and has then stayed the same
/// for a while, and answers it. A reader that has not begun has not
/// settled: only one that got as far as `least` and stopped has.
fn settled(count: &AtomicUsize, least: usize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = count.load(Ordering::SeqCst);
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let now = count.load(Ordering::SeqCst);
        if now == last && now >= least {
            return now;
        }
        assert!(
            Instant::now() < deadline,
            "{now} bytes read after thirty seconds, of at least {least}, and not settled"
        );
        last = now;
    }
}

#[test]
fn a_workers_frames_are_read_only_a_few_ahead_of_what_its_server_takes() {
    let mut frame = Vec::new();
    let line = FromWorker::Log {
        level: LogLevel::Info,
        message: "x".repeat(4_000),
        dropped_bytes: 0,
    };
    frames::write(&mut frame, &line).expect("a frame");
    let read = Arc::new(AtomicUsize::new(0));
    let (sender, taken) = frames_ahead();
    let output = Endless {
        frame: frame.clone(),
        at: 0,
        read: Arc::clone(&read),
    };
    read_frames(output, sender).expect("a reader");

    // Nothing is taken: the reader fills what may wait, and then waits
    // itself, however much more the worker has to say. The frames that
    // wait, the one in the reader's hand, and what its buffer took ahead.
    let most = (FRAMES_AHEAD + 2) * frame.len() + (16 << 10);
    let ahead = settled(&read, FRAMES_AHEAD * frame.len());
    assert!(
        ahead <= most,
        "{ahead} bytes read ahead of a server that takes nothing"
    );

    // Taken, they make room for as many again, and no more.
    for _ in 0..3 * FRAMES_AHEAD {
        let frame = taken
            .recv_timeout(Duration::from_secs(10))
            .expect("a frame")
            .expect("a frame that reads");
        assert!(matches!(frame, FromWorker::Log { .. }), "{frame:?}");
    }
    let taken_bytes = 3 * FRAMES_AHEAD * frame.len();
    let after = settled(&read, taken_bytes + FRAMES_AHEAD * frame.len());
    assert!(after <= most + taken_bytes, "{after} bytes read");

    // A server that takes no more, for good, ends the reader.
    drop(taken);
    let ended = settled(&read, after);
    assert!(ended <= most + taken_bytes, "{ended} bytes read");
}
