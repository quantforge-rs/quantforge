//! Reservations: the margin an entry has been promised from its account's
//! pool, held from the moment it is sized until its outcome is known (ADR
//! 0002 §6).

use super::{ExecutionScope, disagrees, from_stored, not_the_same, to_stored};
use crate::model::exact::Exact;
use crate::model::futures::PoolScope;
use crate::model::{InstrumentId, ModelError, Money, OperationId, TimestampMs};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of [`Reservation`]'s stored form.
pub const RESERVATION_FORMAT: u32 = 1;

const RECORD: &str = "reservation";

/// The asset an account's USD pool is stated in.
const USD: &str = "USD";

/// Whether a reservation still counts against its pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReservationState {
    Held,
    Released,
}

impl ReservationState {
    pub const ALL: [Self; 2] = [Self::Held, Self::Released];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Held => "held",
            Self::Released => "released",
        }
    }
}

impl fmt::Display for ReservationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a reservation stopped counting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReleaseReason {
    /// The entry filled: its margin is the position's now, and the venue's
    /// own figures show it.
    Opened,
    /// The entry opened nothing: refused, expired, unfilled or never
    /// placed.
    NotOpened,
}

impl ReleaseReason {
    pub const ALL: [Self; 2] = [Self::Opened, Self::NotOpened];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::NotOpened => "not_opened",
        }
    }
}

/// When and why a reservation was released.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub reason: ReleaseReason,
    pub at_ms: TimestampMs,
}

/// What one entry reserves: its margin, in its pool's unit, and the risk
/// it was sized to, in its settlement asset. Sizing is handed the total of
/// the reservations held against a pool; the store decides between
/// entries that reserve from one pool at once, so the total a sizing saw
/// is the total its reservation is added to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReservationParts")]
pub struct Reservation {
    format: u32,
    scope: ExecutionScope,
    operation: OperationId,
    instrument: InstrumentId,
    pool: PoolScope,
    margin: Money,
    risk: Money,
    reserved_at_ms: TimestampMs,
    released: Option<Release>,
}

impl Reservation {
    /// A reservation for the entry `operation` sends. Refuses a margin
    /// that is not above zero or not in the pool's unit, and a risk that
    /// is not above zero or not in the instrument's settlement asset.
    pub fn held(
        scope: ExecutionScope,
        operation: OperationId,
        instrument: InstrumentId,
        pool: PoolScope,
        margin: Money,
        risk: Money,
        reserved_at_ms: TimestampMs,
    ) -> Result<Self, ModelError> {
        Self::try_from(ReservationParts {
            format: RESERVATION_FORMAT,
            scope,
            operation,
            instrument,
            pool,
            margin,
            risk,
            reserved_at_ms,
            released: None,
        })
    }

    pub const fn format(&self) -> u32 {
        self.format
    }

    pub fn scope(&self) -> &ExecutionScope {
        &self.scope
    }

    /// The entry the margin is reserved for: one reservation each.
    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    pub fn pool(&self) -> &PoolScope {
        &self.pool
    }

    /// The margin reserved, in the pool's unit.
    pub fn margin(&self) -> &Money {
        &self.margin
    }

    /// The risk the entry was sized to, in its settlement asset.
    pub fn risk(&self) -> &Money {
        &self.risk
    }

    pub const fn reserved_at_ms(&self) -> TimestampMs {
        self.reserved_at_ms
    }

    pub const fn released(&self) -> Option<Release> {
        self.released
    }

    pub const fn state(&self) -> ReservationState {
        match self.released {
            None => ReservationState::Held,
            Some(_) => ReservationState::Released,
        }
    }

    /// The reservation once released. One released stays released.
    pub fn release(&self, reason: ReleaseReason, at_ms: TimestampMs) -> Result<Self, ModelError> {
        if self.released.is_some() {
            return Err(ModelError::InvalidTransition {
                machine: "reservation",
                from: ReservationState::Released.as_str(),
                to: ReservationState::Released.as_str(),
            });
        }
        Ok(Self {
            released: Some(Release { reason, at_ms }),
            ..self.clone()
        })
    }

