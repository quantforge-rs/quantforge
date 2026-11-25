//! What a job that means harm tries, as workloads: to write outside its
//! scratch directory, and to reach the processes around it. Each answers
//! what it was allowed, so that a test holds a worker to what it says it
//! enforces. Nothing here harms where it is allowed: what `escape` writes
//! goes into a directory its test made, and `others` sends no signal that
//! does anything.

use std::path::Path;

use serde_json::{Value, json};

/// One attempt, as a report names it: what was tried, whether the worker
/// was allowed it, and how it ended.
fn attempt(what: &str, tried: std::io::Result<()>) -> Value {
    json!({
        "attempt": what,
        "allowed": tried.is_ok(),
        "outcome": tried.map_or_else(|error| format!("refused: {error}"), |()| "done".to_string()),
    })
}

/// Tries each way out of the scratch directory a job has: a path that
/// climbs out of it, a path from the root, a link it makes to a directory
/// outside, and a second name it gives a file outside. `outside` is a
/// directory the test made, holding `canary.txt`, which every attempt that
/// is allowed leaves a file in, or changes.
pub(super) fn escape(outside: &str) -> Value {
    let outside = Path::new(outside);
    let write = |path: &Path| std::fs::write(path, b"escaped");
    let mut attempts = vec![
        attempt(
            "a path that climbs out",
            write(Path::new("../escaped-climbing.txt")),
        ),
        attempt(
            "a path from the root",
            write(&outside.join("escaped-rooted.txt")),
        ),
        attempt(
            "the canary, by its path",
            write(&outside.join("canary.txt")),
        ),
    ];
    attempts.extend(links(outside));
    json!({ "attempts": attempts })
}

/// A link to the directory outside, written through; and a second name for
/// the canary, written through.
#[cfg(unix)]
fn links(outside: &Path) -> Vec<Value> {
    let through_a_link = std::os::unix::fs::symlink(outside, "way-out")
        .and_then(|()| std::fs::write("way-out/escaped-linked.txt", b"escaped"));
    let by_another_name = std::fs::hard_link(outside.join("canary.txt"), "canary-too")
        .and_then(|()| std::fs::write("canary-too", b"escaped"));
    vec![
        attempt("a link to a directory outside", through_a_link),
        attempt("another name for the canary", by_another_name),
    ]
}

#[cfg(not(unix))]
fn links(_outside: &Path) -> Vec<Value> {
    Vec::new()
}

/// What the worker is answered when it reaches for the processes around
/// it: a signal for itself and one for its server, each the signal that
/// only asks whether it may be sent; its server's resource limits; a
/// thread and a process of its own; and a pair of sockets, which a job
/// that had one could send to any Unix socket's address.
#[cfg(unix)]
pub(super) fn others() -> Value {
    use rustix::process::{getpid, getppid, test_kill_process};

    let answer = |called: std::io::Result<()>| match called {
        Ok(()) => "allowed".to_string(),
        Err(error) => format!("refused: {error}"),
    };
    let server = getppid();
    let thread = std::thread::Builder::new().spawn(|| ()).and_then(|thread| {
        thread
            .join()
            .map_err(|_| std::io::Error::other("the thread panicked"))
    });
    json!({
        "signal_itself": answer(test_kill_process(getpid()).map_err(Into::into)),
        "signal_server": answer(match server {
            Some(server) => test_kill_process(server).map_err(Into::into),
            None => Err(std::io::Error::other("the worker has no parent")),
        }),
        "limit_server": answer(limit(server)),
        "thread": answer(thread),
        "process": answer(process()),
        "socket_pair": answer(std::os::unix::net::UnixDatagram::pair().map(drop)),
    })
}

#[cfg(not(unix))]
pub(super) fn others() -> Value {
    json!({ "others": "no signals or resource limits here" })
}

/// Takes the server's core dumps away, which changes nothing of what it
/// does: the limit a job would take is its CPU time, which ends it.
#[cfg(target_os = "linux")]
fn limit(server: Option<rustix::process::Pid>) -> std::io::Result<()> {
    use rustix::process::{Resource, Rlimit, prlimit};

    // Without a process to name, the call would mean the worker itself.
    let Some(server) = server else {
        return Err(std::io::Error::other("the worker has no parent"));
    };
    let none = Rlimit {
        current: Some(0),
        maximum: Some(0),
    };
    prlimit(Some(server), Resource::Core, none)
        .map(drop)
        .map_err(Into::into)
}

#[cfg(all(unix, not(target_os = "linux")))]
fn limit(_server: Option<rustix::process::Pid>) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "no process sets another's limits here",
    ))
}

/// Starts a process of the worker's own, this program again, and waits for
/// it: `--version` runs nothing of a worker's.
#[cfg(unix)]
fn process() -> std::io::Result<()> {
    std::process::Command::new(std::env::current_exe()?)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(drop)
}
