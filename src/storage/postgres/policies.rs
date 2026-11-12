//! The policy store on PostgreSQL, with SQLite's answers: profiles, each
//! recorded once, and every revision of their policies, appended and never
//! changed. A change is one guarded insert that finds the profile at the
//! revision before it; of two made from the same revision, the unique key
//! lets one in and the other changes nothing.

use tokio_postgres::error::SqlState;
use tokio_postgres::types::{ToSql, Type};
use tokio_postgres::{IsolationLevel, Row};

use super::bridge::Effect;
use super::{PostgresStore, copying};
use crate::model::TimestampMs;
use crate::model::futures::{
    PolicyRevision, PolicyStamp, ProfileId, ProfileIdentity, ProfilePolicy,
};
use crate::ports::{PolicyStore, StorageError};
use crate::storage::policies::{
    ProfileColumns, already_recorded, another_identity, first_again, no_profile, not_first,
    other_environment, revision_column, stored_policy, stored_revision, tag_in_use, unbound,
};
use crate::storage::runs::parse_environment;

const PROFILES: &str = "SELECT profile_id, venue, environment, account_kind, account_id, \
                        category, owner_tag FROM automation_profiles ORDER BY profile_id";

const PROFILE: &str = "SELECT profile_id, venue, environment, account_kind, account_id, \
                       category, owner_tag FROM automation_profiles WHERE profile_id = $1";

const BOUND: &str = "SELECT environment FROM store_control WHERE id = 1";

const INSERT_PROFILE: &str = r#"
    INSERT INTO automation_profiles(
      profile_id, venue, environment, account_kind, account_id, category, owner_tag,
      created_at_ms
    )
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
    "#;

const INSERT_FIRST: &str = r#"
    INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json)
    VALUES ($1, $2, $3, $4)
    "#;

/// The profile's key, which an insert of a profile already recorded runs
/// into; any other unique key of the table is the owner tag's.
const PROFILE_KEY: &str = "automation_profiles_pkey";

const SAVE: &str = concat!(
    r#"
    INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json)
    SELECT $1, $2, $3, $4
    WHERE (SELECT max(revision) FROM policy_revisions WHERE profile_id = $1) = $5
      AND EXISTS (
        SELECT 1 FROM automation_profiles
        WHERE profile_id = $1 AND venue = $6 AND environment = $7 AND account_kind = $8
          AND account_id = $9 AND category = $10 AND owner_tag = $11
      )
      AND "#,
    copying::not_copying!(),
    r#"
    ON CONFLICT (profile_id, revision) DO NOTHING
    "#
);

const NEWEST: &str = "SELECT revision, policy_json FROM policy_revisions WHERE profile_id = $1 \
                      ORDER BY revision DESC LIMIT 1";

const AT: &str = "SELECT policy_json FROM policy_revisions WHERE profile_id = $1 AND revision = $2";

const STAMPS: &str = "SELECT revision, changed_at_ms FROM policy_revisions WHERE profile_id = $1 \
                      ORDER BY revision";

const NEWEST_REVISION: &str = "SELECT max(revision) FROM policy_revisions WHERE profile_id = $1";

fn columns(row: &Row) -> Result<ProfileColumns, tokio_postgres::Error> {
    Ok(ProfileColumns {
        profile_id: row.try_get(0)?,
        venue: row.try_get(1)?,
        environment: row.try_get(2)?,
        account_kind: row.try_get(3)?,
        account_id: row.try_get(4)?,
        category: row.try_get(5)?,
        owner_tag: row.try_get(6)?,
    })
}

/// The seven identity columns as a statement's parameters, in the order of
/// the table's.
fn identity_parameters(own: &ProfileColumns) -> [(&(dyn ToSql + Sync), Type); 7] {
    [
        (&own.profile_id, Type::TEXT),
        (&own.venue, Type::TEXT),
        (&own.environment, Type::TEXT),
        (&own.account_kind, Type::TEXT),
        (&own.account_id, Type::TEXT),
        (&own.category, Type::TEXT),
        (&own.owner_tag, Type::TEXT),
    ]
}

