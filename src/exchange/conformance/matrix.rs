//! The feature matrix: each exchange's capabilities next to its
//! conformance results, rendered as markdown tables.

use super::{Check, Outcome, Skip};
use crate::ports::{
    ExchangeCapabilities, MarketDataCapabilities, OrderCapabilities, StreamCapabilities,
};

/// One exchange's column of the matrix.
#[derive(Clone, Debug)]
pub struct ExchangeReport {
    /// The column heading, normally the exchange's registry name.
    pub name: String,
    pub capabilities: ExchangeCapabilities,
    /// What each check came to. A check left out reads "not run".
    pub outcomes: Vec<(Check, Outcome)>,
}

/// Two markdown tables, a row per capability and a row per check, with a
/// column per report. Notes stay out: they describe one run, and the
/// matrix describes the connector.
pub fn render_matrix(reports: &[ExchangeReport]) -> String {
    let names: Vec<&str> = reports.iter().map(|report| report.name.as_str()).collect();
    let mut out = String::from("## Capabilities\n\n");
    table_header(&mut out, &["Capability"], &names);
    let columns: Vec<_> = reports
        .iter()
        .map(|report| capability_rows(&report.capabilities))
        .collect();
    for (index, (label, _)) in capability_rows(&ExchangeCapabilities::default())
        .iter()
        .enumerate()
    {
        let values = columns.iter().map(|rows| rows[index].1.clone());
        table_row(&mut out, std::iter::once(label.to_string()).chain(values));
    }

    out.push_str("\n## Conformance\n\n");
    table_header(&mut out, &["Check", "Verified live"], &names);
    for &check in Check::ALL {
        let label = format!("`{}`: {}", check.id(), check.summary());
        let live = if check.runs_live() {
            "testnet"
        } else {
            "fixtures only"
        };
        let cells = reports.iter().map(|report| {
            cell(
                report
                    .outcomes
                    .iter()
                    .find(|(ran, _)| *ran == check)
                    .map(|(_, outcome)| outcome),
            )
        });
        table_row(&mut out, [label, live.to_string()].into_iter().chain(cells));
    }
    out
}

/// The matrix's capability rows, labelled.
fn capability_rows(capabilities: &ExchangeCapabilities) -> [(&'static str, String); 8] {
    // Destructured whole: a capability added to the descriptor does not
    // compile here until the matrix has a row for it.
    let ExchangeCapabilities {
        orders:
            OrderCapabilities {
                market_by_base_quantity,
                market_by_quote_quantity,
                client_order_ids,
            },
        market_data:
            MarketDataCapabilities {
                max_klines_per_request,
                symbol_rules,
                server_time,
            },
        streams: StreamCapabilities { klines, user_data },
    } = *capabilities;
    [
        (
            "market orders sized by base quantity",
            yes_no(market_by_base_quantity),
        ),
        (
            "market orders sized by quote quantity",
            yes_no(market_by_quote_quantity),
        ),
        ("client order ids", yes_no(client_order_ids)),
        (
            "candles per request, at most",
            max_klines_per_request.to_string(),
        ),
        ("symbol rules", yes_no(symbol_rules)),
        ("server time", yes_no(server_time)),
        ("candle stream", yes_no(klines)),
        ("account and order stream", yes_no(user_data)),
    ]
}

fn yes_no(supported: bool) -> String {
    if supported { "yes" } else { "no" }.to_string()
}

fn cell(outcome: Option<&Outcome>) -> String {
    match outcome {
        None => "not run".to_string(),
        Some(Outcome::Passed { .. }) => "pass".to_string(),
        Some(Outcome::Failed(_)) => "**fail**".to_string(),
        Some(Outcome::Inconclusive(_)) => "inconclusive".to_string(),
        Some(Outcome::Skipped(Skip::Unsupported(capability))) => {
            format!("not supported: {capability}")
        }
        Some(Outcome::Skipped(Skip::NoCredentials)) => "needs credentials".to_string(),
        Some(Outcome::Skipped(Skip::ReplayOnly)) => "fixtures only".to_string(),
    }
}

fn table_header(out: &mut String, leading: &[&str], names: &[&str]) {
    let headings: Vec<String> = leading
        .iter()
        .chain(names)
        .map(|heading| heading.to_string())
        .collect();
    let rule = vec!["---".to_string(); headings.len()];
    table_row(out, headings);
    table_row(out, rule);
}

fn table_row(out: &mut String, cells: impl IntoIterator<Item = String>) {
    out.push('|');
    for cell in cells {
        out.push(' ');
        out.push_str(&cell);
        out.push_str(" |");
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(name: &str, outcomes: Vec<(Check, Outcome)>) -> ExchangeReport {
        ExchangeReport {
            name: name.to_string(),
            capabilities: ExchangeCapabilities::default(),
            outcomes,
        }
    }

    #[test]
    fn every_capability_and_every_check_gets_a_row_with_a_cell_per_exchange() {
        let matrix = render_matrix(&[report("first", Vec::new()), report("second", Vec::new())]);
        let (capabilities, checks) = matrix.split_once("## Conformance").expect("two sections");
        for (table, rows, cells) in [(capabilities, 8, 3), (checks, Check::ALL.len(), 4)] {
            let lines: Vec<&str> = table.lines().filter(|line| line.starts_with('|')).collect();
            // A heading row and a rule row above the rows themselves.
            assert_eq!(lines.len(), rows + 2, "{table}");
            for line in lines {
                assert_eq!(line.matches('|').count(), cells + 1, "{line}");
            }
        }
        assert!(
            matrix.contains("| Capability | first | second |"),
            "{matrix}"
        );
        assert!(
            matrix.contains("| Check | Verified live | first | second |"),
            "{matrix}"
        );
        assert!(
            matrix.contains("| candles per request, at most | 0 | 0 |"),
            "{matrix}"
        );
    }

    #[test]
    fn each_outcome_renders_as_its_cell_and_notes_stay_out() {
        let matrix = render_matrix(&[report(
            "only",
            vec![
                (
                    Check::ServerTime,
                    Outcome::Passed {
                        note: Some("note-text".to_string()),
                    },
                ),
                (
                    Check::ClosedBars,
                    Outcome::Failed("failure-text".to_string()),
                ),
                (
                    Check::KlineBounds,
                    Outcome::Inconclusive("inconclusive-text".to_string()),
                ),
                (
                    Check::KlinePaging,
                    Outcome::Skipped(Skip::Unsupported("server time")),
                ),
                (Check::KlineGaps, Outcome::Skipped(Skip::NoCredentials)),
                (Check::ClockSkew, Outcome::Skipped(Skip::ReplayOnly)),
            ],
        )]);
        for (check, cell) in [
            (Check::ServerTime, "pass"),
            (Check::ClosedBars, "**fail**"),
            (Check::KlineBounds, "inconclusive"),
            (Check::KlinePaging, "not supported: server time"),
            (Check::KlineGaps, "needs credentials"),
            (Check::ClockSkew, "fixtures only"),
            (Check::SymbolRules, "not run"),
        ] {
            let row = matrix
                .lines()
                .find(|line| line.starts_with(&format!("| `{}`", check.id())))
                .expect("check row");
            assert!(row.ends_with(&format!("| {cell} |")), "{row}");
        }
        for detail in ["note-text", "failure-text", "inconclusive-text"] {
            assert!(!matrix.contains(detail), "{detail} in {matrix}");
        }
    }
}
