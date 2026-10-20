//! Fixtures mode: every registered exchange passes every check it supports
//! from its fixtures, the feature matrix doc says so, the recorder keeps
//! credentials out of what it writes, and no fixture holds a secret.

use quantforge::exchange::conformance::{
    Check, Environment, ExchangeReport, Outcome, Skip, Target, run,
};
use quantforge::exchange::{self, ConnectorConfig};
use quantforge::{ApiCredentials, ExchangeId};
use std::path::Path;
use url::Url;

use crate::fixture::{self, Interaction, Source};
use crate::proxy::Proxy;
use crate::replay::Replay;
use crate::report::{self, Provenance};
use crate::{profiles, registered_factory, replay_check, skip_while_recording};

struct Results {
    reports: Vec<ExchangeReport>,
    provenance: Vec<Provenance>,
    problems: Vec<String>,
}

/// Every check of every registered exchange, replayed from its fixture.
async fn fixture_results() -> Results {
    let mut results = Results {
        reports: Vec::new(),
        provenance: Vec::new(),
        problems: Vec::new(),
    };
    for &exchange in exchange::registered() {
        let name = exchange::name(exchange);
        let fixtures = fixture::load_all(exchange);
        for id in fixtures.keys() {
            if Check::from_id(id).is_none() {
                results
                    .problems
                    .push(format!("{name}: fixture {id}.json is for no check"));
            }
        }
        let mut outcomes = Vec::new();
        for &check in Check::ALL {
            let fixture = fixtures.get(check.id());
            let interactions = fixture
                .map(|fixture| fixture.interactions.clone())
                .unwrap_or_default();
            let replayed =
                replay_check(exchange, check, interactions, registered_factory(exchange)).await;
            let id = check.id();
            match (&replayed.outcome, fixture) {
                (Outcome::Skipped(Skip::Unsupported(_)), None) => {}
                (Outcome::Skipped(Skip::Unsupported(missing)), Some(_)) => results.problems.push(
                    format!("{name}: {id} cannot run without {missing}, so its fixture is stale"),
                ),
                (_, None) => results.problems.push(format!(
                    "{name}: {id} has no fixture; record it with CONFORMANCE_MODE=record, or \
                     write it by hand if the check runs against fixtures only"
                )),
                (Outcome::Passed { .. }, Some(_)) if replayed.problems.is_empty() => {}
                (outcome, Some(_)) => results.problems.push(format!(
                    "{name}: {id} came to {outcome:?}; the replay saw {:?}",
                    replayed.problems
                )),
            }
            outcomes.push((check, replayed.outcome));
        }
        results.reports.push(ExchangeReport {
            name: name.to_string(),
            capabilities: capabilities(exchange),
            outcomes,
        });
        results
            .provenance
            .push(Provenance::of(name, fixtures.values()));
    }
    results
}

fn capabilities(exchange: ExchangeId) -> quantforge::ExchangeCapabilities {
    exchange::connect(
        exchange,
        ConnectorConfig {
            base_url: Url::parse("http://127.0.0.1:9/").expect("url"),
            credentials: None,
        },
    )
    .expect("connectors")
    .market_data
    .capabilities()
}

#[tokio::test]
async fn every_registered_exchange_passes_every_check_from_its_fixtures() {
    if skip_while_recording("every_registered_exchange_passes_every_check_from_its_fixtures") {
        return;
    }
    let results = fixture_results().await;
    assert!(
        results.problems.is_empty(),
        "\n{}",
        results.problems.join("\n")
    );
}

fn updating() -> bool {
    matches!(
        std::env::var("UPDATE_SNAPSHOTS").ok().as_deref(),
        Some("1") | Some("true")
    )
}

#[tokio::test]
async fn the_feature_matrix_doc_matches_the_fixture_results() {
    if skip_while_recording("the_feature_matrix_doc_matches_the_fixture_results") {
        return;
    }
    let results = fixture_results().await;
    let actual = report::document(&results.reports, &results.provenance);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/exchange-feature-matrix.md");
    if updating() {
        std::fs::write(&path, &actual).expect("write the feature matrix doc");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "missing {}: {err}; run `UPDATE_SNAPSHOTS=1 cargo test --test conformance`",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "docs/exchange-feature-matrix.md is stale; refresh it with \
         `UPDATE_SNAPSHOTS=1 cargo test --test conformance` and review the diff\n\
         --- committed\n{expected}\n--- generated\n{actual}"
    );
}

