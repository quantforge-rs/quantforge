//! A process that dies while it works, on each backend. A writer takes
//! signals through their whole life, round after round: admitted, its
//! order recorded with its reservation, settled by a fill that opens a
//! cycle, the cycle closed. It reports each step once the store answered.
//! In one test it is killed with `SIGKILL` at no step in particular; in
//! the other it dies at each boundary in turn, right after the step is
//! answered and before the next begins.
//!
//! Whatever the store then holds is whole: every write the writer
//! reported is there, and every write that is there is there with all
//! its parts, whether or not it was reported. A process that starts on
//! the store finds what was left unfinished, asks the venue for it, and
//! goes on.
//!
//! As `tests/storage/crash.rs` says of its own: this is an application
//! crash, and says nothing of the operating system or the power.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use quantforge::automation::{Evidence, ExecutionLedger, Resumed};
use quantforge::model::Asset;
use quantforge::model::execution::{Change, Record};
use quantforge::model::futures::{CloseReason, FuturesOrderStatus, IntentState, PoolScope};
use quantforge::{ExecutionStore, StoreControl};

use crate::fixtures::{fenced, line, reported};
use crate::orders::started;
use crate::support::{Backend, Manual, Sqlite, T, block_on, count, scalar};
use crate::venue::Venue;

/// Set for the child: the address of the store it writes to.
const WRITER: &str = "QF_TEST_EXECUTION_WRITER";

/// Set for a child that dies on its own: the round and the step it dies
/// after, as `round:step`.
const DIES_AFTER: &str = "QF_TEST_EXECUTION_WRITER_DIES_AFTER";

/// What the child prints once a step is answered: the round and the step.
const ACKNOWLEDGED: &str = "acknowledged ";

/// How many acknowledgements a crash waits for: some rounds whole, and
/// one cut short.
const ACKNOWLEDGEMENTS: usize = 30;

const STEPS: [&str; 4] = ["admitted", "attempt", "settled", "closed"];

fn usdt() -> PoolScope {
    PoolScope::Asset(Asset::new("USDT").expect("an asset"))
}

/// Takes one signal after another through its whole life, until stopped
/// from outside.
fn write_until_stopped(store: &(impl ExecutionStore + StoreControl)) {
    let line = line("bybit linear");
    let clock = Manual::at(T);
    let ledger = ExecutionLedger::new(store, &clock);
    let account = line.identity.account_ref().clone();
    let lease = line.lease(store, "writer", T);
    let last = std::env::var(DIES_AFTER).ok();
    let said = |round: u32, step: &str| {
        println!("{ACKNOWLEDGED}{round} {step}");
        if last.as_deref() == Some(format!("{round}:{step}").as_str()) {
            // No destructor runs and nothing is given back: the process
            // is gone between two steps. An exit, not an abort, so that no
            // platform writes a dump or asks anyone about it.
            std::process::exit(9);
        }
    };
    // Far more rounds than a writer lives through before it is killed.
    for round in 0..1_000_000_u32 {
        let now = T + i64::from(round) * 10_000;
        clock.set(now);
        let lease = store
            .renew_lease(&lease.fence(), 60_000, now)
            .expect("asked")
            .expect("its own lease");
        let entry = started(store, &ledger, &line, "entry", &format!("s{round}"), 0);
        said(round, STEPS[0]);

        let pool = store.pool(&account, &usdt()).expect("the pool");
        let held = line.reservation(entry.operation, "205.18548", now);
        let sent = ledger
            .record_attempt(
                &lease.fence(),
                &entry.intent,
                &entry.id,
                Some((held, pool.revision)),
            )
            .expect("recorded");
        said(round, STEPS[1]);

        let order = reported(&line, &entry.id, FuturesOrderStatus::Filled, "4.38", "4.38");
        let trade = format!("t-{round}");
        let fills = [line
            .fill(&trade, &entry.id, "4.38", "234.25", entry.operation, None)
            .fill()
            .clone()];
        let evidence = Evidence::Order {
            order: &order,
            fills: &fills,
        };
        let filled = ledger
            .settle(&lease.fence(), &sent, evidence)
            .expect("settled");
        said(round, STEPS[2]);

        // The position is closed by hand, so that the next round's entry
        // finds it flat.
        let cycle = filled.record.cycle().expect("the cycle it opened");
        close(store, &line, cycle, &lease.fence(), now + 5_000);
        said(round, STEPS[3]);
    }
}

