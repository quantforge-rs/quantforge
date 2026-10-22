//! A closed cycle's realized PnL (ADR 0002 §12): gross, fees, funding and
//! net, each in the settlement asset with its source, provisional until the
//! venue's records are final. Realized PnL means the net: gross − fees +
//! funding, where fees are a cost and funding received is positive. A
//! component not yet known stays pending, never zero.

use crate::model::{Asset, ModelError, Money};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// Whether a cycle's accounting can still change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AccountingStatus {
    Provisional,
    Final,
}

impl AccountingStatus {
    pub const INITIAL: Self = Self::Provisional;

    pub const ALL: [Self; 2] = [Self::Provisional, Self::Final];

    /// Every allowed move, as the parity matrix lists them.
    pub const TRANSITIONS: [(Self, Self); 1] = [(Self::Provisional, Self::Final)];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisional => "provisional",
            Self::Final => "final",
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Final)
    }
}

/// Which of the venue's records a figure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PnlSource {
    /// Trade or execution records.
    Fills,
    /// Income records, such as funding and commissions.
    IncomeRecords,
    /// The venue's per-close summary.
    ClosedPnl,
    /// The account's transaction log.
    TransactionLog,
    /// Computed from the other figures.
    Derived,
}

/// One amount and where it came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PnlFigure {
    pub amount: Money,
    pub source: PnlSource,
}

/// A cycle's realized PnL. The figures always agree: once gross, fees and
/// funding are all known, net is gross − fees + funding. Fees the venue
/// charged in another asset are listed apart and left out of net, since
/// v0.3 never converts between assets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RealizedPnlParts")]
pub struct RealizedPnl {
    settle: Asset,
    gross: Option<PnlFigure>,
    fees: Option<PnlFigure>,
    funding: Option<PnlFigure>,
    net: Option<PnlFigure>,
    other_fees: Vec<Money>,
    status: AccountingStatus,
}

impl RealizedPnl {
    /// From the components; net follows once all three are known.
    pub fn from_components(
        settle: Asset,
        gross: Option<PnlFigure>,
        fees: Option<PnlFigure>,
        funding: Option<PnlFigure>,
        other_fees: Vec<Money>,
    ) -> Result<Self, ModelError> {
        let net = match (&gross, &fees, &funding) {
            (Some(gross), Some(fees), Some(funding)) => Some(PnlFigure {
                amount: gross
                    .amount
                    .checked_sub(&fees.amount)?
                    .checked_add(&funding.amount)?,
                source: PnlSource::Derived,
            }),
            _ => None,
        };
        Self::checked(
            settle,
            gross,
            fees,
            funding,
            net,
            other_fees,
            AccountingStatus::INITIAL,
        )
    }

    /// From a venue's net summary, such as a closed-PnL record. Gross is
    /// derived from it once fees and funding are known; the summary is
    /// never added to its own components.
    pub fn from_net(
        settle: Asset,
        net: PnlFigure,
        fees: Option<PnlFigure>,
        funding: Option<PnlFigure>,
        other_fees: Vec<Money>,
    ) -> Result<Self, ModelError> {
        let gross = match (&fees, &funding) {
            (Some(fees), Some(funding)) => Some(PnlFigure {
                amount: net
                    .amount
                    .checked_add(&fees.amount)?
                    .checked_sub(&funding.amount)?,
                source: PnlSource::Derived,
            }),
            _ => None,
        };
        Self::checked(
            settle,
            gross,
            fees,
            funding,
            Some(net),
            other_fees,
            AccountingStatus::INITIAL,
        )
    }

