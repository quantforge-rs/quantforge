//! `docs/signal-payload.md` held to the code and to the shared cases: the
//! format's fields, limits and problems, a signal's identity and its worked
//! example, the order of the rules a signal is resolved by, the receipts,
//! the alert templates, and the lists of cases. The lists sit between
//! `<!-- begin: name -->` and `<!-- end: name -->`.

use std::collections::BTreeSet;

use quantforge::model::Refusal;
use quantforge::model::futures::{SIGNAL_FUTURE_ALLOWANCE_MS, SignalAgeLimit, TickerSpelling};
use quantforge::protocol::signal::{
    MAX_SIGNAL_BYTES, MAX_SIGNAL_DEPTH, MAX_TICKER_CHARS, ReceiptState, SIGNAL_FIELDS,
    SIGNAL_VERSION, Signal, SignalProblemCode, SignalReceipt,
};
use serde_json::Value;

use crate::cases::{Payload, ProblemAnswer, fixture, read};

const PAGE: &str = "docs/signal-payload.md";

/// The lines of a page between a list's markers.
fn section_of(page: &str, name: &str) -> Vec<String> {
    let doc = read(page);
    let lines: Vec<&str> = doc.lines().collect();
    let at = |edge: &str| {
        let marker = format!("<!-- {edge}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("{page} has no {marker}"))
    };
    lines[at("begin") + 1..at("end")]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

fn section(name: &str) -> Vec<String> {
    section_of(PAGE, name)
}

/// The items of a list, each joined onto one line: `- `name`: statement`,
/// a name alone, or a name without backticks.
fn items_of(page: &str, name: &str) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    for line in section_of(page, name) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(item) = line.strip_prefix("- ") {
            let (id, statement) = match item.strip_prefix('`').and_then(|rest| rest.split_once('`'))
            {
                Some((id, rest)) => (id, rest.trim_start_matches(':').trim()),
                None => item.split_once(": ").unwrap_or((item, "")),
            };
            items.push((id.to_string(), statement.to_string()));
        } else {
            let (_, statement) = items
                .last_mut()
                .unwrap_or_else(|| panic!("a line before the first item: {line}"));
            if !statement.is_empty() {
                statement.push(' ');
            }
            statement.push_str(line.trim());
        }
    }
    items
}

fn items(name: &str) -> Vec<(String, String)> {
    items_of(PAGE, name)
}

fn names(list: &str) -> Vec<String> {
    let listed: Vec<String> = items(list).into_iter().map(|(name, _)| name).collect();
    let distinct: BTreeSet<&String> = listed.iter().collect();
    assert_eq!(distinct.len(), listed.len(), "{list} names each once");
    listed
}

/// A list's one value.
fn value(name: &str) -> String {
    let lines: Vec<String> = section(name)
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let [value] = lines.as_slice() else {
        panic!("{name} holds one value: {lines:?}");
    };
    value.clone()
}

/// The words a statement puts in backticks.
fn quoted(statement: &str) -> Vec<&str> {
    statement.split('`').skip(1).step_by(2).collect()
}

/// The fenced blocks of a list, each as its lines.
fn fenced(name: &str) -> Vec<Vec<String>> {
    let mut blocks = Vec::new();
    let mut open: Option<Vec<String>> = None;
    for line in section(name) {
        match (&mut open, line.starts_with("```")) {
            (None, true) => open = Some(Vec::new()),
            (Some(block), true) => {
                blocks.push(std::mem::take(block));
                open = None;
            }
            (Some(block), false) => block.push(line),
            (None, false) => {}
        }
    }
    assert!(open.is_none(), "{name} leaves a block open");
    blocks
}

/// A payload's problems, as the page writes them.
fn said(problems: &[ProblemAnswer]) -> String {
    let written: Vec<String> = problems
        .iter()
        .map(|problem| match &problem.field {
            Some(field) => format!("`{}` `{field}`", problem.code),
            None => format!("`{}`", problem.code),
        })
        .collect();
    written.join(", ")
}

