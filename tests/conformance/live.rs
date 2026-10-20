//! Testnet and record modes: the suite live against one exchange's testnet,
//! through the recording proxy. Nothing here runs unless
//! `CONFORMANCE_MODE` asks for it; exported keys alone never place an
//! order.

use quantforge::exchange::conformance::{
    Check, Environment, ExchangeReport, Outcome, Skip, Target, run,
};
use quantforge::exchange::{self, ConnectorConfig};
use quantforge::{ApiCredentials, ExchangeId, ms_to_rfc3339, now_utc_ms};
use std::io::Write as _;
use std::time::Duration;

use crate::fixture::{self, Fixture, Interaction, Source};
use crate::proxy::Proxy;
use crate::{Mode, mode, profiles, registered_factory, replay_check, report};

/// How long one live check may take before it is called off.
const CHECK_TIMEOUT: Duration = Duration::from_secs(120);

/// A live run, as the environment describes it.
struct LiveRun {
    exchange: ExchangeId,
    credentials: ApiCredentials,
    record: bool,
    only: Option<Check>,
}

impl LiveRun {
    /// The live run the environment asks for, or `None` in fixtures mode.
    fn from_env() -> Option<Self> {
        let record = match mode() {
            Mode::Fixtures => return None,
            Mode::Testnet => false,
            Mode::Record => true,
        };
        let names: Vec<&str> = exchange::registered()
            .iter()
            .map(|&exchange| exchange::name(exchange))
            .collect();
        let named = std::env::var("CONFORMANCE_EXCHANGE").unwrap_or_default();
        let exchange = exchange::lookup(&named).unwrap_or_else(|| {
            panic!(
                "CONFORMANCE_EXCHANGE={named:?} names no exchange; the keys belong to one of: {}",
                names.join(", ")
            )
        });
        let credentials = ApiCredentials::resolve(
            std::env::var("QF_API_KEY").ok(),
            std::env::var("QF_API_SECRET").ok(),
        )
        .unwrap_or_else(|| {
            panic!("the live modes need QF_API_KEY and QF_API_SECRET, {named}'s testnet keys")
        });
        let only = std::env::var("CONFORMANCE_CHECK")
            .ok()
            .filter(|id| !id.is_empty())
            .map(|id| {
                Check::from_id(&id)
                    .unwrap_or_else(|| panic!("CONFORMANCE_CHECK={id:?} is no check"))
            });
        Some(Self {
            exchange,
            credentials,
            record,
            only,
        })
    }

    fn checks(&self) -> Vec<Check> {
        match self.only {
            Some(check) => vec![check],
            None => Check::ALL.to_vec(),
        }
    }
}

#[tokio::test]
async fn the_suite_passes_live_against_the_selected_testnet() {
    let Some(live) = LiveRun::from_env() else {
        let _ = writeln!(
            std::io::stderr(),
            "SKIP (conformance testnet mode) live::the_suite_passes_live_against_the_selected_testnet: \
             set CONFORMANCE_MODE=testnet or record to run it"
        );
        return;
    };
    let profile = profiles::profile(live.exchange);
    let upstream = profile.testnet();
    let proxy = Proxy::start(live.exchange, profile, upstream.clone());
    let factory = registered_factory(live.exchange);

    let mut outcomes = Vec::new();
    let mut notes = Vec::new();
    for check in live.checks() {
        let _ = proxy.take();
        let target = Target {
            factory: factory.clone(),
            symbol: profile.symbol(),
            interval: profile.interval,
            base_url: proxy.base_url(),
            credentials: Some(live.credentials.clone()),
            environment: Environment::Live,
        };
        let mut outcome = tokio::time::timeout(CHECK_TIMEOUT, run(&target, check))
            .await
            .unwrap_or_else(|_| {
                Outcome::Inconclusive(format!("timed out after {} s", CHECK_TIMEOUT.as_secs()))
            });
        let traffic = proxy.take();
        if live.record && matches!(outcome, Outcome::Passed { .. }) {
            if let Err(reason) = record(live.exchange, check, traffic, upstream.as_str()).await {
                outcome = Outcome::Failed(reason);
            }
        }
        if let Outcome::Passed { note: Some(note) } = &outcome {
            notes.push((check, note.clone()));
        }
        println!("{}", line(check, &outcome));
        outcomes.push((check, outcome));
    }

    let capabilities = exchange::connect(
        live.exchange,
        ConnectorConfig {
            base_url: proxy.base_url(),
            credentials: None,
        },
    )
    .expect("connectors")
    .market_data
    .capabilities();
    let summary = report::live_summary(
        &ExchangeReport {
            name: exchange::name(live.exchange).to_string(),
            capabilities,
            outcomes: outcomes.clone(),
        },
        &notes,
    );
    println!("\n{summary}");
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("step summary");
        file.write_all(summary.as_bytes())
            .expect("write step summary");
    }

    let unsettled: Vec<String> = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Outcome::Failed(_) | Outcome::Inconclusive(_)))
        .map(|(check, outcome)| line(*check, outcome))
        .collect();
    assert!(unsettled.is_empty(), "\n{}", unsettled.join("\n"));
}

/// Writes `check`'s fixture from the live `traffic` that just passed, once
/// a replay of it passes too: a check whose requests depend on anything
/// but the responses would replay differently, and is refused.
async fn record(
    exchange: ExchangeId,
    check: Check,
    traffic: Vec<Interaction>,
    upstream: &str,
) -> Result<(), String> {
    let replayed = replay_check(
        exchange,
        check,
        traffic.clone(),
        registered_factory(exchange),
    )
    .await;
    if !matches!(replayed.outcome, Outcome::Passed { .. }) || !replayed.problems.is_empty() {
        return Err(format!(
            "passed live but replays as {:?}, with {:?}: the check is not deterministic, so its \
             fixture was not written",
            replayed.outcome, replayed.problems
        ));
    }
    let recorded_on = ms_to_rfc3339(now_utc_ms());
    fixture::save(
        exchange,
        check,
        &Fixture {
            check: check.id().to_string(),
            exchange: exchange::name(exchange).to_string(),
            source: Source::Recorded,
            recorded_on: Some(recorded_on.chars().take(10).collect()),
            recorded_from: Some(upstream.to_string()),
            note: None,
            interactions: traffic,
        },
    );
    Ok(())
}

fn line(check: Check, outcome: &Outcome) -> String {
    let id = check.id();
    match outcome {
        Outcome::Passed { note: None } => format!("PASS {id}"),
        Outcome::Passed { note: Some(note) } => format!("PASS {id} ({note})"),
        Outcome::Failed(reason) => format!("FAIL {id}: {reason}"),
        Outcome::Inconclusive(reason) => format!("INCONCLUSIVE {id}: {reason}"),
        Outcome::Skipped(Skip::ReplayOnly) => format!("SKIP {id}: fixtures only"),
        Outcome::Skipped(Skip::NoCredentials) => format!("SKIP {id}: needs credentials"),
        Outcome::Skipped(Skip::Unsupported(missing)) => format!("SKIP {id}: needs {missing}"),
        _ => format!("{id}: {outcome:?}"),
    }
}
