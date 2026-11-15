//! Leases and the store's standing, decided: what a backend's lease row
//! and control row mean, and what an acquisition, a renewal, a release or
//! a change of activation comes to. Both backends read their rows, ask
//! here, and write what they are told.

use crate::model::execution::{
    Fence, FenceToken, Fenced, Lease, LeaseAnswer, LeaseHolder, LeaseRequest,
};
use crate::model::futures::{ActivationEpoch, PositionKey, ProfileId};
use crate::model::{
    ControlEvent, Environment, ModelError, StoreActivation, StoreId, StoreIdentity, TimestampMs,
};
use crate::ports::StorageError;

/// What a store's control row says of its authority to execute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreStanding {
    pub(crate) store_id: StoreId,
    pub(crate) environment: Option<Environment>,
    pub(crate) activation: StoreActivation,
    pub(crate) epoch: ActivationEpoch,
}

impl StoreStanding {
    pub(crate) fn of(identity: &StoreIdentity) -> Self {
        Self {
            store_id: identity.store_id.clone(),
            environment: identity.environment,
            activation: identity.activation,
            epoch: identity.activation_epoch,
        }
    }

    fn is_active_at(&self, epoch: ActivationEpoch) -> bool {
        self.activation == StoreActivation::Active && self.epoch == epoch
    }

    /// Refuses what is not of the environment the store is bound to: a
    /// store keeps the executions of one environment, as it holds the
    /// profiles of one, and of none before it is bound. `what` names the
    /// thing asked for, such as `intent main/…`.
    pub(crate) fn keeps(
        &self,
        asked: Environment,
        what: impl std::fmt::Display,
    ) -> Result<(), StorageError> {
        match self.environment {
            Some(bound) if bound == asked => Ok(()),
            Some(bound) => Err(StorageError::InvalidArgument(format!(
                "{what} is for {asked}, and the store is bound to {bound}; a store keeps the \
                 executions of its own environment only"
            ))),
            None => Err(StorageError::InvalidArgument(format!(
                "the store is bound to no environment yet, and {what} needs one; bind the store \
                 first"
            ))),
        }
    }
}

/// A position's lease row: the last fence token issued for it, and the
/// lease held now, if one is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LeaseRow {
    pub(crate) last_fence: u64,
    pub(crate) lease: Option<Lease>,
}

/// A lease row's columns, as either backend reads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct LeaseColumns {
    pub(crate) fence: i64,
    pub(crate) holder: Option<String>,
    pub(crate) profile_id: Option<String>,
    pub(crate) activation_epoch: Option<i64>,
    pub(crate) acquired_at_ms: Option<i64>,
    pub(crate) expires_at_ms: Option<i64>,
}

fn unreadable(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidArgument(format!("a stored lease cannot be read: {error}"))
}

impl LeaseColumns {
    /// The columns of a row that holds `lease`.
    pub(crate) fn holding(lease: &Lease) -> Self {
        Self {
            fence: i64::try_from(lease.fence.get()).unwrap_or(i64::MAX),
            holder: Some(lease.holder.to_string()),
            profile_id: Some(lease.profile.to_string()),
            activation_epoch: Some(i64::from(lease.epoch.0)),
            acquired_at_ms: Some(lease.acquired_at_ms),
            expires_at_ms: Some(lease.expires_at_ms),
        }
    }

    /// The columns of a row whose lease was given back: its token stays,
    /// so the next grant takes the one after it.
    pub(crate) fn released(last_fence: u64) -> Self {
        Self {
            fence: i64::try_from(last_fence).unwrap_or(i64::MAX),
            ..Self::default()
        }
    }

    /// The row the columns hold for the position `key`.
    pub(crate) fn row(self, key: &PositionKey) -> Result<LeaseRow, StorageError> {
        let last_fence = u64::try_from(self.fence).map_err(|_| unreadable("a token below zero"))?;
        let Some(holder) = self.holder else {
            return Ok(LeaseRow {
                last_fence,
                lease: None,
            });
        };
        let parts = (
            self.profile_id,
            self.activation_epoch,
            self.acquired_at_ms,
            self.expires_at_ms,
        );
        let (Some(profile), Some(epoch), Some(acquired_at_ms), Some(expires_at_ms)) = parts else {
            return Err(unreadable("a held lease with a part missing"));
        };
        let model = |error: ModelError| unreadable(error);
        Ok(LeaseRow {
            last_fence,
            lease: Some(Lease {
                key: key.clone(),
                profile: ProfileId::new(profile).map_err(model)?,
                holder: LeaseHolder::new(holder).map_err(model)?,
                fence: FenceToken::new(last_fence).map_err(model)?,
                epoch: ActivationEpoch(
                    u32::try_from(epoch).map_err(|_| unreadable("an epoch out of range"))?,
                ),
                acquired_at_ms,
                expires_at_ms,
            }),
        })
    }
}

