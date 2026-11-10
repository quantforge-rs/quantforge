//! Workloads the tests run in place of a strategy, to try a worker's
//! isolation before any user code runs in one: a server's workers run them
//! only when the server starts them in the synthetic mode, which `serve run`
//! never does. A job's bundle names its workload in its `workload`
//! parameter; each answers a report, holds its worker until it is killed,
//! runs into a limit, or stops the worker.

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::frames::{Finished, JobFrame, ResultText};
use super::isolation::{self, Posture};
use crate::model::Candle;
use crate::protocol::{ErrorBody, ErrorCode};

/// Set by `plant` in the process that ran it, which a worker that went on
/// to another job would still have.
static PLANTED: AtomicBool = AtomicBool::new(false);

/// The file `plant` leaves in its scratch directory.
const PLANTED_FILE: &str = "planted";

/// The file `hold` leaves in its scratch directory, holding its job's ID.
const HELD_FILE: &str = "held";

/// What a worker saw of itself as it started, before its sandbox closed
/// `/proc` to it.
#[derive(Debug)]
pub(super) struct Seen {
    pid: u32,
    cwd: PathBuf,
    env: Vec<String>,
    descriptors: Option<Vec<u64>>,
}

impl Seen {
    pub(super) fn now(scratch: &Path) -> Self {
        // The names only: the report says what a worker was given, never
        // what any of it holds.
        let mut env: Vec<String> = std::env::vars_os()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        env.sort();
        Self {
            pid: std::process::id(),
            cwd: scratch.to_path_buf(),
            env,
            descriptors: isolation::descriptors(),
        }
    }
}

