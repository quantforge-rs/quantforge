//! The reports the suite writes: `docs/exchange-feature-matrix.md` from
//! the fixtures, and a live run's summary.

use quantforge::exchange::conformance::{Check, ExchangeReport, render_matrix};
use std::collections::BTreeSet;

use crate::fixture::{Fixture, Source};

/// Where one exchange's fixtures came from.
pub(crate) struct Provenance {
    name: String,
    recorded: usize,
    authored: usize,
    recorded_from: BTreeSet<String>,
    last_recorded: Option<String>,
}

impl Provenance {
    pub(crate) fn of<'a>(name: &str, fixtures: impl IntoIterator<Item = &'a Fixture>) -> Self {
        let mut provenance = Self {
            name: name.to_string(),
            recorded: 0,
            authored: 0,
            recorded_from: BTreeSet::new(),
            last_recorded: None,
        };
        for fixture in fixtures {
            match fixture.source {
                Source::Recorded => provenance.recorded += 1,
                Source::Authored => provenance.authored += 1,
            }
            provenance
                .recorded_from
                .extend(fixture.recorded_from.iter().cloned());
            // ISO dates sort as text.
            provenance.last_recorded = provenance
                .last_recorded
                .take()
                .max(fixture.recorded_on.clone());
        }
        provenance
    }
}

/// The doc's opening, with the live and fixtures-only checks counted from
/// the suite itself.
fn intro() -> String {
    let live = Check::ALL.iter().filter(|check| check.runs_live()).count();
    let fixtures_only = Check::ALL.len() - live;
    format!(
        "\
# Exchange feature matrix

What each registered exchange supports, and how its connector fares
against the conformance suite in
[`src/exchange/conformance/`](../src/exchange/conformance/mod.rs), whose
module docs state the contract. The results below replay each exchange's
committed fixtures. The scheduled `conformance` workflow also runs the
{live} checks marked `testnet` live against each testnet every day; the
{fixtures_only} marked `fixtures only` script faults no testnet produces on
demand, so they run against hand-written fixtures alone.

This page is generated: refresh it with
`UPDATE_SNAPSHOTS=1 cargo test --test conformance`, never by hand. To add
an exchange, see [adding-an-exchange.md](adding-an-exchange.md).

"
    )
}

const FIXTURES_NOTE: &str = "
A recorded fixture is the transcript of a passing live run against the
testnet, replayed request for request. A hand-written one scripts a fault
no testnet produces on demand — clock skew, rate limits, outages, garbled
responses — from the exchange's documented error responses, so those
checks run against fixtures only.
";

const NOT_COVERED: &str = "
## Not covered yet

- Applying the price tick to orders: the connectors place market orders
  only, so no order carries a price. The tick is checked against candle
  prices instead.
- Compensating clock drift: a request outside the exchange's time window
  fails as `clock_skew`, and resynchronizing is the caller's job.
- Streams: no connector pushes candles or account updates yet.
";

/// The feature matrix doc.
pub(crate) fn document(reports: &[ExchangeReport], provenance: &[Provenance]) -> String {
    let mut doc = intro();
    doc.push_str(&render_matrix(reports));
    doc.push_str(
        "\n## Fixtures\n\n\
         | Exchange | Recorded | Hand-written | Recorded from | Last recorded |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for exchange in provenance {
        let from: Vec<String> = exchange
            .recorded_from
            .iter()
            .map(|url| format!("`{url}`"))
            .collect();
        doc.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            exchange.name,
            exchange.recorded,
            exchange.authored,
            if from.is_empty() {
                "—".to_string()
            } else {
                from.join(", ")
            },
            exchange.last_recorded.as_deref().unwrap_or("—"),
        ));
    }
    doc.push_str(FIXTURES_NOTE);
    doc.push_str(NOT_COVERED);
    doc
}

/// A live run's summary: its matrix, and what each passing check noted.
pub(crate) fn live_summary(report: &ExchangeReport, notes: &[(Check, String)]) -> String {
    let mut summary = format!(
        "# Conformance on the {} testnet\n\n{}",
        report.name,
        render_matrix(std::slice::from_ref(report))
    );
    if !notes.is_empty() {
        summary.push_str("\n## Notes\n\n");
        for (check, note) in notes {
            summary.push_str(&format!("- `{}`: {note}\n", check.id()));
        }
    }
    summary
}
