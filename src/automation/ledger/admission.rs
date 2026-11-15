//! Admission: one delivery recorded with what became of it, in one write.

use super::{ExecutionLedger, LedgerError};
use crate::model::execution::{
    Change, DeliveryWrite, EventKey, EventKind, ExecutionScope, ExecutionUnit, IntentRecord,
    OutboxEvent, Record, SignalRecord, Stored,
};
use crate::model::futures::{
    Delivery, EntryIntent, ExitIntent, ExternalIntent, ExternalSignal, IntentKind, PositionCycleId,
    ProfileIdentity, ResolvedSignal, SignalRefusal,
};
use crate::model::{Refusal, TimestampMs};

/// How often a delivery is decided again when other deliveries of its
/// signal keep getting in first.
const TRIES: usize = 3;

/// What a delivery was to its profile's inbox, with the signal's record
/// as it stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// The first delivery of its signal: recorded, with this outcome.
    Recorded(SignalRecord),
    /// A delivery already recorded. Nothing was done twice, and the
    /// record is the first delivery's, whatever it came to.
    Duplicate(SignalRecord),
    /// Another payload under a recorded signal's ID: refused as
    /// `conflict`, and told to the outbox. The record is the first
    /// delivery's, unchanged.
    Conflict(SignalRecord),
}

impl Admission {
    /// The signal's record, as the inbox holds it now.
    pub fn record(&self) -> &SignalRecord {
        match self {
            Self::Recorded(record) | Self::Duplicate(record) | Self::Conflict(record) => record,
        }
    }

    /// What the delivery was to the inbox.
    pub const fn delivery(&self) -> Delivery {
        match self {
            Self::Recorded(_) => Delivery::New,
            Self::Duplicate(_) => Delivery::Duplicate,
            Self::Conflict(_) => Delivery::Conflict,
        }
    }
}

/// What a signal comes to: its record once judged, the intent recorded
/// with an admission, and the event that tells of it.
struct Judgment {
    signal: SignalRecord,
    intent: Option<IntentRecord>,
    event: OutboxEvent,
}

/// What an exit is for.
enum ExitTarget {
    /// The open cycle its own profile entered before it fired.
    Cycle(PositionCycleId),
    /// An entry of its own profile that fired before it and is not
    /// settled yet: the exit waits, and applies to the cycle it opens.
    Entry,
    Nothing,
}

