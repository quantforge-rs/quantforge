//! A call past its deadline: the query is cancelled on the server, its
//! connection discarded, and the next call does not queue behind it.

mod common;

use std::time::{Duration, Instant};

use postgres_bridge_spike::bridge::{BridgeConfig, Effect, PgBridge};

use common::{Observer, unique};

/// One connection, a deadline far shorter than the statement timeout.
fn bridge(url: &str, name: &str) -> PgBridge {
    let mut config = BridgeConfig::new(url, name);
    config.max_size = 1;
    config.deadline = Duration::from_millis(300);
    PgBridge::connect(&config).expect("a bridge")
}

fn sleep(bridge: &PgBridge, effect: Effect, seconds: u32) -> Result<(), String> {
    bridge
        .execute(effect, move |client| {
            Box::pin(async move {
                client
                    .batch_execute(&format!("SELECT pg_sleep({seconds})"))
                    .await
            })
        })
        .map_err(|error| error.kind().to_string())
}

#[test]
fn a_read_past_its_deadline_is_cancelled_and_its_connection_discarded() {
    let url = database_or_skip!();
    let observer = Observer::connect(&url);
    let name = unique("cancel");
    let bridge = bridge(&url, &name);

    // Far longer than the observer waits below: without the cancel the
    // server would still be sleeping when it looks, since it notices a
    // vanished client only when it next writes to it.
    let started = Instant::now();
    assert_eq!(
        sleep(&bridge, Effect::Read, 30),
        Err("unavailable".to_string())
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // The pool's only connection was discarded, not left busy: the next
    // call gets a fresh one at once.
    let started = Instant::now();
    assert_eq!(sleep(&bridge, Effect::Read, 0), Ok(()));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );

    // And the server stopped the abandoned query instead of running it out.
    assert_eq!(observer.wait_for(0, |observer| observer.sleeping(&name)), 0);
}

#[test]
fn a_write_past_its_deadline_has_an_unknown_outcome() {
    let url = database_or_skip!();
    let bridge = bridge(&url, &unique("cancel_write"));
    assert_eq!(
        sleep(&bridge, Effect::Write, 5),
        Err("outcome_unknown".to_string())
    );
}

#[test]
fn a_statement_the_database_refuses_is_its_own_kind() {
    let url = database_or_skip!();
    let bridge = bridge(&url, &unique("refused"));
    let refused = bridge
        .execute(Effect::Read, |client| {
            Box::pin(async move { client.batch_execute("SELECT * FROM no_such_table").await })
        })
        .expect_err("refused");
    assert_eq!(refused.kind(), "database");
}

#[test]
fn a_database_that_does_not_answer_is_unavailable() {
    let mut config = BridgeConfig::new("postgres://nobody@127.0.0.1:1/none", "nowhere");
    config.connect_timeout = Duration::from_millis(500);
    let refused = PgBridge::connect(&config).expect_err("nothing listens on port 1");
    assert_eq!(refused.kind(), "unavailable", "{refused}");
}