/// Why a unit written under `fence` is not applied, if it is not: the
/// lease is not its writer's, or the store is not active at the epoch the
/// lease was granted under.
pub(crate) fn fenced(
    fence: &Fence,
    row: Option<&LeaseRow>,
    standing: &StoreStanding,
) -> Option<Fenced> {
    let Some(lease) = row.and_then(|row| row.lease.as_ref()) else {
        return Some(Fenced::NotHeld);
    };
    if !fence.is_of(lease) {
        return Some(Fenced::Lost {
            holder: lease.holder.clone(),
            token: lease.fence,
        });
    }
    if standing.is_active_at(lease.epoch) {
        return None;
    }
    Some(Fenced::StoreNotActive {
        activation: standing.activation,
        epoch: standing.epoch,
    })
}

/// What a store that may not execute says to whoever asks it for a lease.
fn not_active(standing: &StoreStanding) -> StorageError {
    StorageError::InvalidArgument(format!(
        "store {} is {}, and only an active store grants a lease; an operator activates it once \
         no other store of its account is active",
        standing.store_id, standing.activation
    ))
}

/// The answer to `request`, and the lease to write where one is granted.
pub(crate) fn acquire(
    request: &LeaseRequest,
    row: Option<&LeaseRow>,
    standing: &StoreStanding,
) -> Result<LeaseAnswer, StorageError> {
    let position = format!("the position on {}", request.key.instrument());
    standing.keeps(request.key.account().environment(), position)?;
    if standing.activation != StoreActivation::Active {
        return Err(not_active(standing));
    }
    let current = row.and_then(|row| row.lease.as_ref());
    let last_fence = row.map_or(0, |row| row.last_fence);
    request
        .decide(current, last_fence, standing.epoch)
        .map_err(|error| StorageError::InvalidArgument(error.to_string()))
}

/// The lease `fence` names, extended, or `None` where it is not its
/// holder's to extend.
pub(crate) fn renew(
    fence: &Fence,
    row: Option<&LeaseRow>,
    standing: &StoreStanding,
    ttl_ms: i64,
    now_ms: TimestampMs,
) -> Result<Option<Lease>, StorageError> {
    if fenced(fence, row, standing).is_some() {
        return Ok(None);
    }
    let Some(lease) = row.and_then(|row| row.lease.as_ref()) else {
        return Ok(None);
    };
    let expires_at_ms = now_ms
        .checked_add(ttl_ms)
        .filter(|_| ttl_ms > 0)
        .ok_or_else(|| {
            StorageError::InvalidArgument(format!(
                "a lease lasts some time, and {ttl_ms} ms from {now_ms} is none a clock holds"
            ))
        })?;
    // The lease is its holder's whatever the clock says, until another
    // holder takes it: only its end moves.
    Ok(Some(Lease {
        expires_at_ms,
        ..lease.clone()
    }))
}

/// Whether `fence` names the lease the row holds, which is then released.
pub(crate) fn releases(fence: &Fence, row: Option<&LeaseRow>) -> bool {
    row.and_then(|row| row.lease.as_ref())
        .is_some_and(|lease| fence.is_of(lease))
}

/// What an activation runs with its change of the record: every lease
/// row gives its lease up and keeps its token, so no lease outlives the
/// epoch that granted it, and none that was written behind the store's
/// back is ever held to be its own. The same text on both backends.
pub(crate) const VOID_LEASES: &str = "UPDATE position_leases SET holder = NULL, profile_id = \
     NULL, activation_epoch = NULL, acquired_at_ms = NULL, expires_at_ms = NULL WHERE holder \
     IS NOT NULL";

/// What a change of activation comes to: the store's standing after it,
/// and the event its log keeps.
pub(crate) fn activation_change(
    standing: &StoreStanding,
    to: StoreActivation,
    at_ms: TimestampMs,
) -> Result<(StoreStanding, ControlEvent), StorageError> {
    let refuse = |message: String| Err(StorageError::InvalidArgument(message));
    let id = &standing.store_id;
    match (standing.activation, to) {
        (StoreActivation::Active, StoreActivation::Active) => {
            return refuse(format!(
                "store {id} is already active, at epoch {}; each activation is a new epoch, so \
                 it is made once",
                standing.epoch.0
            ));
        }
        (from, StoreActivation::Deactivated) if from != StoreActivation::Active => {
            return refuse(format!(
                "store {id} is {from}, and only an active store is deactivated"
            ));
        }
        _ => {}
    }
    standing
        .activation
        .move_to(to)
        .map_err(|error| StorageError::InvalidArgument(error.to_string()))?;
    if to == StoreActivation::Active && standing.environment.is_none() {
        return refuse(format!(
            "store {id} is bound to no environment yet; bind it before it is activated"
        ));
    }
    let epoch = match to {
        StoreActivation::Active => {
            ActivationEpoch(standing.epoch.0.checked_add(1).ok_or_else(|| {
                StorageError::InvalidArgument(format!("store {id} has no epoch left"))
            })?)
        }
        _ => standing.epoch,
    };
    let event = ControlEvent {
        from: standing.activation,
        to,
        epoch,
        at_ms,
    };
    let after = StoreStanding {
        activation: to,
        epoch,
        ..standing.clone()
    };
    Ok((after, event))
}

#[cfg(test)]
mod tests;
