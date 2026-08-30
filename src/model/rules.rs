//! Exchange-reported trading rules and account balances.

use super::Symbol;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetBalance {
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

impl AssetBalance {
    pub fn total(&self) -> Decimal {
        self.free + self.locked
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SymbolRules {
    pub symbol: Symbol,
    pub base_asset: String,
    pub quote_asset: String,
    pub min_qty: Option<Decimal>,
    pub max_qty: Option<Decimal>,
    pub step_size: Option<Decimal>,
    pub market_min_qty: Option<Decimal>,
    pub market_max_qty: Option<Decimal>,
    pub market_step_size: Option<Decimal>,
    pub min_notional: Option<Decimal>,
    pub tick_size: Option<Decimal>,
}

impl SymbolRules {
    pub fn effective_market_step_size(&self) -> Option<Decimal> {
        self.market_step_size.or(self.step_size)
    }

    pub fn effective_market_min_qty(&self) -> Option<Decimal> {
        self.market_min_qty.or(self.min_qty)
    }

    pub fn effective_market_max_qty(&self) -> Option<Decimal> {
        self.market_max_qty.or(self.max_qty)
    }
}
