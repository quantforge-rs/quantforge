//! An intent's orders: the attempt recorded before it is sent, and the
//! venue's evidence that settles it.

use super::{ExecutionLedger, LedgerError};
use crate::model::execution::{
    Change, CycleRecord, EventKey, EventKind, ExecutionUnit, Fence, FillRecord, IntentRecord,
    OrderEvidence, OrderRecord, OrderRefusal, OutboxEvent, PoolGuard, Record, ReleaseReason,
    Reservation, Stale, Stored,
};
use crate::model::futures::{
    ClientOrderId, FuturesFill, FuturesOrder, IntentKind, IntentState, NotPlacedEvidence,
    OrderAttempt, PositionCycle, PositionCycleId, Rejection,
};
use crate::model::{ModelError, OperationId, TimestampMs};

/// How often a settlement is made again when other entries keep
/// reserving from, or releasing to, its pool meanwhile.
const TRIES: usize = 4;

/// What is known of one attempt of an intent.
#[derive(Clone, Copy, Debug)]
pub enum Evidence<'e> {
    /// The order as the venue reports it, with the fills known for it.
    Order {
        order: &'e FuturesOrder,
        fills: &'e [FuturesFill],
    },
    /// The venue's definitive refusal: it answered, and executed nothing.
    Rejection(&'e Rejection),
    /// The request ended with no answer that says what became of it.
    Unknown,
    /// ADR 0002 §9's proof that an entry was never placed.
    NotPlaced(&'e NotPlacedEvidence),
}

fn invalid(reason: String) -> LedgerError {
    LedgerError::Model(ModelError::InvalidRecord {
        record: "ledger write",
        reason,
    })
}

impl ExecutionLedger<'_> {
    /// Records that `intent`'s next order is about to be sent as `id`,
    /// under the position's lease, and answers the intent as it then
    /// stands: `submitting`, with the attempt. **Call this before the
    /// request, never after**: a process that dies in between leaves the
    /// attempt recorded, and whoever resumes asks the venue for it.
    ///
    /// An entry's margin is reserved in the same write: `reservation` is
    /// its reservation and the revision its pool was read at when it was
    /// sized. If the pool has moved since, nothing is written and the
    /// answer is [`LedgerError::Stale`]: size the entry again.
    ///
    /// An entry is first held to what the store holds once more, since
    /// admission looked without a lease and things may have changed: an
    /// incident opened since halts it, a position opened since refuses it,
    /// and so does another entry now unresolved. Then nothing is written,
    /// the answer is [`LedgerError::Refused`], and the entry stays
    /// `admitted` for its executor to end. Under the lease the position
    /// cannot change between this look and the write; an incident, which
    /// needs no lease, still can, in the moment between the two.
    pub fn record_attempt(
        &self,
        fence: &Fence,
        intent: &Stored<IntentRecord>,
        id: &ClientOrderId,
        reservation: Option<(Reservation, u64)>,
    ) -> Result<Stored<IntentRecord>, LedgerError> {
        let held = intent.record.intent();
        if let (IntentKind::Entry(entry), true) = (held.kind(), held.attempts().is_empty()) {
            if let Some(refusal) = self.entry_refusal(entry)? {
                return Err(LedgerError::Refused(refusal));
            }
        }
        let now_ms = self.clock.now_ms();
        let operation = id.operation();
        let mut next = intent.record.intent().clone();
        next.record_attempt(OrderAttempt {
            client_order_id: id.clone(),
            sent_at_ms: now_ms,
        })?;
        let record = intent.record.with_intent(next)?;
        let scope = record.scope().clone();
        let instrument = record.key().instrument().clone();
        let order = OrderRecord::recorded(
            scope.clone(),
            operation,
            id.to_string(),
            instrument.clone(),
            now_ms,
        )?;
        let event = OutboxEvent::pending(
            scope,
            EventKey::of_operation(operation, &format!("submitted:{}", id.attempt())),
            EventKind::OrderSubmitted,
            now_ms,
        )
        .on(&instrument)
        .of_signal(record.signal())
        .of_operation(operation)
        .with("client_order_id", id)
        .with("leg", id.leg().as_str());
        let mut unit = ExecutionUnit::at(now_ms)
            .under(fence.clone())
            .with(Change::replace(
                Record::Intent(record.clone()),
                intent.revision,
            ))
            .with(Change::new(Record::Order(order)))
            .with(Change::new(Record::Event(event)));
        if let Some((held, revision)) = reservation {
            unit.pool = Some(PoolGuard { revision });
            unit.changes.push(Change::new(Record::Reservation(held)));
        }
        self.apply(&unit)?;
        Ok(Stored {
            record,
            revision: intent.revision + 1,
        })
    }

    /// Applies what the venue said of one of `intent`'s attempts, under
    /// the position's lease, in one write: the intent moved by the
    /// evidence, the order's record, each fill not recorded yet, the
    /// position's cycle opened by an entry's first fill, the entry's
    /// reservation released once it is settled, and the event that tells
    /// of it. Evidence that changes nothing writes nothing. The fills
    /// given with an order are that order's, by the venue's own order ID:
    /// a fill of another order is refused.
    pub fn settle(
        &self,
        fence: &Fence,
        intent: &Stored<IntentRecord>,
        evidence: Evidence<'_>,
    ) -> Result<Stored<IntentRecord>, LedgerError> {
        for _ in 0..TRIES {
            match self.settle_once(fence, intent, evidence) {
                // Another entry reserved from the pool meanwhile: only the
                // pool's revision is read again.
                Err(LedgerError::Stale(Stale::Pool { .. })) => continue,
                settled => return settled,
            }
        }
        Err(LedgerError::Busy)
    }

    fn settle_once(
        &self,
        fence: &Fence,
        intent: &Stored<IntentRecord>,
        evidence: Evidence<'_>,
    ) -> Result<Stored<IntentRecord>, LedgerError> {
        let now_ms = self.clock.now_ms();
        let held = &intent.record;
        let scope = held.scope().clone();
        let instrument = held.key().instrument().clone();
        let latest = held.intent().attempts().last().cloned();
        let attempt = match evidence {
            Evidence::Order { order, .. } => order.client_order_id(),
            Evidence::Rejection(rejection) => Some(rejection.client_order_id().clone()),
            Evidence::NotPlaced(proof) => Some(proof.client_order_id.clone()),
            Evidence::Unknown => latest.map(|attempt| attempt.client_order_id),
        }
        .ok_or_else(|| invalid("the evidence names none of the intent's attempts".to_string()))?;
        // The operation the attempt was sent under, which every record of
        // the attempt names.
        let operation = attempt.operation();
        let order = self
            .store
            .order(&scope, &attempt.to_string())?
            .ok_or_else(|| {
                invalid(format!(
                    "no order was recorded as {attempt}, and an order is recorded before it is \
                     sent"
                ))
            })?;

        let mut next = held.intent().clone();
        let mut new_fills = Vec::new();
        let reported = match evidence {
            Evidence::Order { order: seen, fills } => {
                for fill in fills {
                    let known = self.store.fill(&scope, &instrument, &fill.trade_id)?;
                    if known.is_none() {
                        next.on_fill_of(seen, fill)?;
                        new_fills.push(fill.clone());
                    }
                }
                next.on_order(seen)?;
                order
                    .record
                    .reported(OrderEvidence::Futures(Box::new(seen.clone())))?
            }
            Evidence::Rejection(rejection) => {
                next.on_rejection(rejection)?;
                order.record.refused(OrderRefusal {
                    kind: rejection.kind().to_string(),
                    message: rejection.message().to_string(),
                })?
            }
            Evidence::Unknown => {
                next.move_to(IntentState::OutcomeUnknown)?;
                order.record.unknown()?
            }
            Evidence::NotPlaced(proof) => {
                next.entry_not_placed(proof)?;
                order.record.clone()
            }
        };
        let state = next.state();
        let moved = state != held.intent().state();
        if !moved && new_fills.is_empty() && reported == order.record {
            return Ok(intent.clone());
        }

        let mut record = held.with_intent(next)?;
        let mut unit = ExecutionUnit::at(now_ms).under(fence.clone());
        // An entry's first fill opens its position's cycle, numbered as
        // the operation that opened it.
        let exposed = matches!(state, IntentState::PartiallyFilled | IntentState::Filled);
        if let (IntentKind::Entry(entry), true, None) =
            (held.intent().kind(), exposed, held.cycle())
        {
            let cycle = PositionCycleId::new(operation.get())?;
            let opened_at_ms = new_fills
                .iter()
                .map(|fill| fill.time_ms)
                .min()
                .unwrap_or(now_ms);
            let opened = CycleRecord::new(PositionCycle::open(
                cycle,
                entry.key.clone(),
                entry.profile.clone(),
                entry.side,
                operation,
                opened_at_ms,
            ))?;
            record = record.in_cycle(cycle)?;
            unit.changes.push(Change::new(Record::Cycle(opened)));
            if let Some(carried) = self.store.operation(operation)? {
                let bound = carried.record.in_cycle(cycle)?;
                unit.changes
                    .push(Change::replace(Record::Operation(bound), carried.revision));
            }
        }
        unit.changes.push(Change::replace(
            Record::Intent(record.clone()),
            intent.revision,
        ));
        if reported != order.record {
            unit.changes
                .push(Change::replace(Record::Order(reported), order.revision));
        }
        for fill in new_fills {
            let fill = FillRecord::new(scope.clone(), fill, Some(operation), record.cycle())?;
            unit.changes.push(Change::new(Record::Fill(fill)));
        }
        if state.is_terminal() && matches!(held.intent().kind(), IntentKind::Entry(_)) {
            self.release(&mut unit, operation, state, now_ms)?;
        }
        if moved {
            let (kind, word) = match state {
                IntentState::OutcomeUnknown => (
                    EventKind::OutcomeUnknown,
                    format!("unknown:{}", attempt.attempt()),
                ),
                _ => (EventKind::OrderSettled, format!("settled:{state}")),
            };
            let event = OutboxEvent::pending(
                scope,
                EventKey::of_operation(operation, &word),
                kind,
                now_ms,
            )
            .on(&instrument)
            .of_signal(record.signal())
            .of_operation(operation)
            .with("client_order_id", &attempt)
            .with("state", state);
            let event = match record.cycle() {
                Some(cycle) => event.of_cycle(cycle),
                None => event,
            };
            unit.changes.push(Change::new(Record::Event(event)));
        }
        self.apply(&unit)?;
        Ok(Stored {
            record,
            revision: intent.revision + 1,
        })
    }

    /// Adds to `unit` the release of the reservation `operation`'s entry
    /// holds, if it holds one, with the revision its pool stands at.
    fn release(
        &self,
        unit: &mut ExecutionUnit,
        operation: OperationId,
        state: IntentState,
        now_ms: TimestampMs,
    ) -> Result<(), LedgerError> {
        let Some(held) = self.store.reservation(operation)? else {
            return Ok(());
        };
        if held.record.released().is_some() {
            return Ok(());
        }
        let reason = match state {
            IntentState::Filled => ReleaseReason::Opened,
            _ => ReleaseReason::NotOpened,
        };
        let key = held.record.scope().position_key(held.record.instrument())?;
        let pool = self.store.pool(key.account(), held.record.pool())?;
        unit.pool = Some(PoolGuard {
            revision: pool.revision,
        });
        unit.changes.push(Change::replace(
            Record::Reservation(held.record.release(reason, now_ms)?),
            held.revision,
        ));
        Ok(())
    }
}
