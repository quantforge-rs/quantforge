//! Whose an execution record is.

use super::disagrees;
use crate::model::futures::{PositionKey, ProfileId, ProfileIdentity};
use crate::model::{
    AccountRef, Category, Environment, ExchangeId, InstrumentId, ModelError, Venue, VenueAccountId,
};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The venue, product family, environment and account a record belongs
/// to, and the profile that acts, where one does. A signal, an intent, a
/// cycle, a reservation and an incident are always a profile's; an
/// operation started by a command, and its orders, need none.
///
/// The environment is here although a store is bound to one: it is the
/// second safeguard of ADR 0002 §2, so a row never reads as another
/// environment's whatever store it ends up in.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "ScopeParts")]
pub struct ExecutionScope {
    venue: Venue,
    category: Category,
    environment: Environment,
    account: VenueAccountId,
    profile: Option<ProfileId>,
}

impl ExecutionScope {
    /// The scope of everything `profile` does.
    pub fn of_profile(profile: &ProfileIdentity) -> Self {
        let account = profile.account_ref();
        Self {
            venue: account.venue(),
            category: profile.category(),
            environment: account.environment(),
            account: account.venue_account_id().clone(),
            profile: Some(profile.id().clone()),
        }
    }

    /// The scope of an account's own operations on one product line, with
    /// no profile: what a command acts under.
    pub fn of_account(
        exchange: ExchangeId,
        environment: Environment,
        account: VenueAccountId,
    ) -> Self {
        Self {
            venue: exchange.venue(),
            category: exchange.category(),
            environment,
            account,
            profile: None,
        }
    }

    /// The scope of `profile`'s records on the position `key` names.
    pub fn of_position(key: &PositionKey, profile: &ProfileId) -> Self {
        let account = key.account();
        Self {
            venue: account.venue(),
            category: key.instrument().category(),
            environment: account.environment(),
            account: account.venue_account_id().clone(),
            profile: Some(profile.clone()),
        }
    }

    pub const fn venue(&self) -> Venue {
        self.venue
    }

    pub const fn category(&self) -> Category {
        self.category
    }

    pub const fn environment(&self) -> Environment {
        self.environment
    }

    pub fn account(&self) -> &VenueAccountId {
        &self.account
    }

    pub fn profile(&self) -> Option<&ProfileId> {
        self.profile.as_ref()
    }

    /// The product line the scope trades.
    pub const fn exchange(&self) -> ExchangeId {
        ExchangeId::from_parts(self.venue, self.category)
    }

    /// The scope with no profile: the account's, which an order, a fill,
    /// an incident and an event are known within.
    pub(super) fn of_its_account(&self) -> Self {
        Self {
            profile: None,
            ..self.clone()
        }
    }

    /// The profile, for a record that is always a profile's.
    pub(super) fn profile_of(&self, record: &'static str) -> Result<&ProfileId, ModelError> {
        self.profile
            .as_ref()
            .ok_or_else(|| disagrees(record, "it names no profile, and is always a profile's"))
    }

    /// Whether `instrument` trades on the scope's product line.
    pub fn trades(&self, instrument: &InstrumentId) -> bool {
        instrument.exchange == self.exchange()
    }

    /// Refuses an instrument of another product line than the scope's.
    pub(super) fn holding(
        &self,
        record: &'static str,
        instrument: &InstrumentId,
    ) -> Result<(), ModelError> {
        if !self.trades(instrument) {
            return Err(disagrees(
                record,
                format!("{instrument} does not trade on {}", self.exchange()),
            ));
        }
        Ok(())
    }

    /// Whether the scope is the account `key` names, on its product line.
    pub fn covers(&self, key: &PositionKey) -> bool {
        let account = key.account();
        self.trades(key.instrument())
            && self.environment == account.environment()
            && &self.account == account.venue_account_id()
    }

    /// The position `instrument` is on this scope's account.
    pub fn position_key(&self, instrument: &InstrumentId) -> Result<PositionKey, ModelError> {
        let account = AccountRef::new(self.venue, self.environment, self.account.clone())?;
        PositionKey::new(account, instrument.clone())
    }
}

