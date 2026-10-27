//! The store itself (ADR 0002 §2 and §15): its identity, the environment it
//! is bound to, the activation that alone gives it execution authority, the
//! identity every journal carries, and the log of the schema migrations it
//! went through. The control contract these types spell out is
//! `docs/storage-schema.md`: QUA-37 defines it and records the migration
//! defaults, ALG-738 persists its transitions, ALG-767 enforces them and
//! ALG-768 verifies a transfer. Nothing here moves a store between its
//! activation states yet.

use super::futures::ActivationEpoch;
use super::identity::text_id;
use super::{
    Environment, ExecutionMode, MarketId, ModelError, RunStatus, TimestampMs, VenueAccountId,
};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Whether `text` is exactly 32 lowercase hex digits.
fn hex32(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

text_id!(
    /// A store's identity: a random UUID without hyphens, assigned when the
    /// store is created or first migrated to schema 5. A copy of the file
    /// keeps it, which is what makes a copy recognizable; a transfer to
    /// another backend makes a new one.
    StoreId,
    "store id",
    "32 lowercase hex digits",
    hex32
);

impl StoreId {
    /// A fresh random store ID.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }
}

/// Whether a store may execute. Activation is an operator's action on one
/// store, never a row that a copy or a transfer carries over: a copy of an
/// active store is activated again, with a new epoch, after its source was
/// deactivated, and a transferred store starts `inactive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoreActivation {
    /// Never activated, or transferred: the migration default.
    Inactive,
    /// The one store its account's automation executes from.
    Active,
    /// Handed over to another store; it cannot arm again until reactivated.
    Deactivated,
}

impl StoreActivation {
    pub const ALL: [Self; 3] = [Self::Inactive, Self::Active, Self::Deactivated];
    /// The migration default, and the state of every transferred store.
    pub const INITIAL: Self = Self::Inactive;
    /// The transition table: activation, handover, and reactivation after a
    /// rollback. Every move changes the epoch the store's orders carry.
    pub const TRANSITIONS: [(Self, Self); 3] = [
        (Self::Inactive, Self::Active),
        (Self::Active, Self::Deactivated),
        (Self::Deactivated, Self::Active),
    ];

    /// `inactive`, `active` or `deactivated`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Active => "active",
            Self::Deactivated => "deactivated",
        }
    }

    /// No state is final: a store can always be handed over or taken back.
    pub const fn is_terminal(self) -> bool {
        false
    }

    /// Whether [`TRANSITIONS`](Self::TRANSITIONS) has the move.
    pub fn can_move_to(self, to: Self) -> bool {
        Self::TRANSITIONS.contains(&(self, to))
    }

    /// The state after a transition, or why it is refused.
    pub fn move_to(self, to: Self) -> Result<Self, ModelError> {
        if !self.can_move_to(to) {
            return Err(ModelError::InvalidTransition {
                machine: "store",
                from: self.as_str(),
                to: to.as_str(),
            });
        }
        Ok(to)
    }
}

impl fmt::Display for StoreActivation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StoreActivation {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|state| state.as_str() == value)
            .ok_or_else(|| ModelError::InvalidIdentifier {
                what: "store activation",
                value: value.to_string(),
                rule: "inactive, active or deactivated",
            })
    }
}

/// What a store knows about itself: its ID, the schema it is at, the
/// environment it is bound to, declared once by its operator and never
/// inferred from a URL, and its activation with the epoch of its last one.
/// Epoch 0 means never activated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreIdentity {
    pub store_id: StoreId,
    pub schema_version: u32,
    pub environment: Option<Environment>,
    pub activation: StoreActivation,
    pub activation_epoch: ActivationEpoch,
}

/// One step of the schema chain a store went through, as its log records
/// it. A store created at the current schema records one step from 0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaMigration {
    pub from_version: u32,
    pub to_version: u32,
    pub applied_at_ms: TimestampMs,
    /// The QuantForge version that applied it.
    pub quantforge_version: String,
}

/// Where a journal's identity came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IdentitySource {
    /// Written by a build before schema 5, which recorded no environment
    /// and no account. Nothing is guessed for it: it is driven again only
    /// once an operator adopts it.
    Legacy,
    /// Recorded by the process that started the run.
    Recorded,
    /// Given by an operator to a legacy run.
    Adopted,
}

impl IdentitySource {
    pub const ALL: [Self; 3] = [Self::Legacy, Self::Recorded, Self::Adopted];

    /// `legacy`, `recorded` or `adopted`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Recorded => "recorded",
            Self::Adopted => "adopted",
        }
    }
}

impl fmt::Display for IdentitySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for IdentitySource {
    type Err = ModelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|source| source.as_str() == value)
            .ok_or_else(|| ModelError::InvalidIdentifier {
                what: "identity source",
                value: value.to_string(),
                rule: "legacy, recorded or adopted",
            })
    }
}

/// The identity a journal carries beside its state: the store's environment
/// when the run started, and the account that traded it. A dry run has no
/// account. For a live Spot run the account is the fingerprint of the API
/// key in use on every environment, which identifies the key, not the
/// account: two keys of one account read as two accounts, and nothing arms
/// on it (ADR 0002 §2; ALG-759 brings the venue-issued IDs).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIdentity {
    pub environment: Option<Environment>,
    pub account: Option<VenueAccountId>,
    pub source: IdentitySource,
}

impl RunIdentity {
    /// The identity of a run a build before schema 5 wrote.
    pub const fn legacy() -> Self {
        Self {
            environment: None,
            account: None,
            source: IdentitySource::Legacy,
        }
    }

