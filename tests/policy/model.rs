//! Every shared case on the pure model: each change makes the document the
//! independent model made, byte for byte, or is refused in its words; each
//! instrument trades on the settings it expects, from the source it
//! expects; and each entry gets the snapshot, the stop or the refusal it
//! expects.

use quantforge::model::futures::{
    EntryStop, POLICY_FORMAT, POLICY_SNAPSHOT_FORMAT, PolicyProblem, PolicySnapshot, ProfilePolicy,
};
use serde_json::{Value, json};

use crate::cases::{Case, Line, Step, change, effective_json, fixture, price};

/// What a step of `case` is called in a failure.
fn named(case: &Case, step: usize) -> String {
    format!("{}, step {step}", case.id)
}

/// Applies a change step to `policy`, held to the fixture's answer.
fn apply(policy: &mut ProfilePolicy, line: &Line, step: &Step, at: &str) {
    let asked = step.change.as_ref().expect("a change");
    match policy.apply(&change(line, asked)) {
        Ok(Some(next)) => {
            let document = step
                .document
                .as_deref()
                .unwrap_or_else(|| panic!("{at}: {asked} made a revision the fixture has not"));
            assert_eq!(next.to_stored().expect("stored"), document, "{at}");
            assert_eq!(
                next.revision().get(),
                policy.revision().get() + 1,
                "{at}: one revision on"
            );
            // What a store hands back is the same policy, in the same text.
            let back = ProfilePolicy::from_stored(document).expect("read back");
            assert_eq!(back, next, "{at}");
            assert_eq!(back.to_stored().expect("stored"), document, "{at}");
            *policy = next;
        }
        Ok(None) => assert_eq!(step.unchanged, Some(true), "{at}: {asked} changed nothing"),
        Err(error) => {
            let refused = step.refused.as_ref().and_then(Value::as_str);
            assert_eq!(Some(error.to_string().as_str()), refused, "{at}");
        }
    }
}

/// The stop an entry uses, in the fixture's spelling.
fn stop_json(snapshot: &PolicySnapshot, signal_stop: Option<&str>) -> Value {
    match snapshot.stop_for(signal_stop.map(price)) {
        Ok(EntryStop::Signal(price)) => json!({"signal": price.to_string()}),
        Ok(EntryStop::Fallback(percent)) => json!({"fallback": percent.to_string()}),
        Err(refusal) => json!({"refused": {
            "refusal": refusal.refusal.as_str(),
            "message": refusal.message,
        }}),
    }
}

/// Asks an entry step of `policy`, held to the fixture's answer.
pub(crate) fn enter(policy: &ProfilePolicy, line: &Line, step: &Step, at: &str) {
    let symbol = step.entry.as_deref().expect("an entry");
    match policy.entry_snapshot(line.listed(symbol)) {
        Ok(snapshot) => {
            let text = serde_json::to_string(&snapshot).expect("json");
            assert_eq!(Some(text.as_str()), step.snapshot.as_deref(), "{at}");
            let back: PolicySnapshot = serde_json::from_str(&text).expect("read back");
            assert_eq!(back, snapshot, "{at}");
            assert_eq!(snapshot.revision(), policy.revision(), "{at}");
            assert_eq!(
                Some(stop_json(&snapshot, step.signal_stop.as_deref())),
                step.stop,
                "{at}"
            );
        }
        Err(refusal) => {
            let answered = json!({
                "refusal": refusal.refusal.as_str(),
                "message": refusal.message,
            });
            assert_eq!(Some(answered), step.refused, "{at}");
            assert_eq!(step.snapshot, None, "{at}");
        }
    }
}

/// A check's problems, in the fixture's spelling.
pub(crate) fn problems_json(problems: &[PolicyProblem]) -> Vec<Value> {
    problems
        .iter()
        .map(|problem| {
            json!({
                "subject": problem.subject.to_string(),
                "kind": problem.kind.as_str(),
                "message": problem.message,
            })
        })
        .collect()
}

/// Replays `case` on the pure model.
pub(crate) fn replay(line: &Line, case: &Case) -> ProfilePolicy {
    let mut policy = ProfilePolicy::new(line.identity.clone());
    assert_eq!(
        policy.to_stored().expect("stored"),
        case.created,
        "{}",
        case.id
    );
    for (n, step) in case.steps.iter().enumerate() {
        let at = named(case, n);
        if step.change.is_some() {
            apply(&mut policy, line, step, &at);
        } else if let Some(symbol) = &step.effective {
            let effective = policy
                .effective(line.listed(symbol).id())
                .unwrap_or_else(|error| panic!("{at}: {error}"));
            assert_eq!(Some(effective_json(&effective)), step.expect, "{at}");
        } else if step.entry.is_some() {
            enter(&policy, line, step, &at);
        } else if step.check == Some(true) {
            let problems = problems_json(&policy.check(&line.instruments));
            assert_eq!(Some(&problems), step.problems.as_ref(), "{at}");
        } else {
            panic!("{at}: a step that asks nothing this harness knows");
        }
    }
    policy
}

#[test]
fn every_case_on_every_product_line_answers_as_the_independent_model_does() {
    let fixture = fixture();
    assert_eq!(fixture.format, 1);
    assert_eq!(fixture.policy_format, POLICY_FORMAT);
    assert_eq!(fixture.snapshot_format, POLICY_SNAPSHOT_FORMAT);
    assert_eq!(fixture.lines.len(), 4, "both venues, both families");
    let per_line = fixture.cases.len() / fixture.lines.len();
    for line in &fixture.lines {
        let cases: Vec<&Case> = fixture
            .cases
            .iter()
            .filter(|case| case.line == line.name)
            .collect();
        assert_eq!(
            cases.len(),
            per_line,
            "{}: the same cases on every line",
            line.name
        );
        for case in cases {
            replay(line, case);
        }
    }
}

#[test]
fn the_same_operations_make_the_same_policy_on_binance_and_on_bybit() {
    // The documents differ in who trades what, and in nothing else: the
    // same case leaves the same settings on every line.
    let fixture = fixture();
    let settings = |policy: &ProfilePolicy| {
        let mut json = serde_json::to_value(policy).expect("json");
        json["identity"] = Value::Null;
        for asset in json["assets"].as_array_mut().expect("assets") {
            asset["instrument"] = Value::Null;
        }
        for capital in json["capital"].as_array_mut().expect("capital") {
            capital["amount"]["asset"] = Value::Null;
        }
        json
    };
    for case in fixture
        .cases
        .iter()
        .filter(|case| case.line == "binance linear")
    {
        let name = case.id.split('/').next().expect("a name");
        if name == "capital" || name == "venue_check" {
            // Their amounts and their venue lists differ by family and venue.
            continue;
        }
        let here = settings(&replay(fixture.line(&case.line), case));
        for other in fixture
            .cases
            .iter()
            .filter(|other| other.id.starts_with(name))
        {
            if other.line.ends_with("linear") {
                let there = settings(&replay(fixture.line(&other.line), other));
                assert_eq!(here, there, "{} against {}", case.id, other.id);
            }
        }
    }
}
