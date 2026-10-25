//! One owned position cycle as the store records it (ADR 0002 §2, §7, §10,
//! §12): from flat, through an entry, back to flat. It opens at the entry's
//! first fill and carries its protection, the stop QuantForge recorded, why
//! it closed, and its realized PnL.

use super::{
    CancelAttribution, CoveredStop, PositionCycleId, PositionKey, PositionSnapshot, ProfileId,
    ProtectionState, RealizedPnl, RecordedStop, StopOrder, StopStatus,
};
use crate::model::{ModelError, OperationId, PositionSide, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Why a cycle closed, as the evidence shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CloseReason {
    ExitSignal,
    StopLoss,
    Manual,
    Liquidation,
    Adl,
    /// QuantForge's protective close after its stop could not be placed.
    Protective,
    Unknown,
}

impl CloseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExitSignal => "exit_signal",
            Self::StopLoss => "stop_loss",
            Self::Manual => "manual",
            Self::Liquidation => "liquidation",
            Self::Adl => "adl",
            Self::Protective => "protective",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for CloseReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Evidence that the operator removed a position's stop on purpose, which
/// QuantForge does not restore: the venue reporting the stop cancelled by
/// the account's user, or the operator's command to leave the position
/// unprotected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorRemoval {
    by_command: bool,
}

impl OperatorRemoval {
    /// Refuses a stop the venue does not report as cancelled by the user.
    pub fn user_cancel(stop: &StopOrder) -> Result<Self, ModelError> {
        if stop.status != StopStatus::Canceled(CancelAttribution::ByUser) {
            return Err(ModelError::MissingEvidence {
                machine: "position protection",
                to: "unprotected_by_operator",
                evidence: "a stop the venue reports as cancelled by the user".to_string(),
            });
        }
        Ok(Self { by_command: false })
    }

    /// The operator's explicit command to leave the position unprotected.
    pub fn command() -> Self {
        Self { by_command: true }
    }

    pub const fn is_command(&self) -> bool {
        self.by_command
    }
}

/// One cycle of an owned position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "PositionCycleParts")]
pub struct PositionCycle {
    id: PositionCycleId,
    key: PositionKey,
    profile: ProfileId,
    side: PositionSide,
    opened_by: OperationId,
    protection: ProtectionState,
    recorded_stop: Option<RecordedStop>,
    close_reason: Option<CloseReason>,
    pnl: Option<RealizedPnl>,
    opened_at_ms: TimestampMs,
    closed_at_ms: Option<TimestampMs>,
}

impl PositionCycle {
    /// A cycle opened by the first fill of `opened_by`'s entry. It waits
    /// for its stop.
    pub fn open(
        id: PositionCycleId,
        key: PositionKey,
        profile: ProfileId,
        side: PositionSide,
        opened_by: OperationId,
        opened_at_ms: TimestampMs,
    ) -> Self {
        Self {
            id,
            key,
            profile,
            side,
            opened_by,
            protection: ProtectionState::INITIAL,
            recorded_stop: None,
            close_reason: None,
            pnl: None,
            opened_at_ms,
            closed_at_ms: None,
        }
    }

    pub const fn id(&self) -> PositionCycleId {
        self.id
    }

    pub fn key(&self) -> &PositionKey {
        &self.key
    }

    pub fn profile(&self) -> &ProfileId {
        &self.profile
    }

    pub const fn side(&self) -> PositionSide {
        self.side
    }

    pub const fn opened_by(&self) -> OperationId {
        self.opened_by
    }

    pub const fn protection(&self) -> ProtectionState {
        self.protection
    }

    pub const fn recorded_stop(&self) -> Option<RecordedStop> {
        self.recorded_stop
    }

    pub const fn close_reason(&self) -> Option<CloseReason> {
        self.close_reason
    }

    pub fn pnl(&self) -> Option<&RealizedPnl> {
        self.pnl.as_ref()
    }

    pub const fn opened_at_ms(&self) -> TimestampMs {
        self.opened_at_ms
    }

    pub const fn closed_at_ms(&self) -> Option<TimestampMs> {
        self.closed_at_ms
    }

    /// The stop QuantForge placed, which later read-backs must show.
    pub fn record_stop(&mut self, stop: RecordedStop) {
        self.recorded_stop = Some(stop);
    }

    /// A move that needs no evidence.
    pub fn move_protection(&mut self, to: ProtectionState) -> Result<(), ModelError> {
        self.protection = self.protection.move_to(to)?;
        Ok(())
    }

    /// A read-back showed a stop covering this cycle's position.
    pub fn verified(&mut self, covered: &CoveredStop) -> Result<(), ModelError> {
        let stop = covered.stop();
        if &stop.instrument != self.key.instrument() || stop.position_side != self.side {
            return Err(self.foreign("a stop on this position"));
        }
        self.protection = self.protection.verified(covered)?;
        Ok(())
    }

