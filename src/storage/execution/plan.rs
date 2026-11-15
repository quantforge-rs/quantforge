//! One unit, decided: what a backend reads for it, and from what it read,
//! what it writes and answers. Nothing here touches a database, so both
//! backends decide every unit by the same code: a backend opens its
//! transaction, reads what [`Plan`] names, hands the [`Snapshot`] to
//! [`Plan::decide`], and writes what comes back.
//!
//! The order of the checks is part of the contract, since the first that
//! fails is the one answered: the store's environment, then the fence,
//! then the delivery, then the pool's revision, then each change in the
//! unit's order, then the slots.

use super::lease::{self, LeaseRow, StoreStanding};
use super::read::{self, Query, RawRow};
use super::rows;
use super::tables::{self, DELIVERIES, Named, PoolKey, Table};
use crate::model::execution::{
    Commit, ExecutionUnit, Expect, Found, Record, RecordKind, SignalDelivery, Stale, Stored,
};
use crate::model::futures::Delivery;
use crate::model::{TimestampMs, TransferValue};
use crate::ports::StorageError;
use sha2::{Digest, Sha256};

/// What a backend writes for a unit, in this order: the pool, then table
/// by table in the order a copy takes them ([`Table::rank`]), and within
/// a table in the unit's own order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Write {
    /// A new row. Where another writer got the same key in first, the
    /// backend starts the unit over, and the unit is decided again.
    Insert {
        table: &'static Table,
        row: Vec<TransferValue>,
    },
    /// A row replaced where it still stands at `expected`.
    Update {
        table: &'static Table,
        key: Vec<Named>,
        expected: u64,
        row: Vec<TransferValue>,
    },
    /// The pool's revision moved to `to`.
    Pool { key: PoolKey, to: u64 },
}

impl Write {
    /// Where the write comes among its unit's. The pool's row is held
    /// from before the unit read, so it comes first.
    fn rank(&self) -> usize {
        match self {
            Self::Pool { .. } => 0,
            Self::Insert { table, .. } | Self::Update { table, .. } => table.rank() + 1,
        }
    }
}

/// What a unit comes to, once its snapshot is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Nothing is written.
    Answer(Commit),
    /// These are written, all or none, and then this is the answer.
    Write(Vec<Write>, Commit),
}

impl Decision {
    /// `writes`, in the order every unit writes its tables, then `answer`.
    /// The sort is stable: rows of one table keep the unit's order, which
    /// is the order the store numbers them in.
    fn write(mut writes: Vec<Write>, answer: Commit) -> Self {
        writes.sort_by_key(Write::rank);
        Self::Write(writes, answer)
    }
}

/// Everything a backend reads before a unit is decided, beside the lease
/// row and the store's standing that a fenced unit is held to.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The pool whose revision is read, and locked.
    pub(crate) pool: Option<PoolKey>,
    /// Rows read by their keys, each locked where it is found. A backend
    /// reads them in [`Plan::lock_order`], and answers them in this one.
    pub(crate) lookups: Vec<Query>,
    /// One number for each key the unit may write a first row under, in
    /// rising order. A row that is found can be locked; one that is not
    /// cannot, so a backend whose writers run side by side holds these
    /// instead, from before it reads until it ends: no other unit then
    /// writes a first row under any of them in between, and what the unit
    /// read as absent stays absent while it is decided.
    pub(crate) guards: Vec<i64>,
    delivery: Option<usize>,
    conflict_event: Option<usize>,
    /// For each change, where its record's own row is looked up, and where
    /// the holder of the slot it would take is.
    changes: Vec<(usize, Option<usize>)>,
    columns: Vec<Vec<Named>>,
}

/// What a backend read for a [`Plan`].
#[derive(Clone, Debug)]
pub(crate) struct Snapshot {
    /// The store's own record, which every unit is held to.
    pub(crate) standing: StoreStanding,
    /// The lease row of the position a fenced unit names.
    pub(crate) lease: Option<LeaseRow>,
    /// 0 for a pool nothing was ever reserved from.
    pub(crate) pool_revision: u64,
    /// One for each lookup, in the plan's order: the row found, if one
    /// was.
    pub(crate) found: Vec<Option<RawRow>>,
}

impl Snapshot {
    /// A snapshot with nothing read yet but the store's standing.
    pub(crate) fn of(standing: StoreStanding) -> Self {
        Self {
            standing,
            lease: None,
            pool_revision: 0,
            found: Vec::new(),
        }
    }
}

fn refused(error: impl std::fmt::Display) -> StorageError {
    StorageError::InvalidArgument(error.to_string())
}

