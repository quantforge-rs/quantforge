//! A futures instrument as its venue lists it, normalized: identity,
//! contract kind, status and trading rules, with every quantity in the unit
//! its family trades in. Universe rule 1 (ADR 0002 §3) selects crypto
//! perpetuals in trading status, the linear ones settled in USDT.

use crate::model::units::decimal_text;
use crate::model::{
    Asset, Category, FaceValue, InstrumentId, Leverage, ModelError, Money, Price, Quantity,
    Refusal, TimestampMs,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The version of the universe rule [`FuturesInstrument::universe`]
/// applies; the parity matrix freezes the same number.
pub const UNIVERSE_RULE: u32 = 1;

/// The asset linear contracts settle in under rule 1.
const LINEAR_SETTLE: &str = "USDT";

/// What kind of contract an instrument is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContractKind {
    /// A crypto perpetual: the only kind rule 1 keeps.
    Perpetual,
    /// A perpetual on stocks, commodities or currencies.
    TradFiPerpetual,
    /// A future that delivers at a date.
    Dated { delivery_ms: TimestampMs },
}

/// Whether the venue currently trades the instrument.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TradingStatus {
    Trading,
    /// Any other status, with the venue's own word for it.
    Other(String),
}

/// Where an inverse contract's face value comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FaceValueSource {
    /// A field of the venue's instrument metadata.
    Metadata,
    /// A rule the venue documents, where no field carries it.
    DocumentedRule,
}

/// What one inverse contract is worth, and how the venue states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InverseContract {
    pub face_value: FaceValue,
    pub source: FaceValueSource,
}

/// The leverage a venue accepts for an instrument, where it lists it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LeverageFilter {
    pub min: Leverage,
    pub max: Leverage,
    #[serde(with = "decimal_text")]
    pub step: Decimal,
}

/// An instrument's metadata as an adapter reads it, before it is checked.
/// [`FuturesInstrument`] is the checked form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstrumentSpec {
    pub id: InstrumentId,
    pub base: Asset,
    pub quote: Asset,
    pub contract: ContractKind,
    pub status: TradingStatus,
    /// A scheduled delisting: the only expiry a perpetual has.
    pub delist_at_ms: Option<TimestampMs>,
    /// The face value, for an inverse instrument and only for one.
    pub inverse: Option<InverseContract>,
    pub tick: Price,
    pub step: Quantity,
    pub market_step: Quantity,
    pub min_qty: Quantity,
    pub market_min_qty: Quantity,
    pub market_max_qty: Quantity,
    /// In the quote asset; `None` when the instrument has no such filter.
    pub min_notional: Option<Money>,
    pub leverage: Option<LeverageFilter>,
    /// When the rules were read: they change, and must not be cached
    /// indefinitely.
    pub observed_at_ms: TimestampMs,
}

/// A futures instrument whose metadata is consistent: a linear or inverse
/// family, quantities in the family's unit, and an inverse instrument that
/// settles in its base coin with a face value. Every adapter's instruments
/// pass the same checks.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "InstrumentSpec", into = "InstrumentSpec")]
pub struct FuturesInstrument {
    spec: InstrumentSpec,
}

impl FuturesInstrument {
    pub fn spec(&self) -> &InstrumentSpec {
        &self.spec
    }

    pub fn id(&self) -> &InstrumentId {
        &self.spec.id
    }

    pub fn category(&self) -> Category {
        self.spec.id.category()
    }

    /// Universe rule 1: `Ok` for a crypto perpetual in trading status whose
    /// linear form settles in USDT; otherwise why it is outside.
    pub fn universe(&self) -> Result<(), UniverseRefusal> {
        match self.spec.contract {
            ContractKind::Perpetual => {}
            ContractKind::TradFiPerpetual => return Err(UniverseRefusal::TradFi),
            ContractKind::Dated { delivery_ms } => {
                return Err(UniverseRefusal::Dated { delivery_ms });
            }
        }
        if let TradingStatus::Other(status) = &self.spec.status {
            return Err(UniverseRefusal::NotTrading {
                status: status.clone(),
            });
        }
        let settle = &self.spec.id.settle;
        if self.category() == Category::Linear && settle.as_str() != LINEAR_SETTLE {
            return Err(UniverseRefusal::NotUsdtSettled {
                settle: settle.clone(),
            });
        }
        Ok(())
    }
}

