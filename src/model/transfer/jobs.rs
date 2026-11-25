//! The job store's tables, as both backends' DDL declare them: the bundles
//! a server was sent, the jobs that run them, their results, their logs and
//! their provenance.

use super::tables::{TransferTable, blob, integer, text};

/// The bundles a server was sent, by their names. A batch holds at most
/// two: a bundle's canonical tar can take 8 MiB.
pub const BUNDLES: TransferTable = TransferTable {
    name: "bundles",
    columns: &[
        text("code_hash"),
        integer("size_bytes"),
        blob("data"),
        integer("stored_at_ms"),
    ],
    key: "code_hash",
    by_market: false,
    numbered: false,
    max_batch: Some(2),
};

/// The results of the jobs that succeeded, by their names in byte order.
/// A row holds a whole result file, packed, so a batch holds few.
pub const JOB_RESULTS: TransferTable = TransferTable {
    name: "job_results",
    columns: &[
        text("hash"),
        integer("size_bytes"),
        blob("data"),
        integer("stored_at_ms"),
    ],
    key: "hash",
    by_market: false,
    numbered: false,
    max_batch: Some(2),
};

/// The jobs, by the sequence number the store gave each, which keeps their
/// order: the queue runs them in it, and lists them newest first.
pub const JOBS: TransferTable = TransferTable {
    name: "jobs",
    columns: &[
        integer("seq"),
        text("job_id"),
        text("state"),
        text("bundle_hash"),
        text("request_json"),
        integer("submitted_at_ms"),
        integer("started_at_ms"),
        integer("finished_at_ms"),
        text("results_json"),
        text("failure_json"),
    ],
    key: "seq",
    by_market: false,
    numbered: true,
    max_batch: None,
};

/// What is kept of each job's log, by the job's ID in byte order. A row
/// holds a whole log, so a batch holds few.
pub const JOB_LOGS: TransferTable = TransferTable {
    name: "job_logs",
    columns: &[text("job_id"), text("log_json"), integer("stored_at_ms")],
    key: "job_id",
    by_market: false,
    numbered: false,
    max_batch: Some(4),
};

/// Each job's provenance, by the job's ID in byte order. A row holds a
/// whole record, which can name hundreds of segments, so a batch holds few.
pub const JOB_PROVENANCE: TransferTable = TransferTable {
    name: "job_provenance",
    columns: &[
        text("job_id"),
        text("identity"),
        text("provenance_json"),
        text("backend"),
        integer("schema_version"),
        integer("recorded_at_ms"),
    ],
    key: "job_id",
    by_market: false,
    numbered: false,
    max_batch: Some(4),
};
