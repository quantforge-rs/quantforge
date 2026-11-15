//! Fills as the ledger keeps them.

use super::{ExecutionScope, from_stored, to_stored};
use crate::model::futures::{FuturesFill, PositionCycleId};
use crate::model::{ModelError, OperationId, VenueTradeId};
use serde::{Deserialize, Serialize};

/// The version of [`FillRecord`]'s stored form.
pub const FILL_RECORD_FORMAT: u32 = 1;

const FILL: &str = "fill record";

/// One fill as the ledger keeps it, with the operation and the cycle it
/// was found to belong to. A fill is recorded once, under its venue's
/// trade ID, and never changed: the same fill reported by a query and by
/// the stream is one row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "FillRecordParts")]
pub struct FillRecord {
    format: u32,
    scope: ExecutionScope,
    fill: FuturesFill,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
}

impl FillRecord {
    /// A fill on `scope`'s account. One that QuantForge's own order made
    /// names its operation; a fill of anyone else's order names none.
    pub fn new(
        scope: ExecutionScope,
        fill: FuturesFill,
        operation: Option<OperationId>,
        cycle: Option<PositionCycleId>,
    ) -> Result<Self, ModelError> {
        Self::try_from(FillRecordParts {
            format: FILL_RECORD_FORMAT,
            scope,
            fill,
            operation,
            cycle,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    pub fn fill(&self) -> &FuturesFill {
        &self.fill
    }

    pub fn trade_id(&self) -> &VenueTradeId {
        &self.fill.trade_id
    }

    pub const fn operation(&self) -> Option<OperationId> {
        self.operation
    }

    pub const fn cycle(&self) -> Option<PositionCycleId> {
        self.cycle
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(FILL, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(FILL, FILL_RECORD_FORMAT, text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FillRecordParts {
    format: u32,
    scope: ExecutionScope,
    fill: FuturesFill,
    operation: Option<OperationId>,
    cycle: Option<PositionCycleId>,
}

impl TryFrom<FillRecordParts> for FillRecord {
    type Error = ModelError;

    fn try_from(parts: FillRecordParts) -> Result<Self, Self::Error> {
        if parts.format != FILL_RECORD_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: FILL,
                found: parts.format,
                expected: FILL_RECORD_FORMAT,
            });
        }
        parts.scope.holding(FILL, &parts.fill.instrument)?;
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            fill: parts.fill,
            operation: parts.operation,
            cycle: parts.cycle,
        })
    }
}