impl TryFrom<InstrumentSpec> for FuturesInstrument {
    type Error = ModelError;

    fn try_from(spec: InstrumentSpec) -> Result<Self, Self::Error> {
        let refuse = |reason: String| ModelError::UnsupportedInstrument {
            instrument: spec.id.to_string(),
            reason,
        };
        let category = spec.id.category();
        if !category.is_futures() {
            return Err(refuse(
                "a spot instrument is not a futures contract".to_string(),
            ));
        }
        match (category, &spec.inverse) {
            (Category::Inverse, None) => {
                return Err(refuse(
                    "an inverse contract needs its face value".to_string(),
                ));
            }
            (Category::Linear, Some(_)) => {
                return Err(refuse("a linear contract has no face value".to_string()));
            }
            _ => {}
        }
        if category == Category::Inverse && spec.id.settle != spec.base {
            return Err(refuse(format!(
                "an inverse contract settles in its base coin {}, not {}",
                spec.base, spec.id.settle
            )));
        }
        for quantity in [
            spec.step,
            spec.market_step,
            spec.min_qty,
            spec.market_min_qty,
            spec.market_max_qty,
        ] {
            quantity.in_unit_of(category)?;
        }
        if spec.step.is_zero() || spec.market_step.is_zero() {
            return Err(refuse("a quantity step must be positive".to_string()));
        }
        if spec.market_min_qty.value() > spec.market_max_qty.value() {
            return Err(refuse("the market minimum exceeds the maximum".to_string()));
        }
        if let Some(notional) = &spec.min_notional {
            if notional.asset() != &spec.quote {
                return Err(refuse(format!(
                    "the minimum notional is in {}, not the quote asset {}",
                    notional.asset(),
                    spec.quote
                )));
            }
        }
        if let Some(leverage) = &spec.leverage {
            if leverage.min > leverage.max || leverage.step <= Decimal::ZERO {
                return Err(refuse("the leverage filter is inconsistent".to_string()));
            }
        }
        Ok(Self { spec })
    }
}

impl From<FuturesInstrument> for InstrumentSpec {
    fn from(instrument: FuturesInstrument) -> Self {
        instrument.spec
    }
}

/// Why universe rule 1 leaves an instrument out. Every one is refused as
/// [`Refusal::OutsideUniverse`], with this as its reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniverseRefusal {
    TradFi,
    Dated { delivery_ms: TimestampMs },
    NotTrading { status: String },
    NotUsdtSettled { settle: Asset },
}

impl UniverseRefusal {
    pub const fn refusal(&self) -> Refusal {
        Refusal::OutsideUniverse
    }
}

impl fmt::Display for UniverseRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TradFi => f.write_str("TradFi perpetuals are outside the v0.3 universe"),
            Self::Dated { .. } => f.write_str("dated futures are outside the v0.3 universe"),
            Self::NotTrading { status } => write!(f, "the venue lists it as {status}, not trading"),
            Self::NotUsdtSettled { settle } => {
                write!(
                    f,
                    "linear contracts settled in {settle} are refused; only USDT is"
                )
            }
        }
    }
}