impl PolicyStore for PostgresStore {
    fn create_profile(
        &self,
        policy: &ProfilePolicy,
        created_at_ms: TimestampMs,
    ) -> Result<(), StorageError> {
        if policy.revision() != PolicyRevision::FIRST {
            return Err(not_first(policy));
        }
        let identity = policy.identity().clone();
        let own = ProfileColumns::of(&identity);
        let revision = revision_column(policy.revision());
        let policy_json = policy.to_stored().map_err(StorageError::other)?;
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let profile = identity.id();
                let asked = identity.account_ref().environment();
                // The profile and the first revision of its policy, or
                // neither: each refusal rolls the transaction back as it
                // drops.
                let transaction = client
                    .build_transaction()
                    .isolation_level(IsolationLevel::ReadCommitted)
                    .start()
                    .await?;
                let bound: Option<String> =
                    transaction.query_typed_one(BOUND, &[]).await?.try_get(0)?;
                match bound.map(|text| parse_environment(&text)).transpose() {
                    Ok(Some(bound)) if bound == asked => {}
                    Ok(Some(bound)) => return Ok(Err(other_environment(profile, asked, bound))),
                    Ok(None) => return Ok(Err(unbound(profile))),
                    Err(error) => return Ok(Err(StorageError::other(error))),
                }
                let [id, venue, environment, kind, account, category, tag] =
                    identity_parameters(&own);
                let inserted = transaction
                    .execute_typed(
                        INSERT_PROFILE,
                        &[
                            id,
                            venue,
                            environment,
                            kind,
                            account,
                            category,
                            tag,
                            (&created_at_ms, Type::INT8),
                        ],
                    )
                    .await;
                if let Err(error) = inserted {
                    if error.code() != Some(&SqlState::UNIQUE_VIOLATION) {
                        return Err(error);
                    }
                    let key = error.as_db_error().and_then(|error| error.constraint());
                    return Ok(Err(if key == Some(PROFILE_KEY) {
                        already_recorded(profile)
                    } else {
                        tag_in_use(identity.owner_tag())
                    }));
                }
                // Read once the insert holds the table.
                if let Some(refused) = copying::refusal(&*transaction).await? {
                    return Ok(Err(refused));
                }
                transaction
                    .execute_typed(
                        INSERT_FIRST,
                        &[
                            (&own.profile_id, Type::TEXT),
                            (&revision, Type::INT8),
                            (&created_at_ms, Type::INT8),
                            (&policy_json, Type::TEXT),
                        ],
                    )
                    .await?;
                transaction.commit().await?;
                Ok(Ok(()))
            })
        })?
    }

    fn profiles(&self) -> Result<Vec<ProfileIdentity>, StorageError> {
        let rows: Vec<ProfileColumns> = self.bridge.execute(Effect::Read, |client| {
            Box::pin(async move {
                client
                    .query_typed(PROFILES, &[])
                    .await?
                    .iter()
                    .map(columns)
                    .collect()
            })
        })?;
        rows.into_iter().map(ProfileColumns::identity).collect()
    }

    fn profile_policy(&self, profile: &ProfileId) -> Result<Option<ProfilePolicy>, StorageError> {
        let id = profile.as_str().to_string();
        let newest: Option<(i64, String)> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_opt(NEWEST, &[(&id, Type::TEXT)])
                    .await?
                    .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
                    .transpose()
            })
        })?;
        newest
            .map(|(revision, text)| stored_policy(&text, profile.as_str(), revision))
            .transpose()
    }

    fn profile_policy_at(
        &self,
        profile: &ProfileId,
        revision: PolicyRevision,
    ) -> Result<Option<ProfilePolicy>, StorageError> {
        let id = profile.as_str().to_string();
        let revision = revision_column(revision);
        let text: Option<String> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed_opt(AT, &[(&id, Type::TEXT), (&revision, Type::INT8)])
                    .await?
                    .map(|row| row.try_get(0))
                    .transpose()
            })
        })?;
        text.map(|text| stored_policy(&text, profile.as_str(), revision))
            .transpose()
    }

    fn policy_revisions(&self, profile: &ProfileId) -> Result<Vec<PolicyStamp>, StorageError> {
        let id = profile.as_str().to_string();
        let rows: Vec<(i64, i64)> = self.bridge.execute(Effect::Read, move |client| {
            Box::pin(async move {
                client
                    .query_typed(STAMPS, &[(&id, Type::TEXT)])
                    .await?
                    .iter()
                    .map(|row| Ok((row.try_get(0)?, row.try_get(1)?)))
                    .collect()
            })
        })?;
        rows.into_iter()
            .map(|(revision, changed_at_ms)| {
                Ok(PolicyStamp {
                    revision: stored_revision(revision)?,
                    changed_at_ms,
                })
            })
            .collect()
    }

    fn save_policy(
        &self,
        policy: &ProfilePolicy,
        changed_at_ms: TimestampMs,
    ) -> Result<bool, StorageError> {
        let profile = policy.identity().id().clone();
        let Some(before) = policy.revision().previous() else {
            return Err(first_again(&profile));
        };
        let own = ProfileColumns::of(policy.identity());
        let (revision, before) = (revision_column(policy.revision()), revision_column(before));
        let policy_json = policy.to_stored().map_err(StorageError::other)?;
        self.bridge.execute(Effect::Write, move |client| {
            Box::pin(async move {
                let [id, venue, environment, kind, account, category, tag] =
                    identity_parameters(&own);
                let recorded = client
                    .execute_typed(
                        SAVE,
                        &[
                            id,
                            (&revision, Type::INT8),
                            (&changed_at_ms, Type::INT8),
                            (&policy_json, Type::TEXT),
                            (&before, Type::INT8),
                            venue,
                            environment,
                            kind,
                            account,
                            category,
                            tag,
                        ],
                    )
                    .await?;
                if recorded > 0 {
                    return Ok(Ok(true));
                }
                // Nothing written: held off by a copy, no such profile, not
                // this profile, or a profile that has moved on.
                if let Some(refused) = copying::refusal(&**client).await? {
                    return Ok(Err(refused));
                }
                let held = client
                    .query_typed_opt(PROFILE, &[(&own.profile_id, Type::TEXT)])
                    .await?
                    .map(|row| columns(&row))
                    .transpose()?;
                match held {
                    None => return Ok(Err(no_profile(&profile))),
                    Some(held) if held != own => return Ok(Err(another_identity(&profile))),
                    Some(_) => {}
                }
                // The policy's own profile, and no copy holds the store now:
                // the profile has moved on, unless it still stands where
                // this change was made from, and a copy that has finished
                // since held the write off.
                let newest: Option<i64> = client
                    .query_typed_one(NEWEST_REVISION, &[(&own.profile_id, Type::TEXT)])
                    .await?
                    .try_get(0)?;
                Ok(if newest == Some(before) {
                    Err(copying::finished_since())
                } else {
                    Ok(false)
                })
            })
        })?
    }
}