impl ExecutionLedger<'_> {
    /// Records a delivery that is not judged yet: `received`, or what the
    /// inbox already holds under its signal's ID.
    pub fn receive(
        &self,
        route: &ProfileIdentity,
        signal: &ExternalSignal,
        payload: &str,
    ) -> Result<Admission, LedgerError> {
        let now_ms = self.clock.now_ms();
        let scope = ExecutionScope::of_profile(route);
        let received = SignalRecord::received(scope, signal.clone(), payload, now_ms)?;
        let mut unit = ExecutionUnit::at(now_ms);
        unit.delivery = Some(DeliveryWrite {
            conflict_event: conflict_event(&received),
            record: received.clone(),
        });
        Ok(match self.deliver(&unit)? {
            None => Admission::Recorded(received),
            Some((Delivery::Conflict, held)) => Admission::Conflict(held.record),
            Some((_, held)) => Admission::Duplicate(held.record),
        })
    }

    /// Records a delivery for `route`, the profile its URL selected, with
    /// what became of it, in one write. `resolved` is what the signal
    /// asks of that profile ([`SignalService::resolve`]), or the refusal
    /// it is recorded with.
    ///
    /// What the store holds decides the rest. An entry is refused while
    /// an incident halts it (`incident_open`), on a position already held
    /// (`pyramiding`, `reversal`), and while an earlier entry on its
    /// position is unresolved (`unresolved_entry`). An exit is admitted
    /// for the open cycle its own profile entered before it fired, or for
    /// such an entry still on its way; any other exit has nothing to do.
    /// Whoever sends an admitted entry holds it to the same rules again
    /// under the position's lease, where they are decided for good.
    ///
    /// A delivery already recorded and not judged yet is judged here, in
    /// place.
    ///
    /// [`SignalService::resolve`]: crate::automation::SignalService::resolve
    pub fn admit(
        &self,
        route: &ProfileIdentity,
        signal: &ExternalSignal,
        payload: &str,
        resolved: &Result<ResolvedSignal, SignalRefusal>,
    ) -> Result<Admission, LedgerError> {
        let scope = ExecutionScope::of_profile(route);
        for _ in 0..TRIES {
            let now_ms = self.clock.now_ms();
            let received = SignalRecord::received(scope.clone(), signal.clone(), payload, now_ms)?;
            let judgment = self.judge(&received, resolved, now_ms)?;
            let mut unit = ExecutionUnit::at(now_ms);
            unit.delivery = Some(DeliveryWrite {
                conflict_event: conflict_event(&received),
                record: judgment.signal.clone(),
            });
            unit.changes = judgment.changes();
            let held = match self.deliver(&unit) {
                Ok(None) => return Ok(Admission::Recorded(judgment.signal)),
                Ok(Some((Delivery::Conflict, held))) => {
                    return Ok(Admission::Conflict(held.record));
                }
                Ok(Some((_, held))) => held,
                Err(LedgerError::Stale(_)) => continue,
                Err(error) => return Err(error),
            };
            if held.record.state().is_judged() {
                return Ok(Admission::Duplicate(held.record));
            }
            match self.judge_recorded(&held, resolved) {
                Ok(judged) => return Ok(Admission::Duplicate(judged)),
                // Another writer judged it meanwhile: read what it came to.
                Err(LedgerError::Stale(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(LedgerError::Busy)
    }

    /// Judges a signal the inbox holds as `received`, in place.
    fn judge_recorded(
        &self,
        held: &Stored<SignalRecord>,
        resolved: &Result<ResolvedSignal, SignalRefusal>,
    ) -> Result<SignalRecord, LedgerError> {
        let now_ms = self.clock.now_ms();
        let judgment = self.judge(&held.record, resolved, now_ms)?;
        let mut unit = ExecutionUnit::at(now_ms).with(Change::replace(
            Record::Signal(judgment.signal.clone()),
            held.revision,
        ));
        unit.changes.extend(judgment.changes());
        self.apply(&unit)?;
        Ok(judgment.signal)
    }

    /// What `received` comes to, by what it resolved to and what the
    /// store holds.
    fn judge(
        &self,
        received: &SignalRecord,
        resolved: &Result<ResolvedSignal, SignalRefusal>,
        now_ms: TimestampMs,
    ) -> Result<Judgment, LedgerError> {
        let event = |record: &SignalRecord, kind: EventKind| {
            OutboxEvent::pending(
                record.scope().clone(),
                EventKey::of_signal(record.profile(), kind.as_str(), record.id()),
                kind,
                now_ms,
            )
            .of_signal(record.id())
            .with("ticker", &record.signal().ticker)
            .with("side", record.signal().side)
        };
        let refused = |refusal: &SignalRefusal| -> Result<Judgment, LedgerError> {
            let signal = received.rejected(refusal.clone(), now_ms)?;
            let event = event(&signal, EventKind::SignalRejected)
                .with("refusal", refusal.refusal)
                .with("message", &refusal.message);
            Ok(Judgment {
                signal,
                intent: None,
                event,
            })
        };
        let resolved = match resolved {
            Ok(resolved) => resolved,
            Err(refusal) => return refused(refusal),
        };
        let mut intent = ExternalIntent::new(resolved.intent.clone());
        let mut cycle = None;
        match &resolved.intent {
            IntentKind::Entry(entry) => {
                if let Some(refusal) = self.entry_refusal(entry)? {
                    return refused(&refusal);
                }
            }
            IntentKind::Exit(exit) => match self.exit_target(exit)? {
                ExitTarget::Cycle(id) => cycle = Some(id),
                ExitTarget::Entry => {}
                ExitTarget::Nothing => {
                    let note = format!(
                        "profile {} holds no {} position on {} that this exit is for",
                        exit.profile,
                        exit.side,
                        exit.key.instrument().symbol
                    );
                    let signal = received.noop(note.as_str(), now_ms)?;
                    let event = event(&signal, EventKind::SignalNoop).with("note", note);
                    return Ok(Judgment {
                        signal,
                        intent: None,
                        event,
                    });
                }
            },
        }
        intent.admit(resolved.policy.clone())?;
        let record = IntentRecord::new(intent);
        if record.scope() != received.scope() {
            return Err(LedgerError::Model(
                crate::model::ModelError::InvalidRecord {
                    record: "admission",
                    reason: format!(
                        "the signal resolved for {}, and its delivery is {}'s",
                        record.scope(),
                        received.scope()
                    ),
                },
            ));
        }
        let record = match cycle {
            Some(cycle) => record.in_cycle(cycle)?,
            None => record,
        };
        let signal = received.admitted(now_ms)?;
        let event = event(&signal, EventKind::SignalAdmitted)
            .on(record.key().instrument())
            .with("kind", kind_word(&resolved.intent));
        let event = match &resolved.intent {
            IntentKind::Entry(entry) => {
                let event = event.with("expected_price", entry.expected_price);
                match entry.stop {
                    Some(stop) => event.with("stop", stop),
                    None => event,
                }
            }
            IntentKind::Exit(_) => event,
        };
        Ok(Judgment {
            signal,
            intent: Some(record),
            event,
        })
    }

    /// Why the store's records refuse `entry`, if they do.
    fn entry_refusal(&self, entry: &EntryIntent) -> Result<Option<SignalRefusal>, LedgerError> {
        let key = &entry.key;
        let symbol = &key.instrument().symbol;
        let halting = self.store.open_incidents()?;
        let halting = halting
            .iter()
            .find(|incident| incident.record.halts_entries(&entry.profile, key));
        if let Some(incident) = halting {
            return Ok(Some(SignalRefusal {
                refusal: Refusal::IncidentOpen,
                message: format!(
                    "an open incident halts entries on {symbol}: {}; clear it once the position \
                     is flat or protected",
                    incident.record.message()
                ),
            }));
        }
        if let Some(open) = self.store.open_cycle(key)? {
            if let Err(refusal) = entry.against(Some(open.record.cycle().side())) {
                return Ok(Some(refusal));
            }
        }
        let unresolved = self.store.position_intents(key)?;
        if unresolved.iter().any(|held| held.record.blocks_entries()) {
            return Ok(Some(SignalRefusal {
                refusal: Refusal::UnresolvedEntry,
                message: format!(
                    "an earlier entry on {symbol} is still unresolved, and no other entry is \
                     taken on its position until it settles"
                ),
            }));
        }
        Ok(None)
    }

    /// What `exit` closes, by the store's records.
    fn exit_target(&self, exit: &ExitIntent) -> Result<ExitTarget, LedgerError> {
        let scope = ExecutionScope::of_position(&exit.key, &exit.profile);
        if let Some(open) = self.store.open_cycle(&exit.key)? {
            // Another profile's position on the same account is not this
            // exit's to close, whatever its alerts are named.
            if open.record.cycle().profile() != &exit.profile {
                return Ok(ExitTarget::Nothing);
            }
            let opened_by = self.store.operation(open.record.cycle().opened_by())?;
            let entry = match opened_by.as_ref().and_then(|held| held.record.signal()) {
                Some(signal) => self.store.intent(&scope, signal)?,
                None => None,
            };
            let closes = entry.is_some_and(|held| match held.record.intent().kind() {
                IntentKind::Entry(entry) => exit.closes(entry),
                IntentKind::Exit(_) => false,
            });
            return Ok(if closes {
                ExitTarget::Cycle(open.record.id())
            } else {
                ExitTarget::Nothing
            });
        }
        let waiting = self.store.position_intents(&exit.key)?;
        let closes = waiting
            .iter()
            .any(|held| match held.record.intent().kind() {
                IntentKind::Entry(entry) => exit.closes(entry),
                IntentKind::Exit(_) => false,
            });
        Ok(if closes {
            ExitTarget::Entry
        } else {
            ExitTarget::Nothing
        })
    }
}

impl Judgment {
    /// What the judgment writes beside the signal's own record.
    fn changes(&self) -> Vec<Change> {
        let intent = self.intent.clone().map(Record::Intent);
        intent
            .into_iter()
            .chain([Record::Event(self.event.clone())])
            .map(Change::new)
            .collect()
    }
}

const fn kind_word(kind: &IntentKind) -> &'static str {
    match kind {
        IntentKind::Entry(_) => "entry",
        IntentKind::Exit(_) => "exit",
    }
}

/// What the outbox is told if `arrived`'s delivery conflicts with the
/// signal recorded under its ID: once for each payload that does.
fn conflict_event(arrived: &SignalRecord) -> OutboxEvent {
    let word = format!("conflict:{}", arrived.payload_hash());
    OutboxEvent::pending(
        arrived.scope().clone(),
        EventKey::of_signal(arrived.profile(), &word, arrived.id()),
        EventKind::SignalConflict,
        arrived.received_at_ms(),
    )
    .of_signal(arrived.id())
    .with("refusal", Refusal::Conflict)
    .with("payload_hash", arrived.payload_hash())
}
