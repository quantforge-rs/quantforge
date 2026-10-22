//! Who owns a position (ADR 0002 §2): the position key, automation profiles
//! and their owner tags, the store's activation epoch, position cycles and
//! the signals that drive them.

use super::client_order_id::to_base36;
use crate::model::{AccountRef, Category, InstrumentId, ModelError};
use serde::{Deserialize, Serialize};
use std::fmt;

/// One position in one-way mode: an account's exposure to one instrument.
/// Leases and serialization are keyed by it, never by profile. It has no
/// candle interval, so ownership never depends on one.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "PositionKeyParts")]
pub struct PositionKey {
    account: AccountRef,
    instrument: InstrumentId,
}

impl PositionKey {
    /// Refuses a Spot instrument, which holds no position, and an
    /// instrument on another venue than the account's.
    pub fn new(account: AccountRef, instrument: InstrumentId) -> Result<Self, ModelError> {
        if !instrument.category().is_futures() {
            return Err(ModelError::UnsupportedInstrument {
                instrument: instrument.to_string(),
                reason: "a spot instrument holds no position".to_string(),
            });
        }
        if instrument.venue() != account.venue() {
            return Err(ModelError::InvalidIdentifier {
                what: "position key",
                value: format!("{instrument} for a {} account", account.venue()),
                rule: "the instrument must trade on the account's venue",
            });
        }
        Ok(Self {
            account,
            instrument,
        })
    }

    pub fn account(&self) -> &AccountRef {
        &self.account
    }

    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }
}

#[derive(Deserialize)]
struct PositionKeyParts {
    account: AccountRef,
    instrument: InstrumentId,
}

impl TryFrom<PositionKeyParts> for PositionKey {
    type Error = ModelError;

    fn try_from(parts: PositionKeyParts) -> Result<Self, Self::Error> {
        Self::new(parts.account, parts.instrument)
    }
}

