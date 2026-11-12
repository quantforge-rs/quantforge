//! Every shared case on the pure model: the policy built as an operator
//! would set it, the entry admitted under its snapshot, the entry sized,
//! held to its prices and reported after its fills, each answer compared
//! whole with the one the Python model recorded.

use std::collections::{BTreeMap, BTreeSet};

use quantforge::model::futures::{
    AMOUNT_PLACES, DRIFT_LIMIT, Freshness, PolicySnapshot, SizedEntry, SizingFaultKind, SizingInput,
};
use quantforge::model::{Category, PositionSide, Venue};
use serde_json::{Value, json};

use crate::cases::{Case, Fixture, Given, answered, fixture, refused_by_policy};

/// A case's answer on the pure model, and its sized entry where it has one.
pub(crate) fn replay(fixture: &Fixture, case: &Case) -> (Value, Option<SizedEntry>) {
    let admitted_on = case.admitted_on(fixture);
    let given = Given::of(fixture, &case.inputs);
    let policy = case.policy(&admitted_on);
    match policy.entry_snapshot(&admitted_on) {
        Err(refusal) => {
            assert!(
                case.inputs.snapshot.is_none(),
                "{}: the policy refuses the entry: {refusal}",
                case.id
            );
            let settle = &given.instrument.id().settle;
            (refused_by_policy(settle, refusal.refusal.as_str()), None)
        }
        Ok(snapshot) => {
            let stored = serde_json::to_value(&snapshot).expect("a snapshot");
            assert_eq!(
                Some(&stored),
                case.inputs.snapshot.as_ref(),
                "{}: the snapshot the entry is admitted under",
                case.id
            );
            // The stored form reads back to the same snapshot, and sizing
            // works from that one: what a store hands back.
            let read: PolicySnapshot = serde_json::from_value(stored).expect("read back");
            assert_eq!(read, snapshot, "{}", case.id);
            answered(&case.inputs, &given, &read)
        }
    }
}

#[test]
fn the_fixture_is_the_format_and_carries_the_constants_this_build_sizes_with() {
    let fixture = fixture();
    assert_eq!(fixture.format, 2);
    assert_eq!(fixture.generated_by, "spikes/futures-contract/sizing.py");
    assert_eq!(fixture.drift_limit, DRIFT_LIMIT.to_string());
    let limits = Freshness::default();
    assert_eq!(fixture.freshness_ms.prices, limits.prices_ms);
    assert_eq!(fixture.freshness_ms.account, limits.account_ms);
    assert_eq!(fixture.freshness_ms.rules, limits.rules_ms);
    assert!(
        fixture.rounding["amounts"]
            .starts_with(&format!("exact to {AMOUNT_PLACES} decimal places")),
        "{}",
        fixture.rounding["amounts"]
    );
    assert!(
        fixture
            .sizing
            .iter()
            .all(|case| case.inputs.now_ms == fixture.now_ms)
    );
    assert!(
        fixture.sizing.len() >= 100,
        "only {} cases",
        fixture.sizing.len()
    );
}

