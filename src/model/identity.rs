//! Who and what an execution is for (ADR 0002 §2): the environment a store
//! and its connectors are bound to, the venue account, the instrument
//! traded, the venue's own order, algo-order and trade IDs as opaque text,
//! and the operations that change anything on an exchange.

use super::{Asset, Category, ExchangeId, Interval, MarketId, ModelError, Symbol, Venue};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// Where a venue's systems run: real funds on mainnet, or a test
/// environment. It is declared by the operator and never inferred from a
/// URL; every store is bound to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Environment {
    Mainnet,
    Testnet,
    Demo,
}

impl Environment {
    pub const ALL: [Self; 3] = [Self::Mainnet, Self::Testnet, Self::Demo];

    /// `mainnet`, `testnet` or `demo`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Demo => "demo",
        }
    }

    /// Whether orders here trade real funds.
    pub const fn is_production(self) -> bool {
        matches!(self, Self::Mainnet)
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What is traded: a symbol on a product line, and the asset the venue
/// books its PnL in. That is the settle coin for linear, the base coin for
/// inverse and the quote asset for Spot. It comes from venue metadata and
/// is checked again whenever the instrument is resolved.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InstrumentId {
    pub exchange: ExchangeId,
    pub symbol: Symbol,
    pub settle: Asset,
}

impl InstrumentId {
    pub fn new(exchange: ExchangeId, symbol: Symbol, settle: Asset) -> Self {
        Self {
            exchange,
            symbol,
            settle,
        }
    }

    pub const fn venue(&self) -> Venue {
        self.exchange.venue()
    }

    pub const fn category(&self) -> Category {
        self.exchange.category()
    }

    /// The data-series key of this instrument's candles at `interval`.
    pub fn series(&self, interval: Interval) -> MarketId {
        MarketId::new(self.exchange, self.symbol.clone(), interval)
    }
}

impl fmt::Display for InstrumentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.exchange, self.symbol)
    }
}

/// Refuses identifier text that is empty, longer than `max`, or holds
/// spaces or control characters.
fn checked_text(
    what: &'static str,
    value: String,
    max: usize,
    rule: &'static str,
) -> Result<String, ModelError> {
    let valid = !value.is_empty()
        && value.chars().count() <= max
        && !value.chars().any(|c| c.is_whitespace() || c.is_control());
    if !valid {
        return Err(ModelError::InvalidIdentifier { what, value, rule });
    }
    Ok(value)
}

/// An ID a venue issues, kept as opaque text: venues' IDs are numbers on
/// some venues and strings on others, and are never computed with.
macro_rules! venue_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
                checked_text($what, value.into(), 128, "1 to 128 characters without spaces").map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = ModelError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

venue_id!(
    /// A venue's ID for an ordinary order.
    VenueOrderId,
    "venue order id"
);

venue_id!(
    /// A venue's ID for a conditional (algo) order, which lives apart from
    /// its ordinary orders.
    VenueAlgoId,
    "venue algo order id"
);

venue_id!(
    /// A venue's ID for one fill: a trade or an execution.
    VenueTradeId,
    "venue trade id"
);

/// The account behind a set of credentials, as the venue identifies it.
/// Where a venue gives no ID for the keys in use, a fingerprint of the API
/// key stands in, but only outside mainnet (ADR 0002 §2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "AccountIdParts")]
pub enum VenueAccountId {
    /// The venue's own account ID.
    Venue(String),
    /// The first 16 hex digits of the SHA-256 of the API key: never the key
    /// itself, and never a secret.
    KeyFingerprint(String),
}

impl VenueAccountId {
    /// The ID the venue reports for the account.
    pub fn issued(id: impl Into<String>) -> Result<Self, ModelError> {
        checked_text(
            "venue account id",
            id.into(),
            64,
            "1 to 64 characters without spaces",
        )
        .map(Self::Venue)
    }

    /// The stand-in for an account the venue gives no ID for.
    pub fn fingerprint(api_key: &str) -> Self {
        let digest = Sha256::digest(api_key.as_bytes());
        Self::KeyFingerprint(hex::encode(&digest[..8]))
    }

    pub fn is_venue_issued(&self) -> bool {
        matches!(self, Self::Venue(_))
    }
}

/// `VenueAccountId` as stored, checked again on load.
#[derive(Deserialize)]
enum AccountIdParts {
    Venue(String),
    KeyFingerprint(String),
}

impl TryFrom<AccountIdParts> for VenueAccountId {
    type Error = ModelError;

    fn try_from(parts: AccountIdParts) -> Result<Self, Self::Error> {
        match parts {
            AccountIdParts::Venue(id) => Self::issued(id),
            AccountIdParts::KeyFingerprint(text) => {
                let hex = |c: char| c.is_ascii_digit() || ('a'..='f').contains(&c);
                if text.len() != 16 || !text.chars().all(hex) {
                    return Err(ModelError::InvalidIdentifier {
                        what: "key fingerprint",
                        value: text,
                        rule: "16 lowercase hex digits",
                    });
                }
                Ok(Self::KeyFingerprint(text))
            }
        }
    }
}

/// One account on one venue in one environment. Execution keys carry the
/// environment even though stores are bound to one, as a second safeguard.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "AccountRefParts")]
pub struct AccountRef {
    venue: Venue,
    environment: Environment,
    venue_account_id: VenueAccountId,
}

