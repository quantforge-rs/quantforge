//! What a worker runs its job in: a [`WorkerRuntime`], which readies
//! itself before the worker puts its limits and sandbox on itself, says
//! what it offers, and runs the one job the worker is handed. This binary's
//! own runs the tests' synthetic workloads, or nothing (`child`); the one
//! `quantforge_py` compiles runs Python bundles, in a worker that is a
//! Python interpreter, through [`run_backtest`].

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::backtest::{self, Ended};
use super::config::WorkerMode;
use super::frames::{Finished, JobFrame};
use super::isolation::Posture;
use super::report::Reporter;
use super::scratch::BUNDLE_DIR;
use crate::model::Candle;
use crate::protocol::bundle::Runtime;
use crate::protocol::jobs::JobSubmission;
use crate::protocol::{ErrorBody, ErrorCode};
use crate::sdk::Strategy;

/// What runs a worker's job. A worker asks it twice: [`prepare`] once its
/// server has said what it runs, and [`run`] with the job.
///
/// [`prepare`]: WorkerRuntime::prepare
/// [`run`]: WorkerRuntime::run
pub trait WorkerRuntime {
    /// Readies what runs a job for a worker of `mode` whose working
    /// directory is `scratch`, and says what it offers and which
    /// directories outside `scratch` the job still reads. It runs before
    /// the worker puts its limits and sandbox on itself, while it has no
    /// thread but this one: it starts none, and runs no code of a job's. A
    /// runtime that cannot be readied says why, and the worker stops before
    /// it says it is ready, so that its server does not start.
    fn prepare(&mut self, mode: &WorkerMode, scratch: &Path) -> Result<Prepared, String>;

    /// Runs the job to its end, and says what it says while it runs
    /// through `reporter`.
    fn run(&mut self, job: &WorkerJob<'_>, reporter: &mut Reporter<'_>) -> JobEnd;
}

/// What a runtime offers once it is ready.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prepared {
    /// The runtime every bundle is admitted against, or `None` when it
    /// runs none, and the server fails each job as it starts.
    pub runtime: Option<Runtime>,
    /// The directories, beside the scratch directory, the job reads files
    /// from: an interpreter's own. Where the worker's sandbox confines
    /// files, it leaves these readable and nothing in them writable.
    pub readable: Vec<PathBuf>,
}

/// A job as its worker holds it: what its server sent, the candles among
/// it, and the bundle's files in the scratch directory.
pub struct WorkerJob<'a> {
    pub(super) frame: &'a JobFrame,
    pub(super) candles: &'a [Candle],
    pub(super) posture: &'a Posture,
    pub(super) scratch: &'a Path,
}

impl WorkerJob<'_> {
    /// The job.
    pub fn job_id(&self) -> &str {
        &self.frame.job_id
    }

    /// The bundle's entry, as its manifest writes it:
    /// `path/to/module.py:Class`.
    pub fn entry(&self) -> &str {
        &self.frame.entry
    }

    /// The module the entry names, as Python imports it: `path.to.module`.
    pub fn module(&self) -> &str {
        &self.frame.module
    }

    /// The class the entry names.
    pub fn class(&self) -> &str {
        &self.frame.class
    }

    /// The bundle's parameters, as its manifest gives them.
    pub fn params(&self) -> &Map<String, Value> {
        &self.frame.params
    }

    /// The bundle's parameters as JSON text, for a runtime that reads them
    /// in its own language.
    pub fn params_json(&self) -> String {
        Value::Object(self.frame.params.clone()).to_string()
    }

    /// The packages the bundle imports, each with the version its manifest
    /// pins, which the server admitted against the worker's runtime.
    pub fn deps(&self) -> &BTreeMap<String, String> {
        &self.frame.deps
    }

    /// What was asked: the market and range, and the backtest's settings.
    pub fn submission(&self) -> &JobSubmission {
        &self.frame.submission
    }

    /// The range's candles, oldest first, every decimal with its scale.
    pub fn candles(&self) -> &[Candle] {
        self.candles
    }

    /// Where the bundle's files are: `bundle/` in the scratch directory.
    pub fn bundle_dir(&self) -> PathBuf {
        self.scratch.join(BUNDLE_DIR)
    }
}

// The candles could be hundreds of thousands; their count says enough.
impl fmt::Debug for WorkerJob<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerJob")
            .field("job_id", &self.frame.job_id)
            .field("entry", &self.frame.entry)
            .field("bars", &self.candles.len())
            .finish_non_exhaustive()
    }
}

/// How a job ended, before its worker says so.
pub struct JobEnd(pub(super) Ended);

impl JobEnd {
    /// A job that failed, with why: `invalid_request` when its bundle or
    /// its data is at fault, `internal` when the worker is.
    pub fn failed(code: ErrorCode, message: impl Into<String>) -> Self {
        Self(Ended::Said(Finished::Failed {
            error: ErrorBody {
                code,
                message: message.into(),
                supported: None,
            },
        }))
    }
}

impl fmt::Debug for JobEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Ended::Said(Finished::Failed { error }) => f
                .debug_struct("Failed")
                .field("code", &error.code)
                .field("message", &error.message)
                .finish(),
            Ended::Said(Finished::Succeeded { results }) => f
                .debug_struct("Succeeded")
                .field("results", &results.len())
                .finish(),
            Ended::Said(Finished::Backtest { metrics }) => f
                .debug_struct("Backtest")
                .field("metrics", metrics)
                .finish_non_exhaustive(),
            Ended::Backtest(output) => f
                .debug_struct("Backtest")
                .field("metrics", &output.metrics)
                .finish_non_exhaustive(),
        }
    }
}

/// Runs `strategy` over the job's candles in the engine, under the job's
/// settings, as every backtest in a worker runs: the job's progress said
/// through `reporter` as its bars go by, and its result as the rows the
/// worker sends its server. What the engine refuses, a strategy's failure
/// among it, fails the job in the engine's words.
pub fn run_backtest(
    job: &WorkerJob<'_>,
    strategy: &mut dyn Strategy,
    reporter: &mut Reporter<'_>,
) -> JobEnd {
    JobEnd(backtest::run(job.frame, job.candles, strategy, reporter))
}