    /// Whether `self` may replace `previous`: the same reservation,
    /// released once.
    pub(super) fn follows(&self, previous: &Self) -> Result<(), ModelError> {
        let same = Self {
            released: None,
            ..self.clone()
        } == Self {
            released: None,
            ..previous.clone()
        };
        if !same {
            return Err(not_the_same(RECORD, "amounts or identity"));
        }
        if previous.released.is_some() || self.released.is_none() {
            return Err(ModelError::InvalidTransition {
                machine: "reservation",
                from: previous.state().as_str(),
                to: self.state().as_str(),
            });
        }
        Ok(())
    }

    pub fn to_stored(&self) -> Result<String, ModelError> {
        to_stored(RECORD, self)
    }

    pub fn from_stored(text: &str) -> Result<Self, ModelError> {
        from_stored(RECORD, RESERVATION_FORMAT, text)
    }
}

/// The unit a pool's margin is stated in: its asset, or USD.
fn unit_of(pool: &PoolScope) -> &str {
    match pool {
        PoolScope::Asset(asset) => asset.as_str(),
        PoolScope::AccountUsd => USD,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReservationParts {
    format: u32,
    scope: ExecutionScope,
    operation: OperationId,
    instrument: InstrumentId,
    pool: PoolScope,
    margin: Money,
    risk: Money,
    reserved_at_ms: TimestampMs,
    released: Option<Release>,
}

impl TryFrom<ReservationParts> for Reservation {
    type Error = ModelError;

    fn try_from(parts: ReservationParts) -> Result<Self, Self::Error> {
        if parts.format != RESERVATION_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: RECORD,
                found: parts.format,
                expected: RESERVATION_FORMAT,
            });
        }
        parts.scope.profile_of(RECORD)?;
        parts.scope.holding(RECORD, &parts.instrument)?;
        if parts.margin.asset().as_str() != unit_of(&parts.pool) {
            return Err(disagrees(
                RECORD,
                format!(
                    "its margin is in {}, and its pool is stated in {}",
                    parts.margin.asset(),
                    unit_of(&parts.pool)
                ),
            ));
        }
        if parts.risk.asset() != &parts.instrument.settle {
            return Err(disagrees(
                RECORD,
                format!(
                    "its risk is in {}, and {} settles in {}",
                    parts.risk.asset(),
                    parts.instrument.symbol,
                    parts.instrument.settle
                ),
            ));
        }
        if parts.margin.amount() <= Decimal::ZERO || parts.risk.amount() <= Decimal::ZERO {
            return Err(disagrees(RECORD, "its margin and its risk are above zero"));
        }
        Ok(Self {
            format: parts.format,
            scope: parts.scope,
            operation: parts.operation,
            instrument: parts.instrument,
            pool: parts.pool,
            margin: parts.margin,
            risk: parts.risk,
            reserved_at_ms: parts.reserved_at_ms,
            released: parts.released,
        })
    }
}

/// What is held against one pool of one account, and the pool's revision:
/// a number that moves with every reservation made from the pool or
/// released to it. A reservation names the revision its sizing read, and
/// is made only while the pool still stands there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolReservations {
    pub revision: u64,
    /// Every reservation held against the pool, oldest first.
    pub held: Vec<Reservation>,
}

impl PoolReservations {
    /// The margin held against the pool, in `unit`: what sizing is handed
    /// as already reserved. The total is exact or there is none: it is
    /// written with as many places as the longest of its amounts where a
    /// decimal holds it so, and otherwise without its trailing zeros; a
    /// total no decimal holds exactly is refused, never rounded, since a
    /// `Decimal`'s own addition rounds what does not fit and says nothing.
    pub fn total(&self, pool: &PoolScope) -> Result<Money, ModelError> {
        let asset = crate::model::Asset::new(unit_of(pool))?;
        let mut sum = Exact::whole(0);
        let mut places = 0;
        for held in &self.held {
            let margin = held.margin();
            if margin.asset() != &asset {
                return Err(ModelError::AssetMismatch {
                    left: asset.to_string(),
                    right: margin.asset().to_string(),
                });
            }
            sum = sum.plus(&Exact::magnitude(margin.amount()));
            places = places.max(margin.amount().scale());
        }
        let total = Decimal::try_new(1, places)
            .ok()
            .and_then(|grid| sum.floor_on(grid))
            .ok_or_else(|| ModelError::InvalidAmount {
                what: "pool total",
                value: format!("of {} reservations", self.held.len()),
                rule: "an amount a decimal holds exactly",
            })?;
        Ok(Money::new(total, asset))
    }
}

#[cfg(test)]
mod tests;
