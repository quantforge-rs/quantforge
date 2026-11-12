//! The policy store on SQLite: profiles, each recorded once, and every
//! revision of their policies, appended and never changed. A change is one
//! insert that finds the profile at the revision before it, so of two
//! changes made from the same revision one is recorded and the other
//! changes nothing.

use rusqlite::{ErrorCode, OptionalExtension, Row, TransactionBehavior, params};

use super::SqliteStore;
use super::errors::{commit_failed, failed};
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

// Indentation inside this literal is load-bearing: SQLite records the exact
// statement text in `sqlite_master`, so reformatting it would change the
// schema of freshly created databases.
pub(super) const SCHEMA_DDL: &str = r#"
                CREATE TABLE IF NOT EXISTS automation_profiles (
                  profile_id TEXT PRIMARY KEY,
                  venue TEXT NOT NULL,
                  environment TEXT NOT NULL,
                  account_kind TEXT NOT NULL,
                  account_id TEXT NOT NULL,
                  category TEXT NOT NULL,
                  owner_tag TEXT NOT NULL UNIQUE,
                  created_at_ms INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS policy_revisions (
                  seq INTEGER PRIMARY KEY AUTOINCREMENT,
                  profile_id TEXT NOT NULL,
                  revision INTEGER NOT NULL,
                  changed_at_ms INTEGER NOT NULL,
                  policy_json TEXT NOT NULL,
                  UNIQUE (profile_id, revision)
                );
"#;

const PROFILE: &str = r#"
    SELECT profile_id, venue, environment, account_kind, account_id, category, owner_tag
    FROM automation_profiles
"#;

fn columns(row: &Row<'_>) -> rusqlite::Result<ProfileColumns> {
    Ok(ProfileColumns {
        profile_id: row.get(0)?,
        venue: row.get(1)?,
        environment: row.get(2)?,
        account_kind: row.get(3)?,
        account_id: row.get(4)?,
        category: row.get(5)?,
        owner_tag: row.get(6)?,
    })
}

/// Whether an insert into `automation_profiles` ran into the owner tag's
/// unique key.
fn tag_taken(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, Some(message))
            if failure.code == ErrorCode::ConstraintViolation
                && message.ends_with("automation_profiles.owner_tag")
    )
}

