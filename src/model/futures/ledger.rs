//! The account's money movements as the venue records them (ADR 0002 §12):
//! funding, fees, realized PnL and transfers, and one venue's per-close
//! summary, read a time window at a time. Accounting builds each cycle's
//! PnL from these and never adds a summary to its own components.

use crate::model::{
    ExchangeId, InstrumentId, ModelError, Money, PositionSide, Price, Quantity, TimestampMs,
    VenueOrderId, VenueTradeId,
};
use serde::{Deserialize, Serialize};

/// What a ledger entry records.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LedgerKind {
    Funding,
    Fee,
    RealizedPnl,
    Transfer,
    /// Any other kind, with the venue's own name for it.
    Other(String),
}

impl LedgerKind {
    /// The kind's name in a deduplication key.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Funding => "funding",
            Self::Fee => "fee",
            Self::RealizedPnl => "realized_pnl",
            Self::Transfer => "transfer",
            Self::Other(name) => name,
        }
    }
}

/// One entry of the venue's income records or transaction log. The amount
/// is a signed cash flow in its own asset: money received is positive, and
/// money paid, such as a fee, negative.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    pub exchange: ExchangeId,
    /// The venue's ID for the entry.
    pub id: String,
    pub kind: LedgerKind,
    pub instrument: Option<InstrumentId>,
    pub amount: Money,
    pub trade_id: Option<VenueTradeId>,
    pub order_id: Option<VenueOrderId>,
    pub time_ms: TimestampMs,
}

impl LedgerEntry {
    /// What tells one entry from another within an account: the product
    /// line, the kind and the venue's ID, since a venue's IDs are unique
    /// only within one kind of record.
    pub fn key(&self) -> (ExchangeId, &str, &str) {
        (self.exchange, self.kind.as_str(), &self.id)
    }
}

/// A venue's summary of one close: what was closed, at which average
/// prices, its fees, and the PnL net of fees and funding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedPnlRecord {
    pub instrument: InstrumentId,
    pub order_id: VenueOrderId,
    pub side: PositionSide,
    pub closed_size: Quantity,
    pub average_entry: Price,
    pub average_exit: Price,
    pub open_fee: Money,
    pub close_fee: Money,
    pub closed_pnl: Money,
    pub time_ms: TimestampMs,
}

/// One record from a venue's ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LedgerRecord {
    Entry(LedgerEntry),
    ClosedPnl(ClosedPnlRecord),
}

/// A span of time, both ends included.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "TimeWindowParts")]
pub struct TimeWindow {
    start_ms: TimestampMs,
    end_ms: TimestampMs,
}

impl TimeWindow {
    pub fn new(start_ms: TimestampMs, end_ms: TimestampMs) -> Result<Self, ModelError> {
        if start_ms > end_ms {
            return Err(ModelError::InvalidAmount {
                what: "time window",
                value: format!("{start_ms}..={end_ms}"),
                rule: "a start no later than its end",
            });
        }
        Ok(Self { start_ms, end_ms })
    }

    pub const fn start_ms(self) -> TimestampMs {
        self.start_ms
    }

    pub const fn end_ms(self) -> TimestampMs {
        self.end_ms
    }
}

#[derive(Deserialize)]
struct TimeWindowParts {
    start_ms: TimestampMs,
    end_ms: TimestampMs,
}

impl TryFrom<TimeWindowParts> for TimeWindow {
    type Error = ModelError;

    fn try_from(parts: TimeWindowParts) -> Result<Self, Self::Error> {
        Self::new(parts.start_ms, parts.end_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Asset;
    use rust_decimal::Decimal;

    #[test]
    fn ledger_entries_are_told_apart_by_line_kind_and_id() {
        let entry = |kind: LedgerKind| LedgerEntry {
            exchange: ExchangeId::BinanceLinear,
            id: "9001".to_string(),
            kind,
            instrument: None,
            amount: Money::new(Decimal::from(-1), Asset::new("USDT").expect("asset")),
            trade_id: None,
            order_id: None,
            time_ms: 1,
        };
        let funding = entry(LedgerKind::Funding);
        let fee = entry(LedgerKind::Fee);
        assert_ne!(
            funding.key(),
            fee.key(),
            "one ID can name two kinds of record"
        );
        assert_eq!(
            funding.key(),
            (ExchangeId::BinanceLinear, "funding", "9001")
        );
        assert_eq!(
            entry(LedgerKind::Other("INSURANCE_CLEAR".into())).key().1,
            "INSURANCE_CLEAR"
        );
    }

    #[test]
    fn a_time_window_never_ends_before_it_starts() {
        assert!(TimeWindow::new(5, 5).is_ok());
        assert!(TimeWindow::new(6, 5).is_err());
        assert!(serde_json::from_str::<TimeWindow>(r#"{"start_ms":6,"end_ms":5}"#).is_err());
    }
}
