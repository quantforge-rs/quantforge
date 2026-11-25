//! A rerun's result against its job's: file by file by hash, the same hash
//! being the same file, and where a file differs, value by value in the
//! file's own order to the first that differs: `metrics` field by field,
//! the tables row by row, each row column by column, every value as its
//! exact text, so that a decimal's scale counts. A file only the rerun
//! produced differs too. The values are read on a thread of their own,
//! which holds the rerun's slot: a table can be long.

use std::sync::Arc;

use tokio::sync::OwnedSemaphorePermit;
use tracing::error;

use super::Jobs;
use super::queue::patiently;
use super::results::damaged;
use crate::model::{JobId, ResultHash};
use crate::protocol::jobs::{Divergence, ResultComparison, ResultRef};
use crate::protocol::results::{Metrics, ResultKind, decode_equity, decode_trades};
use crate::server::refusal::Refusal;
use crate::server::results::{ResultFile, unpack};

/// One value of a file, under its column's name, as its exact text.
type Cell = (&'static str, String);

/// The job's result files, in the order it lists them, against the
/// rerun's, then each file only the rerun produced. A file of the job's
/// that differs is read from the store and checked against its name, as
/// the result routes read it; the two are then read value by value on a
/// thread that holds `slot`, the rerun's, until it is done.
pub(super) async fn compare(
    jobs: &Jobs,
    job_id: &JobId,
    recorded: &[ResultRef],
    rerun: Vec<ResultFile>,
    slot: Arc<OwnedSemaphorePermit>,
) -> Result<Vec<ResultComparison>, Refusal> {
    let differing: Vec<ResultRef> = recorded
        .iter()
        .filter(|reference| {
            produced(&rerun, reference).is_some_and(|file| file.reference.hash != reference.hash)
        })
        .cloned()
        .collect();
    let (id, differing) = (job_id.clone(), Arc::new(differing));
    let stored: Vec<(String, Vec<u8>)> = patiently(jobs, move |store| {
        differing
            .iter()
            .map(|reference| {
                let bytes = store
                    .load_result(&ResultHash::from_digest(*reference.hash.digest()))
                    .map_err(|failure| Refusal::storage("loading a result", failure))?
                    .ok_or_else(|| {
                        damaged(
                            &id,
                            &reference.name,
                            "the store holds nothing under its name",
                        )
                    })?;
                let file =
                    unpack(&bytes, reference).map_err(|why| damaged(&id, &reference.name, &why))?;
                Ok((reference.name.clone(), file))
            })
            .collect()
    })
    .await?;
    let (id, listed) = (job_id.clone(), recorded.to_vec());
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        compared(&id, &listed, &rerun, &stored)
    })
    .await
    .map_err(|_| Refusal::internal("the server failed while it compared a rerun's result"))?
}