fn listed<'a>(cases: impl Iterator<Item = (&'a str, String)>) -> Vec<(String, String)> {
    cases
        .map(|(id, statement)| (id.to_string(), statement))
        .collect()
}

#[test]
fn the_page_states_the_formats_version_fields_and_limits_as_the_code_has_them() {
    let fixture = fixture();
    assert_eq!(value("signal version"), SIGNAL_VERSION.to_string());
    assert_eq!(value("fixture format"), fixture.format.to_string());
    assert_eq!(names("signal fields"), SIGNAL_FIELDS);
    assert_eq!(names("entry only"), fixture.entry_only);
    // A field is optional exactly where the page says so.
    for (field, statement) in items("signal fields") {
        let optional = !fixture.required["entry"].contains(&field);
        assert_eq!(statement.starts_with("optional; "), optional, "{field}");
    }
    let limits: Vec<String> = section("signal limits")
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        limits,
        [
            format!("- a body: {MAX_SIGNAL_BYTES} bytes"),
            format!("- nesting: {MAX_SIGNAL_DEPTH} levels, the payload's own object included"),
            format!("- a ticker: {MAX_TICKER_CHARS} characters"),
        ]
    );
    let codes: Vec<&str> = SignalProblemCode::ALL
        .iter()
        .map(|code| code.as_str())
        .collect();
    assert_eq!(names("problem codes"), codes);
    let spellings: Vec<&str> = TickerSpelling::ALL
        .iter()
        .map(|spelling| spelling.as_str())
        .collect();
    assert_eq!(names("ticker spellings"), spellings);
}

#[test]
fn the_page_builds_an_id_and_a_canonical_payload_as_the_format_does() {
    let fixture = fixture();
    assert_eq!(names("signal id fields"), fixture.id_fields);
    assert_eq!(
        names("left out of the canonical payload"),
        fixture.not_in_canonical
    );

    let blocks = fenced("worked example");
    let [body, answers] = blocks.as_slice() else {
        panic!("the worked example is a body and its answers: {blocks:?}");
    };
    let body = body.join("\n");
    let signal = Signal::read(body.as_bytes()).expect("a signal");
    assert_eq!(
        answers,
        &[
            signal.signal_id(),
            signal.canonical(),
            signal.payload_hash().to_string()
        ]
    );
    // It is the first Bybit entry of the shared cases, body and answers.
    let case = fixture.payload("by-linear-long-entry");
    assert_eq!(Some(&body), case.body.as_ref());
    let recorded = case.signal.as_ref().expect("a signal");
    assert_eq!(answers[0], recorded.signal_id);
    assert_eq!(answers[2], recorded.hash);
    // An ID is no longer than the page says.
    assert!(read(PAGE).contains("An ID is at most 106 characters"));
    assert_eq!(32 + 5 + 5 + MAX_TICKER_CHARS + 20 + 4, 106);
}

#[test]
fn the_page_lists_the_rules_a_signal_is_resolved_by_in_their_order() {
    let fixture = fixture();
    let rules = items("resolution order");
    let parts: Vec<&str> = rules.iter().map(|(part, _)| part.as_str()).collect();
    assert_eq!(parts, fixture.resolution_order);

    // What each rule refuses a signal as is what the cases show it does.
    let policy_refusals: Vec<String> = items_of("docs/automation-policy.md", "entry refusals")
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (part, statement) in &rules {
        let shown: BTreeSet<&str> = fixture
            .resolutions
            .iter()
            .filter(|case| case.outcome.over.as_deref() == Some(part.as_str()))
            .filter_map(|case| case.outcome.refused.as_deref())
            .collect();
        assert!(!shown.is_empty(), "no case is refused over the {part}");
        if part == "policy" {
            // The policy's own page lists them; this one points there.
            assert!(statement.contains("automation-policy.md#what-an-entry-is-held-to"));
            for refusal in &shown {
                assert!(
                    policy_refusals.iter().any(|known| known == refusal),
                    "{refusal}"
                );
            }
            continue;
        }
        let named: BTreeSet<&str> = quoted(statement).into_iter().collect();
        assert_eq!(named, shown, "{part}");
        for refusal in named {
            assert!(
                Refusal::ALL.iter().any(|known| known.as_str() == refusal),
                "{refusal}"
            );
        }
    }
}