/// Closes `cycle` on the evidence of a flat snapshot.
fn close(
    store: &impl ExecutionStore,
    line: &crate::fixtures::Line,
    cycle: quantforge::model::futures::PositionCycleId,
    fence: &quantforge::model::execution::Fence,
    at_ms: i64,
) {
    let open = store.cycle(cycle).expect("a read").expect("the cycle");
    let clock = Manual::at(at_ms);
    let venue = Venue::new(line.instrument().exchange, &clock);
    let flat = block_on(quantforge::ports::futures::AccountStateConnector::position(
        &venue,
        &line.instrument(),
    ))
    .expect("a snapshot");
    let mut closed = open.record.cycle().clone();
    closed.close(&flat, CloseReason::Manual).expect("closed");
    let record = open.record.with_cycle(closed).expect("a successor");
    let unit = fenced(fence, at_ms).with(Change::replace(Record::Cycle(record), open.revision));
    assert!(store.commit(&unit).expect("a commit").is_applied());
}

/// Not a test: the child the tests below start, which returns at once
/// everywhere else.
#[test]
fn writer() {
    let Ok(address) = std::env::var(WRITER) else {
        return;
    };
    if let Some(store) = Sqlite::at(&address) {
        write_until_stopped(&store);
    }
    #[cfg(feature = "postgres")]
    if let Some(store) = crate::support::postgres::Postgres::at(&address) {
        write_until_stopped(&store);
    }
}

/// Starts this test binary as the writer, which dies on its own after
/// the step `dies_after` names, if it names one.
fn start_writer(address: &str, dies_after: Option<&str>) -> Child {
    let mut writer = Command::new(std::env::current_exe().expect("this test binary"));
    writer
        .args(["crash::writer", "--exact", "--nocapture"])
        .env(WRITER, address)
        .env_remove(DIES_AFTER)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(step) = dies_after {
        writer.env(DIES_AFTER, step);
    }
    writer.spawn().expect("the writer")
}

/// Reads all a writer that dies on its own reported before it died.
fn acknowledged_until_it_died(mut child: Child) -> Vec<(u32, String)> {
    let stdout = child.stdout.take().expect("the writer's stdout");
    let acknowledged = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| {
            let (round, step) = line.strip_prefix(ACKNOWLEDGED)?.split_once(' ')?;
            Some((round.parse().expect("a round"), step.to_string()))
        })
        .collect();
    let end = child.wait().expect("the writer's end");
    assert!(!end.success(), "the writer died: {end:?}");
    acknowledged
}

