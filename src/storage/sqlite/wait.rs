//! How a connection waits for another writer's lock.
//!
//! SQLite's own busy timeout adds up the sleeps it meant to take and never
//! reads a clock, so wherever timers run late it runs late: a 1.5 s wait took
//! 3.7 s in a process at macOS's background priority, past a caller's budget.
//! It also sleeps up to 100 ms between tries, while a writer that commits and
//! starts again finds the lock free at once. So a connection waits here, by
//! the clock, and tries often.
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

thread_local! {
    /// All that one wait for a lock may take on this thread.
    static LIMIT: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    /// When this thread's latest wait began.
    static WAITING_SINCE: Cell<Instant> = Cell::new(Instant::now());
}

/// Makes `connection` give up on a lock once a wait has taken `limit`.
pub(super) fn at_most(connection: &Connection, limit: Duration) -> rusqlite::Result<()> {
    LIMIT.set(limit);
    connection.busy_handler(Some(keep_trying))
}

/// SQLite's question after each failed try of a statement, counted from 0:
/// whether to try again. A statement that took a sleep too long is told so
/// by the clock, however few tries it made.
fn keep_trying(tries: i32) -> bool {
    let now = Instant::now();
    if tries == 0 {
        WAITING_SINCE.set(now);
    }
    let waited = now.saturating_duration_since(WAITING_SINCE.get());
    let left = LIMIT.get().saturating_sub(waited);
    if left.is_zero() {
        return false;
    }
    let pause = Duration::from_millis(u64::from(tries.unsigned_abs()) + 1);
    thread::sleep(pause.min(LONGEST_PAUSE).min(left));
    true
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::CandleStore;
    use crate::storage::sqlite::SqliteStore;

    #[test]
    fn a_wait_ends_by_the_clock_however_few_tries_it_made() {
        LIMIT.set(Duration::from_millis(30));
        assert!(keep_trying(0), "a wait starts with time left");
        thread::sleep(Duration::from_millis(40));
        assert!(!keep_trying(1), "two tries, and its time is gone");
    }

    #[test]
    fn a_wait_keeps_trying_while_it_has_time() {
        LIMIT.set(Duration::from_secs(60));
        assert!((0..15).all(keep_trying));
    }

    #[test]
    fn the_next_statement_waits_afresh() {
        LIMIT.set(Duration::from_millis(30));
        assert!(keep_trying(0));
        thread::sleep(Duration::from_millis(40));
        assert!(!keep_trying(1));
        assert!(keep_trying(0), "a statement starts counting from 0");
    }

    #[test]
    fn no_time_means_no_wait() {
        LIMIT.set(Duration::ZERO);
        assert!(!keep_trying(0));
    }

    /// Only this handler sets the time a wait began: SQLite's own timeout,
    /// which late timers stretch, would leave it as it was.
    #[test]
    fn a_store_waits_here_and_not_in_sqlite() {
        let tempdir = tempdir().expect("tempdir");
        let store = SqliteStore::new(tempdir.path().join("market.sqlite"));
        CandleStore::init(&store).expect("the schema");
        let holder = store.open().expect("a connection");
        holder.execute_batch("BEGIN IMMEDIATE").expect("the lock");

        let before = WAITING_SINCE.get();
        let waiter = store
            .within(Duration::from_millis(40))
            .open()
            .expect("a connection");
        waiter
            .execute_batch("BEGIN IMMEDIATE")
            .expect_err("the lock is held");
        assert!(WAITING_SINCE.get() > before);
    }
}
