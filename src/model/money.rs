//! Money: an asset code and an amount of it. Arithmetic never mixes
//! assets, so USDT is never added to BTC (ADR 0002 §6, §12); a combined
//! figure would need a stated conversion, which v0.3 never makes.

use super::ModelError;
use super::units::{canonical, decimal_text};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// An asset code such as `USDT` or `BTC`: trimmed and ASCII-uppercased like
/// a [`Symbol`](super::Symbol). Venues list assets whose codes are not
/// ASCII, so any other character is kept, but spaces and control
/// characters are refused.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Asset(String);

impl Asset {
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        let code = value.trim();
        if code.is_empty() || code.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(ModelError::InvalidAsset(value));
        }
        Ok(Self(code.to_ascii_uppercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Asset {
    type Error = ModelError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for Asset {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An amount of one asset: settlement money, margin, fees, funding or PnL.
/// Arithmetic refuses to mix assets; a combined figure needs a stated
/// conversion, which v0.3 never makes.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Money {
    #[serde(with = "decimal_text")]
    amount: Decimal,
    asset: Asset,
}

impl Money {
    pub fn new(amount: Decimal, asset: Asset) -> Self {
        Self {
            amount: canonical(amount),
            asset,
        }
    }

    pub fn amount(&self) -> Decimal {
        self.amount
    }

    pub fn asset(&self) -> &Asset {
        &self.asset
    }

    pub fn is_zero(&self) -> bool {
        self.amount.is_zero()
    }

    /// Refuses money in any asset but `asset`.
    pub fn in_asset(self, asset: &Asset) -> Result<Self, ModelError> {
        if &self.asset != asset {
            return Err(ModelError::AssetMismatch {
                left: asset.to_string(),
                right: self.asset.to_string(),
            });
        }
        Ok(self)
    }

    pub fn checked_add(&self, other: &Self) -> Result<Self, ModelError> {
        self.combine(other, Decimal::checked_add)
    }

    pub fn checked_sub(&self, other: &Self) -> Result<Self, ModelError> {
        self.combine(other, Decimal::checked_sub)
    }

    pub fn negated(&self) -> Self {
        Self::new(-self.amount, self.asset.clone())
    }

    fn combine(
        &self,
        other: &Self,
        op: fn(Decimal, Decimal) -> Option<Decimal>,
    ) -> Result<Self, ModelError> {
        if self.asset != other.asset {
            return Err(ModelError::AssetMismatch {
                left: self.asset.to_string(),
                right: other.asset.to_string(),
            });
        }
        let amount = op(self.amount, other.amount).ok_or_else(|| ModelError::InvalidAmount {
            what: "amount",
            value: format!("{} and {}", self.amount, other.amount),
            rule: "within range",
        })?;
        Ok(Self::new(amount, self.asset.clone()))
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.amount, self.asset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(text: &str) -> Decimal {
        Decimal::from_str(text).expect("decimal")
    }

    fn usdt() -> Asset {
        Asset::new("usdt").expect("asset")
    }

    #[test]
    fn money_keeps_its_scale_and_refuses_json_numbers() {
        let money = Money::new(dec("1.10"), usdt());
        let json = serde_json::to_string(&money).expect("json");
        assert_eq!(json, r#"{"amount":"1.10","asset":"USDT"}"#);
        assert!(serde_json::from_str::<Money>(r#"{"amount":1.1,"asset":"USDT"}"#).is_err());
    }

    #[test]
    fn money_never_mixes_assets() {
        let fee = Money::new(dec("0.5"), usdt());
        let btc = Money::new(dec("0.001"), Asset::new("BTC").expect("asset"));
        let error = fee.checked_add(&btc).expect_err("mixed assets");
        assert_eq!(
            error.to_string(),
            "cannot combine USDT with BTC: amounts in different assets are never summed"
        );
        let total = fee.checked_add(&fee).expect("sum");
        assert_eq!(total.to_string(), "1.0 USDT");
        assert!(btc.clone().in_asset(&usdt()).is_err());
        assert_eq!(fee.negated().negated(), fee);
        let mut negative_zero = Decimal::ZERO;
        negative_zero.set_sign_negative(true);
        let zero = Money::new(negative_zero, usdt());
        assert!(!zero.amount().is_sign_negative());
        assert_eq!(
            serde_json::to_string(&zero).expect("json"),
            r#"{"amount":"0","asset":"USDT"}"#
        );
    }

    #[test]
    fn assets_normalize_like_symbols_and_keep_unicode() {
        assert_eq!(Asset::new(" usdt ").expect("asset").as_str(), "USDT");
        assert_eq!(Asset::new("币安人生").expect("asset").as_str(), "币安人生");
        for bad in ["", "  ", "US DT", "US\tDT"] {
            assert!(Asset::new(bad).is_err(), "{bad:?}");
        }
    }
}