/// Defines a text identifier whose rule `$valid` checks, read back through
/// the same check.
macro_rules! text_id {
    ($(#[$doc:meta])* $name:ident, $what:literal, $rule:literal, $valid:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(try_from = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
                let value = value.into();
                let valid: fn(&str) -> bool = $valid;
                if !valid(&value) {
                    return Err(ModelError::InvalidIdentifier {
                        what: $what,
                        value,
                        rule: $rule,
                    });
                }
                Ok(Self(value))
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

text_id!(
    /// An automation profile's name, chosen by the operator.
    ProfileId,
    "profile id",
    "1 to 32 of a-z, 0-9, - and _, starting with a letter or digit",
    |text| {
        let mut bytes = text.bytes();
        bytes
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
            && text.len() <= 32
            && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    }
);

text_id!(
    /// A profile's owner tag: six random lowercase letters and digits,
    /// unique in its store and checked at arming. Every client order ID the
    /// profile sends carries it, which tells its orders from anyone else's.
    OwnerTag,
    "owner tag",
    "6 of a-z and 0-9",
    |text| text.len() == 6 && text.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
);

text_id!(
    /// A signal's ID within its profile. ALG-737 derives it from fields
    /// TradingView renders the same way on every resend; the store keeps it
    /// with the hash of the signal's canonical payload.
    SignalId,
    "signal id",
    "1 to 128 characters without control characters",
    |text| !text.is_empty() && text.chars().count() <= 128 && !text.chars().any(char::is_control)
);

impl OwnerTag {
    /// A fresh random tag. The store refuses one already in use.
    pub fn generate() -> Self {
        let bytes = uuid::Uuid::new_v4().into_bytes();
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[..8]);
        // 36^6 tags; the first eight random bytes reduced onto them.
        let value = u64::from_le_bytes(word) % 36u64.pow(6);
        Self(format!("{:0>6}", to_base36(value)))
    }
}

/// A profile's identity: its ID, the account it trades, its product family
/// and its owner tag. Changing any of them disarms the profile; its policy
/// revision and label are metadata and never do.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ProfileIdentityParts")]
pub struct ProfileIdentity {
    id: ProfileId,
    account_ref: AccountRef,
    category: Category,
    owner_tag: OwnerTag,
}

impl ProfileIdentity {
    /// Refuses Spot: profiles execute external signals on futures only.
    pub fn new(
        id: ProfileId,
        account_ref: AccountRef,
        category: Category,
        owner_tag: OwnerTag,
    ) -> Result<Self, ModelError> {
        if !category.is_futures() {
            return Err(ModelError::InvalidIdentifier {
                what: "profile category",
                value: category.to_string(),
                rule: "a profile trades linear or inverse futures",
            });
        }
        Ok(Self {
            id,
            account_ref,
            category,
            owner_tag,
        })
    }

    pub fn id(&self) -> &ProfileId {
        &self.id
    }

    pub fn account_ref(&self) -> &AccountRef {
        &self.account_ref
    }

    pub const fn category(&self) -> Category {
        self.category
    }

    pub fn owner_tag(&self) -> &OwnerTag {
        &self.owner_tag
    }
}

#[derive(Deserialize)]
struct ProfileIdentityParts {
    id: ProfileId,
    account_ref: AccountRef,
    category: Category,
    owner_tag: OwnerTag,
}

impl TryFrom<ProfileIdentityParts> for ProfileIdentity {
    type Error = ModelError;

    fn try_from(parts: ProfileIdentityParts) -> Result<Self, Self::Error> {
        Self::new(parts.id, parts.account_ref, parts.category, parts.owner_tag)
    }
}

/// The activation epoch of the store that issued an order. A store copied
/// and activated elsewhere gets a new epoch, so an order with the right
/// owner tag but another epoch reveals a second writer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActivationEpoch(pub u32);

/// One position cycle: from flat, through an owned entry, back to flat.
/// Numbered by the store that records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u64")]
pub struct PositionCycleId(u64);

impl PositionCycleId {
    /// Refuses 0, which no store issues.
    pub fn new(value: u64) -> Result<Self, ModelError> {
        if value == 0 {
            return Err(ModelError::InvalidIdentifier {
                what: "position cycle id",
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

impl TryFrom<u64> for PositionCycleId {
    type Error = ModelError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Asset, Environment, ExchangeId, Symbol, Venue, VenueAccountId};

    fn account(venue: Venue) -> AccountRef {
        let id = VenueAccountId::issued("12345").expect("account id");
        AccountRef::new(venue, Environment::Demo, id).expect("account")
    }

    fn instrument(exchange: ExchangeId) -> InstrumentId {
        let symbol = Symbol::new("BTCUSDT").expect("symbol");
        InstrumentId::new(exchange, symbol, Asset::new("USDT").expect("asset"))
    }

    #[test]
    fn a_position_key_refuses_spot_and_an_instrument_on_another_venue() {
        assert!(
            PositionKey::new(account(Venue::Bybit), instrument(ExchangeId::BybitLinear)).is_ok()
        );
        let spot = PositionKey::new(account(Venue::Bybit), instrument(ExchangeId::BybitSpot));
        assert!(
            spot.expect_err("spot")
                .to_string()
                .contains("holds no position")
        );
        let crossed =
            PositionKey::new(account(Venue::Bybit), instrument(ExchangeId::BinanceLinear));
        assert!(crossed.is_err());
    }

    #[test]
    fn a_position_key_carries_no_interval() {
        let key = PositionKey::new(account(Venue::Bybit), instrument(ExchangeId::BybitLinear))
            .expect("key");
        let json = serde_json::to_value(&key).expect("json");
        assert!(!json.to_string().contains("interval"), "{json}");
        let back: PositionKey = serde_json::from_value(json).expect("round trip");
        assert_eq!(back, key);
    }

    #[test]
    fn profile_ids_owner_tags_and_signal_ids_keep_their_rules() {
        assert!(ProfileId::new("main-btc_1").is_ok());
        for bad in ["", "-lead", "Upper", "has space", &"x".repeat(33)] {
            assert!(ProfileId::new(bad).is_err(), "{bad:?}");
        }
        assert!(OwnerTag::new("k3x9p2").is_ok());
        for bad in ["k3x9p", "k3x9p22", "K3X9P2", "k3x-p2"] {
            assert!(OwnerTag::new(bad).is_err(), "{bad:?}");
        }
        let generated = OwnerTag::generate();
        assert!(OwnerTag::new(generated.as_str()).is_ok(), "{generated}");
        assert!(SignalId::new("entry SOLUSDT.P long 1729584000000").is_ok());
        assert!(SignalId::new("").is_err());
        assert!(SignalId::new("bad\nline").is_err());
    }

    #[test]
    fn a_profile_trades_futures_only_and_its_identity_has_the_policys_fields() {
        let id = ProfileId::new("main").expect("id");
        let tag = OwnerTag::new("k3x9p2").expect("tag");
        let spot = ProfileIdentity::new(
            id.clone(),
            account(Venue::Bybit),
            Category::Spot,
            tag.clone(),
        );
        assert!(spot.is_err());
        let identity = ProfileIdentity::new(id, account(Venue::Bybit), Category::Inverse, tag)
            .expect("identity");
        let json = serde_json::to_value(&identity).expect("json");
        let keys: Vec<&String> = json.as_object().expect("object").keys().collect();
        assert_eq!(keys, ["account_ref", "category", "id", "owner_tag"]);
    }

    #[test]
    fn store_issued_numbers_are_positive() {
        assert!(PositionCycleId::new(0).is_err());
        assert_eq!(PositionCycleId::new(9).expect("cycle").get(), 9);
        assert!(serde_json::from_str::<PositionCycleId>("0").is_err());
    }
}
