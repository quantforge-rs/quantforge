//! What a server and its workers say to each other over a worker's standard
//! input and output: frames of a four-byte big-endian length, then that many
//! bytes of JSON, tagged `type`. The frames are no part of the protocol: a
//! server speaks only to workers of its own build, each side's first frame
//! names its build, and a worker refuses a server of another.
//!
//! A server sends `setup` as it starts a worker. The worker puts its limits
//! and sandbox on itself and answers `ready`, with what it enforced and the
//! runtime it offers. The job follows: `job`, then its candles in `candles`
//! frames, oldest first, until all the bars `job` announced have come; the
//! bundle's files are in `bundle/` in the worker's scratch directory by
//! then. The worker answers `finished` once, and the server kills it.

use std::io::{self, Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::config::WorkerMode;
use super::isolation::Posture;
use crate::protocol::ErrorBody;
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::JobSubmission;
use crate::protocol::stream::Bar;

/// The frames' own version, which any change to a frame raises.
const FRAMES: u32 = 1;

/// The build a server and its workers name in their first frames.
pub(super) fn build() -> String {
    format!("quantforge {} frames {FRAMES}", env!("CARGO_PKG_VERSION"))
}

/// The most bytes one frame from a worker may hold: a worker runs a job's
/// code, so nothing it sends is trusted.
pub(super) const MOST_FROM_WORKER: usize = 16 << 20;

/// The most bytes one frame from the server may hold.
pub(super) const MOST_TO_WORKER: usize = 64 << 20;

/// How many bars one `candles` frame carries.
pub(super) const BARS_PER_FRAME: usize = 10_000;

/// A frame from the server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToWorker {
    /// The first frame: the server's build, what the worker runs, and the
    /// limits it puts on itself.
    Setup {
        build: String,
        mode: WorkerMode,
        cpu_secs: u64,
        memory_bytes: u64,
    },
    /// The job, once the worker is ready.
    Job(Box<JobFrame>),
    /// The next run of the job's candles.
    Candles { bars: Vec<Bar> },
}

/// What a job runs, but its candles.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct JobFrame {
    pub(super) job_id: String,
    pub(super) submission: JobSubmission,
    /// The bundle's entry, `path.py:Class`, as its manifest names it.
    pub(super) entry: String,
    /// The bundle's parameters, as its manifest gives them.
    pub(super) params: Map<String, Value>,
    /// How many bars the `candles` frames after it carry in all.
    pub(super) bars: u64,
}

/// A frame from a worker.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum FromWorker {
    /// The answer to `setup`: the worker's build and process, what it
    /// enforced, and the runtime it offers, if any.
    Ready {
        build: String,
        pid: u32,
        posture: Posture,
        runtime: Option<Runtime>,
    },
    /// How the job ended.
    Finished { outcome: Finished },
}

/// How a job ended, as its worker saw it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Finished {
    /// With what it produced.
    Succeeded { results: Vec<ResultText> },
    /// Without results, with why.
    Failed { error: ErrorBody },
}

/// One result, as text.
#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ResultText {
    pub(super) name: String,
    pub(super) media_type: String,
    pub(super) text: String,
}

/// Writes `frame` and flushes it.
pub(super) fn write<W: Write, T: Serialize>(output: &mut W, frame: &T) -> io::Result<()> {
    let body = serde_json::to_vec(frame).map_err(io::Error::other)?;
    let length = u32::try_from(body.len()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a frame cannot hold 4 GiB or more",
        )
    })?;
    output.write_all(&length.to_be_bytes())?;
    output.write_all(&body)?;
    output.flush()
}

/// Reads the next frame: `None` when the input ends before one begins. A
/// frame of more than `most` bytes, or one cut short, is an error.
pub(super) fn read<R: Read, T: DeserializeOwned>(
    input: &mut R,
    most: usize,
) -> io::Result<Option<T>> {
    let mut length = [0_u8; 4];
    let mut filled = 0;
    while filled < length.len() {
        match input.read(&mut length[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    let length = usize::try_from(u32::from_be_bytes(length)).unwrap_or(usize::MAX);
    if length > most {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("a frame of {length} bytes, more than the {most} one may hold"),
        ));
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ErrorCode;

    fn framed<T: Serialize>(frame: &T) -> Vec<u8> {
        let mut bytes = Vec::new();
        write(&mut bytes, frame).expect("a frame");
        bytes
    }

    #[test]
    fn a_frame_is_its_length_then_its_json() {
        let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
        let json = br#"{"type":"candles","bars":[]}"#;
        assert_eq!(
            &bytes[..4],
            &u32::try_from(json.len()).expect("a length").to_be_bytes()
        );
        assert_eq!(&bytes[4..], json);
    }

    #[test]
    fn frames_read_back_as_written_and_the_input_ends_cleanly_between_them() {
        let mut bytes = framed(&FromWorker::Finished {
            outcome: Finished::Failed {
                error: ErrorBody {
                    code: ErrorCode::InvalidRequest,
                    message: "the job failed".to_string(),
                    supported: None,
                },
            },
        });
        bytes.extend(framed(&FromWorker::Finished {
            outcome: Finished::Succeeded {
                results: vec![ResultText {
                    name: "report".to_string(),
                    media_type: "application/json".to_string(),
                    text: "{}".to_string(),
                }],
            },
        }));
        let mut input = bytes.as_slice();
        let first: FromWorker = read(&mut input, 1 << 10).expect("a read").expect("a frame");
        assert!(matches!(
            first,
            FromWorker::Finished { outcome: Finished::Failed { error } }
                if error.message == "the job failed"
        ));
        let second: FromWorker = read(&mut input, 1 << 10).expect("a read").expect("a frame");
        assert!(matches!(
            second,
            FromWorker::Finished { outcome: Finished::Succeeded { results } }
                if results.len() == 1 && results[0].text == "{}"
        ));
        let end: Option<FromWorker> = read(&mut input, 1 << 10).expect("a clean end");
        assert!(end.is_none());
    }

    #[test]
    fn a_frame_longer_than_allowed_is_refused_before_it_is_read() {
        let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
        let error = read::<_, ToWorker>(&mut bytes.as_slice(), 8).expect_err("too long");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error.to_string(),
            "a frame of 28 bytes, more than the 8 one may hold"
        );
    }

    #[test]
    fn a_frame_cut_short_or_not_json_is_an_error() {
        let bytes = framed(&ToWorker::Candles { bars: Vec::new() });
        for cut in [2, 4, bytes.len() - 1] {
            let error =
                read::<_, ToWorker>(&mut &bytes[..cut], 1 << 10).expect_err("a frame cut short");
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof, "cut at {cut}");
        }
        let mut garbage = 3_u32.to_be_bytes().to_vec();
        garbage.extend(b"abc");
        let error = read::<_, ToWorker>(&mut garbage.as_slice(), 1 << 10).expect_err("not json");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_build_names_this_release_and_the_frames_version() {
        assert_eq!(
            build(),
            format!("quantforge {} frames 1", env!("CARGO_PKG_VERSION"))
        );
    }
}
