//! `serve run` as a process: it says where it listens, answers its routes,
//! and stops cleanly on a signal, finishing a request in progress first
//! unless a second signal says not to wait. Only unix has the signals, so
//! those tests sit in [`signals`]; every platform runs the rest.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use predicates::prelude::*;
use quantforge::protocol::{MIN_SUPPORTED, PROTO_VERSION};

use crate::harness::{TestBed, isolated_std_cmd, unpinned_cmd};

/// How long one step may take before the test fails instead of hanging.
const PATIENCE: Duration = Duration::from_secs(30);

/// A running `serve run` on a free loopback port, logging errors only, so
/// that its stdout holds just its `key: value` lines. Dropping it kills the
/// process: declare it after its bed, whose directory outlives it.
struct Served {
    child: Child,
    lines: Receiver<String>,
    stderr: Option<JoinHandle<String>>,
    address: SocketAddr,
}

impl Served {
    /// Starts `serve run` in the bed and waits until it says where it
    /// listens.
    fn start(bed: &TestBed) -> Self {
        let mut cmd = isolated_std_cmd();
        cmd.current_dir(bed.dir())
            .arg("--db")
            .arg(bed.db_path())
            .args(["--log-level", "error", "serve", "run"])
            .args(["--listen", "127.0.0.1:0", "--segment-cache-dir"])
            .arg(bed.dir().join("segments"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("serve run starts");
        let stdout = child.stdout.take().expect("its stdout");
        let stderr = child.stderr.take().expect("its stderr");
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            text
        });
        let mut served = Self {
            child,
            lines,
            stderr: Some(stderr),
            address: SocketAddr::from(([127, 0, 0, 1], 0)),
        };
        let url = served.next("listening");
        served.address = url
            .strip_prefix("http://")
            .and_then(|address| address.parse().ok())
            .unwrap_or_else(|| panic!("listening on {url:?}, not http:// and an address"));
        served
    }

    /// The value of the next stdout line, which must be `key: value`.
    fn next(&self, key: &str) -> String {
        let line = self
            .lines
            .recv_timeout(PATIENCE)
            .unwrap_or_else(|error| panic!("no `{key}:` line: {error}"));
        line.strip_prefix(&format!("{key}: "))
            .unwrap_or_else(|| panic!("expected `{key}: ...`, got {line:?}"))
            .to_string()
    }

    /// Waits for the process to exit.
    fn wait(&mut self) -> ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("the process status") {
                return status;
            }
            assert!(Instant::now() < deadline, "serve run did not exit");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Everything the process wrote to stderr, once it has exited.
    fn stderr(&mut self) -> String {
        self.stderr
            .take()
            .expect("stderr is read once")
            .join()
            .expect("the stderr reader")
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Reads a response to the end of its connection: its status and body.
fn response(mut stream: TcpStream) -> (u16, String) {
    let mut text = String::new();
    stream.read_to_string(&mut text).expect("a response");
    let (head, body) = text
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("no end of headers in {text:?}"));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status in {head:?}"));
    (status, body.to_string())
}

/// A connection to `address` that gives up on a silent server.
fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(address).expect("a connection");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("a read timeout");
    stream
}