impl fmt::Display for ExecutionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.exchange(), self.environment)?;
        match &self.profile {
            Some(profile) => write!(f, " profile {profile}"),
            None => Ok(()),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScopeParts {
    venue: Venue,
    category: Category,
    environment: Environment,
    account: VenueAccountId,
    profile: Option<ProfileId>,
}

impl TryFrom<ScopeParts> for ExecutionScope {
    type Error = ModelError;

    fn try_from(parts: ScopeParts) -> Result<Self, Self::Error> {
        // A profile executes external signals on futures only.
        if parts.profile.is_some() && !parts.category.is_futures() {
            return Err(disagrees(
                "execution scope",
                "it names a profile on spot, and a profile trades linear or inverse futures",
            ));
        }
        Ok(Self {
            venue: parts.venue,
            category: parts.category,
            environment: parts.environment,
            account: parts.account,
            profile: parts.profile,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::futures::OwnerTag;
    use crate::model::{Asset, Symbol};

    fn identity(venue: Venue, category: Category) -> ProfileIdentity {
        let account = VenueAccountId::issued("71002233").expect("an account id");
        ProfileIdentity::new(
            ProfileId::new("main").expect("a profile id"),
            AccountRef::new(venue, Environment::Demo, account).expect("an account"),
            category,
            OwnerTag::new("k3x9p2").expect("an owner tag"),
        )
        .expect("an identity")
    }

    fn instrument(exchange: ExchangeId) -> InstrumentId {
        InstrumentId::new(
            exchange,
            Symbol::new("BTCUSDT").expect("a symbol"),
            Asset::new("USDT").expect("an asset"),
        )
    }

    #[test]
    fn a_scope_trades_its_own_product_line_and_no_other_venues_or_familys() {
        let scope = ExecutionScope::of_profile(&identity(Venue::Bybit, Category::Linear));
        assert_eq!(scope.exchange(), ExchangeId::BybitLinear);
        assert!(scope.trades(&instrument(ExchangeId::BybitLinear)));
        for other in [
            ExchangeId::BybitInverse,
            ExchangeId::BybitSpot,
            ExchangeId::from_parts(Venue::ALL[0], Category::Linear),
        ] {
            assert!(!scope.trades(&instrument(other)), "{other}");
        }
        let key = scope
            .position_key(&instrument(ExchangeId::BybitLinear))
            .expect("a key");
        assert!(scope.covers(&key));
        let inverse = ExecutionScope::of_profile(&identity(Venue::Bybit, Category::Inverse));
        assert!(!inverse.covers(&key), "another family's scope");
        assert_eq!(
            ExecutionScope::of_position(&key, &ProfileId::new("main").expect("a profile id")),
            scope
        );
    }

    #[test]
    fn a_stored_scope_keeps_its_spelling_and_a_profile_never_trades_spot() {
        let scope = ExecutionScope::of_profile(&identity(Venue::Bybit, Category::Linear));
        let json = serde_json::to_string(&scope).expect("json");
        assert_eq!(
            json,
            r#"{"venue":"Bybit","category":"Linear","environment":"Demo","account":{"Venue":"71002233"},"profile":"main"}"#
        );
        assert_eq!(
            serde_json::from_str::<ExecutionScope>(&json).expect("read"),
            scope
        );
        let spot = json.replace("Linear", "Spot");
        assert!(serde_json::from_str::<ExecutionScope>(&spot).is_err());
        // An account's own scope names no profile, on any product line.
        let fingerprint = VenueAccountId::fingerprint("a key");
        let own =
            ExecutionScope::of_account(ExchangeId::BybitSpot, Environment::Mainnet, fingerprint);
        assert_eq!(own.profile(), None);
        let round = serde_json::to_string(&own).expect("json");
        assert_eq!(
            serde_json::from_str::<ExecutionScope>(&round).expect("read"),
            own
        );
        assert_eq!(own.to_string(), "bybit_spot mainnet");
        assert_eq!(scope.to_string(), "bybit_linear demo profile main");
    }
}
