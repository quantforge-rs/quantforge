//! The exchange conformance suite, run against every registered exchange.
//!
//! - **Fixtures mode**, the default and what CI runs: every check replays
//!   its transcript from `fixtures/<exchange>/<check>.json`, with no
//!   network. [`offline`] also keeps the feature matrix doc, the fixtures,
//!   and the recorder honest, and [`controls`] proves every check can fail.
//! - **Testnet mode**, `CONFORMANCE_MODE=testnet`: [`live`] runs every check
//!   that can run live against the testnet of the exchange
//!   `CONFORMANCE_EXCHANGE` names,
//!   signing with that exchange's testnet keys from `QF_API_KEY` and
//!   `QF_API_SECRET`. The checks place small market orders there.
//! - **Record mode**, `CONFORMANCE_MODE=record`: testnet mode, and every
//!   live check that passes, and replays exactly as it ran, rewrites its
//!   fixture. `CONFORMANCE_CHECK=<id>` narrows a live run to one check.
//!
//! Run the live modes alone, with their output:
//! `cargo test --test conformance live:: -- --nocapture`.

mod controls;
mod fixture;
mod live;
mod offline;
mod profiles;
mod proxy;
mod replay;
mod report;
mod server;

use quantforge::exchange::conformance::{Check, Environment, Factory, Outcome, Target, run};
use quantforge::{ApiCredentials, ExchangeId};

use fixture::Interaction;
use replay::Replay;

/// What a run of the suite does, from `CONFORMANCE_MODE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Fixtures,
    Testnet,
    Record,
}

/// The mode `CONFORMANCE_MODE` selects. Anything unrecognized fails rather
/// than falling back, so a typo never quietly runs the wrong mode.
pub(crate) fn mode() -> Mode {
    match std::env::var("CONFORMANCE_MODE").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("") | Ok("fixtures") => Mode::Fixtures,
        Ok("testnet") => Mode::Testnet,
        Ok("record") => Mode::Record,
        Ok(other) => {
            panic!("CONFORMANCE_MODE={other:?} is not a mode: use fixtures, testnet, or record")
        }
        Err(err) => panic!("CONFORMANCE_MODE is unreadable: {err}"),
    }
}

/// Whether `test` must stand down because record mode is rewriting the
/// fixtures it reads; says so on stderr, past the test harness's capture.
pub(crate) fn skip_while_recording(test: &str) -> bool {
    if mode() != Mode::Record {
        return false;
    }
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr(),
        "SKIP (record mode) {test}: it reads the fixtures being rewritten"
    );
    true
}

/// What a replayed check comes to, and what went wrong with the replay.
pub(crate) struct Replayed {
    pub(crate) outcome: Outcome,
    pub(crate) problems: Vec<String>,
}

/// Runs `check` against connectors from `factory`, served by a replay of
/// `interactions`, signing with credentials no exchange would accept.
pub(crate) async fn replay_check(
    exchange: ExchangeId,
    check: Check,
    interactions: Vec<Interaction>,
    factory: Factory,
) -> Replayed {
    let profile = profiles::profile(exchange);
    let replay = Replay::start(interactions, profile);
    let target = Target {
        factory,
        symbol: profile.symbol(),
        interval: profile.interval,
        base_url: replay.base_url(),
        credentials: Some(replay_credentials()),
        environment: Environment::Replay,
    };
    let outcome = run(&target, check).await;
    Replayed {
        outcome,
        problems: replay.problems(),
    }
}

/// The registry's own factory for `exchange`.
pub(crate) fn registered_factory(exchange: ExchangeId) -> Factory {
    std::sync::Arc::new(move |config| quantforge::exchange::connect(exchange, config))
}

fn replay_credentials() -> ApiCredentials {
    ApiCredentials {
        api_key: "conformance-key".to_string(),
        secret: "conformance-secret".to_string(),
    }
}
