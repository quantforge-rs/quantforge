//! What a transfer refuses before it writes anything: a store at another
//! schema, a target holding another store's copy or rows of its own, an
//! active target, a target bound to another environment, a source whose
//! runs may still be writing to it, and a resumed copy holding rows the
//! source has since changed or deleted.

use super::verify::{self, Coverage};
use super::{RUNS_NAMED, TransferEngine, VerifyReport, key_of};
use crate::engine::EngineError;
use crate::model::transfer::BOT_RUNS;
use crate::model::{
    Environment, RunStatus, StoreIdentity, TRANSFER_SCHEMA, TRANSFER_TABLES, TransferRecord,
    TransferValue, describe_environment,
};

impl TransferEngine<'_> {
    /// Refuses a source with a run that may still be writing to it.
    pub(super) fn refuse_open_runs(&self) -> Result<(), EngineError> {
        if self.config.assume_quiescent {
            return Ok(());
        }
        let open = self.open_runs()?;
        if open.is_empty() {
            return Ok(());
        }
        let mut named = open
            .iter()
            .take(RUNS_NAMED)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if open.len() > RUNS_NAMED {
            named.push_str(&format!(" and {} more", open.len() - RUNS_NAMED));
        }
        Err(refused(format!(
            "the source has runs that may still be writing to it: {named}; stop them first, or, \
             once you have checked that nothing writes to the source, run again with \
             --assume-quiescent (a process that died leaves its run running)"
        )))
    }

    /// The source's runs that recorded no end, as `run (status)`.
    fn open_runs(&self) -> Result<Vec<String>, EngineError> {
        let (Some(id), Some(status)) = (
            BOT_RUNS.column_index("run_id"),
            BOT_RUNS.column_index("status"),
        ) else {
            return Err(EngineError::InvalidState(
                "the runs table has no run_id or status column".to_string(),
            ));
        };
        let open = [RunStatus::Starting.as_str(), RunStatus::Running.as_str()];
        let limit = BOT_RUNS.batch_rows(self.config.batch_rows);
        let mut found = Vec::new();
        let mut after = None;
        loop {
            let rows = self
                .source
                .read_rows(&BOT_RUNS, None, after.as_ref(), limit)?;
            for row in &rows {
                if let (Some(TransferValue::Text(run)), Some(TransferValue::Text(state))) =
                    (row.get(id), row.get(status))
                {
                    if open.contains(&state.as_str()) {
                        found.push(format!("{run} ({state})"));
                    }
                }
            }
            match rows.last() {
                Some(last) if rows.len() == limit => after = Some(key_of(&BOT_RUNS, last)?.clone()),
                _ => return Ok(found),
            }
        }
    }

    /// Refuses to resume a copy over rows the source has changed or deleted
    /// since the copy began: an earlier attempt wrote them from an earlier
    /// snapshot, and a copy never rewrites a row the target holds.
    pub(super) fn refuse_stale_rows(&self) -> Result<(), EngineError> {
        let mut report = VerifyReport::default();
        for table in &TRANSFER_TABLES {
            verify::table(
                self.source,
                self.target,
                table,
                self.config.batch_rows,
                Coverage::Held,
                &mut report,
            )?;
        }
        let Some(first) = report.differences.first() else {
            return Ok(());
        };
        Err(refused(format!(
            "the source changed since this copy began: the target holds {} row(s) the source \
             has since changed or deleted (first: {first}); a copy never rewrites a row it \
             holds, so this one cannot be finished, but the source is untouched: give the copy \
             a new, empty database and run the command again",
            report.difference_count
        )))
    }

    /// Refuses a target that holds rows of any transferred table.
    pub(super) fn refuse_rows_of_its_own(&self) -> Result<(), EngineError> {
        let mut holding = Vec::new();
        for table in &TRANSFER_TABLES {
            let holds = if table.by_market {
                !self.target.markets(table)?.is_empty()
            } else {
                !self.target.read_rows(table, None, None, 1)?.is_empty()
            };
            if holds {
                holding.push(table.name);
            }
        }
        if holding.is_empty() {
            return Ok(());
        }
        Err(refused(format!(
            "the target already holds rows of its own ({}), so it cannot become a copy of the \
             source; give the copy an empty database",
            holding.join(", ")
        )))
    }
}

pub(super) fn refused(reason: String) -> EngineError {
    EngineError::TransferRefused(reason)
}

pub(super) fn refuse_other_schemas(
    source: &StoreIdentity,
    target: &StoreIdentity,
) -> Result<(), EngineError> {
    for (side, identity) in [("source", source), ("target", target)] {
        if identity.schema_version != TRANSFER_SCHEMA {
            return Err(refused(format!(
                "the {side} is at schema {}, and this build copies schema {TRANSFER_SCHEMA}",
                identity.schema_version
            )));
        }
    }
    Ok(())
}

pub(super) fn refuse_another_source(
    record: &TransferRecord,
    source: &StoreIdentity,
) -> Result<(), EngineError> {
    if record.source_store_id == source.store_id {
        return Ok(());
    }
    Err(refused(format!(
        "the target holds a copy of store {}, not of this source, store {}; give this copy a \
         database of its own",
        record.source_store_id, source.store_id
    )))
}

/// The environment to bind the target to: the source's, carried to a
/// target bound to none. A target bound to another is refused.
pub(super) fn binding(
    source: &StoreIdentity,
    target: &StoreIdentity,
) -> Result<Option<Environment>, EngineError> {
    match (source.environment, target.environment) {
        (None, None) => Ok(None),
        (Some(environment), None) => Ok(Some(environment)),
        (Some(source), Some(target)) if source == target => Ok(None),
        (source, Some(target)) => Err(refused(format!(
            "the target is bound to {target}, and the source to {}; give the copy a database of \
             its own",
            describe_environment(source)
        ))),
    }
}