/// The number that stands for one key of one table: the first eight bytes
/// of the SHA-256 of the table's name and the key's values. Two keys that
/// share a number only wait for each other.
fn guard_of(table: &Table, key: &[Named]) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(table.name().as_bytes());
    for (_, value) in key {
        hasher.update([0x1f]);
        hasher.update(value.to_string().as_bytes());
    }
    let digest = hasher.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&digest[..8]);
    i64::from_be_bytes(first)
}

/// The slot a record takes while it holds it: the one unresolved entry of
/// its position, or its one open cycle.
fn slot_of(record: &Record) -> Result<Option<Query>, StorageError> {
    let (key, column) = match record {
        Record::Intent(intent) if intent.blocks_entries() => (intent.key(), "entry_slot"),
        Record::Cycle(cycle) if cycle.is_open() => (cycle.key(), "open_slot"),
        _ => return Ok(None),
    };
    let mut equals = tables::position_columns(key);
    equals.push((column, TransferValue::Integer(1)));
    let table = tables::table_of(record.kind());
    Ok(Some(Query {
        table,
        equals,
        one_of: None,
        order: table.spec.key,
        limit: None,
    }))
}

impl Plan {
    /// What `unit` needs read, or why it is not a unit a store applies.
    pub(crate) fn of(unit: &ExecutionUnit) -> Result<Self, StorageError> {
        unit.check().map_err(refused)?;
        let mut plan = Self {
            pool: unit
                .reserved_pool()
                .map_err(refused)?
                .map(|(key, pool)| PoolKey::of(key.account(), &pool)),
            lookups: Vec::new(),
            guards: Vec::new(),
            delivery: None,
            conflict_event: None,
            changes: Vec::new(),
            columns: Vec::new(),
        };
        if let Some(delivery) = &unit.delivery {
            // A delivery may be the first of its signal, and may tell of a
            // conflict for the first time.
            let signal = rows::derived(&Record::Signal(delivery.record.clone()))?;
            plan.guard(&tables::SIGNALS, &signal)?;
            plan.delivery = Some(plan.look(read::lookup(RecordKind::Signal, &signal)?));
            let event = rows::derived(&Record::Event(delivery.conflict_event.clone()))?;
            plan.guard(&tables::EVENTS, &event)?;
            plan.conflict_event = Some(plan.look(read::lookup(RecordKind::Event, &event)?));
        }
        for change in &unit.changes {
            let columns = rows::derived(&change.record)?;
            if change.expect == Expect::Absent {
                plan.guard(tables::table_of(change.record.kind()), &columns)?;
            }
            let own = plan.look(read::lookup(change.record.kind(), &columns)?);
            let slot = slot_of(&change.record)?.map(|query| plan.look(query));
            plan.changes.push((own, slot));
            plan.columns.push(columns);
        }
        plan.guards.sort_unstable();
        plan.guards.dedup();
        Ok(plan)
    }

    fn guard(&mut self, table: &'static Table, columns: &[Named]) -> Result<(), StorageError> {
        self.guards.push(guard_of(table, &table.key_of(columns)?));
        Ok(())
    }

    fn look(&mut self, query: Query) -> usize {
        self.lookups.push(query);
        self.lookups.len() - 1
    }