/// `GET path` on a connection the server closes once it has answered.
fn get(address: SocketAddr, path: &str) -> (u16, String) {
    let mut stream = connect(address);
    let request = format!("GET {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("send the request");
    response(stream)
}

#[test]
fn serve_run_says_where_it_listens_and_answers_health_and_version() {
    let bed = TestBed::new();
    let mut served = Served::start(&bed);
    assert_eq!(get(served.address, "/healthz"), (200, "ok".to_string()));
    let version = format!(
        r#"{{"version":"{}","proto_version":{},"min_supported":{}}}"#,
        env!("CARGO_PKG_VERSION"),
        PROTO_VERSION.0,
        MIN_SUPPORTED.0
    );
    assert_eq!(get(served.address, "/version"), (200, version));
    assert!(bed.dir().join("segments").is_dir(), "no segment cache");
    assert!(bed.db_path().is_file(), "no database");

    served.child.kill().expect("stop the server");
    served.wait();
    assert_eq!(served.stderr(), "");
}

#[test]
fn serve_run_fails_naming_an_address_it_cannot_listen_on() {
    let bed = TestBed::new();
    let taken = TcpListener::bind("127.0.0.1:0").expect("a listener");
    let address = taken.local_addr().expect("its address").to_string();
    let mut cmd = unpinned_cmd(&bed);
    cmd.args(["--log-level", "error", "serve", "run", "--listen", &address]);
    cmd.assert()
        .code(1)
        .stdout(predicate::str::contains("listening").not())
        .stderr(predicate::str::contains(format!(
            "failed to listen on {address}"
        )));
}

#[cfg(unix)]
mod signals {
    use std::process::Command;

    use super::*;

    /// How long a fresh connection's first bytes are given to reach the
    /// server before the test goes on as if the server had read them.
    const SETTLE: Duration = Duration::from_millis(500);

    impl Served {
        /// The stdout lines not yet read, once the process has exited.
        fn rest_of_stdout(&self) -> Vec<String> {
            self.lines.iter().collect()
        }

        /// Sends the signal `name`, such as `TERM`, as `kill -s` does.
        fn signal(&self, name: &str) {
            let pid = self.child.id().to_string();
            let status = Command::new("kill")
                .args(["-s", name, &pid])
                .status()
                .expect("kill runs");
            assert!(status.success(), "kill -s {name} {pid}: {status}");
        }

        /// A request still in progress: a fresh connection whose headers
        /// lack the blank line that ends them. The server keeps such a
        /// connection open through its shutdown, until it can answer.
        fn start_request(&self) -> TcpStream {
            let mut stream = connect(self.address);
            stream
                .write_all(b"GET /healthz HTTP/1.1\r\nHost: test\r\nConnection: close\r\n")
                .expect("send the headers");
            thread::sleep(SETTLE);
            stream
        }

        /// Waits until the server refuses new connections: it has stopped
        /// accepting them.
        fn wait_until_refused(&self) {
            let deadline = Instant::now() + PATIENCE;
            while TcpStream::connect(self.address).is_ok() {
                assert!(Instant::now() < deadline, "serve run kept accepting");
                thread::sleep(Duration::from_millis(20));
            }
        }
    }

    fn stops_cleanly_on(signal: &str, reported: &str) {
        let bed = TestBed::new();
        let mut served = Served::start(&bed);
        served.signal(signal);
        let status = served.wait();
        assert!(status.success(), "{status}");
        assert_eq!(
            served.rest_of_stdout(),
            [
                format!("shutdown: {reported}"),
                "stopped: requests in progress finished".to_string()
            ]
        );
        assert_eq!(served.stderr(), "");
    }

    #[test]
    fn sigterm_stops_serve_run_cleanly() {
        stops_cleanly_on("TERM", "SIGTERM");
    }

    #[test]
    fn sigint_stops_serve_run_cleanly() {
        stops_cleanly_on("INT", "SIGINT");
    }

    #[test]
    fn serve_run_answers_a_request_in_progress_before_it_exits() {
        let bed = TestBed::new();
        let mut served = Served::start(&bed);
        let mut request = served.start_request();
        served.signal("TERM");
        assert_eq!(served.next("shutdown"), "SIGTERM");
        served.wait_until_refused();

        request.write_all(b"\r\n").expect("finish the request");
        assert_eq!(response(request), (200, "ok".to_string()));
        assert!(served.wait().success());
        assert_eq!(served.next("stopped"), "requests in progress finished");
    }

    #[test]
    fn a_second_signal_stops_serve_run_without_waiting() {
        let bed = TestBed::new();
        let mut served = Served::start(&bed);
        let _request = served.start_request();
        served.signal("TERM");
        assert_eq!(served.next("shutdown"), "SIGTERM");
        served.signal("INT");

        assert_eq!(served.wait().code(), Some(1));
        assert!(
            served.rest_of_stdout().is_empty(),
            "it reported stopping cleanly"
        );
        let stderr = served.stderr();
        assert!(
            stderr.contains(
                "SIGINT received while finishing requests in progress; stopped without waiting \
                 for them"
            ),
            "{stderr}"
        );
    }
}