    /// The identity the process starting a run records.
    pub const fn recorded(
        environment: Option<Environment>,
        account: Option<VenueAccountId>,
    ) -> Self {
        Self {
            environment,
            account,
            source: IdentitySource::Recorded,
        }
    }

    /// The identity an operator gives a legacy run.
    pub const fn adopted(
        environment: Option<Environment>,
        account: Option<VenueAccountId>,
    ) -> Self {
        Self {
            environment,
            account,
            source: IdentitySource::Adopted,
        }
    }

    /// Whether an invocation with `current` may drive a run recorded with
    /// this identity: never a legacy run, and only the same environment
    /// and the same account, an unbound store and no account included. A
    /// run never inherits a newly selected environment or key silently.
    pub fn matches(&self, current: &Self) -> Result<(), ModelError> {
        if self.source == IdentitySource::Legacy {
            return Err(ModelError::UnverifiedIdentity);
        }
        if self.environment != current.environment {
            return Err(ModelError::IdentityMismatch {
                subject: "environment",
                recorded: describe_environment(self.environment),
                current: describe_environment(current.environment),
            });
        }
        if self.account != current.account {
            return Err(ModelError::IdentityMismatch {
                subject: "account",
                recorded: describe_account(self.account.as_ref()),
                current: describe_account(current.account.as_ref()),
            });
        }
        Ok(())
    }
}

/// `mainnet`, or `undeclared` for a store bound to no environment.
pub fn describe_environment(environment: Option<Environment>) -> String {
    environment.map_or_else(|| "undeclared".to_string(), |env| env.to_string())
}

/// `venue:<id>`, `key_fingerprint:<hex>`, or `none` for a dry run.
pub fn describe_account(account: Option<&VenueAccountId>) -> String {
    match account {
        None => "none".to_string(),
        Some(VenueAccountId::Venue(id)) => format!("venue:{id}"),
        Some(VenueAccountId::KeyFingerprint(fingerprint)) => {
            format!("key_fingerprint:{fingerprint}")
        }
    }
}

/// A journal the migration found without a verified identity, as `store
/// status` lists it. The execution mode is `None` for a run QuantForge
/// 0.2.0 wrote, which did not record it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LegacyRun {
    pub run_id: String,
    pub market: MarketId,
    pub status: RunStatus,
    pub execution_mode: Option<ExecutionMode>,
    pub started_at_ms: TimestampMs,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KeyFingerprint;

    #[test]
    fn a_store_activates_hands_over_and_reactivates_only_in_that_order() {
        let allowed = [
            (StoreActivation::Inactive, StoreActivation::Active),
            (StoreActivation::Active, StoreActivation::Deactivated),
            (StoreActivation::Deactivated, StoreActivation::Active),
        ];
        for from in StoreActivation::ALL {
            for to in StoreActivation::ALL {
                let expected = allowed.contains(&(from, to));
                assert_eq!(from.can_move_to(to), expected, "{from} -> {to}");
                assert_eq!(from.move_to(to).is_ok(), expected, "{from} -> {to}");
            }
        }
        let error = StoreActivation::Inactive
            .move_to(StoreActivation::Deactivated)
            .expect_err("a handover needs an active store");
        assert_eq!(
            error.to_string(),
            "store cannot move from inactive to deactivated"
        );
        for state in StoreActivation::ALL {
            assert_eq!(state.as_str().parse::<StoreActivation>().ok(), Some(state));
        }
        assert!("armed".parse::<StoreActivation>().is_err());
    }

    #[test]
    fn store_ids_are_uuids_without_hyphens() {
        let generated = StoreId::generate();
        assert!(hex32(generated.as_str()), "{generated}");
        assert!(StoreId::new("0123456789abcdef0123456789abcdef").is_ok());
        for bad in ["", "0123456789ABCDEF0123456789ABCDEF", "0123456789abcdef"] {
            assert!(StoreId::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_run_is_driven_only_by_the_identity_that_recorded_it() {
        let key = VenueAccountId::KeyFingerprint(KeyFingerprint::of("k"));
        let other = VenueAccountId::KeyFingerprint(KeyFingerprint::of("other"));
        let recorded = RunIdentity::recorded(Some(Environment::Testnet), Some(key.clone()));
        assert!(recorded.matches(&recorded).is_ok());
        let unbound = RunIdentity::recorded(None, None);
        assert!(unbound.matches(&unbound).is_ok());

        let legacy = RunIdentity::legacy();
        let error = legacy.matches(&recorded).expect_err("legacy");
        assert!(error.to_string().contains("never verified"), "{error}");

        let rebound = RunIdentity::recorded(Some(Environment::Testnet), None);
        let error = unbound.matches(&rebound).expect_err("environment");
        assert_eq!(
            error.to_string(),
            "environment was recorded as undeclared, but this invocation is testnet"
        );
        let other_key = RunIdentity::recorded(Some(Environment::Testnet), Some(other));
        let error = recorded.matches(&other_key).expect_err("account");
        assert!(
            error
                .to_string()
                .starts_with("account was recorded as key_fingerprint:"),
            "{error}"
        );
        let adopted = RunIdentity::adopted(Some(Environment::Testnet), Some(key));
        assert!(adopted.matches(&recorded).is_ok());
        for source in IdentitySource::ALL {
            assert_eq!(source.as_str().parse::<IdentitySource>().ok(), Some(source));
        }
    }
}
