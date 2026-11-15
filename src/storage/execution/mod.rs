//! The execution store, as both backends share it
//! (`docs/execution-store.md`): the tables and the row each record is
//! written as ([`tables`], [`rows`]), every unit decided from what a backend read
//! for it ([`plan`]), leases and the store's activation decided the same
//! way ([`lease`]), and every stored row checked on its way back to a
//! record ([`read`], [`reads`]). A backend runs statements; what they mean is here,
//! once, so SQLite and PostgreSQL answer alike by construction.

pub(crate) mod columns;
pub(crate) mod lease;
pub(crate) mod plan;
pub(crate) mod read;
pub(crate) mod reads;
pub(crate) mod rows;
pub(crate) mod tables;

use crate::model::execution::{NewOperation, OperationRecord, Replacement, Stored};
use crate::ports::StorageError;

/// Holds an operation asked for again under a recorded key to the one the
/// key names: the same kind, on the same instrument, in the same scope,
/// for the same signal, and, where it replaces an order, replacing the
/// same one. Any other is refused, since a key names one operation for
/// good.
pub(crate) fn same_operation(
    asked: &NewOperation,
    held: Stored<OperationRecord>,
) -> Result<Stored<OperationRecord>, StorageError> {
    let record = &held.record;
    let same = record.scope() == &asked.scope
        && record.kind() == asked.kind
        && record.instrument() == &asked.instrument
        && record.signal() == asked.signal.as_ref()
        && record.replacement().map(Replacement::original)
            == asked.replacement.as_ref().map(Replacement::original);
    if !same {
        let key = record.key().map(ToString::to_string).unwrap_or_default();
        return Err(StorageError::InvalidArgument(format!(
            "operation key {key} already names operation {}, a {} on {}; a key names one \
             operation for good",
            record.id(),
            record.kind(),
            record.instrument()
        )));
    }
    Ok(held)
}

/// The read methods of [`crate::ports::ExecutionStore`], for a store that
/// implements [`read::Rows`]: each is the shared read of the same name.
macro_rules! execution_reads {
    () => {
        fn signal(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            signal: &$crate::model::futures::SignalId,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::SignalRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::signal(self, scope, signal)
        }

        fn received_signals(
            &self,
            limit: usize,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::SignalRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::received_signals(self, limit)
        }

        fn deliveries(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            signal: &$crate::model::futures::SignalId,
        ) -> Result<Vec<$crate::model::execution::SignalDelivery>, $crate::ports::StorageError> {
            $crate::storage::execution::reads::deliveries(self, scope, signal)
        }

        fn intent(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            signal: &$crate::model::futures::SignalId,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::IntentRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::intent(self, scope, signal)
        }

        fn position_intents(
            &self,
            key: &$crate::model::futures::PositionKey,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::IntentRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::position_intents(self, key)
        }

        fn unsettled_intents(
            &self,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::IntentRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::unsettled_intents(self)
        }

        fn operation(
            &self,
            id: $crate::model::OperationId,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::OperationRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::operation(self, id)
        }

        fn unsettled_replacements(
            &self,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::OperationRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::unsettled_replacements(self)
        }

        fn order(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            client_order_id: &str,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::OrderRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::order(self, scope, client_order_id)
        }

        fn orders(
            &self,
            operation: $crate::model::OperationId,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::OrderRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::orders(self, operation)
        }

        fn fill(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            instrument: &$crate::model::InstrumentId,
            trade: &$crate::model::VenueTradeId,
        ) -> Result<Option<$crate::model::execution::FillRecord>, $crate::ports::StorageError> {
            $crate::storage::execution::reads::fill(self, scope, instrument, trade)
        }

        fn fills(
            &self,
            cycle: $crate::model::futures::PositionCycleId,
        ) -> Result<Vec<$crate::model::execution::FillRecord>, $crate::ports::StorageError> {
            $crate::storage::execution::reads::fills(self, cycle)
        }

        fn cycle(
            &self,
            id: $crate::model::futures::PositionCycleId,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::CycleRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::cycle(self, id)
        }

        fn open_cycle(
            &self,
            key: &$crate::model::futures::PositionKey,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::CycleRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::open_cycle(self, key)
        }

        fn open_cycles(
            &self,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::CycleRecord>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::open_cycles(self)
        }

        fn pool(
            &self,
            account: &$crate::model::AccountRef,
            pool: &$crate::model::futures::PoolScope,
        ) -> Result<$crate::model::execution::PoolReservations, $crate::ports::StorageError> {
            $crate::storage::execution::reads::pool(self, account, pool)
        }

        fn reservation(
            &self,
            operation: $crate::model::OperationId,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::Reservation>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::reservation(self, operation)
        }

        fn incident(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            key: &$crate::model::execution::IncidentKey,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::Incident>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::incident(self, scope, key)
        }

        fn open_incidents(
            &self,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::Incident>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::open_incidents(self)
        }

        fn event(
            &self,
            scope: &$crate::model::execution::ExecutionScope,
            key: &$crate::model::execution::EventKey,
        ) -> Result<
            Option<$crate::model::execution::Stored<$crate::model::execution::OutboxEvent>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::event(self, scope, key)
        }

        fn pending_events(
            &self,
            limit: usize,
        ) -> Result<
            Vec<$crate::model::execution::Stored<$crate::model::execution::OutboxEvent>>,
            $crate::ports::StorageError,
        > {
            $crate::storage::execution::reads::pending_events(self, limit)
        }
    };
}
pub(crate) use execution_reads;