    fn checked(
        settle: Asset,
        gross: Option<PnlFigure>,
        fees: Option<PnlFigure>,
        funding: Option<PnlFigure>,
        net: Option<PnlFigure>,
        other_fees: Vec<Money>,
        status: AccountingStatus,
    ) -> Result<Self, ModelError> {
        for figure in [&gross, &fees, &funding, &net].into_iter().flatten() {
            figure.amount.clone().in_asset(&settle)?;
        }
        if let Some(fee) = other_fees.iter().find(|fee| fee.asset() == &settle) {
            return Err(ModelError::AssetMismatch {
                left: "fees in other assets".to_string(),
                right: fee.asset().to_string(),
            });
        }
        if let (Some(gross), Some(fees), Some(funding), Some(net)) = (&gross, &fees, &funding, &net)
        {
            let expected = gross
                .amount
                .checked_sub(&fees.amount)?
                .checked_add(&funding.amount)?;
            if expected.amount() != net.amount.amount() {
                return Err(ModelError::InvalidAmount {
                    what: "net pnl",
                    value: net.amount.to_string(),
                    rule: "gross − fees + funding",
                });
            }
        }
        if status == AccountingStatus::Final
            && [&gross, &fees, &funding, &net].iter().any(|f| f.is_none())
        {
            return Err(Self::pending());
        }
        Ok(Self {
            settle,
            gross,
            fees,
            funding,
            net,
            other_fees,
            status,
        })
    }

    fn pending() -> ModelError {
        ModelError::MissingEvidence {
            machine: "accounting",
            to: AccountingStatus::Final.as_str(),
            evidence: "gross, fees, funding and net all known".to_string(),
        }
    }

    pub fn settle(&self) -> &Asset {
        &self.settle
    }

    pub fn gross(&self) -> Option<&PnlFigure> {
        self.gross.as_ref()
    }

    pub fn fees(&self) -> Option<&PnlFigure> {
        self.fees.as_ref()
    }

    pub fn funding(&self) -> Option<&PnlFigure> {
        self.funding.as_ref()
    }

    /// The realized PnL, net; `None` while it is pending.
    pub fn net(&self) -> Option<&PnlFigure> {
        self.net.as_ref()
    }

    pub fn other_fees(&self) -> &[Money] {
        &self.other_fees
    }

    pub const fn status(&self) -> AccountingStatus {
        self.status
    }

    /// The venue's records are final. Refused while any figure is pending.
    pub fn finalize(mut self) -> Result<Self, ModelError> {
        if [&self.gross, &self.fees, &self.funding, &self.net]
            .iter()
            .any(|f| f.is_none())
        {
            return Err(Self::pending());
        }
        self.status = AccountingStatus::Final;
        Ok(self)
    }

    /// The return on the position: net ÷ entry notional, both in the
    /// settlement asset, rounded half-even to 8 places; `None` while net is
    /// pending.
    pub fn return_on(&self, entry_notional: &Money) -> Result<Option<Decimal>, ModelError> {
        let notional = entry_notional.clone().in_asset(&self.settle)?;
        if notional.amount() <= Decimal::ZERO {
            return Err(ModelError::InvalidAmount {
                what: "entry notional",
                value: notional.to_string(),
                rule: "positive",
            });
        }
        let Some(net) = &self.net else {
            return Ok(None);
        };
        let ratio = net
            .amount
            .amount()
            .checked_div(notional.amount())
            .ok_or_else(|| ModelError::InvalidAmount {
                what: "return",
                value: net.amount.to_string(),
                rule: "within range",
            })?;
        Ok(Some(ratio.round_dp_with_strategy(
            8,
            RoundingStrategy::MidpointNearestEven,
        )))
    }
}

#[derive(Deserialize)]
struct RealizedPnlParts {
    settle: Asset,
    gross: Option<PnlFigure>,
    fees: Option<PnlFigure>,
    funding: Option<PnlFigure>,
    net: Option<PnlFigure>,
    other_fees: Vec<Money>,
    status: AccountingStatus,
}

impl TryFrom<RealizedPnlParts> for RealizedPnl {
    type Error = ModelError;

