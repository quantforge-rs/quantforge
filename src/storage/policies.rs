//! What both backends share of the policy store: a profile's identity as
//! its columns, a stored policy read back, and the refusals, word for
//! word.

use crate::model::futures::{OwnerTag, PolicyRevision, ProfileId, ProfileIdentity, ProfilePolicy};
use crate::model::{AccountRef, Category, Environment, ModelError, Venue, VenueAccountId};
use crate::ports::StorageError;
use crate::storage::runs::parse_environment;

/// The account kinds the `account_kind` column spells, as a run's does.
const KIND_VENUE: &str = "venue";
const KIND_KEY_FINGERPRINT: &str = "key_fingerprint";

/// A profile's identity as the columns of `automation_profiles` hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileColumns {
    pub(crate) profile_id: String,
    pub(crate) venue: String,
    pub(crate) environment: String,
    pub(crate) account_kind: String,
    pub(crate) account_id: String,
    pub(crate) category: String,
    pub(crate) owner_tag: String,
}

impl ProfileColumns {
    pub(crate) fn of(identity: &ProfileIdentity) -> Self {
        let account = identity.account_ref();
        let kind = match account.venue_account_id() {
            VenueAccountId::Venue(_) => KIND_VENUE,
            VenueAccountId::KeyFingerprint(_) => KIND_KEY_FINGERPRINT,
        };
        Self {
            profile_id: identity.id().to_string(),
            venue: account.venue().as_str().to_string(),
            environment: account.environment().as_str().to_string(),
            account_kind: kind.to_string(),
            account_id: account.venue_account_id().as_str().to_string(),
            category: identity.category().as_str().to_string(),
            owner_tag: identity.owner_tag().to_string(),
        }
    }

    /// The identity the columns hold, or why they hold none a build wrote.
    pub(crate) fn identity(self) -> Result<ProfileIdentity, StorageError> {
        let unreadable = |error: ModelError| {
            StorageError::InvalidArgument(format!("a stored profile cannot be read: {error}"))
        };
        let named = |what: &'static str, value: &str, rule: &'static str| {
            unreadable(ModelError::InvalidIdentifier {
                what,
                value: value.to_string(),
                rule,
            })
        };
        let venue = Venue::ALL
            .into_iter()
            .find(|venue| venue.as_str() == self.venue)
            .ok_or_else(|| named("venue", &self.venue, "a venue this build knows"))?;
        let category = Category::ALL
            .into_iter()
            .find(|category| category.as_str() == self.category)
            .ok_or_else(|| named("category", &self.category, "spot, linear or inverse"))?;
        let environment = parse_environment(&self.environment).map_err(unreadable)?;
        let account_id = match self.account_kind.as_str() {
            KIND_VENUE => VenueAccountId::issued(self.account_id),
            KIND_KEY_FINGERPRINT => crate::model::KeyFingerprint::new(self.account_id)
                .map(VenueAccountId::KeyFingerprint),
            other => return Err(named("account kind", other, "venue or key_fingerprint")),
        }
        .map_err(unreadable)?;
        let account = AccountRef::new(venue, environment, account_id).map_err(unreadable)?;
        let id = ProfileId::new(self.profile_id).map_err(unreadable)?;
        let owner_tag = OwnerTag::new(self.owner_tag).map_err(unreadable)?;
        ProfileIdentity::new(id, account, category, owner_tag).map_err(unreadable)
    }
}

/// A policy read back from its row, held to the profile and the revision
/// the row is keyed by: a store never answers another profile's policy, or
/// another revision's, under a key.
pub(crate) fn stored_policy(
    text: &str,
    profile: &str,
    revision: i64,
) -> Result<ProfilePolicy, StorageError> {
    let unreadable = |why: String| {
        StorageError::InvalidArgument(format!(
            "the stored policy of profile {profile} at revision {revision} cannot be read: {why}"
        ))
    };
    let policy = ProfilePolicy::from_stored(text).map_err(|error| unreadable(error.to_string()))?;
    if policy.identity().id().as_str() != profile {
        return Err(unreadable(format!(
            "it is the policy of profile {}",
            policy.identity().id()
        )));
    }
    if i64::try_from(policy.revision().get()).ok() != Some(revision) {
        return Err(unreadable(format!("it is revision {}", policy.revision())));
    }
    Ok(policy)
}

/// A revision as its column holds it. Every revision fits: the model
/// refuses one a signed 64-bit column could not keep.
pub(crate) fn revision_column(revision: PolicyRevision) -> i64 {
    i64::try_from(revision.get()).unwrap_or(i64::MAX)
}

/// A revision read back from its column.
pub(crate) fn stored_revision(value: i64) -> Result<PolicyRevision, StorageError> {
    u64::try_from(value)
        .ok()
        .and_then(|value| PolicyRevision::new(value).ok())
        .ok_or_else(|| {
            StorageError::InvalidArgument(format!("stored policy revision {value} is out of range"))
        })
}

pub(crate) fn not_first(policy: &ProfilePolicy) -> StorageError {
    StorageError::InvalidArgument(format!(
        "a profile is created with revision 1 of its policy, not revision {}",
        policy.revision()
    ))
}

pub(crate) fn first_again(profile: &ProfileId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "revision 1 of profile {profile}'s policy is written when the profile is created, and \
         never again"
    ))
}

pub(crate) fn already_recorded(profile: &ProfileId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "profile {profile} is already recorded; a profile is created once"
    ))
}

pub(crate) fn tag_in_use(tag: &OwnerTag) -> StorageError {
    StorageError::InvalidArgument(format!(
        "owner tag {tag} already belongs to another profile of this store; give the new profile \
         a tag of its own"
    ))
}

pub(crate) fn unbound(profile: &ProfileId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store is bound to no environment yet, and profile {profile} needs one; bind the \
         store first"
    ))
}

pub(crate) fn other_environment(
    profile: &ProfileId,
    asked: Environment,
    bound: Environment,
) -> StorageError {
    StorageError::InvalidArgument(format!(
        "profile {profile} is for {asked}, and the store is bound to {bound}; a store holds \
         profiles of its own environment only"
    ))
}

pub(crate) fn no_profile(profile: &ProfileId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the store holds no profile {profile}; create it first"
    ))
}

pub(crate) fn another_identity(profile: &ProfileId) -> StorageError {
    StorageError::InvalidArgument(format!(
        "the policy names another account, product family or owner tag than profile {profile} \
         was created with; a profile's identity never changes"
    ))
}