#[test]
fn the_page_states_the_age_limits_and_the_held_side_rules_as_the_model_has_them() {
    let fixture = fixture();
    let limits: Vec<String> = section("age limits")
        .into_iter()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(
        limits,
        [
            format!(
                "- an entry's age limit: {} seconds, unless the profile sets another",
                SignalAgeLimit::DEFAULT.secs()
            ),
            format!(
                "- a profile's own limit: from {} to {} seconds",
                SignalAgeLimit::MIN_SECS,
                SignalAgeLimit::MAX_SECS
            ),
            format!(
                "- ahead of the clock: at most {} seconds, for every profile",
                SIGNAL_FUTURE_ALLOWANCE_MS / 1_000
            ),
        ]
    );

    // Flat, the same side, the other side: and what the cases refuse each as.
    let held: Vec<String> = section("held side")
        .into_iter()
        .filter(|line| line.starts_with("- "))
        .collect();
    let [flat, same, other] = held.as_slice() else {
        panic!("three rules: {held:?}");
    };
    let refusals = |same_side: bool| -> BTreeSet<&str> {
        fixture
            .held
            .iter()
            .filter(|case| {
                case.held.is_some() && (case.held.as_deref() == Some(&case.entry)) == same_side
            })
            .map(|case| case.refused.as_deref().expect("a refusal"))
            .collect()
    };
    assert!(flat.starts_with("- flat: ") && quoted(flat).is_empty());
    assert!(
        fixture
            .held
            .iter()
            .filter(|case| case.held.is_none())
            .all(|case| case.refused.is_none())
    );
    assert!(same.starts_with("- the same side: "));
    assert_eq!(
        quoted(same).into_iter().collect::<BTreeSet<_>>(),
        refusals(true)
    );
    assert!(other.starts_with("- the other side: "));
    assert_eq!(
        quoted(other).into_iter().collect::<BTreeSet<_>>(),
        refusals(false)
    );
}

#[test]
fn the_page_shows_the_receipts_the_format_writes() {
    let fixture = fixture();
    assert_eq!(names("receipt fields"), fixture.receipt_fields);
    let states: Vec<&str> = ReceiptState::ALL
        .iter()
        .map(|state| state.as_str())
        .collect();
    assert_eq!(names("receipt states"), states);

    let blocks = fenced("receipt examples");
    let [examples] = blocks.as_slice() else {
        panic!("one block of receipts: {blocks:?}");
    };
    let valid: Vec<&Value> = fixture
        .receipts
        .iter()
        .filter(|case| case.valid)
        .map(|case| &case.body)
        .collect();
    assert_eq!(examples.len(), valid.len());
    for (line, recorded) in examples.iter().zip(valid) {
        let receipt: SignalReceipt = serde_json::from_str(line).expect("a receipt");
        // Exactly what the format writes, and one of the shared cases.
        assert_eq!(&serde_json::to_string(&receipt).expect("json"), line);
        assert_eq!(&serde_json::to_value(&receipt).expect("json"), recorded);
    }
}

#[test]
fn the_page_holds_the_templates_of_the_cases_word_for_word() {
    let fixture = fixture();
    let blocks: Vec<String> = fixture
        .templates
        .iter()
        .map(|template| {
            format!(
                "**`{}`.** {}.\n\n```json\n{}\n```",
                template.name, template.note, template.template
            )
        })
        .collect();
    assert_eq!(section("alert templates").join("\n"), blocks.join("\n\n"));
}

