//! The identity columns of a run (`environment`, `account_kind`,
//! `account_id`, `identity_source`): how a `RunIdentity` is written to
//! them and read back.

use super::super::to_from_sql_error;
use crate::model::{Environment, IdentitySource, KeyFingerprint, RunIdentity, VenueAccountId};
use crate::{ModelError, StorageError};
use rusqlite::Row;
use std::str::FromStr;

/// `mainnet`, `testnet` or `demo`, as the columns spell it.
pub(in crate::storage::sqlite) fn parse_environment(text: &str) -> Result<Environment, ModelError> {
    Environment::ALL
        .into_iter()
        .find(|environment| environment.as_str() == text)
        .ok_or_else(|| ModelError::InvalidIdentifier {
            what: "environment",
            value: text.to_string(),
            rule: "mainnet, testnet or demo",
        })
}

/// The account kinds the `account_kind` column spells.
const KIND_VENUE: &str = "venue";
const KIND_KEY_FINGERPRINT: &str = "key_fingerprint";

/// An identity as its three columns: environment, account kind, account.
pub(in crate::storage::sqlite) fn identity_columns(
    identity: &RunIdentity,
) -> (Option<&'static str>, Option<&'static str>, Option<&str>) {
    let environment = identity.environment.map(Environment::as_str);
    let (kind, id) = match &identity.account {
        None => (None, None),
        Some(VenueAccountId::Venue(id)) => (Some(KIND_VENUE), Some(id.as_str())),
        Some(VenueAccountId::KeyFingerprint(fingerprint)) => {
            (Some(KIND_KEY_FINGERPRINT), Some(fingerprint.as_str()))
        }
    };
    (environment, kind, id)
}

/// The identity in the columns `environment`, `account_kind`, `account_id`
/// and `identity_source`, read at `offset` of `row`.
pub(super) fn decode_identity(
    row: &Row<'_>,
    offset: usize,
) -> Result<RunIdentity, rusqlite::Error> {
    let environment = row
        .get::<_, Option<String>>(offset)?
        .map(|text| parse_environment(&text))
        .transpose()
        .map_err(to_from_sql_error)?;
    let kind: Option<String> = row.get(offset + 1)?;
    let id: Option<String> = row.get(offset + 2)?;
    let account = match (kind.as_deref(), id) {
        (None, None) => None,
        (Some(KIND_VENUE), Some(id)) => {
            Some(VenueAccountId::issued(id).map_err(to_from_sql_error)?)
        }
        (Some(KIND_KEY_FINGERPRINT), Some(id)) => Some(VenueAccountId::KeyFingerprint(
            KeyFingerprint::new(id).map_err(to_from_sql_error)?,
        )),
        (kind, id) => {
            return Err(to_from_sql_error(StorageError::InvalidArgument(format!(
                "account kind {kind:?} with account {id:?} is not an identity"
            ))));
        }
    };
    let source =
        IdentitySource::from_str(&row.get::<_, String>(offset + 3)?).map_err(to_from_sql_error)?;
    Ok(RunIdentity {
        environment,
        account,
        source,
    })
}