    /// The operator removed the stop on purpose.
    pub fn removed_by_operator(&mut self, removal: &OperatorRemoval) -> Result<(), ModelError> {
        self.protection = self.protection.removed_by_operator(removal)?;
        Ok(())
    }

    /// A REST snapshot showed the position gone; the cycle ends for
    /// `reason`.
    pub fn close(
        &mut self,
        snapshot: &PositionSnapshot,
        reason: CloseReason,
    ) -> Result<(), ModelError> {
        if &snapshot.instrument != self.key.instrument() {
            return Err(self.foreign("a snapshot of this position"));
        }
        self.protection = self.protection.closed(snapshot, self.side)?;
        self.close_reason = Some(reason);
        self.closed_at_ms = Some(snapshot.observed_at_ms);
        Ok(())
    }

    /// The cycle's PnL, in its instrument's settlement asset.
    pub fn set_pnl(&mut self, pnl: RealizedPnl) -> Result<(), ModelError> {
        let settle = &self.key.instrument().settle;
        if pnl.settle() != settle {
            return Err(ModelError::AssetMismatch {
                left: settle.to_string(),
                right: pnl.settle().to_string(),
            });
        }
        self.pnl = Some(pnl);
        Ok(())
    }

    fn foreign(&self, evidence: &str) -> ModelError {
        ModelError::MissingEvidence {
            machine: "position protection",
            to: "its next state",
            evidence: evidence.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct PositionCycleParts {
    id: PositionCycleId,
    key: PositionKey,
    profile: ProfileId,
    side: PositionSide,
    opened_by: OperationId,
    protection: ProtectionState,
    recorded_stop: Option<RecordedStop>,
    close_reason: Option<CloseReason>,
    pnl: Option<RealizedPnl>,
    opened_at_ms: TimestampMs,
    closed_at_ms: Option<TimestampMs>,
}

impl TryFrom<PositionCycleParts> for PositionCycle {
    type Error = ModelError;

    fn try_from(parts: PositionCycleParts) -> Result<Self, Self::Error> {
        let closed = parts.protection == ProtectionState::Closed;
        if closed != parts.close_reason.is_some() || closed != parts.closed_at_ms.is_some() {
            return Err(ModelError::InvalidIdentifier {
                what: "position cycle",
                value: parts.id.get().to_string(),
                rule: "a closed cycle has a close reason and time, and an open one has neither",
            });
        }
        let mut cycle = Self::open(
            parts.id,
            parts.key,
            parts.profile,
            parts.side,
            parts.opened_by,
            parts.opened_at_ms,
        );
        cycle.protection = parts.protection;
        cycle.recorded_stop = parts.recorded_stop;
        cycle.close_reason = parts.close_reason;
        cycle.closed_at_ms = parts.closed_at_ms;
        if let Some(pnl) = parts.pnl {
            cycle.set_pnl(pnl)?;
        }
        Ok(cycle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AccountRef, Asset, Environment, ExchangeId, InstrumentId, Symbol, Venue, VenueAccountId,
    };

    fn cycle() -> PositionCycle {
        let id = VenueAccountId::issued("1").expect("account id");
        let account = AccountRef::new(Venue::Binance, Environment::Demo, id).expect("account");
        let symbol = Symbol::new("ETHUSD_PERP").expect("symbol");
        let eth = Asset::new("ETH").expect("asset");
        let instrument = InstrumentId::new(ExchangeId::BinanceInverse, symbol, eth);
        PositionCycle::open(
            PositionCycleId::new(1).expect("cycle"),
            PositionKey::new(account, instrument).expect("key"),
            ProfileId::new("main").expect("profile"),
            PositionSide::Short,
            OperationId::new(1).expect("operation"),
            0,
        )
    }

    #[test]
    fn a_stored_cycle_is_closed_with_a_reason_and_a_time_or_with_neither() {
        let json = serde_json::to_value(cycle()).expect("json");
        let back: PositionCycle = serde_json::from_value(json.clone()).expect("round trip");
        assert_eq!(back, cycle());
        let mut closed_without_reason = json.clone();
        closed_without_reason["protection"] = serde_json::json!("Closed");
        assert!(serde_json::from_value::<PositionCycle>(closed_without_reason).is_err());
        let mut open_with_reason = json;
        open_with_reason["close_reason"] = serde_json::json!("Manual");
        assert!(serde_json::from_value::<PositionCycle>(open_with_reason).is_err());
    }

    #[test]
    fn close_reasons_have_their_report_names() {
        let reasons = [
            CloseReason::ExitSignal,
            CloseReason::StopLoss,
            CloseReason::Manual,
            CloseReason::Liquidation,
            CloseReason::Adl,
            CloseReason::Protective,
            CloseReason::Unknown,
        ];
        assert_eq!(
            reasons.map(CloseReason::as_str),
            [
                "exit_signal",
                "stop_loss",
                "manual",
                "liquidation",
                "adl",
                "protective",
                "unknown"
            ]
        );
    }
}