#[test]
fn the_page_lists_every_case() {
    let fixture = fixture();
    let signals = |payload: &&Payload| payload.signal.is_some();
    assert_eq!(
        items("valid payloads"),
        listed(
            fixture
                .payloads
                .iter()
                .filter(signals)
                .map(|payload| (payload.id.as_str(), payload.note.clone()))
        )
    );
    let refused = || {
        fixture
            .payloads
            .iter()
            .filter(|payload| payload.problems.is_some())
    };
    let problems = |payload: &Payload| said(payload.problems.as_deref().expect("problems"));
    assert_eq!(
        items("invalid payloads"),
        listed(refused().map(|payload| {
            (
                payload.id.as_str(),
                format!("{} ({})", payload.note, problems(payload)),
            )
        }))
    );
    // What the schema alone would take of them.
    assert_eq!(
        items("beyond the schema"),
        listed(
            refused()
                .filter(|payload| payload.schema_accepts == Some(true))
                .map(|payload| (payload.id.as_str(), problems(payload)))
        )
    );
    assert_eq!(
        items("resolution cases"),
        listed(fixture.resolutions.iter().map(|case| {
            let answer = match (&case.outcome.refused, &case.outcome.resolved) {
                (Some(refusal), None) => format!("`{refusal}`"),
                (None, Some(resolved)) => format!(
                    "{} {} on {}",
                    resolved["kind"].as_str().expect("a kind"),
                    resolved["side"].as_str().expect("a side"),
                    resolved["instrument"]["symbol"].as_str().expect("a symbol")
                ),
                other => panic!("{}: {other:?}", case.id),
            };
            (case.id.as_str(), format!("{} ({answer})", case.note))
        }))
    );
    assert_eq!(
        items("exit cases"),
        listed(fixture.exits.iter().map(|case| {
            let answer = if case.closes { "closes it" } else { "a no-op" };
            (case.id.as_str(), format!("{} ({answer})", case.note))
        }))
    );
    assert_eq!(
        items("delivery cases"),
        listed(fixture.deliveries.iter().map(|case| {
            let steps: Vec<String> = case
                .steps
                .iter()
                .map(|step| format!("{} {}", step.payload, step.outcome))
                .collect();
            (
                case.id.as_str(),
                format!("{} ({})", case.note, steps.join(", ")),
            )
        }))
    );
}

#[test]
fn every_open_point_about_tradingview_is_an_unverified_item_with_its_owner() {
    let venues = read("docs/futures-venues.md");
    let open: Vec<String> = section("unverified items")
        .into_iter()
        .filter(|line| line.starts_with("- "))
        .collect();
    assert_eq!(open.len(), 5);
    for line in open {
        let number = line
            .strip_prefix("- ")
            .and_then(|rest| rest.split_once(": "))
            .map(|(number, _)| number)
            .unwrap_or_else(|| panic!("an item is a number and what is open: {line}"));
        let row = venues
            .lines()
            .find(|row| row.starts_with(&format!("| {number} | ")))
            .unwrap_or_else(|| panic!("docs/futures-venues.md has no unverified item {number}"));
        // The alert-setup story owns it, and it says what holds meanwhile.
        let cells: Vec<&str> = row.split(" | ").collect();
        assert_eq!(cells.len(), 4, "{row}");
        assert!(cells[3].contains("ALG-754"), "{row}");
        assert!(cells[2].len() > 40, "{row}");
    }
}

#[test]
fn the_page_points_at_files_that_exist() {
    let page = read(PAGE);
    let mut found = 0;
    for (at, _) in page.match_indices("](") {
        let target = &page[at + 2..];
        let target = &target[..target.find(')').expect("a closed link")];
        if target.starts_with('#') || target.starts_with("http") {
            continue;
        }
        let file = target.split('#').next().expect("a path");
        let path = if let Some(above) = file.strip_prefix("../") {
            above.to_string()
        } else {
            format!("docs/{file}")
        };
        assert!(!read(&path).is_empty(), "{target}");
        found += 1;
    }
    assert!(found >= 10, "{found}");
    // Each section the contents name is there.
    for line in page
        .lines()
        .filter(|line| line.starts_with("- [") && line.ends_with(')'))
    {
        let title = &line[3..line.find("](#").expect("a contents line")];
        let heading = format!("## {title}");
        assert!(page.lines().any(|line| line == heading), "{title}");
    }
}