/// Runs the workload the job's bundle names.
pub(super) fn run(job: &JobFrame, candles: &[Candle], seen: &Seen, posture: &Posture) -> Finished {
    let text = |name: &str| job.params.get(name).and_then(Value::as_str);
    let number = |name: &str| job.params.get(name).and_then(Value::as_u64);
    match text("workload") {
        Some("report") => report(&json!({
            "bars": candles.len(),
            "first_open_ms": candles.first().map(|candle| candle.open_time_ms),
            "last_open_ms": candles.last().map(|candle| candle.open_time_ms),
            "digest": digest(candles),
            "job_id": job.job_id,
            "entry": job.entry,
        })),
        Some("expect") => expect(candles, text("digest").unwrap_or_default()),
        Some("inspect") => report(&inspect(seen, posture)),
        Some("probe") => report(&probe(
            job.params
                .get("fds")
                .and_then(Value::as_array)
                .map(|fds| fds.iter().filter_map(Value::as_u64).collect())
                .unwrap_or_default(),
        )),
        Some("plant") => {
            PLANTED.store(true, Ordering::SeqCst);
            let written = std::fs::write(PLANTED_FILE, b"left by an earlier job");
            report(&json!({ "written": written.is_ok(), "seen": inspect(seen, posture) }))
        }
        Some("read") => report(&read(text("path").unwrap_or_default())),
        Some("connect") => report(&connect(text("address").unwrap_or_default())),
        Some("connect_unix") => report(&connect_unix(text("path").unwrap_or_default())),
        Some("chatter") => report(&chatter(number("kib").unwrap_or(1024))),
        Some("leave") => report(&leave()),
        Some("spawn") => report(&spawn(
            text("program").unwrap_or_default(),
            job.params
                .get("args")
                .and_then(Value::as_array)
                .map(|args| args.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default(),
        )),
        Some("spin") => spin(),
        Some("allocate") => allocate(
            number("chunk_mib").unwrap_or(16),
            number("most_mib").unwrap_or(1024),
        ),
        Some("hold") => {
            let _ = std::fs::write(HELD_FILE, job.job_id.as_bytes());
            sleep()
        }
        Some("sleep") => sleep(),
        Some("crash") => std::process::abort(),
        Some("exit") => std::process::exit(
            number("code")
                .and_then(|code| i32::try_from(code).ok())
                .unwrap_or(3),
        ),
        Some("fail") => Finished::Failed {
            error: ErrorBody {
                code: ErrorCode::InvalidRequest,
                message: text("message").unwrap_or("the job failed").to_string(),
                supported: None,
            },
        },
        other => Finished::Failed {
            error: ErrorBody {
                code: ErrorCode::InvalidRequest,
                message: format!("no synthetic workload is named {other:?}"),
                supported: None,
            },
        },
    }
}

/// A report: one JSON result named `report`.
fn report(value: &Value) -> Finished {
    Finished::Succeeded {
        results: vec![ResultText {
            name: "report".to_string(),
            media_type: "application/json".to_string(),
            text: value.to_string(),
        }],
    }
}

/// The SHA-256 of the candles as text, one line each, every decimal with
/// its scale: what reaches the worker exactly hashes the same.
fn digest(candles: &[Candle]) -> String {
    let mut hash = Sha256::new();
    for candle in candles {
        hash.update(format!(
            "{} {} {} {} {} {} {} {:?}\n",
            candle.open_time_ms,
            candle.close_time_ms,
            candle.open,
            candle.high,
            candle.low,
            candle.close,
            candle.volume,
            candle.trades
        ));
    }
    hex::encode(hash.finalize())
}

/// Succeeds when the candles hash to `expected`, and fails saying what they
/// hash to otherwise.
fn expect(candles: &[Candle], expected: &str) -> Finished {
    let actual = digest(candles);
    if actual == expected {
        return report(&json!({ "bars": candles.len(), "digest": actual }));
    }
    Finished::Failed {
        error: ErrorBody {
            code: ErrorCode::InvalidRequest,
            message: format!(
                "the {} candles hash to {actual}, not {expected}",
                candles.len()
            ),
            supported: None,
        },
    }
}

/// What the worker was given and what its scratch directory holds now.
fn inspect(seen: &Seen, posture: &Posture) -> Value {
    json!({
        "pid": seen.pid,
        "cwd": seen.cwd,
        "env": seen.env,
        "descriptors": seen.descriptors,
        "planted": PLANTED.load(Ordering::SeqCst),
        "planted_file": Path::new(PLANTED_FILE).exists(),
        "scratch": listing(Path::new(".")),
        "posture": posture,
    })
}

/// Whether each of the descriptors `fds` is open now, looked up by its
/// number, whatever the listing of them says.
#[cfg(unix)]
fn probe(fds: Vec<u64>) -> Value {
    let open: Vec<Value> = fds
        .into_iter()
        .map(|fd| {
            let path = Path::new(isolation::LISTED).join(fd.to_string());
            json!({ "fd": fd, "open": std::fs::metadata(path).is_ok() })
        })
        .collect();
    json!({ "open": open })
}

#[cfg(not(unix))]
fn probe(fds: Vec<u64>) -> Value {
    json!({ "fds": fds, "open": "no descriptor numbers here" })
}

/// Every file under `dir`, by its path from it, in order.
fn listing(dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(dir) {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found.sort();
    found
}

/// Reads the file, or lists the directory, at `path`.
fn read(path: &str) -> Value {
    let done = match std::fs::metadata(path) {
        Ok(found) if found.is_dir() => {
            std::fs::read_dir(path).map(|entries| format!("listed {} entries", entries.count()))
        }
        _ => std::fs::read(path).map(|bytes| format!("read {} bytes", bytes.len())),
    };
    json!({
        "path": path,
        "allowed": done.is_ok(),
        "outcome": done.unwrap_or_else(|error| format!("refused: {error}")),
    })
}

/// Opens a TCP connection to `address`.
fn connect(address: &str) -> Value {
    let done = address
        .parse::<SocketAddr>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        .and_then(|address| TcpStream::connect_timeout(&address, Duration::from_secs(2)));
    json!({
        "address": address,
        "allowed": done.is_ok(),
        "outcome": done.map_or_else(|error| format!("refused: {error}"), |_| "connected".to_string()),
    })
}

/// Opens a connection to the Unix socket at `path`, as a client of a
/// database listening on one would.
#[cfg(unix)]
fn connect_unix(path: &str) -> Value {
    let done = std::os::unix::net::UnixStream::connect(path);
    json!({
        "path": path,
        "allowed": done.is_ok(),
        "outcome": done.map_or_else(|error| format!("refused: {error}"), |_| "connected".to_string()),
    })
}

#[cfg(not(unix))]
fn connect_unix(path: &str) -> Value {
    json!({ "path": path, "allowed": false, "outcome": "no Unix sockets here" })
}

/// Starts `program` with `args`, its standard streams closed, and leaves it
/// running.
fn spawn(program: &str, args: Vec<&str>) -> Value {
    let started = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    json!({
        "program": program,
        "allowed": started.is_ok(),
        "pid": started.as_ref().ok().map(std::process::Child::id),
        "outcome": started.map_or_else(|error| format!("refused: {error}"), |_| "started".to_string()),
    })
}

/// What the worker is answered when it tries to leave its process group,
/// the way a process a job started could outlive it: with `setsid`, for a
/// session of its own, or with `setpgid`, for a group of its own.
#[cfg(unix)]
fn leave() -> Value {
    let answer = |called: rustix::io::Result<()>| match called {
        Ok(()) => "allowed".to_string(),
        Err(error) => format!("refused: {}", std::io::Error::from(error)),
    };
    json!({
        "setsid": answer(rustix::process::setsid().map(drop)),
        "setpgid": answer(rustix::process::setpgid(None, None)),
    })
}

#[cfg(not(unix))]
fn leave() -> Value {
    json!({ "setsid": "no process groups here", "setpgid": "no process groups here" })
}

/// Writes `kib` KiB to standard error.
fn chatter(kib: u64) -> Value {
    let mut stderr = std::io::stderr().lock();
    let line = [b'x'; 1024];
    let written = (0..kib).all(|_| stderr.write_all(&line).is_ok());
    json!({ "kib": kib, "written": written })
}

/// Waits until the worker is stopped.
fn sleep() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// Uses CPU time until the worker is stopped.
fn spin() -> ! {
    let mut turns: u64 = 0;
    loop {
        turns = std::hint::black_box(turns.wrapping_add(1));
    }
}

/// Takes memory `chunk_mib` MiB at a time, writing every byte, up to
/// `most_mib`: an allocation past the worker's limit aborts it, and one
/// that never meets a limit answers how much it took.
fn allocate(chunk_mib: u64, most_mib: u64) -> Finished {
    let chunk = usize::try_from(chunk_mib.saturating_mul(1 << 20)).unwrap_or(usize::MAX);
    let mut held = Vec::new();
    let mut taken = 0;
    while taken < most_mib {
        held.push(vec![1_u8; chunk]);
        taken += chunk_mib;
    }
    report(&json!({ "allocated_mib": taken, "chunks": held.len() }))
}
