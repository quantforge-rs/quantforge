//! `docs/risk-sizing.md` held to the code and to the shared cases: what
//! sizing is given, the refusals and the faults in their order, the limits
//! and constants, the figures an answer carries, the worked examples, and
//! the list of cases. The lists sit between `<!-- begin: name -->` and
//! `<!-- end: name -->`. And the cases ADR 0002 links, held to the fixture.

use std::collections::BTreeSet;
use std::path::Path;

use quantforge::model::Refusal;
use quantforge::model::futures::{
    AMOUNT_PLACES, DRIFT_LIMIT, Freshness, SizingFaultKind, SizingInput,
};
use serde_json::Value;

use crate::cases::{Fixture, fixture, fixture_path};

const PAGE: &str = "docs/risk-sizing.md";

fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The lines of the page between a list's markers.
fn section(name: &str) -> Vec<String> {
    let doc = read(PAGE);
    let lines: Vec<&str> = doc.lines().collect();
    let at = |edge: &str| {
        let marker = format!("<!-- {edge}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("{PAGE} has no {marker}"))
    };
    lines[at("begin") + 1..at("end")]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// The items of a list, `- `name`: statement`, each joined onto one line.
fn items(name: &str) -> Vec<(String, String)> {
    let mut items: Vec<(String, String)> = Vec::new();
    for line in section(name) {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(item) = line.strip_prefix("- `") {
            let (id, statement) = item
                .split_once("`: ")
                .unwrap_or_else(|| panic!("an item is `name`: statement: {line}"));
            items.push((id.to_string(), statement.to_string()));
        } else {
            let (_, statement) = items
                .last_mut()
                .unwrap_or_else(|| panic!("a line before the first item: {line}"));
            statement.push(' ');
            statement.push_str(line.trim());
        }
    }
    items
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

fn words<T>(all: &[T], word: fn(&T) -> &'static str) -> Vec<String> {
    all.iter().map(|one| word(one).to_string()).collect()
}

/// The fault a case's answer names, as the page spells it: `stale rules`.
fn fault_of(result: &Value) -> Option<String> {
    let fault = result.get("fault")?;
    let word = |key: &str| fault[key].as_str().expect("a word");
    Some(format!("{} {}", word("kind"), word("input")))
}

/// The cases whose names start with `prefix`, in the fixture's order.
fn chained<'a>(fixture: &'a Fixture, prefix: &str, but: &str) -> Vec<&'a Value> {
    fixture
        .sizing
        .iter()
        .filter(|case| case.id.starts_with(prefix) && (but.is_empty() || !case.id.starts_with(but)))
        .map(|case| &case.result)
        .collect()
}

#[test]
fn the_page_names_what_sizing_is_given_and_the_kinds_of_fault_as_the_code_does() {
    assert_eq!(
        names("sizing inputs"),
        words(&SizingInput::ALL, |input| input.as_str())
    );
    assert_eq!(
        names("fault kinds"),
        words(&SizingFaultKind::ALL, |kind| kind.as_str())
    );
}

/// The page's refusals are the parity matrix's first eleven, in the
/// matrix's order, and that is the order sizing gives them in: each of the
/// `order-` cases breaks every later rule as well, and is refused for its
/// own.
#[test]
fn the_refusals_are_listed_in_the_order_sizing_gives_them() {
    let fixture = fixture();
    let listed = names("sizing refusals");
    assert_eq!(
        listed,
        words(&Refusal::ALL[..11], |refusal| refusal.as_str())
    );
    let mut given = vec![fixture.case("refuse-capital-currency").result["reason"].clone()];
    given.extend(
        chained(&fixture, "order-", "")
            .into_iter()
            .map(|result| result["reason"].clone()),
    );
    let given: Vec<&str> = given
        .iter()
        .map(|reason| reason.as_str().expect("a reason"))
        .collect();
    assert_eq!(listed, given);
}

/// Likewise the faults: each `chain-` case has everything looked at after
/// its own fault wrong as well.
#[test]
fn the_faults_are_listed_in_the_order_sizing_looks_for_them() {
    let fixture = fixture();
    let faults = |results: Vec<&Value>| -> Vec<String> {
        results
            .into_iter()
            .map(|result| fault_of(result).expect("a fault"))
            .collect()
    };
    let request = names("fault order");
    assert_eq!(
        request,
        faults(chained(&fixture, "chain-", "chain-submit-"))
    );
    let submission = names("submission fault order");
    assert_eq!(submission, faults(chained(&fixture, "chain-submit-", "")));
    // Nothing a case faults with is left off the page.
    let listed: BTreeSet<&String> = request.iter().chain(&submission).collect();
    for case in &fixture.sizing {
        if let Some(fault) = fault_of(&case.result) {
            assert!(
                listed.contains(&fault),
                "{}: {fault} is not on the page",
                case.id
            );
        }
    }
    assert_eq!(listed.len(), request.len() + submission.len());
}

#[test]
fn the_limits_and_constants_on_the_page_are_the_codes_and_the_fixtures() {
    let fixture = fixture();
    let limits = Freshness::default();
    let expected = [
        ("prices", limits.prices_ms, fixture.freshness_ms.prices),
        ("account", limits.account_ms, fixture.freshness_ms.account),
        ("rules", limits.rules_ms, fixture.freshness_ms.rules),
    ];
    let listed = items("freshness limits");
    assert_eq!(listed.len(), expected.len());
    for ((name, statement), (input, code, recorded)) in listed.iter().zip(expected) {
        assert_eq!(name, input);
        let number = statement.split(',').next().expect("a number");
        assert_eq!(number, code.to_string(), "{name}");
        assert_eq!(code, recorded, "{name}");
    }
    assert_eq!(value("amount places"), AMOUNT_PLACES.to_string());
    assert_eq!(value("drift limit"), DRIFT_LIMIT.to_string());
    assert_eq!(value("drift limit"), fixture.drift_limit);
    assert_eq!(value("fixture format"), fixture.format.to_string());
}

#[test]
fn the_figures_on_the_page_are_the_ones_an_answer_carries() {
    let fixture = fixture();
    let keys = |answer: &Value, but: &[&str]| -> BTreeSet<String> {
        let object = answer.as_object().expect("an answer");
        object
            .keys()
            .filter(|key| !but.contains(&key.as_str()))
            .cloned()
            .collect()
    };
    let sized = &fixture.case("usd-pool-linear-fits").result;
    let listed: BTreeSet<String> = names("sizing figures").into_iter().collect();
    assert_eq!(listed, keys(sized, &["settle", "warnings", "status"]));
    let reported = fixture
        .fills
        .iter()
        .map(|case| &case.result)
        .find(|result| result["status"] == "reported")
        .expect("a fill report");
    let listed: BTreeSet<String> = names("fill report").into_iter().collect();
    assert_eq!(listed, keys(reported, &["status"]));
}

#[test]
fn the_worked_examples_are_the_cases_own_figures_one_for_each_cell() {
    let fixture = fixture();
    let rows = section("worked examples");
    assert_eq!(
        rows[0],
        "| Cell | Case | E | S′ | Quantity | Risk | Margin |"
    );
    let mut cells = BTreeSet::new();
    for row in &rows[2..] {
        let columns: Vec<&str> = row.trim_matches('|').split('|').map(str::trim).collect();
        let [cell, id, entry, stop, quantity, risk, margin] = columns.as_slice() else {
            panic!("a row has seven columns: {row}");
        };
        let case = fixture.case(id.trim_matches('`'));
        let result = &case.result;
        let text = |key: &str| result[key].as_str().expect("a figure");
        assert_eq!(result["status"], "sized", "{id}");
        assert_eq!(cell.trim_matches('`'), case.cell, "{id}");
        assert_eq!(*entry, case.inputs.expected_price.to_string(), "{id}");
        assert_eq!(*stop, text("stop"), "{id}");
        assert_eq!(*quantity, text("qty"), "{id}");
        assert_eq!(
            *risk,
            format!("{} {}", text("risk"), text("settle")),
            "{id}"
        );
        let needed = format!("{} {}", text("required_margin"), text("settle"));
        assert_eq!(*margin, needed, "{id}");
        cells.insert(case.cell.clone());
    }
    let every: BTreeSet<String> = fixture
        .sizing
        .iter()
        .map(|case| case.cell.clone())
        .collect();
    assert_eq!(cells, every, "one example for each venue, family and side");
    assert_eq!(cells.len(), 8);
}

#[test]
fn the_prd_example_is_quoted_from_the_cases_not_from_the_prd() {
    let fixture = fixture();
    let listed = items("prd example");
    assert_eq!(listed.len(), 2);
    for (id, statement) in listed {
        let result = &fixture.case(&id).result;
        let text = |key: &str| result[key].as_str().expect("a figure");
        let expected = format!(
            "{} SOL, risking {} USDT of a {} USDT budget.",
            text("qty"),
            text("risk"),
            text("budget")
        );
        assert_eq!(statement, expected, "{id}");
    }
}

#[test]
fn the_page_lists_every_case_of_the_fixture_with_what_it_shows() {
    let fixture = fixture();
    let sizing: Vec<(String, String)> = fixture
        .sizing
        .iter()
        .map(|case| (case.id.clone(), case.note.clone()))
        .collect();
    assert_eq!(items("sizing cases"), sizing);
    let fills: Vec<(String, String)> = fixture
        .fills
        .iter()
        .map(|case| (case.id.clone(), case.note.clone()))
        .collect();
    assert_eq!(items("fill cases"), fills);
}

/// ADR 0002 links `spikes/futures-contract/sizing-cases.json`; the crate's
/// tests read the copy under `tests/`. Where both are there, in a checkout,
/// they are the same cases. A packaged crate has no `spikes/`.
#[test]
fn the_cases_adr_0002_links_are_the_cases_the_tests_read() {
    let linked = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("spikes")
        .join("futures-contract")
        .join("sizing-cases.json");
    let Ok(linked) = std::fs::read_to_string(&linked) else {
        eprintln!("skipping: no {}", linked.display());
        return;
    };
    let read = std::fs::read_to_string(fixture_path()).expect("the fixture");
    let linked: Value = serde_json::from_str(&linked).expect("the linked cases");
    let read: Value = serde_json::from_str(&read).expect("the fixture");
    assert!(
        linked == read,
        "the two files hold different cases: write both again"
    );
}
