//! What every PostgreSQL connection is given, and what it found: its waits,
//! its commit setting, and the server settings a commit relies on
//! (`docs/storage-durability.md`).

use std::time::Duration;

use tokio_postgres::{Client, Row};

use crate::model::{DurabilityReport, DurabilitySetting};
use crate::storage::settings::lock_wait;

/// A session's limits as statements. Set with `SET`, not the `options`
/// startup parameter, which PgBouncer refuses.
pub(super) fn session_limits(lock: Duration, statement: Duration) -> String {
    format!(
        "SET lock_timeout = {}; SET statement_timeout = {};",
        millis(lock),
        millis(statement)
    )
}

/// A limit as PostgreSQL takes it. Zero would switch the limit off.
fn millis(limit: Duration) -> u128 {
    limit.as_millis().max(1)
}

/// The limits of a call with `remaining` time: a lock gives up before a
/// statement and a statement before the call, so that the server says what
/// ran out rather than the bridge calling the outcome unknown. A budget
/// only ever tightens the session's own limits.
pub(super) fn limits_within(remaining: Duration, lock: Duration, statement: Duration) -> String {
    session_limits(
        lock_wait(remaining).min(lock),
        statement_wait(remaining).min(statement),
    )
}

/// The part of a call's `remaining` time a statement may take.
pub(super) fn statement_wait(remaining: Duration) -> Duration {
    remaining * 7 / 8
}

/// Raises a commit that would not wait for its own flush, and leaves
/// anything stronger as the operator set it, standbys included.
pub(super) const FLUSHED_COMMITS: &str = "SELECT set_config('synchronous_commit', 'local', false) \
     WHERE current_setting('synchronous_commit') = 'off'";

const EFFECTIVE: &str = "SELECT current_setting('fsync'), current_setting('full_page_writes'), \
     current_setting('synchronous_commit')";

/// What the pool runs before it hands an enforcing store a connection, in
/// place of its usual check that the connection lives: it fails on one that
/// fell short, so a server weakened during a long run is found at the next
/// call. The pool then drops the connection and opens another, which the
/// hook refuses or, for `synchronous_commit`, sets right.
pub(super) const STILL_DURABLE: &str = "SELECT 1 / CASE WHEN current_setting('fsync') = 'on' \
     AND current_setting('full_page_writes') = 'on' \
     AND current_setting('synchronous_commit') <> 'off' THEN 1 ELSE 0 END";

/// What an operator does about a connection that falls short. `fsync` and
/// `full_page_writes` are the server's; `synchronous_commit` falls short
/// only when something between here and the server drops session settings.
const REMEDY: &str = "set them in the server's configuration and reload it, or use a database \
     that has them, reached without a pooler that drops session settings";

/// The settings a connection actually has.
pub(super) async fn effective(client: &Client) -> Result<DurabilityReport, tokio_postgres::Error> {
    report(&client.query_typed_one(EFFECTIVE, &[]).await?)
}

fn report(row: &Row) -> Result<DurabilityReport, tokio_postgres::Error> {
    Ok(judged(row.try_get(0)?, row.try_get(1)?, row.try_get(2)?))
}

/// The three settings, each against what a commit needs.
pub(super) fn judged(
    fsync: String,
    full_page_writes: String,
    synchronous_commit: String,
) -> DurabilityReport {
    let on = |name, effective: String| DurabilitySetting {
        name,
        met: effective == "on",
        effective,
        required: "on",
    };
    let settings = vec![
        on("fsync", fsync),
        on("full_page_writes", full_page_writes),
        DurabilitySetting {
            name: "synchronous_commit",
            met: synchronous_commit != "off",
            effective: synchronous_commit,
            required: "local or stronger",
        },
    ];
    DurabilityReport::new(settings, REMEDY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lock_gives_up_before_a_statement_and_a_statement_before_the_call() {
        for remaining in [Duration::from_millis(40), Duration::from_secs(2)] {
            assert!(lock_wait(remaining) < statement_wait(remaining));
            assert!(statement_wait(remaining) < remaining);
        }
    }

    #[test]
    fn a_limit_is_never_zero_which_would_switch_it_off() {
        assert_eq!(
            session_limits(Duration::ZERO, Duration::ZERO),
            "SET lock_timeout = 1; SET statement_timeout = 1;"
        );
    }

    #[test]
    fn each_setting_is_judged_against_what_a_commit_needs() {
        let report = |fsync: &str, full_page_writes: &str, commit: &str| {
            judged(
                fsync.to_string(),
                full_page_writes.to_string(),
                commit.to_string(),
            )
        };
        for commit in ["local", "on", "remote_write", "remote_apply"] {
            assert!(report("on", "on", commit).is_met(), "{commit}");
        }
        for (fsync, full_page_writes, commit, problem) in [
            ("off", "on", "on", "fsync is off, and needs on"),
            ("on", "off", "on", "full_page_writes is off, and needs on"),
            (
                "on",
                "on",
                "off",
                "synchronous_commit is off, and needs local or stronger",
            ),
        ] {
            let judged = report(fsync, full_page_writes, commit);
            assert!(!judged.is_met());
            let found = judged.problem().expect("a problem");
            assert!(found.starts_with(problem), "{found}");
        }
    }
}