impl AccountRef {
    /// Refuses a key fingerprint on mainnet, where the account ID must come
    /// from the venue.
    pub fn new(
        venue: Venue,
        environment: Environment,
        venue_account_id: VenueAccountId,
    ) -> Result<Self, ModelError> {
        if environment.is_production() && !venue_account_id.is_venue_issued() {
            return Err(ModelError::InvalidIdentifier {
                what: "mainnet account",
                value: format!("{venue} {environment}"),
                rule: "needs the venue's account id, not a key fingerprint",
            });
        }
        Ok(Self {
            venue,
            environment,
            venue_account_id,
        })
    }

    pub const fn venue(&self) -> Venue {
        self.venue
    }

    pub const fn environment(&self) -> Environment {
        self.environment
    }

    pub fn venue_account_id(&self) -> &VenueAccountId {
        &self.venue_account_id
    }
}

#[derive(Deserialize)]
struct AccountRefParts {
    venue: Venue,
    environment: Environment,
    venue_account_id: VenueAccountId,
}

impl TryFrom<AccountRefParts> for AccountRef {
    type Error = ModelError;

    fn try_from(parts: AccountRefParts) -> Result<Self, Self::Error> {
        Self::new(parts.venue, parts.environment, parts.venue_account_id)
    }
}

/// One action that changes something on an exchange, numbered by the store
/// that records it. Spot replacements use it as well as futures orders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct OperationId(u64);

impl OperationId {
    /// Refuses 0, which no store issues.
    pub fn new(value: u64) -> Result<Self, ModelError> {
        if value == 0 {
            return Err(ModelError::InvalidIdentifier {
                what: "operation id",
                value: value.to_string(),
                rule: "positive",
            });
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for OperationId {
    type Error = ModelError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account_id() -> VenueAccountId {
        VenueAccountId::issued("12345").expect("account id")
    }

    #[test]
    fn a_key_fingerprint_never_identifies_a_mainnet_account() {
        let fingerprint = VenueAccountId::fingerprint("test-api-key");
        assert!(!fingerprint.is_venue_issued());
        let error = AccountRef::new(Venue::Bybit, Environment::Mainnet, fingerprint.clone())
            .expect_err("mainnet fingerprint");
        assert!(error.to_string().contains("venue's account id"), "{error}");
        for environment in [Environment::Testnet, Environment::Demo] {
            assert!(AccountRef::new(Venue::Bybit, environment, fingerprint.clone()).is_ok());
        }
        assert!(AccountRef::new(Venue::Bybit, Environment::Mainnet, account_id()).is_ok());
        let demo = AccountRef::new(Venue::Bybit, Environment::Demo, fingerprint).expect("account");
        let mut json = serde_json::to_value(demo).expect("json");
        json["environment"] = serde_json::json!("Mainnet");
        assert!(serde_json::from_value::<AccountRef>(json).is_err());
    }

    #[test]
    fn a_fingerprint_is_sixteen_hex_digits_of_the_keys_hash() {
        // Only the first 8 bytes of the hash are kept, so the key cannot be
        // recovered from the store.
        let VenueAccountId::KeyFingerprint(text) = VenueAccountId::fingerprint("test-api-key")
        else {
            panic!("a fingerprint");
        };
        assert_eq!(text.len(), 16);
        assert!(!text.contains("test-api-key"));
        let json =
            serde_json::to_string(&VenueAccountId::KeyFingerprint(text.clone())).expect("json");
        let back: VenueAccountId = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, VenueAccountId::KeyFingerprint(text));
        let bad = r#"{"KeyFingerprint":"not-hex"}"#;
        assert!(serde_json::from_str::<VenueAccountId>(bad).is_err());
    }

    #[test]
    fn environments_are_declared_names_and_only_mainnet_is_production() {
        let names = Environment::ALL.map(|env| (env.as_str(), env.is_production()));
        assert_eq!(
            names,
            [("mainnet", true), ("testnet", false), ("demo", false)]
        );
    }

    #[test]
    fn an_instruments_data_series_drops_the_settlement_asset_and_adds_an_interval() {
        let instrument = InstrumentId::new(
            ExchangeId::BybitInverse,
            Symbol::new("BTCUSD").expect("symbol"),
            Asset::new("BTC").expect("asset"),
        );
        let series = instrument.series(Interval::M1);
        assert_eq!(series.exchange, ExchangeId::BybitInverse);
        assert_eq!(series.symbol.as_str(), "BTCUSD");
        assert_eq!(instrument.category(), Category::Inverse);
        assert_eq!(instrument.to_string(), "bybit_inverse BTCUSD");
    }

    #[test]
    fn venue_ids_are_opaque_text() {
        let order = VenueOrderId::new("8389765519").expect("id");
        assert_eq!(order.as_str(), "8389765519");
        assert!(VenueTradeId::new("1321003749386327552").is_ok());
        for bad in ["", "has space", "tab\t"] {
            assert!(VenueAlgoId::new(bad).is_err(), "{bad:?}");
        }
        let number = serde_json::from_str::<VenueOrderId>("123");
        assert!(number.is_err(), "a number is not text");
        assert!(OperationId::new(0).is_err());
        let seven = OperationId::new(7).expect("id");
        assert_eq!(serde_json::to_string(&seven).expect("json"), "7");
        assert!(serde_json::from_str::<OperationId>("0").is_err());
    }
}
