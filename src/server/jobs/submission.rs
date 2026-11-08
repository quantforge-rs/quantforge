//! What an upload and a submission must pass, in the order the protocol
//! states it: a bundle's name, its strict reading, the spec and admission;
//! a submission's market and range in their canonical forms, its settings,
//! the bundle, and the candles that must cover the whole range. Nothing is
//! stored until everything passed.

use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use super::Jobs;
use crate::model::{BundleHash, JobId, JobRecord, MarketId, NewJob, ms_to_rfc3339, now_utc_ms};
use crate::protocol::ContentHash;
use crate::protocol::bundle::Bundle;
use crate::protocol::jobs::{BacktestSettings, JobSubmission};
use crate::server::history::stored_span;
use crate::server::refusal::Refusal;
use crate::server::selection::{Selection, named};
use crate::server::store::ServedStore;

/// What a stored upload answers: whether the bundle is new.
pub(super) struct Stored {
    pub(super) new: bool,
    pub(super) size_bytes: u64,
}

/// Checks an upload sent under `name` and stores it once.
pub(super) async fn upload(
    jobs: &Jobs,
    name: ContentHash,
    tar: Vec<u8>,
) -> Result<Stored, Refusal> {
    let (limits, runtime) = (jobs.inner.limits, jobs.inner.worker.runtime());
    // Hashing and reading megabytes is work for a blocking thread.
    let (digest, tar) = tokio::task::spawn_blocking(move || {
        let digest: [u8; 32] = Sha256::digest(&tar).into();
        if digest != *name.digest() {
            return Err(Refusal::invalid(format!(
                "the upload hashes to {}, not to {name}, the name it was sent under; send a \
                 bundle's canonical tar under its code hash",
                ContentHash::sha256(digest)
            )));
        }
        let bundle = Bundle::from_canonical_tar(&tar, &limits)
            .map_err(|refused| Refusal::invalid(refused.to_string()))?;
        if let Some(runtime) = &runtime {
            bundle
                .admit(runtime)
                .map_err(|refused| Refusal::invalid(refused.to_string()))?;
        }
        Ok((digest, tar))
    })
    .await
    .map_err(|_| Refusal::internal("the server failed while reading the upload"))??;
    let hash = BundleHash::from_digest(digest);
    let size_bytes = u64::try_from(tar.len()).unwrap_or(u64::MAX);
    let new = jobs
        .inner
        .store
        .call(move |store| {
            store
                .store_bundle(&hash, &tar, now_utc_ms())
                .map_err(|failure| Refusal::storage("storing a bundle", failure))
        })
        .await?;
    Ok(Stored { new, size_bytes })
}

/// Checks a submission, then queues its job.
pub(super) async fn submit(jobs: &Jobs, submission: JobSubmission) -> Result<JobRecord, Refusal> {
    let selection = Selection::read_data(&submission.data)?;
    let (bars, max_span_bars) = (selection.bars(), jobs.inner.job_limits.max_span_bars);
    if bars > max_span_bars {
        return Err(Refusal::invalid(format!(
            "the range spans {bars} bars of {}, more than the {max_span_bars} this server runs \
             one job on; ask for a shorter range",
            selection.interval.as_str()
        )));
    }
    settings(&submission.backtest)?;
    let (limits, runtime) = (jobs.inner.limits, jobs.inner.worker.runtime());
    let record = jobs
        .inner
        .store
        .call(move |store| {
            let hash = BundleHash::from_digest(*submission.bundle.digest());
            let tar = store
                .load_bundle(&hash)
                .map_err(|failure| Refusal::storage("loading a bundle", failure))?
                .ok_or_else(|| {
                    Refusal::invalid(format!(
                        "the server holds no bundle {hash}; upload it first with PUT \
                         /v1/bundles/{hash}"
                    ))
                })?;
            let bundle = Bundle::from_canonical_tar(&tar, &limits).map_err(|refused| {
                tracing::error!(bundle = %hash, %refused, "a stored bundle cannot be read back");
                Refusal::internal(format!("the server's copy of bundle {hash} is damaged"))
            })?;
            if let Some(runtime) = &runtime {
                bundle
                    .admit(runtime)
                    .map_err(|refused| Refusal::invalid(refused.to_string()))?;
            }
            let category = selection.category(store)?;
            let market = selection.market(category);
            covered(store, &market, &selection)?;
            let mut accepted = submission;
            accepted.data.category = Some(category.as_str().to_string());
            let job = NewJob {
                job_id: JobId::generate(),
                bundle: hash,
                request_json: serde_json::to_string(&accepted)
                    .map_err(|failure| Refusal::internal(failure.to_string()))?,
                submitted_at_ms: now_utc_ms(),
            };
            store
                .submit_job(&job)
                .map_err(|failure| Refusal::storage("queuing a job", failure))?;
            store
                .job(&job.job_id)
                .map_err(|failure| Refusal::storage("reading a job", failure))?
                .ok_or_else(|| Refusal::internal("the server lost the job it queued"))
        })
        .await?;
    jobs.inner.wake.notify_one();
    Ok(record)
}

/// The settings a backtest runs with: cash above zero, fees zero or more.
fn settings(backtest: &BacktestSettings) -> Result<(), Refusal> {
    let cash = backtest.initial_cash.get();
    if cash <= Decimal::ZERO {
        return Err(Refusal::invalid(format!(
            "initial_cash is {cash}; a backtest starts with cash above zero"
        )));
    }
    let fee = backtest.fee_bps.get();
    if fee < Decimal::ZERO {
        return Err(Refusal::invalid(format!(
            "fee_bps is {fee}; a fee is zero or more basis points"
        )));
    }
    Ok(())
}

/// Refuses a range outside the bars the store holds of `market`, from its
/// first to its last.
fn covered(
    store: &dyn ServedStore,
    market: &MarketId,
    selection: &Selection,
) -> Result<(), Refusal> {
    let (start, end) = (selection.start_open_ms, selection.end_open_ms);
    let Some((first, last)) = stored_span(store, market)? else {
        return Err(Refusal::invalid(format!(
            "the server stores no bars of {}; a job runs only on candles the server holds",
            named(market)
        )));
    };
    if start < first || end > last {
        return Err(Refusal::invalid(format!(
            "the server stores {} from {} to {}, which does not cover the job's range from {} \
             to {}; ask for a range inside it",
            named(market),
            ms_to_rfc3339(first),
            ms_to_rfc3339(last),
            ms_to_rfc3339(start),
            ms_to_rfc3339(end)
        )));
    }
    Ok(())
}