// Recording a replay through the proxy must give back the fixture itself,
// request for request and byte for byte, and must never write down the
// credentials it forwarded.
#[tokio::test]
async fn recording_a_replayed_check_reproduces_its_fixture_without_the_credentials() {
    if skip_while_recording(
        "recording_a_replayed_check_reproduces_its_fixture_without_the_credentials",
    ) {
        return;
    }
    const SENTINEL_KEY: &str = "SENTINEL-API-KEY";
    const SENTINEL_SECRET: &str = "SENTINEL-API-SECRET";
    let mut recorded = 0;
    for &exchange in exchange::registered() {
        let profile = profiles::profile(exchange);
        for (id, fixture) in fixture::load_all(exchange) {
            if fixture.source != Source::Recorded {
                continue;
            }
            let check = Check::from_id(&id).expect("a recorded check");
            // The fixture's account ids are blanked already; the upstream
            // serves real-looking ones, so the proxy must blank them again.
            let upstream = Replay::start(with_account_ids(&fixture.interactions), profile);
            let proxy = Proxy::start(exchange, profile, upstream.base_url());
            let target = Target {
                factory: registered_factory(exchange),
                symbol: profile.symbol(),
                interval: profile.interval,
                base_url: proxy.base_url(),
                credentials: Some(ApiCredentials {
                    api_key: SENTINEL_KEY.to_string(),
                    secret: SENTINEL_SECRET.to_string(),
                }),
                environment: Environment::Replay,
            };
            let outcome = run(&target, check).await;
            assert!(
                matches!(outcome, Outcome::Passed { .. }),
                "{id}: {outcome:?}"
            );
            assert_eq!(upstream.problems(), Vec::<String>::new(), "{id}");

            let recording = serde_json::to_string(&proxy.take()).expect("recording json");
            for secret in [SENTINEL_KEY, SENTINEL_SECRET] {
                assert!(!recording.contains(secret), "{id}: {secret} was recorded");
            }
            assert!(
                !recording.contains("354937868"),
                "{id}: an account id was recorded"
            );
            assert_eq!(
                recording,
                serde_json::to_string(&fixture.interactions).expect("fixture json"),
                "{id}: the recording differs from the fixture"
            );
            recorded += 1;
        }
    }
    assert!(
        recorded > 0,
        "no recorded fixture to record through the proxy"
    );
}

/// `interactions` with every blanked account id set to one an exchange
/// could have sent.
fn with_account_ids(interactions: &[Interaction]) -> Vec<Interaction> {
    interactions
        .iter()
        .map(|interaction| {
            let mut interaction = interaction.clone();
            if let Some(json) = &interaction.response.json {
                let identified = json.get().replace("\"uid\":0", "\"uid\":354937868");
                interaction.response.json =
                    Some(serde_json::value::RawValue::from_string(identified).expect("json"));
            }
            interaction
        })
        .collect()
}

/// The longest run of letters and digits a fixture may hold. API keys and
/// secrets, and the signatures made with them, are longer.
const LONGEST_PLAIN_TOKEN: usize = 39;

#[test]
fn no_fixture_holds_a_key_shaped_token() {
    if skip_while_recording("no_fixture_holds_a_key_shaped_token") {
        return;
    }
    let mut checked = 0;
    for &exchange in exchange::registered() {
        let Ok(entries) = std::fs::read_dir(fixture::dir(exchange)) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("fixture entry").path();
            let text = std::fs::read_to_string(&path).expect("read fixture");
            let longest = text
                .split(|c: char| !c.is_ascii_alphanumeric())
                .map(str::len)
                .max()
                .unwrap_or(0);
            assert!(
                longest <= LONGEST_PLAIN_TOKEN,
                "{} holds a {longest}-character token, shaped like a key or a signature",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked > 0, "found no fixtures to check");
}
