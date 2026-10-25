//! The venue account behind a set of credentials (ADR 0002 §2): the ID the
//! venue reports for the keys in use or, outside mainnet, a fingerprint of
//! the API key, and the reference that binds it to a venue and an
//! environment. Both forms are checked when built and again on load, so
//! no unchecked text reaches a persisted key.

use super::identity::{plain_text, text_id};
use super::{Environment, ModelError, Venue};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

text_id!(
    /// The account ID a venue reports for the keys in use.
    IssuedAccountId,
    "venue account id",
    "1 to 64 characters without spaces",
    |text| plain_text(text, 64)
);

/// Whether `text` is exactly 16 lowercase hex digits.
fn hex16(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

text_id!(
    /// The first 16 hex digits of the SHA-256 of an API key: never the key
    /// itself, and never a secret. SHA-256 is the right hash here. The key
    /// is a random token the venue issued, not a password a person chose,
    /// so a deliberately slow hash would add nothing; and the fingerprint
    /// only identifies an account outside mainnet, nothing authenticates
    /// with it.
    KeyFingerprint,
    "key fingerprint",
    "16 lowercase hex digits",
    hex16
);

impl KeyFingerprint {
    /// The fingerprint of `api_key`.
    pub fn of(api_key: &str) -> Self {
        let digest = Sha256::digest(api_key.as_bytes());
        Self(hex::encode(&digest[..8]))
    }
}

/// The account behind a set of credentials, as the venue identifies it.
/// Where a venue gives no ID for the keys in use, a fingerprint of the API
/// key stands in, but only outside mainnet (ADR 0002 §2).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VenueAccountId {
    /// The venue's own account ID.
    Venue(IssuedAccountId),
    /// The stand-in for an account the venue gives no ID for.
    KeyFingerprint(KeyFingerprint),
}

impl VenueAccountId {
    /// The ID the venue reports for the account.
    pub fn issued(id: impl Into<String>) -> Result<Self, ModelError> {
        IssuedAccountId::new(id).map(Self::Venue)
    }

    /// The stand-in for an account the venue gives no ID for.
    pub fn fingerprint(api_key: &str) -> Self {
        Self::KeyFingerprint(KeyFingerprint::of(api_key))
    }

    pub fn is_venue_issued(&self) -> bool {
        matches!(self, Self::Venue(_))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Venue(id) => id.as_str(),
            Self::KeyFingerprint(fingerprint) => fingerprint.as_str(),
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
        let VenueAccountId::KeyFingerprint(fingerprint) =
            VenueAccountId::fingerprint("test-api-key")
        else {
            panic!("a fingerprint");
        };
        assert!(hex16(fingerprint.as_str()), "{fingerprint}");
        assert!(!fingerprint.as_str().contains("test-api-key"));
        let id = VenueAccountId::KeyFingerprint(fingerprint);
        let json = serde_json::to_string(&id).expect("json");
        let back: VenueAccountId = serde_json::from_str(&json).expect("round trip");
        assert_eq!(back, id);
        let bad = r#"{"KeyFingerprint":"not-hex"}"#;
        assert!(serde_json::from_str::<VenueAccountId>(bad).is_err());
    }

    #[test]
    fn an_account_id_is_checked_when_built_and_again_on_load() {
        assert!(IssuedAccountId::new("").is_err());
        assert!(IssuedAccountId::new("has space").is_err());
        assert!(KeyFingerprint::new("short").is_err());
        for bad in [r#"{"Venue":""}"#, r#"{"Venue":"has space"}"#] {
            assert!(
                serde_json::from_str::<VenueAccountId>(bad).is_err(),
                "{bad}"
            );
        }
        assert_eq!(account_id().as_str(), "12345");
        let back: VenueAccountId =
            serde_json::from_str(r#"{"Venue":"12345"}"#).expect("an issued id");
        assert_eq!(back, account_id());
    }
}
