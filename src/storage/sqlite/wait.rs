//! How a connection waits for another writer's lock.
//!
//! SQLite's own busy timeout adds up the sleeps it meant to take and never
//! reads a clock, so wherever timers run late it runs late: a 1.5 s wait took
//! 3.7 s in a process at macOS's background priority, past a caller's budget.
//! It also sleeps up to 100 ms between tries, while a writer that commits and
//! starts again finds the lock free at once. So a connection waits here, by
//! the clock, and tries often.
//!
//! A call can meet a held lock more than once, as it opens and again at its
//! write, and a budget has to cover all of that: see [`Waits`].
//!
//! rusqlite's handler is a plain `fn`, so the limit is kept per thread: a
//! connection lives within one call, on the caller's thread, and is given
//! its limit as it opens.

use std::cell::Cell;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::Connection;

/// The longest sleep between two tries: a try costs next to nothing, and each
/// is a chance at a lock other writers hold for only a flush at a time.
const LONGEST_PAUSE: Duration = Duration::from_millis(10);

/// How a connection's waits for locks spend their limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Waits {
    /// Each statement may wait for all of it.
    Apart,
    /// They all end once it has passed since the connection opened: a call
    /// with a budget, however many locks it meets.
    Together,
}

thread_local! {
    /// All that a wait for a lock may take on this thread.
    static LIMIT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    /// Whether this thread's waits share that limit.
    static WAITS: Cell<Waits> = const { Cell::new(Waits::Apart) };
    /// When the limit started to run.
    static WAITING_SINCE: Cell<Instant> = Cell::new(Instant::now());
}

/// Makes `connection` give up on a lock once `limit` is spent.
pub(super) fn at_most(
    connection: &Connection,
    limit: Duration,
    waits: Waits,
) -> rusqlite::Result<()> {
    start(limit, waits);
    connection.busy_handler(Some(keep_trying))
}

/// Sets this thread's limit running.
fn start(limit: Duration, waits: Waits) {
    LIMIT.set(limit);
    WAITS.set(waits);
    WAITING_SINCE.set(Instant::now());
}

/// SQLite's question after each failed try of a statement, counted from 0:
/// whether to try again.
fn keep_trying(tries: i32) -> bool {
    try_again(thread::sleep, tries)
}

/// [`keep_trying`], pausing through `sleep`, which in a test can run late.
/// The clock alone decides, however few tries were made, and it is read again
/// on waking, so a timer that ran late ends the wait instead of buying one
/// more try.
fn try_again(sleep: impl FnOnce(Duration), tries: i32) -> bool {
    if tries == 0 && WAITS.get() == Waits::Apart {
        WAITING_SINCE.set(Instant::now());
    }
    let pause = Duration::from_millis(u64::from(tries.unsigned_abs()) + 1).min(LONGEST_PAUSE);
    if time_left() <= pause {
        return false;
    }
    sleep(pause);
    !time_left().is_zero()
}