impl PolicyStore for SqliteStore {
    fn create_profile(
        &self,
        policy: &ProfilePolicy,
        created_at_ms: TimestampMs,
    ) -> Result<(), StorageError> {
        if policy.revision() != PolicyRevision::FIRST {
            return Err(not_first(policy));
        }
        let identity = policy.identity();
        let profile = identity.id();
        let asked = identity.account_ref().environment();
        let own = ProfileColumns::of(identity);
        let policy_json = policy.to_stored().map_err(StorageError::other)?;
        let mut connection = self.open()?;
        // The profile and the first revision of its policy, or neither.
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(failed)?;
        let bound: Option<String> = transaction
            .query_row(
                "SELECT environment FROM store_control WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(failed)?;
        match bound.map(|text| parse_environment(&text)).transpose() {
            Ok(Some(bound)) if bound == asked => {}
            Ok(Some(bound)) => return Err(other_environment(profile, asked, bound)),
            Ok(None) => return Err(unbound(profile)),
            Err(error) => return Err(StorageError::other(error)),
        }
        // Asked before the insert, which holds the write lock: a profile
        // recorded again is said to be so, whatever else of it is taken,
        // and not by whichever key SQLite happens to check first.
        let recorded: bool = transaction
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM automation_profiles WHERE profile_id = ?1)",
                params![own.profile_id],
                |row| row.get(0),
            )
            .map_err(failed)?;
        if recorded {
            return Err(already_recorded(profile));
        }
        let inserted = transaction.execute(
            r#"
            INSERT INTO automation_profiles(
              profile_id, venue, environment, account_kind, account_id, category, owner_tag,
              created_at_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                own.profile_id,
                own.venue,
                own.environment,
                own.account_kind,
                own.account_id,
                own.category,
                own.owner_tag,
                created_at_ms
            ],
        );
        if let Err(error) = inserted {
            return Err(if tag_taken(&error) {
                tag_in_use(identity.owner_tag())
            } else {
                failed(error)
            });
        }
        transaction
            .execute(
                r#"
                INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![
                    profile.as_str(),
                    revision_column(policy.revision()),
                    created_at_ms,
                    policy_json
                ],
            )
            .map_err(failed)?;
        transaction.commit().map_err(commit_failed)
    }

    fn profiles(&self) -> Result<Vec<ProfileIdentity>, StorageError> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(&format!("{PROFILE} ORDER BY profile_id"))
            .map_err(failed)?;
        let rows = statement.query_map([], columns).map_err(failed)?;
        let rows: Vec<ProfileColumns> = rows.collect::<Result<_, _>>().map_err(failed)?;
        rows.into_iter().map(ProfileColumns::identity).collect()
    }

    fn profile_policy(&self, profile: &ProfileId) -> Result<Option<ProfilePolicy>, StorageError> {
        let connection = self.open()?;
        let newest: Option<(i64, String)> = connection
            .query_row(
                r#"
                SELECT revision, policy_json FROM policy_revisions
                WHERE profile_id = ?1 ORDER BY revision DESC LIMIT 1
                "#,
                params![profile.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(failed)?;
        newest
            .map(|(revision, text)| stored_policy(&text, profile.as_str(), revision))
            .transpose()
    }

    fn profile_policy_at(
        &self,
        profile: &ProfileId,
        revision: PolicyRevision,
    ) -> Result<Option<ProfilePolicy>, StorageError> {
        let revision = revision_column(revision);
        let connection = self.open()?;
        let text: Option<String> = connection
            .query_row(
                "SELECT policy_json FROM policy_revisions WHERE profile_id = ?1 AND revision = ?2",
                params![profile.as_str(), revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(failed)?;
        text.map(|text| stored_policy(&text, profile.as_str(), revision))
            .transpose()
    }

    fn policy_revisions(&self, profile: &ProfileId) -> Result<Vec<PolicyStamp>, StorageError> {
        let connection = self.open()?;
        let mut statement = connection
            .prepare(
                r#"
                SELECT revision, changed_at_ms FROM policy_revisions
                WHERE profile_id = ?1 ORDER BY revision
                "#,
            )
            .map_err(failed)?;
        let rows = statement
            .query_map(params![profile.as_str()], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(failed)?;
        let rows: Vec<(i64, i64)> = rows.collect::<Result<_, _>>().map_err(failed)?;
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
        let identity = policy.identity();
        let profile = identity.id();
        let Some(before) = policy.revision().previous() else {
            return Err(first_again(profile));
        };
        let policy_json = policy.to_stored().map_err(StorageError::other)?;
        let own = ProfileColumns::of(identity);
        let connection = self.open()?;
        // One statement: it writes only where the profile is the policy's
        // own and stands at the revision before, so of two changes made
        // from one revision the second finds the profile moved on.
        let recorded = connection
            .execute(
                r#"
                INSERT INTO policy_revisions(profile_id, revision, changed_at_ms, policy_json)
                SELECT ?1, ?2, ?3, ?4
                WHERE (SELECT max(revision) FROM policy_revisions WHERE profile_id = ?1) = ?5
                  AND EXISTS (
                    SELECT 1 FROM automation_profiles
                    WHERE profile_id = ?1 AND venue = ?6 AND environment = ?7
                      AND account_kind = ?8 AND account_id = ?9 AND category = ?10
                      AND owner_tag = ?11
                  )
                "#,
                params![
                    profile.as_str(),
                    revision_column(policy.revision()),
                    changed_at_ms,
                    policy_json,
                    revision_column(before),
                    own.venue,
                    own.environment,
                    own.account_kind,
                    own.account_id,
                    own.category,
                    own.owner_tag,
                ],
            )
            .map_err(commit_failed)?;
        if recorded > 0 {
            return Ok(true);
        }
        // Nothing written: no such profile, not this profile, or a profile
        // that has moved on.
        let held: Option<ProfileColumns> = connection
            .query_row(
                &format!("{PROFILE} WHERE profile_id = ?1"),
                params![profile.as_str()],
                columns,
            )
            .optional()
            .map_err(failed)?;
        match held {
            None => Err(no_profile(profile)),
            Some(held) if held != own => Err(another_identity(profile)),
            Some(_) => Ok(false),
        }
    }
}