    fn try_from(parts: RealizedPnlParts) -> Result<Self, Self::Error> {
        Self::checked(
            parts.settle,
            parts.gross,
            parts.fees,
            parts.funding,
            parts.net,
            parts.other_fees,
            parts.status,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn settle() -> Asset {
        Asset::new("USDT").expect("asset")
    }

    fn usdt(text: &str, source: PnlSource) -> PnlFigure {
        let amount = Money::new(Decimal::from_str(text).expect("decimal"), settle());
        PnlFigure { amount, source }
    }

    // The PRD's p.10 close: 4.38 SOL bought at 234.23 and sold at 250.00 is
    // +69.0726 USDT gross on an entry notional of 1,025.9274 USDT (+6.73 %).
    // The fees are a 0.05 % taker fee on both legs (1,025.9274 and 1,095.00
    // USDT); the funding paid is illustrative. Expected values were computed
    // in Python's decimal module at 60 digits.
    #[test]
    fn net_is_gross_minus_fees_plus_funding_and_its_return_rounds_half_even() {
        let pnl = RealizedPnl::from_components(
            settle(),
            Some(usdt("69.0726", PnlSource::Fills)),
            Some(usdt("1.0604637", PnlSource::Fills)),
            Some(usdt("-0.5", PnlSource::IncomeRecords)),
            Vec::new(),
        )
        .expect("pnl");
        let net = pnl.net().expect("net");
        assert_eq!(net.amount.amount().to_string(), "67.5121363");
        assert_eq!(net.source, PnlSource::Derived);
        let notional = usdt("1025.9274", PnlSource::Fills).amount;
        let gross_only = RealizedPnl::from_net(
            settle(),
            usdt("69.0726", PnlSource::Fills),
            Some(usdt("0", PnlSource::Fills)),
            Some(usdt("0", PnlSource::Fills)),
            Vec::new(),
        )
        .expect("gross only");
        let ratio = gross_only
            .return_on(&notional)
            .expect("return")
            .expect("known");
        assert_eq!(ratio.to_string(), "0.06732699");
    }

    #[test]
    fn a_pending_component_leaves_net_pending_and_the_record_provisional() {
        let pnl = RealizedPnl::from_components(
            settle(),
            Some(usdt("10", PnlSource::Fills)),
            Some(usdt("1", PnlSource::Fills)),
            None,
            Vec::new(),
        )
        .expect("pnl");
        assert!(pnl.net().is_none());
        assert_eq!(pnl.status(), AccountingStatus::Provisional);
        assert!(pnl.clone().finalize().is_err());
        let notional = usdt("100", PnlSource::Fills).amount;
        assert_eq!(pnl.return_on(&notional).expect("return"), None);
    }

    #[test]
    fn a_venue_summary_derives_gross_and_is_never_added_to_its_components() {
        let pnl = RealizedPnl::from_net(
            settle(),
            usdt("8.5", PnlSource::ClosedPnl),
            Some(usdt("1", PnlSource::Fills)),
            Some(usdt("-0.5", PnlSource::TransactionLog)),
            Vec::new(),
        )
        .expect("pnl")
        .finalize()
        .expect("final");
        assert_eq!(
            pnl.gross().expect("gross").amount.amount().to_string(),
            "10.0"
        );
        assert_eq!(pnl.net().expect("net").amount.amount().to_string(), "8.5");
        assert_eq!(pnl.status(), AccountingStatus::Final);
    }

    #[test]
    fn mixed_assets_and_inconsistent_figures_are_refused() {
        let btc_fee = Money::new(Decimal::ONE, Asset::new("BTC").expect("asset"));
        let in_btc = PnlFigure {
            amount: btc_fee.clone(),
            source: PnlSource::Fills,
        };
        let gross = Some(usdt("10", PnlSource::Fills));
        assert!(RealizedPnl::from_components(settle(), gross, Some(in_btc), None, vec![]).is_err());
        let elsewhere = RealizedPnl::from_components(settle(), None, None, None, vec![btc_fee])
            .expect("fees in another asset are kept apart");
        assert_eq!(elsewhere.other_fees().len(), 1);
        let figures = ["10", "1", "0"].map(|text| Some(usdt(text, PnlSource::Fills)));
        let [gross, fees, funding] = figures;
        let pnl =
            RealizedPnl::from_components(settle(), gross, fees, funding, vec![]).expect("pnl");
        let mut tampered = serde_json::to_value(pnl).expect("json");
        tampered["net"]["amount"]["amount"] = serde_json::Value::String("99".to_string());
        assert!(serde_json::from_value::<RealizedPnl>(tampered).is_err());
    }
}
