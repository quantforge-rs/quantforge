//! Resuming an intent whose outcome is open: the venue is asked for the
//! attempt that was recorded, and nothing is sent.

use super::{Evidence, ExecutionLedger, LedgerError};
use crate::model::ModelError;
use crate::model::execution::{Fence, IntentRecord, Stored};
use crate::model::futures::{
    FuturesFill, IntentKind, IntentState, NotPlacedEvidence, OrderLookup, TimeWindow,
};
use crate::ports::futures::{AccountStateConnector, OrderQueryConnector};

/// What asking the venue came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumed {
    /// The venue's evidence settled the intent, or moved it on: the
    /// record as it stands now.
    Settled(Stored<IntentRecord>),
    /// Nothing proves what became of the attempt yet. It is asked for
    /// again later, and never sent again meanwhile.
    Unknown(Stored<IntentRecord>),
    /// An exit whose attempt the venue answered for and does not know. A
    /// close can only reduce, so it may be sent again, as a new attempt
    /// recorded first. An entry never comes to this.
    MayRetry(Stored<IntentRecord>),
}

impl Resumed {
    /// The intent's record as it stands.
    pub fn intent(&self) -> &Stored<IntentRecord> {
        match self {
            Self::Settled(intent) | Self::Unknown(intent) | Self::MayRetry(intent) => intent,
        }
    }
}

impl ExecutionLedger<'_> {
    /// Finds out what became of `intent`'s latest attempt, under the
    /// position's lease, by asking the venue for the client order ID that
    /// was recorded before the attempt was sent. This is what a process
    /// does for every unsettled intent it finds when it starts, and
    /// before any retry. **It never sends an order.**
    ///
    /// - The venue knows the order: it is settled by what the venue
    ///   reports, with the fills it lists for it.
    /// - The venue does not know it: an attempt found `submitting` is
    ///   recorded as of unknown outcome first, since nobody knows whether
    ///   its request left. An entry is then never placed only on ADR 0002
    ///   §9's proof: the propagation delay passed, a verified lookup inside
    ///   the venue's retention, and a position snapshot from after it that
    ///   is still flat. Short of that it stays unknown, blocking its
    ///   position, and is asked for again. An exit may be sent again.
    pub async fn resume(
        &self,
        fence: &Fence,
        intent: &Stored<IntentRecord>,
        orders: &dyn OrderQueryConnector,
        account: &dyn AccountStateConnector,
    ) -> Result<Resumed, LedgerError> {
        let open =
            |state: IntentState| state.is_unresolved() || state == IntentState::PartiallyFilled;
        let record = &intent.record;
        if !open(record.intent().state()) {
            return Ok(Resumed::Settled(intent.clone()));
        }
        let Some(attempt) = record.intent().attempts().last().cloned() else {
            return Ok(Resumed::Settled(intent.clone()));
        };
        let instrument = record.key().instrument().clone();
        let id = &attempt.client_order_id;
        let queried_at_ms = match orders.order(&instrument, id).await? {
            OrderLookup::Found(order) => {
                let window = TimeWindow::new(
                    attempt.sent_at_ms,
                    self.clock.now_ms().max(attempt.sent_at_ms),
                )?;
                let fills: Vec<FuturesFill> = orders
                    .fills(&instrument, window)
                    .await?
                    .into_iter()
                    .filter(|fill| fill.client_order_id().as_ref() == Some(id))
                    .collect();
                let evidence = Evidence::Order {
                    order: &order,
                    fills: &fills,
                };
                let settled = self.settle(fence, intent, evidence)?;
                return Ok(if settled.record.intent().state().is_unresolved() {
                    Resumed::Unknown(settled)
                } else {
                    Resumed::Settled(settled)
                });
            }
            OrderLookup::NotFound { queried_at_ms } => queried_at_ms,
        };
        // The venue reported fills of it before: that it lists the order
        // no longer says nothing new, and proves nothing.
        if record.intent().state() == IntentState::PartiallyFilled {
            return Ok(Resumed::Unknown(intent.clone()));
        }
        // Recorded, and the venue does not know it: whether the request
        // ever left is unknown, and is recorded as that before anything
        // else is concluded.
        let unknown = if record.intent().state() == IntentState::Submitting {
            self.settle(fence, intent, Evidence::Unknown)?
        } else {
            intent.clone()
        };
        if matches!(record.intent().kind(), IntentKind::Exit(_)) {
            return Ok(Resumed::MayRetry(unknown));
        }
        let proof = NotPlacedEvidence {
            client_order_id: id.clone(),
            queried_at_ms,
            guarantee: orders.capabilities().order_lookup,
            position: account.position(&instrument).await?,
        };
        match self.settle(fence, &unknown, Evidence::NotPlaced(&proof)) {
            Ok(settled) => Ok(Resumed::Settled(settled)),
            // Not proven yet: the entry stays unknown, and is not sent.
            Err(LedgerError::Model(ModelError::MissingEvidence { .. })) => {
                Ok(Resumed::Unknown(unknown))
            }
            Err(error) => Err(error),
        }
    }
}
