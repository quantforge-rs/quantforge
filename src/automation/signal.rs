//! The signal service: what a signal asks of the profile its webhook URL
//! selected, held to that profile's newest policy and to what its venue
//! lists now.
//!
//! - **The URL chooses the profile; the signal never does.** A signal for
//!   another profile, or one that asserts another exchange or family, is
//!   refused before anything else is read.
//! - **What needs no venue is decided first.** A stale or misaddressed
//!   signal is refused without a request to the venue, so a venue that is
//!   down delays an admission and never changes one.
//! - **One revision answers.** The ticker is resolved through the same
//!   revision of the policy an entry is admitted under.
//! - **A refusal is an outcome.** It is recorded with the signal, and the
//!   same signal is refused the same way again; only a store or a venue
//!   that could not be read is an error to retry.

use std::fmt;

use thiserror::Error;

use super::policy::{PolicyError, PolicyService};
use crate::model::futures::{
    ExternalSignal, ProfileId, ResolvedSignal, SignalAgeLimit, SignalRefusal,
};
use crate::ports::futures::InstrumentConnector;
use crate::ports::{Clock, PolicyStore};

/// Why a signal was not resolved.
#[derive(Debug, Error)]
pub enum SignalError {
    /// The signal is refused: what it is recorded with, for good.
    #[error("{0}")]
    Refused(SignalRefusal),

    /// The profile's policy or its venue's list could not be read, or the
    /// URL selects a profile the store does not hold. Nothing was decided.
    #[error(transparent)]
    Policy(#[from] PolicyError),
}

/// The signal service over one store.
pub struct SignalService<'a> {
    policies: PolicyService<'a>,
    clock: &'a dyn Clock,
}

impl fmt::Debug for SignalService<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalService").finish_non_exhaustive()
    }
}

impl<'a> SignalService<'a> {
    pub fn new(store: &'a dyn PolicyStore, clock: &'a dyn Clock) -> Self {
        Self {
            policies: PolicyService::new(store, clock),
            clock,
        }
    }

    /// What `signal` asks of `route`, the profile its webhook URL selected:
    /// the entry or the exit a store records, an entry with the policy it
    /// is admitted under, or the refusal it is recorded with
    /// ([`ExternalSignal::resolve`] lists the rules, in their order).
    ///
    /// `venue` is the connector of the profile's own product line and
    /// environment, and `limit` the profile's age limit. The clock is read
    /// once, so an entry is not found timely and then stale.
    pub async fn resolve(
        &self,
        route: &ProfileId,
        signal: &ExternalSignal,
        venue: &dyn InstrumentConnector,
        limit: SignalAgeLimit,
    ) -> Result<ResolvedSignal, SignalError> {
        let policy = self.policies.policy(route)?;
        let now_ms = self.clock.now_ms();
        signal
            .addressed_to(policy.identity())
            .map_err(SignalError::Refused)?;
        signal.timely(now_ms, limit).map_err(SignalError::Refused)?;
        let listed = self.policies.listed(&policy, venue).await?;
        signal
            .resolve(&policy, &listed, now_ms, limit)
            .map_err(SignalError::Refused)
    }
}