/// An instrument's prices from its venue, each with the time it was taken:
/// the mark and index prices together, and the book's best bid and ask.
/// Stops trigger on the mark price; a USD margin pool converts at the index
/// price.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceSnapshot {
    pub instrument: InstrumentId,
    pub mark: Price,
    pub index: Option<Price>,
    pub marked_at_ms: TimestampMs,
    pub bid: Option<Price>,
    pub ask: Option<Price>,
    pub quoted_at_ms: Option<TimestampMs>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ExchangeId, Symbol};
    use std::str::FromStr;

    fn qty(category: Category, text: &str) -> Quantity {
        let value = Decimal::from_str(text).expect("decimal");
        Quantity::for_category(category, value).expect("quantity")
    }

    fn asset(code: &str) -> Asset {
        Asset::new(code).expect("asset")
    }

    /// A trading perpetual on `exchange`, with whole-unit quantity rules.
    fn spec(exchange: ExchangeId, symbol: &str, settle: &str, quote: &str) -> InstrumentSpec {
        let category = exchange.category();
        let symbol = Symbol::new(symbol).expect("symbol");
        InstrumentSpec {
            id: InstrumentId::new(exchange, symbol, asset(settle)),
            base: asset("BTC"),
            quote: asset(quote),
            contract: ContractKind::Perpetual,
            status: TradingStatus::Trading,
            delist_at_ms: None,
            inverse: None,
            tick: Price::new(Decimal::from_str("0.10").expect("tick")).expect("price"),
            step: qty(category, "1"),
            market_step: qty(category, "1"),
            min_qty: qty(category, "1"),
            market_min_qty: qty(category, "1"),
            market_max_qty: qty(category, "60000"),
            min_notional: None,
            leverage: None,
            observed_at_ms: 1,
        }
    }

    /// A USDT-settled linear perpetual with a 5 USDT minimum notional.
    fn linear() -> InstrumentSpec {
        let mut linear = spec(ExchangeId::BybitLinear, "BTCUSDT", "USDT", "USDT");
        linear.min_notional = Some(Money::new(Decimal::from(5), asset("USDT")));
        linear
    }

    /// An inverse perpetual worth 100 USD a contract by its metadata, as
    /// Binance's BTCUSD_PERP is.
    fn inverse() -> InstrumentSpec {
        let mut inverse = spec(ExchangeId::BinanceInverse, "BTCUSD_PERP", "BTC", "USD");
        inverse.inverse = Some(InverseContract {
            face_value: FaceValue::new(Decimal::from(100)).expect("face value"),
            source: FaceValueSource::Metadata,
        });
        inverse
    }

    /// A spec builder, and a change made to what it builds.
    type Build = fn() -> InstrumentSpec;
    type Change = fn(&mut InstrumentSpec);

    fn instrument(spec: InstrumentSpec) -> FuturesInstrument {
        FuturesInstrument::try_from(spec).expect("instrument")
    }

    #[test]
    fn rule_one_keeps_trading_crypto_perpetuals_settled_in_usdt() {
        for spec in [linear(), inverse()] {
            assert_eq!(instrument(spec).universe(), Ok(()));
        }
    }

    #[test]
    fn rule_one_refuses_tradfi_dated_halted_and_non_usdt_linear_contracts() {
        let cases: [(Change, &str); 4] = [
            (
                |spec| spec.contract = ContractKind::TradFiPerpetual,
                "TradFi perpetuals are outside the v0.3 universe",
            ),
            (
                |spec| spec.contract = ContractKind::Dated { delivery_ms: 7 },
                "dated futures are outside the v0.3 universe",
            ),
            (
                |spec| spec.status = TradingStatus::Other("SETTLING".into()),
                "the venue lists it as SETTLING, not trading",
            ),
            (
                |spec| *spec = spec_with_settle("USDC"),
                "linear contracts settled in USDC are refused; only USDT is",
            ),
        ];
        for (change, reason) in cases {
            let mut spec = linear();
            change(&mut spec);
            let refusal = instrument(spec).universe().expect_err("refused");
            assert_eq!(refusal.to_string(), reason);
            assert_eq!(refusal.refusal(), Refusal::OutsideUniverse);
        }
    }

    fn spec_with_settle(settle: &str) -> InstrumentSpec {
        spec(ExchangeId::BybitLinear, "BTCPERP", settle, settle)
    }

    #[test]
    fn inconsistent_metadata_is_refused() {
        let cases: [(Build, Change); 5] = [
            (inverse, |spec| {
                spec.min_qty = qty(Category::Linear, "0.001")
            }),
            (inverse, |spec| spec.inverse = None),
            (inverse, |spec| spec.id.settle = asset("USD")),
            (linear, |spec| spec.id.exchange = ExchangeId::BybitSpot),
            (linear, |spec| {
                spec.min_notional = Some(Money::new(Decimal::from(5), asset("BTC")));
            }),
        ];
        for (build, change) in cases {
            let mut spec = build();
            change(&mut spec);
            assert!(FuturesInstrument::try_from(spec).is_err());
        }
    }

    #[test]
    fn an_instrument_is_checked_again_when_read_back() {
        let json = serde_json::to_value(instrument(inverse())).expect("json");
        let back: FuturesInstrument = serde_json::from_value(json.clone()).expect("back");
        assert_eq!(back, instrument(inverse()));
        let mut broken = json;
        broken["inverse"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<FuturesInstrument>(broken).is_err());
    }
}