/// What is left of the limit on this thread.
fn time_left() -> Duration {
    LIMIT.get().saturating_sub(WAITING_SINCE.get().elapsed())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tempfile::tempdir;

    use super::*;
    use crate::storage::settings::lock_wait;
    use crate::storage::sqlite::SqliteStore;
    use crate::{
        CandleStore, Environment, ExchangeId, Interval, MarketId, SegmentStore, StorageErrorKind,
        StorageLimits, StoreControl, Symbol,
    };

    /// Long enough that no stall of a loaded machine spends it by accident.
    const LIMIT_OF_A_TEST: Duration = Duration::from_millis(300);

    fn initialized(tempdir: &tempfile::TempDir, limits: StorageLimits) -> SqliteStore {
        let store = SqliteStore::with_limits(tempdir.path().join("market.sqlite"), limits);
        CandleStore::init(&store).expect("the schema");
        store
    }

    #[test]
    fn a_wait_ends_by_the_clock_however_few_tries_it_made() {
        start(LIMIT_OF_A_TEST, Waits::Apart);
        assert!(keep_trying(0), "a wait starts with time left");
        thread::sleep(LIMIT_OF_A_TEST);
        assert!(!keep_trying(1), "two tries, and its time is gone");
    }

    #[test]
    fn a_wait_keeps_trying_while_it_has_time() {
        start(Duration::from_secs(60), Waits::Apart);
        assert!((0..15).all(keep_trying));
    }

    #[test]
    fn the_next_statement_waits_afresh() {
        start(LIMIT_OF_A_TEST, Waits::Apart);
        assert!(keep_trying(0));
        thread::sleep(LIMIT_OF_A_TEST);
        assert!(!keep_trying(1));
        assert!(keep_trying(0), "a statement starts counting from 0");
    }

    #[test]
    fn waits_together_never_start_afresh() {
        start(LIMIT_OF_A_TEST, Waits::Together);
        assert!(keep_trying(0));
        thread::sleep(LIMIT_OF_A_TEST);
        assert!(!keep_trying(0), "tries count from 0 and the clock does not");
    }

    #[test]
    fn no_time_means_no_wait() {
        start(Duration::ZERO, Waits::Apart);
        assert!(!keep_trying(0));
    }

    #[test]
    fn a_sleep_that_runs_past_the_limit_ends_the_wait() {
        start(LIMIT_OF_A_TEST, Waits::Apart);
        assert!(!try_again(|_| thread::sleep(LIMIT_OF_A_TEST), 0));
    }

    #[test]
    fn a_sleep_there_is_no_time_for_is_not_taken() {
        start(LONGEST_PAUSE / 2, Waits::Apart);
        let mut slept = false;
        assert!(!try_again(|_| slept = true, 100));
        assert!(!slept);
    }

    /// Only this handler sets the time a wait began: SQLite's own timeout,
    /// which late timers stretch, would leave it as it was.
    #[test]
    fn a_store_waits_here_and_not_in_sqlite() {
        let tempdir = tempdir().expect("tempdir");
        let limits = StorageLimits {
            wait_budget: Duration::from_millis(40),
            ..StorageLimits::default()
        };
        let store = initialized(&tempdir, limits);
        let holder = store.open().expect("a connection");
        holder.execute_batch("BEGIN IMMEDIATE").expect("the lock");

        let waiter = store.open().expect("a connection");
        let before = WAITING_SINCE.get();
        waiter
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("the lock is held");
        assert!(WAITING_SINCE.get() > before);
    }

    #[test]
    fn the_statements_of_a_call_with_a_budget_have_one_wait_between_them() {
        let tempdir = tempdir().expect("tempdir");
        let store = initialized(&tempdir, StorageLimits::default());
        let holder = store.open().expect("a connection");
        holder.execute_batch("BEGIN IMMEDIATE").expect("the lock");

        let budget = Duration::from_secs(1);
        let waiter = store.within(budget).open().expect("a connection");
        waiter
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("the lock is held");
        let second = Instant::now();
        waiter
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("the lock is held");
        let took = second.elapsed();
        assert!(took < budget / 4, "it waited again, for {took:?}");
    }

    /// Held up as it opens, as by a last close's checkpoint, and then at its
    /// write by a writer that got in first. Should the call get in first
    /// instead it is held up once, and the rest of its time is the disk's.
    #[test]
    fn a_call_held_up_twice_keeps_its_budget() {
        let tempdir = tempdir().expect("tempdir");
        let store = initialized(&tempdir, StorageLimits::default());
        let whole_file = Connection::open(store.path()).expect("a connection");
        whole_file
            .execute_batch("PRAGMA locking_mode = EXCLUSIVE; BEGIN IMMEDIATE; COMMIT;")
            .expect("the file");
        let writer = Connection::open(store.path()).expect("a connection");

        let budget = Duration::from_secs(4);
        let limit = lock_wait(budget);
        let (call_ended, end_of_call) = mpsc::channel::<()>();
        let holders = thread::spawn(move || {
            thread::sleep(limit - Duration::from_millis(100));
            drop(whole_file);
            writer
                .execute_batch("BEGIN IMMEDIATE")
                .expect("the writer's lock");
            let _ = end_of_call.recv();
        });

        let started = Instant::now();
        let answer = store.within(budget).bind_environment(Environment::Testnet);
        let took = started.elapsed();
        drop(call_ended);
        holders.join().expect("the holders");

        if let Err(refused) = answer {
            assert_eq!(refused.kind(), StorageErrorKind::Contention, "{refused}");
            // Two waits would take all but 100 ms of the limit, and then all
            // of it: well past this, which leaves a stalled machine room.
            let allowance = Duration::from_secs(1);
            assert!(took < budget + allowance, "it took {took:?} of {budget:?}");
        }
    }

    /// A refresh is many calls on one connection: its budget bounds each of
    /// them, so a long refresh can still wait.
    #[test]
    fn a_refresh_with_a_budget_keeps_its_waits_apart() {
        let tempdir = tempdir().expect("tempdir");
        let store = initialized(&tempdir, StorageLimits::default());
        let market = MarketId::new(
            ExchangeId::BybitLinear,
            Symbol::new("BTCUSDT").expect("a symbol"),
            Interval::M1,
        );
        store
            .within(Duration::from_secs(1))
            .refresh_segments(&market)
            .expect("a refresh");
        assert_eq!(WAITS.get(), Waits::Apart);
    }
}
