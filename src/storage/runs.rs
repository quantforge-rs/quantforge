//! What every backend's run journal and store record share, so both apply
//! one set of rules with one set of messages: a run's identity as its four
//! columns, its state from `state_json`, the refusals of the journal, a
//! legacy run as `StoreControl::legacy_runs` lists it, and the state an
//! adoption writes.

use std::str::FromStr;

use crate::model::{
    Environment, ExchangeId, ExecutionMode, IdentitySource, Interval, KeyFingerprint, LegacyRun,
    MarketId, RunIdentity, RunStatus, StoreId, Symbol, VenueAccountId,
};
use crate::{BotRunState, ModelError, StorageError};

/// `mainnet`, `testnet` or `demo`, as the columns spell it.
pub(crate) fn parse_environment(text: &str) -> Result<Environment, ModelError> {
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
pub(crate) fn identity_columns(
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
/// and `identity_source`.
pub(crate) fn identity_from_columns(
    environment: Option<&str>,
    kind: Option<&str>,
    id: Option<&str>,
    source: &str,
) -> Result<RunIdentity, StorageError> {
    let invalid = |error: ModelError| StorageError::InvalidArgument(error.to_string());
    let environment = environment
        .map(parse_environment)
        .transpose()
        .map_err(invalid)?;
    let account = match (kind, id) {
        (None, None) => None,
        (Some(KIND_VENUE), Some(id)) => Some(VenueAccountId::issued(id).map_err(invalid)?),
        (Some(KIND_KEY_FINGERPRINT), Some(id)) => Some(VenueAccountId::KeyFingerprint(
            KeyFingerprint::new(id).map_err(invalid)?,
        )),
        (kind, id) => {
            return Err(StorageError::InvalidArgument(format!(
                "account kind {kind:?} with account {id:?} is not an identity"
            )));
        }
    };
    let source = IdentitySource::from_str(source).map_err(invalid)?;
    Ok(RunIdentity {
        environment,
        account,
        source,
    })
}

/// A run's state from its JSON. A run QuantForge 0.2.0 wrote recorded no
/// execution mode, and loads only once an operator has stated it.
pub(crate) fn decode_state(run_id: &str, json: &str) -> Result<BotRunState, StorageError> {
    match serde_json::from_str::<BotRunState>(json) {
        Ok(state) => Ok(state),
        Err(err) => {
            let value: serde_json::Value =
                serde_json::from_str(json).map_err(StorageError::other)?;
            if value.get("execution_mode").is_none() {
                return Err(StorageError::InvalidArgument(format!(
                    "run {run_id} was recorded by QuantForge 0.2.0 without its execution mode; \
                     adopt it with `quantforge store adopt-run --run-id {run_id} --execution-mode \
                     <dry-run|live> --yes`, or start a new run"
                )));
            }
            Err(StorageError::other(err))
        }
    }
}

/// A new run is never legacy: only a migration marks runs so.
pub(crate) fn refuse_legacy_creation(
    state: &BotRunState,
    identity: &RunIdentity,
) -> Result<(), StorageError> {
    if identity.source == IdentitySource::Legacy {
        return Err(StorageError::InvalidArgument(format!(
            "run {} cannot be created as a legacy run; only a migration marks runs legacy",
            state.run_id
        )));
    }
    Ok(())
}

/// A run is created once.
pub(crate) fn already_exists(run_id: &str) -> StorageError {
    StorageError::InvalidArgument(format!(
        "run {run_id} already exists; resume it with --run-id, or start a new run"
    ))
}

/// A run is saved only after it is created.
pub(crate) fn never_created(run_id: &str) -> StorageError {
    StorageError::InvalidArgument(format!(
        "run {run_id} was never created; a run is created with its identity before it is saved"
    ))
}

/// A store keeps the environment it was bound to.
pub(crate) fn already_bound(store_id: &StoreId, bound: Environment) -> StorageError {
    StorageError::InvalidArgument(format!(
        "store {store_id} is bound to {bound}; a store keeps its environment for life"
    ))
}

/// A legacy run from its row, as `StoreControl::legacy_runs` lists it.
pub(crate) fn legacy_run(
    run_id: String,
    exchange: &str,
    symbol: &str,
    interval: &str,
    state_json: &str,
    started_at_ms: i64,
) -> Result<LegacyRun, StorageError> {
    let invalid = |error: ModelError| StorageError::InvalidArgument(error.to_string());
    let exchange = ExchangeId::from_storage_name(exchange).ok_or_else(|| {
        StorageError::InvalidArgument(format!(
            "run {run_id} is stored under unknown exchange {exchange:?}"
        ))
    })?;
    let symbol = Symbol::new(symbol).map_err(invalid)?;
    let interval = Interval::from_str(interval).map_err(invalid)?;
    let state: serde_json::Value = serde_json::from_str(state_json).map_err(StorageError::other)?;
    let status: RunStatus =
        serde_json::from_value(state["status"].clone()).map_err(StorageError::other)?;
    let execution_mode = match state.get("execution_mode") {
        None => None,
        Some(value) => Some(
            serde_json::from_value::<ExecutionMode>(value.clone()).map_err(StorageError::other)?,
        ),
    };
    Ok(LegacyRun {
        run_id,
        market: MarketId::new(exchange, symbol, interval),
        status,
        execution_mode,
        started_at_ms,
    })
}

/// Only an adopted identity adopts a run.
pub(crate) fn refuse_unadopted(identity: &RunIdentity) -> Result<(), StorageError> {
    if identity.source != IdentitySource::Adopted {
        return Err(StorageError::InvalidArgument(format!(
            "an adopted run's identity source is adopted, not {}",
            identity.source
        )));
    }
    Ok(())
}

/// The state an adoption writes for run `run_id`, found with `source` and
/// `state_json`, or `None` for a run that does not exist: a legacy run
/// only; the operator's mode only for a run that recorded none, and
/// required for it; and a record that loads as a run, or nothing is
/// written.
pub(crate) fn adopted_state(
    run_id: &str,
    found: Option<(String, String)>,
    execution_mode: Option<ExecutionMode>,
) -> Result<String, StorageError> {
    let Some((source, state_json)) = found else {
        return Err(StorageError::InvalidArgument(format!(
            "no run found for run_id={run_id}"
        )));
    };
    if source != IdentitySource::Legacy.as_str() {
        return Err(StorageError::InvalidArgument(format!(
            "run {run_id} already has a {source} identity; only a legacy run is adopted"
        )));
    }
    let mut state: serde_json::Value =
        serde_json::from_str(&state_json).map_err(StorageError::other)?;
    match (state.get("execution_mode").is_some(), execution_mode) {
        (true, Some(_)) => {
            return Err(StorageError::InvalidArgument(format!(
                "run {run_id} already records its execution mode"
            )));
        }
        (false, None) => {
            return Err(StorageError::InvalidArgument(format!(
                "run {run_id} recorded no execution mode; the operator states it"
            )));
        }
        (false, Some(mode)) => {
            let token = serde_json::to_value(mode).map_err(StorageError::other)?;
            state["execution_mode"] = token;
        }
        (true, None) => {}
    }
    // The adopted record must load as a run, or the adoption is refused
    // before anything is written.
    serde_json::from_value::<BotRunState>(state.clone()).map_err(|err| {
        StorageError::InvalidArgument(format!(
            "run {run_id} cannot be adopted: its state does not load ({err})"
        ))
    })?;
    serde_json::to_string(&state).map_err(StorageError::other)
}
