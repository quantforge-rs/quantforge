//! What a copy writes, refuses, and does when it is cut off and run again.

use std::sync::atomic::Ordering;

use super::*;

#[test]
fn a_copy_writes_every_row_once_in_batches_and_records_it_finished() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    let summary = engine(&source, &target, 3).copy().expect("copy");

    assert_eq!(summary.start, CopyStart::Started);
    assert_eq!(target.all(), source.all());
    let read: Vec<(&str, u64, u64)> = summary
        .tables
        .iter()
        .map(|copy| (copy.table, copy.rows, copy.written))
        .collect();
    let expected: Vec<(&str, u64, u64)> = TRANSFER_TABLES
        .iter()
        .map(|table| {
            let rows = rows_of(table).len() as u64;
            (table.name, rows, rows)
        })
        .collect();
    assert_eq!(read, expected);
    let record = target.transfer_record().expect("record").expect("a record");
    assert_eq!(record.state, TransferState::Copied);
    assert_eq!(record.source_store_id.as_str(), SOURCE_ID);
    assert!(target.numbering_moved.load(Ordering::SeqCst));
}

#[test]
fn a_copy_cut_off_and_run_again_converges_on_the_same_target_without_duplicates() {
    let source = Memory::source();
    let whole = Memory::new(TARGET_ID);
    engine(&source, &whole, 2).copy().expect("uncut copy");

    for cut in [0, 1, 5, 13] {
        let target = Memory::new(TARGET_ID);
        target.writes_left.store(cut, Ordering::SeqCst);
        let error = engine(&source, &target, 2).copy().expect_err("cut off");
        assert!(matches!(
            error,
            EngineError::CopyStopped {
                source: StorageError::OutcomeUnknown(_),
                ..
            }
        ));
        let record = target.transfer_record().expect("record").expect("begun");
        assert_eq!(record.state, TransferState::Copying, "cut {cut}");

        target.writes_left.store(usize::MAX, Ordering::SeqCst);
        let summary = engine(&source, &target, 2).copy().expect("resumed");
        assert_eq!(summary.start, CopyStart::Resumed);
        assert_eq!(target.all(), whole.all(), "cut {cut}");
        assert!(
            engine(&source, &target, 2)
                .verify()
                .expect("verify")
                .is_ok()
        );
    }
}

/// Cuts a copy off once the candles and the runs are copied, lets `change`
/// write to the source, and expects the copy run again to be refused before
/// it writes, naming `first` as the first row the source no longer holds
/// alike.
fn refused_on_resume(change: impl Fn(&Memory), first: &str) {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    target.writes_left.store(3, Ordering::SeqCst);
    engine(&source, &target, 7).copy().expect_err("cut off");
    change(&source);
    source.put(&ORDER_EVENTS, row_of(&ORDER_EVENTS, 8, 0));
    target.writes_left.store(usize::MAX, Ordering::SeqCst);
    let (writes, held) = (target.writes.load(Ordering::SeqCst), target.all());

    let reason = refusal(engine(&source, &target, 7).copy());
    assert_eq!(
        reason,
        format!(
            "the source changed since this copy began: the target holds 1 row(s) the source \
             has since changed or deleted (first: {first}); a copy never rewrites a row it \
             holds, so this one cannot be finished, but the source is untouched: give the copy \
             a new, empty database and run the command again"
        )
    );
    assert_eq!(target.writes.load(Ordering::SeqCst), writes);
    assert_eq!(target.all(), held);
    let record = target.transfer_record().expect("record").expect("begun");
    assert_eq!(record.state, TransferState::Copying);
    assert!(!target.numbering_moved.load(Ordering::SeqCst));
}

#[test]
fn a_copy_resumed_over_rows_the_source_changed_or_deleted_is_refused_before_it_writes() {
    let state = BOT_RUNS.column_index("state_json").expect("state");
    refused_on_resume(
        |source| {
            let mut row = row_of(&BOT_RUNS, 3, 0);
            row[state] = text("state_json.31");
            source.put(&BOT_RUNS, row);
        },
        "bot_runs run_id=\"key-003\": state_json differs: source \"state_json.31\", target \
         \"state_json.30\"",
    );
    let (exchange, symbol, interval) = MARKETS[0];
    refused_on_resume(
        |source| source.remove(&CANDLES, &row_of(&CANDLES, 2, 0)),
        &format!("candles {exchange} {symbol} {interval} open_time_ms=6: only in the target"),
    );
}

#[test]
fn a_copy_resumed_from_a_source_that_only_gained_rows_takes_them() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    target.writes_left.store(3, Ordering::SeqCst);
    engine(&source, &target, 7).copy().expect_err("cut off");
    // New rows among those copied and among those still to copy.
    source.put(&CANDLES, row_of(&CANDLES, 0, 1));
    source.put(&BOT_RUNS, row_of(&BOT_RUNS, 8, 0));
    source.put(&ORDER_EVENTS, row_of(&ORDER_EVENTS, 8, 0));
    target.writes_left.store(usize::MAX, Ordering::SeqCst);

    let summary = engine(&source, &target, 7).copy().expect("resumed");
    assert_eq!(summary.start, CopyStart::Resumed);
    assert_eq!(target.all(), source.all());
    let record = target.transfer_record().expect("record").expect("a record");
    assert_eq!(record.state, TransferState::Copied);
    assert!(
        engine(&source, &target, 7)
            .verify()
            .expect("verify")
            .is_ok()
    );
}