/// Reads the writer's acknowledgements, kills it once there are enough,
/// and returns all it reported: the round and the step of each.
fn acknowledged_until_killed(mut child: Child) -> Vec<(u32, String)> {
    let stdout = child.stdout.take().expect("the writer's stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut acknowledged = Vec::new();
    let read = |line: String, acknowledged: &mut Vec<(u32, String)>| {
        if let Some((round, step)) = line
            .strip_prefix(ACKNOWLEDGED)
            .and_then(|said| said.split_once(' '))
        {
            acknowledged.push((round.parse().expect("a round"), step.to_string()));
        }
    };
    for line in lines.by_ref() {
        read(line.expect("a line"), &mut acknowledged);
        if acknowledged.len() == ACKNOWLEDGEMENTS {
            break;
        }
    }
    assert_eq!(acknowledged.len(), ACKNOWLEDGEMENTS, "the writer stopped");
    child.kill().expect("killed");
    // What it had reported by the time it died counts too.
    for line in lines.map_while(Result::ok) {
        read(line, &mut acknowledged);
    }
    child.wait().expect("the writer's end");
    acknowledged
}

fn a_killed_writer_leaves_every_write_whole_and_what_it_left_is_resumed<B: Backend>(backend: &B) {
    let store = backend.open();
    store.activate(T).expect("activated");
    let acknowledged = acknowledged_until_killed(start_writer(&backend.address(), None));
    whole_and_resumed(backend, &store, &acknowledged);
}

/// The writer dies right after each step of its second signal in turn:
/// admitted, its attempt recorded, settled, its cycle closed. Each time
/// the store holds exactly what was answered, and nothing of the step
/// that never began.
fn a_writer_that_dies_at_each_boundary_leaves_exactly_what_it_was_answered<B: Backend>(
    fresh: impl Fn() -> Option<B>,
) {
    // What the second signal's life has left behind at each boundary:
    // intents that are sent and unsettled, reservations held, cycles open.
    for (step, unsettled, held, open) in [
        (STEPS[0], 0, 0, 0),
        (STEPS[1], 1, 1, 0),
        (STEPS[2], 0, 0, 1),
        (STEPS[3], 0, 0, 0),
    ] {
        let Some(backend) = fresh() else {
            return;
        };
        let store = backend.open();
        store.activate(T).expect("activated");
        let writer = start_writer(&backend.address(), Some(&format!("1:{step}")));
        let acknowledged = acknowledged_until_it_died(writer);
        let position = STEPS
            .iter()
            .position(|known| *known == step)
            .expect("a step");
        assert_eq!(acknowledged.len(), STEPS.len() + position + 1, "{step}");
        assert_eq!(acknowledged.last(), Some(&(1, step.to_string())));

        let number = |sql: &str| -> usize { scalar(&backend, sql).parse().expect("a count") };
        assert_eq!(count(&backend, "signal_inbox"), 2, "{step}");
        assert_eq!(count(&backend, "execution_intents"), 2, "{step}");
        let sent =
            number("SELECT CAST(COUNT(*) AS TEXT) FROM execution_intents WHERE entry_slot = 1");
        assert_eq!(sent, unsettled, "entries on their way after {step}");
        assert_eq!(
            count(&backend, "execution_orders"),
            1 + usize::from(position >= 1)
        );
        assert_eq!(
            count(&backend, "execution_fills"),
            1 + usize::from(position >= 2)
        );
        let reserved =
            number("SELECT CAST(COUNT(*) AS TEXT) FROM margin_reservations WHERE state = 'held'");
        assert_eq!(reserved, held, "margin held after {step}");
        let cycles =
            number("SELECT CAST(COUNT(*) AS TEXT) FROM position_cycles WHERE open_slot = 1");
        assert_eq!(cycles, open, "open cycles after {step}");
        whole_and_resumed(&backend, &store, &acknowledged);
    }
}

/// Whatever a writer that died left: every step it reported is there,
/// everything there is whole, and a process that starts on the store
/// resumes it.
fn whole_and_resumed<B: Backend>(backend: &B, store: &B::Store, acknowledged: &[(u32, String)]) {
    let line = line("bybit linear");
    let number = |sql: &str| -> usize { scalar(backend, sql).parse().expect("a count") };
    let state = |state: &str| {
        number(&format!(
            "SELECT CAST(COUNT(*) AS TEXT) FROM execution_intents WHERE state = '{state}'"
        ))
    };
    let events = |kind: &str| {
        number(&format!(
            "SELECT CAST(COUNT(*) AS TEXT) FROM outbox_events WHERE kind = '{kind}'"
        ))
    };

    // Every step it reported is in the store.
    let reported = |step: &str| acknowledged.iter().filter(|(_, said)| said == step).count();
    assert!(count(backend, "signal_inbox") >= reported("admitted"));
    assert!(count(backend, "execution_orders") >= reported("attempt"));
    assert!(state("filled") >= reported("settled"));
    let closed =
        number("SELECT CAST(COUNT(*) AS TEXT) FROM position_cycles WHERE open_slot IS NULL");
    assert!(closed >= reported("closed"));

    // And everything in the store is there whole, reported or not.
    let signals = count(backend, "signal_inbox");
    let intents = count(backend, "execution_intents");
    let orders = count(backend, "execution_orders");
    let filled = state("filled");
    assert_eq!(intents, signals, "an admitted signal has its intent");
    assert_eq!(events("signal_admitted"), signals, "and its event");
    assert_eq!(count(backend, "signal_deliveries"), signals);
    assert_eq!(
        orders,
        intents - state("admitted"),
        "a sent intent, its order"
    );
    assert_eq!(events("order_submitted"), orders);
    assert_eq!(
        count(backend, "margin_reservations"),
        orders,
        "and its margin"
    );
    assert_eq!(
        count(backend, "position_cycles"),
        filled,
        "a fill, its cycle"
    );
    assert_eq!(count(backend, "execution_fills"), filled);
    assert_eq!(events("order_settled"), filled);
    let held =
        number("SELECT CAST(COUNT(*) AS TEXT) FROM margin_reservations WHERE state = 'held'");
    assert_eq!(
        held,
        orders - filled,
        "margin is held only by what is unsettled"
    );
    assert!(orders - filled <= 1, "one entry on its way at most");
    let pool = store
        .pool(line.identity.account_ref(), &usdt())
        .expect("the pool");
    assert_eq!(
        pool.revision as usize,
        orders + filled,
        "each reserve and release"
    );
    assert_eq!(pool.held.len(), held);
    let open =
        number("SELECT CAST(COUNT(*) AS TEXT) FROM position_cycles WHERE open_slot IS NOT NULL");
    assert!(open <= 1, "one open cycle at most");

    // A process that starts on the store takes the position over, asks
    // the venue about what it finds unfinished, and sends nothing.
    let far = T + 86_400_000;
    let clock = Manual::at(far);
    let ledger = ExecutionLedger::new(store, &clock);
    let lease = line.lease(store, "successor", far);
    assert_eq!(lease.fence.get(), 2, "the writer's lease has run out");
    let venue = Venue::new(line.instrument().exchange, &clock);
    for unfinished in store.unsettled_intents().expect("what to resume") {
        if unfinished.record.intent().state() == IntentState::Admitted {
            continue;
        }
        let resumed =
            block_on(ledger.resume(&lease.fence(), &unfinished, &venue, &venue)).expect("asked");
        // The writer sends nothing, so the venue knows nothing: never
        // placed, a day later, with the position flat.
        let Resumed::Settled(settled) = resumed else {
            panic!("settled by the venue's answer: {resumed:?}");
        };
        assert_eq!(settled.record.intent().state(), IntentState::NotPlaced);
    }
    if let Some(left_open) = store.open_cycle(&line.key()).expect("a read") {
        close(store, &line, left_open.record.id(), &lease.fence(), far);
    }
    assert_eq!(
        store
            .pool(line.identity.account_ref(), &usdt())
            .expect("the pool")
            .held,
        [],
        "nothing is left reserved"
    );
    // The store is not wedged: the next signal goes its whole way.
    let next = started(store, &ledger, &line, "entry", "after-the-crash", 0);
    let sent = ledger
        .record_attempt(&lease.fence(), &next.intent, &next.id, None)
        .expect("recorded");
    assert_eq!(sent.record.intent().state(), IntentState::Submitting);
}

mod a_killed_writer_leaves_every_write_whole {
    use super::*;

    #[test]
    fn sqlite() {
        a_killed_writer_leaves_every_write_whole_and_what_it_left_is_resumed(&Sqlite::new());
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres() {
        if let Some(backend) = crate::support::postgres::Postgres::new() {
            a_killed_writer_leaves_every_write_whole_and_what_it_left_is_resumed(&backend);
        }
    }
}

mod a_writer_that_dies_at_each_boundary {
    use super::*;

    #[test]
    fn sqlite() {
        a_writer_that_dies_at_each_boundary_leaves_exactly_what_it_was_answered(|| {
            Some(Sqlite::new())
        });
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres() {
        a_writer_that_dies_at_each_boundary_leaves_exactly_what_it_was_answered(
            crate::support::postgres::Postgres::new,
        );
    }
}