    /// The lookups in the order a backend reads them, and locks what it
    /// finds: table by table in the order a copy takes them, and within a
    /// table by the values that name the row. Every unit locks in this
    /// one order, whatever order it lists its changes in, so no two units
    /// ever hold a row each that the other waits for.
    pub(crate) fn lock_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.lookups.len()).collect();
        order.sort_by_cached_key(|index| {
            let query = &self.lookups[*index];
            let named: Vec<String> = query
                .equals
                .iter()
                .map(|(column, value)| format!("{column}={value}"))
                .collect();
            (query.table.rank(), named)
        });
        order
    }

    /// What `unit` comes to, given what the backend read for this plan.
    pub(crate) fn decide(
        &self,
        unit: &ExecutionUnit,
        snapshot: &Snapshot,
    ) -> Result<Decision, StorageError> {
        let standing = &snapshot.standing;
        if let Some(delivery) = &unit.delivery {
            let signal = &delivery.record;
            let what = format!("signal {}/{}", signal.profile(), signal.id());
            standing.keeps(signal.scope().environment(), what)?;
        }
        for change in &unit.changes {
            let record = &change.record;
            let what = format!("{} {}", record.kind(), record.name());
            standing.keeps(record.scope().environment(), what)?;
        }
        if let Some(fence) = &unit.fence {
            let refusal = lease::fenced(fence, snapshot.lease.as_ref(), standing);
            if let Some(fenced) = refusal {
                return Ok(Decision::Answer(Commit::Fenced(fenced)));
            }
        }
        let found = |index: usize| snapshot.found.get(index).and_then(Option::as_ref);
        let mut writes = Vec::new();
        if let (Some(delivery), Some(index)) = (&unit.delivery, self.delivery) {
            let arrived = &delivery.record;
            let table = &tables::SIGNALS;
            match found(index) {
                Some(row) => {
                    let held = read::decode(table, row)?;
                    let (Record::Signal(signal), revision) = (held.record, held.revision) else {
                        return Err(refused("the inbox holds another kind of record"));
                    };
                    let what = signal.delivery_of(arrived.payload_hash());
                    writes.push(note(arrived, what)?);
                    let stored = Stored {
                        record: signal,
                        revision,
                    };
                    let event = self.conflict_event.filter(|event| found(*event).is_none());
                    return Ok(match what {
                        Delivery::Conflict => {
                            if event.is_some() {
                                let told = Record::Event(delivery.conflict_event.clone());
                                writes.push(first_write(&told, unit.at_ms)?);
                            }
                            Decision::write(writes, Commit::Conflict(stored))
                        }
                        _ => Decision::write(writes, Commit::Duplicate(stored)),
                    });
                }
                None => {
                    writes.push(first_write(&Record::Signal(arrived.clone()), unit.at_ms)?);
                    writes.push(note(arrived, Delivery::New)?);
                }
            }
        }
        if let (Some(key), Some(guard)) = (&self.pool, unit.pool) {
            if guard.revision != snapshot.pool_revision {
                return Ok(Decision::Answer(Commit::Stale(Stale::Pool {
                    revision: snapshot.pool_revision,
                })));
            }
            let to = snapshot
                .pool_revision
                .checked_add(1)
                .ok_or_else(|| refused("the pool's revision cannot move further"))?;
            writes.push(Write::Pool {
                key: key.clone(),
                to,
            });
        }
        for (position, change) in unit.changes.iter().enumerate() {
            let (own, _) = self.changes[position];
            let record = &change.record;
            let table = tables::table_of(record.kind());
            let stale = |found| {
                Decision::Answer(Commit::Stale(Stale::Record {
                    kind: record.kind(),
                    name: record.name(),
                    found,
                }))
            };
            let held = found(own).map(|row| read::decode(table, row)).transpose()?;
            match (change.expect, held) {
                (Expect::Absent, None) => writes.push(first_write(record, unit.at_ms)?),
                (Expect::Absent, Some(held)) => {
                    return Ok(stale(Found::Present {
                        revision: held.revision,
                    }));
                }
                (Expect::Revision(_), None) => return Ok(stale(Found::Absent)),
                (Expect::Revision(expected), Some(held)) if held.revision != expected => {
                    return Ok(stale(Found::Present {
                        revision: held.revision,
                    }));
                }
                (Expect::Revision(expected), Some(held)) => {
                    record.follows(&held.record).map_err(|error| {
                        refused(format!(
                            "{} {} cannot follow the one recorded: {error}",
                            record.kind(),
                            record.name()
                        ))
                    })?;
                    let columns = &self.columns[position];
                    let next = expected
                        .checked_add(1)
                        .ok_or_else(|| refused("the record's revision cannot move further"))?;
                    writes.push(Write::Update {
                        table,
                        key: table.key_of(columns)?,
                        expected,
                        row: table.row(columns, next, unit.at_ms)?,
                    });
                }
            }
        }
        for (position, change) in unit.changes.iter().enumerate() {
            let (_, Some(slot)) = self.changes[position] else {
                continue;
            };
            let Some(row) = found(slot) else { continue };
            let record = &change.record;
            let holder = read::decode(tables::table_of(record.kind()), row)?.record;
            if holder.name() != record.name() {
                return Err(refused(format!(
                    "{kind} {name} cannot take its position's slot, which {kind} {holder} \
                     holds: a position has one unresolved entry and one open cycle at a time",
                    kind = record.kind(),
                    name = record.name(),
                    holder = holder.name(),
                )));
            }
        }
        Ok(Decision::write(writes, Commit::Applied))
    }
}

/// The first row of `record`, at revision 1.
fn first_write(record: &Record, at_ms: TimestampMs) -> Result<Write, StorageError> {
    let table = tables::table_of(record.kind());
    Ok(Write::Insert {
        table,
        row: table.row(&rows::derived(record)?, 1, at_ms)?,
    })
}

/// The note of a delivery that was `what` to its inbox.
fn note(
    arrived: &crate::model::execution::SignalRecord,
    what: Delivery,
) -> Result<Write, StorageError> {
    Ok(Write::Insert {
        table: &DELIVERIES,
        row: tables::delivery_row(&SignalDelivery::of(arrived, what))?,
    })
}
