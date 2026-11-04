//! The identity columns of a run (`environment`, `account_kind`,
//! `account_id`, `identity_source`) read back from a row; the mapping
//! itself is shared with every backend (`storage::runs`).

use super::super::to_from_sql_error;
use crate::model::RunIdentity;
use crate::storage::runs::identity_from_columns;
use rusqlite::Row;

/// The identity in the columns `environment`, `account_kind`, `account_id`
/// and `identity_source`, read at `offset` of `row`.
pub(super) fn decode_identity(
    row: &Row<'_>,
    offset: usize,
) -> Result<RunIdentity, rusqlite::Error> {
    let environment: Option<String> = row.get(offset)?;
    let kind: Option<String> = row.get(offset + 1)?;
    let id: Option<String> = row.get(offset + 2)?;
    let source: String = row.get(offset + 3)?;
    identity_from_columns(
        environment.as_deref(),
        kind.as_deref(),
        id.as_deref(),
        &source,
    )
    .map_err(to_from_sql_error)
}
