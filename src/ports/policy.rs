//! The policy port: a store's automation profiles and every revision of
//! their policies (ADR 0002 §5, §6). `docs/storage-schema.md` is the schema
//! and `docs/storage-conformance.md` the behavior every backend passes.
//!
//! A profile is recorded once, with the first revision of its policy, and
//! its identity never changes. Each change of its policy is a new
//! revision, recorded beside the earlier ones and never changed again, so
//! the revision an entry was admitted under can be read back for good.
//! A store never reads a policy's settings: what they mean is the
//! model's, in [`crate::model::futures::ProfilePolicy`].

use super::StorageError;
use crate::model::TimestampMs;
use crate::model::futures::{
    PolicyRevision, PolicyStamp, ProfileId, ProfileIdentity, ProfilePolicy,
};

pub trait PolicyStore: Send + Sync {
    /// Records a new profile with `policy` as the first revision of its
    /// policy, both or neither. Refuses a policy that is not revision 1, a
    /// profile ID or an owner tag the store already holds, a store bound to
    /// no environment yet, and a profile for another environment than the
    /// store's.
    fn create_profile(
        &self,
        policy: &ProfilePolicy,
        created_at_ms: TimestampMs,
    ) -> Result<(), StorageError>;

    /// Every profile's identity, in the byte order of their IDs.
    fn profiles(&self) -> Result<Vec<ProfileIdentity>, StorageError>;

    /// The newest revision of the profile's policy, or `None` when the
    /// store holds no such profile.
    fn profile_policy(&self, profile: &ProfileId) -> Result<Option<ProfilePolicy>, StorageError>;

    /// The profile's policy as it stood at `revision`, or `None` when the
    /// store holds no such profile or revision.
    fn profile_policy_at(
        &self,
        profile: &ProfileId,
        revision: PolicyRevision,
    ) -> Result<Option<ProfilePolicy>, StorageError>;

    /// When each revision of the profile's policy was made, oldest first;
    /// empty when the store holds no such profile.
    fn policy_revisions(&self, profile: &ProfileId) -> Result<Vec<PolicyStamp>, StorageError>;

    /// Records `policy` as the revision after the profile's newest, and
    /// answers `true`. Answers `false`, having written nothing, when the
    /// profile's newest revision is not the one before `policy`'s: another
    /// change came first, and the caller reads the policy again. Refuses a
    /// first revision, which only [`PolicyStore::create_profile`] writes, a
    /// profile the store does not hold, and a policy whose identity is not
    /// the profile's.
    fn save_policy(
        &self,
        policy: &ProfilePolicy,
        changed_at_ms: TimestampMs,
    ) -> Result<bool, StorageError>;
}