#[test]
fn every_case_reproduces_exactly_every_refusal_and_every_fault_included() {
    let fixture = fixture();
    let mut wrong = Vec::new();
    for case in &fixture.sizing {
        let (got, _) = replay(&fixture, case);
        if got != case.result {
            wrong.push(format!(
                "{} ({}):\n  sized    {got}\n  recorded {}",
                case.id, case.note, case.result
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} cases differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn every_venue_family_and_side_of_the_parity_matrix_is_sized_and_refused() {
    let fixture = fixture();
    let mut seen: BTreeMap<String, BTreeSet<&str>> = BTreeMap::new();
    for case in &fixture.sizing {
        let status = case.result["status"].as_str().expect("a status");
        seen.entry(case.cell.clone()).or_default().insert(status);
        // A case's cell is the product line and side it is sized on.
        let id = &case.inputs.snapshot.as_ref().map_or_else(
            || fixture.instruments[&case.inputs.instrument].id.clone(),
            |snapshot| serde_json::from_value(snapshot["instrument"].clone()).expect("an id"),
        );
        let cell = format!(
            "futures.sizing/{}/{}/{}",
            id.venue(),
            id.category(),
            case.inputs.side
        );
        assert_eq!(case.cell, cell, "{}", case.id);
    }
    let mut cells = Vec::new();
    for venue in Venue::ALL {
        for category in [Category::Linear, Category::Inverse] {
            for side in PositionSide::ALL {
                cells.push(format!("futures.sizing/{venue}/{category}/{side}"));
            }
        }
    }
    assert_eq!(cells.len(), 8);
    assert_eq!(seen.keys().cloned().collect::<Vec<_>>(), {
        let mut sorted = cells.clone();
        sorted.sort();
        sorted
    });
    for cell in cells {
        for status in ["sized", "refused"] {
            assert!(
                seen[&cell].contains(status),
                "no case of {cell} is {status}"
            );
        }
    }
}

#[test]
fn the_cases_reach_every_refusal_sizing_gives_and_every_fault_it_names() {
    let fixture = fixture();
    let reasons: BTreeSet<&str> = fixture
        .sizing
        .iter()
        .filter_map(|case| case.result.get("reason").and_then(Value::as_str))
        .collect();
    let expected = [
        "above_leverage_bracket",
        "above_market_max",
        "below_min_notional",
        "below_min_qty",
        "insufficient_margin",
        "no_capital_in_settlement_asset",
        "no_stop",
        "risk_drift",
        "settlement_wallet_below_risk",
        "stop_crossed",
        "stop_wrong_side",
    ];
    assert_eq!(reasons.into_iter().collect::<Vec<_>>(), expected);

    let mut faults: BTreeSet<(String, String)> = BTreeSet::new();
    for result in fixture
        .sizing
        .iter()
        .map(|case| &case.result)
        .chain(fixture.fills.iter().map(|case| &case.result))
    {
        if let Some(fault) = result.get("fault") {
            let word = |key: &str| fault[key].as_str().expect("a word").to_string();
            faults.insert((word("kind"), word("input")));
        }
    }
    // Every input is faulted at least once, and every kind but the one
    // that needs amounts beyond any venue's.
    let inputs: BTreeSet<&str> = faults.iter().map(|(_, input)| input.as_str()).collect();
    let all_inputs: BTreeSet<&str> = SizingInput::ALL
        .iter()
        .map(|input| input.as_str())
        .collect();
    assert_eq!(inputs, all_inputs);
    let kinds: BTreeSet<&str> = faults.iter().map(|(kind, _)| kind.as_str()).collect();
    let all_kinds: BTreeSet<&str> = SizingFaultKind::ALL
        .iter()
        .filter(|kind| **kind != SizingFaultKind::OutOfRange)
        .map(|kind| kind.as_str())
        .collect();
    assert_eq!(kinds, all_kinds);
}

#[test]
fn every_fill_case_reports_the_risk_taken_beside_the_risk_sized() {
    let fixture = fixture();
    let mut wrong = Vec::new();
    for case in &fixture.fills {
        let (_, entry) = replay(&fixture, fixture.case(&case.entry));
        let entry = entry.unwrap_or_else(|| panic!("{}: its entry is not sized", case.id));
        let fills: Vec<_> = case
            .fills
            .iter()
            .map(|fill| (fill.price, fill.quantity))
            .collect();
        let got = match entry.fill_risk(&fills) {
            Err(fault) => json!({
                "status": "fault",
                "fault": {
                    "kind": fault.kind().as_str(),
                    "input": fault.input().map(|input| input.as_str()),
                },
            }),
            Ok(risk) => json!({
                "status": "reported",
                "filled": risk.filled.value().to_string(),
                "expected": risk.expected.amount().to_string(),
                "actual": risk.actual.amount().to_string(),
                "deviation": risk.deviation.amount().to_string(),
                "budget": risk.budget.amount().to_string(),
                "exceeds_budget": risk.exceeds_budget,
                "past_stop": risk.past_stop,
            }),
        };
        if got != case.result {
            wrong.push(format!(
                "{}:\n  reported {got}\n  recorded {}",
                case.id, case.result
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} fill cases differ:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