#[test]
fn a_finished_copy_is_left_as_it_is_and_writes_nothing() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 4).copy().expect("copy");
    let writes = target.writes.load(Ordering::SeqCst);
    // Even a source with open runs: nothing is written.
    source.put(&BOT_RUNS, {
        let mut row = row_of(&BOT_RUNS, 99, 0);
        row[BOT_RUNS.column_index("status").expect("status")] = text("running");
        row
    });

    let summary = engine(&source, &target, 4).copy().expect("again");
    assert_eq!(summary.start, CopyStart::AlreadyCopied);
    assert!(summary.tables.is_empty());
    assert_eq!(target.writes.load(Ordering::SeqCst), writes);
}

#[test]
fn a_target_holding_rows_of_its_own_is_refused_before_anything_is_written() {
    let source = Memory::source();
    let target = Memory::new(TARGET_ID);
    target.put(&ORDER_EVENTS, row_of(&ORDER_EVENTS, 1, 0));
    target.put(&CANDLES, row_of(&CANDLES, 1, 1));

    let reason = refusal(engine(&source, &target, 4).copy());
    assert!(
        reason.contains("rows of its own (candles, order_events)"),
        "{reason}"
    );
    assert!(target.transfer_record().expect("record").is_none());
    assert_eq!(target.writes.load(Ordering::SeqCst), 0);
}

#[test]
fn the_copy_of_another_store_is_refused_and_so_is_its_verification() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    engine(&source, &target, 4).copy().expect("copy");
    let other = Memory::source().with_identity(|identity| {
        identity.store_id = StoreId::new("00000000000000000000000000000001").expect("id");
    });

    let reason = refusal(engine(&other, &target, 4).copy());
    assert!(
        reason.contains(&format!("a copy of store {SOURCE_ID}")),
        "{reason}"
    );
    assert!(matches!(
        engine(&other, &target, 4).verify(),
        Err(EngineError::TransferRefused(_))
    ));
}

#[test]
fn open_runs_are_refused_by_name_unless_the_source_is_said_to_be_quiescent() {
    let source = Memory::source();
    let status = BOT_RUNS.column_index("status").expect("status");
    for (n, state) in [(50, "running"), (51, "starting")] {
        let mut row = row_of(&BOT_RUNS, n, 0);
        row[status] = text(state);
        source.put(&BOT_RUNS, row);
    }
    let target = Memory::new(TARGET_ID);

    let reason = refusal(engine(&source, &target, 1).copy());
    assert!(
        reason.contains("key-050 (running), key-051 (starting)"),
        "{reason}"
    );
    assert!(reason.contains("--assume-quiescent"), "{reason}");
    assert_eq!(target.writes.load(Ordering::SeqCst), 0);

    let quiescent = TransferEngine::new(
        &source,
        &target,
        TransferConfig {
            batch_rows: 1,
            assume_quiescent: true,
        },
    );
    assert_eq!(quiescent.copy().expect("copy").start, CopyStart::Started);
}

#[test]
fn a_source_that_moved_fails_the_copy_and_its_verification() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    source.moved.store(true, Ordering::SeqCst);
    let reason = refusal(engine(&source, &target, 4).copy());
    assert!(reason.contains("changed while it was copied"), "{reason}");
    let record = target.transfer_record().expect("record").expect("begun");
    assert_eq!(record.state, TransferState::Copying);
    assert!(!target.numbering_moved.load(Ordering::SeqCst));

    source.moved.store(false, Ordering::SeqCst);
    engine(&source, &target, 4).copy().expect("copy");
    source.moved.store(true, Ordering::SeqCst);
    assert!(matches!(
        engine(&source, &target, 4).verify(),
        Err(EngineError::TransferRefused(reason)) if reason.contains("changed while it was verified")
    ));
}

#[test]
fn the_source_environment_is_carried_to_an_unbound_target_and_another_binding_is_refused() {
    let testnet = || {
        Memory::source().with_identity(|identity| identity.environment = Some(Environment::Testnet))
    };
    let (source, target) = (testnet(), Memory::new(TARGET_ID));
    engine(&source, &target, 4).copy().expect("copy");
    let bound = target.transfer_identity().expect("identity").environment;
    assert_eq!(bound, Some(Environment::Testnet));

    let source = testnet();
    let target = Memory::new(TARGET_ID)
        .with_identity(|identity| identity.environment = Some(Environment::Mainnet));
    let reason = refusal(engine(&source, &target, 4).copy());
    assert!(
        reason.contains("bound to mainnet, and the source to testnet"),
        "{reason}"
    );

    let source = Memory::source();
    let target = Memory::new(TARGET_ID)
        .with_identity(|identity| identity.environment = Some(Environment::Demo));
    let reason = refusal(engine(&source, &target, 4).copy());
    assert!(reason.contains("the source to undeclared"), "{reason}");
}

#[test]
fn an_active_target_and_a_store_at_another_schema_are_refused() {
    let source = Memory::source();
    let target = Memory::new(TARGET_ID)
        .with_identity(|identity| identity.activation = StoreActivation::Active);
    let reason = refusal(engine(&source, &target, 4).copy());
    assert!(reason.contains("the target is active"), "{reason}");

    let older = Memory::source().with_identity(|identity| identity.schema_version = 4);
    let reason = refusal(engine(&older, &Memory::new(TARGET_ID), 4).copy());
    assert!(reason.contains("the source is at schema 4"), "{reason}");
}

#[test]
fn a_store_failing_mid_copy_names_the_table_the_copy_stopped_in() {
    let (source, target) = (Memory::source(), Memory::new(TARGET_ID));
    target.writes_left.store(3, Ordering::SeqCst);
    let error = engine(&source, &target, 7).copy().expect_err("cut off");
    assert_eq!(
        error.to_string(),
        "the copy stopped in order_events; the rows it wrote stay, and running it again \
         finishes it"
    );
}