/// Each of the job's files against the rerun's of its name, one that
/// differs against `stored`, the job's copy of it; then each file only the
/// rerun produced, which has no recorded hash to match.
fn compared(
    job_id: &JobId,
    recorded: &[ResultRef],
    rerun: &[ResultFile],
    stored: &[(String, Vec<u8>)],
) -> Result<Vec<ResultComparison>, Refusal> {
    let mut compared = recorded
        .iter()
        .map(|reference| {
            let file = produced(rerun, reference);
            let divergence = match file {
                Some(file) if file.reference.hash != reference.hash => {
                    let ours = stored
                        .iter()
                        .find(|(name, _)| name == &reference.name)
                        .map(|(_, bytes)| bytes.as_slice())
                        .unwrap_or_default();
                    Some(
                        first_divergence(&reference.name, ours, &file.bytes)
                            .map_err(|why| unreadable(job_id, &reference.name, &why))?,
                    )
                }
                _ => None,
            };
            Ok(ResultComparison {
                name: reference.name.clone(),
                recorded: Some(reference.hash),
                rerun: file.map(|file| file.reference.hash),
                divergence,
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    compared.extend(
        rerun
            .iter()
            .filter(|file| {
                recorded
                    .iter()
                    .all(|reference| reference.name != file.reference.name)
            })
            .map(|file| ResultComparison {
                name: file.reference.name.clone(),
                recorded: None,
                rerun: Some(file.reference.hash),
                divergence: None,
            }),
    );
    Ok(compared)
}

/// The rerun's file of the name the job lists `reference` under.
fn produced<'a>(rerun: &'a [ResultFile], reference: &ResultRef) -> Option<&'a ResultFile> {
    rerun
        .iter()
        .find(|file| file.reference.name == reference.name)
}

/// The refusal of two files that cannot be compared: neither is damaged,
/// since both were checked against their names, so this build fails.
fn unreadable(job_id: &JobId, result: &str, why: &str) -> Refusal {
    error!(job = %job_id, result, why, "a rerun's result cannot be compared with its job's");
    Refusal::internal(format!(
        "the server cannot compare job {job_id}'s {result} result with the rerun's"
    ))
}

/// Where two files of the result named `name` first differ, read value by
/// value; where one has no such row, the first column of the other's.
pub(super) fn first_divergence(
    name: &str,
    recorded: &[u8],
    rerun: &[u8],
) -> Result<Divergence, String> {
    let kind = ResultKind::ALL
        .into_iter()
        .find(|kind| kind.name() == name)
        .ok_or_else(|| format!("a result has no file named {name}"))?;
    let (ours, theirs) = (cells(kind, recorded)?, cells(kind, rerun)?);
    let numbered = kind != ResultKind::Metrics;
    for row in 0..ours.len().max(theirs.len()) {
        let at = numbered.then(|| u64::try_from(row + 1).unwrap_or(u64::MAX));
        let (one, other) = (ours.get(row), theirs.get(row));
        if let (Some(one), Some(other)) = (one, other) {
            let differs = one
                .iter()
                .zip(other)
                .find(|((_, recorded), (_, rerun))| recorded != rerun);
            if let Some(((field, recorded), (_, rerun))) = differs {
                return Ok(Divergence {
                    row: at,
                    field: (*field).to_string(),
                    recorded: Some(recorded.clone()),
                    rerun: Some(rerun.clone()),
                });
            }
            continue;
        }
        let first = |cells: Option<&Vec<Cell>>| cells.and_then(|cells| cells.first()).cloned();
        let (recorded, rerun) = (first(one), first(other));
        let field = recorded
            .as_ref()
            .or(rerun.as_ref())
            .map_or("", |cell| cell.0);
        return Ok(Divergence {
            row: at,
            field: field.to_string(),
            recorded: recorded.map(|cell| cell.1),
            rerun: rerun.map(|cell| cell.1),
        });
    }
    Err("the two files hold the same values under different hashes".to_string())
}

/// A file's values, row by row, each under its column's name.
fn cells(kind: ResultKind, bytes: &[u8]) -> Result<Vec<Vec<Cell>>, String> {
    let unreadable = |failure: &dyn std::fmt::Display| format!("a {} file: {failure}", kind.name());
    match kind {
        ResultKind::Metrics => {
            let metrics = Metrics::decode(bytes).map_err(|failure| unreadable(&failure))?;
            Ok(vec![vec![
                ("initial_cash", metrics.initial_cash.to_string()),
                ("final_equity", metrics.final_equity.to_string()),
                ("total_return_pct", metrics.total_return_pct.to_string()),
                ("max_drawdown_pct", metrics.max_drawdown_pct.to_string()),
                ("trade_count", metrics.trade_count.to_string()),
            ]])
        }
        ResultKind::Trades => Ok(decode_trades(bytes)
            .map_err(|failure| unreadable(&failure))?
            .into_iter()
            .map(|trade| {
                vec![
                    ("symbol", trade.symbol),
                    ("entry_time_ms", trade.entry_time_ms.to_string()),
                    ("exit_time_ms", trade.exit_time_ms.to_string()),
                    ("entry_price", trade.entry_price.to_string()),
                    ("exit_price", trade.exit_price.to_string()),
                    ("qty", trade.qty.to_string()),
                    ("gross_quote_pnl", trade.gross_quote_pnl.to_string()),
                ]
            })
            .collect()),
        ResultKind::Equity => Ok(decode_equity(bytes)
            .map_err(|failure| unreadable(&failure))?
            .into_iter()
            .map(|point| {
                vec![
                    ("open_time_ms", point.open_time_ms.to_string()),
                    ("equity", point.equity.to_string()),
                ]
            })
            .collect()),
    }
}
