//! `docs/automation-policy.md` held to the code and to the shared cases:
//! its formats, the fields of a stored policy and of a snapshot, its worked
//! example, the changes, an entry's refusals in their order, the venue's
//! problems, the service's calls, and the list of cases. The lists sit
//! between `<!-- begin: name -->` and `<!-- end: name -->`.

use std::collections::BTreeSet;
use std::path::Path;

use quantforge::model::futures::{
    FuturesInstrument, POLICY_FORMAT, POLICY_SNAPSHOT_FORMAT, PolicyProblemKind, PolicySnapshot,
    ProfilePolicy,
};
use serde_json::{Value, json};

use crate::cases::{Case, Fixture, Line, change, fixture};

fn read(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The lines of the doc between a list's markers.
fn section(name: &str) -> Vec<String> {
    let doc = read("docs/automation-policy.md");
    let lines: Vec<&str> = doc.lines().collect();
    let at = |edge: &str| {
        let marker = format!("<!-- {edge}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/automation-policy.md has no {marker}"))
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
    items(list).into_iter().map(|(name, _)| name).collect()
}

/// A list's one number.
fn number(name: &str) -> u32 {
    let lines: Vec<String> = section(name)
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let [number] = lines.as_slice() else {
        panic!("{name} holds one number: {lines:?}");
    };
    number.parse().expect("a number")
}

/// A fenced JSON block of the doc.
fn example(name: &str) -> Value {
    let text: Vec<String> = section(name)
        .into_iter()
        .filter(|line| !line.starts_with("```"))
        .collect();
    serde_json::from_str(&text.join("\n")).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("an object: {value}"))
        .keys()
        .cloned()
        .collect()
}

fn set(list: &str) -> BTreeSet<String> {
    let listed = names(list);
    let distinct: BTreeSet<String> = listed.iter().cloned().collect();
    assert_eq!(distinct.len(), listed.len(), "{list} names each once");
    distinct
}

/// The case the doc's worked example is taken from, and its line.
fn worked(fixture: &Fixture) -> (&Case, &Line) {
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == "inheritance/bybit_linear")
        .expect("the worked example's case");
    (case, fixture.line(&case.line))
}

/// The policy the worked example's case ends with, as stored.
fn last_document(case: &Case) -> &str {
    case.steps
        .iter()
        .rev()
        .find_map(|step| step.document.as_deref())
        .expect("a document")
}

#[test]
fn the_formats_in_the_doc_are_the_ones_the_code_writes() {
    let fixture = fixture();
    assert_eq!(fixture.format, 1);
    assert_eq!(number("policy format"), POLICY_FORMAT);
    assert_eq!(number("snapshot format"), POLICY_SNAPSHOT_FORMAT);
    assert_eq!(fixture.policy_format, POLICY_FORMAT);
    assert_eq!(fixture.snapshot_format, POLICY_SNAPSHOT_FORMAT);
}

#[test]
fn the_doc_names_every_field_a_stored_policy_and_a_snapshot_hold() {
    let fixture = fixture();
    let (case, line) = worked(&fixture);
    let policy = ProfilePolicy::from_stored(last_document(case)).expect("the policy");
    // What this build writes of it, not what the fixture spells.
    let stored: Value = serde_json::from_str(&policy.to_stored().expect("json")).expect("json");
    assert_eq!(keys(&stored), set("policy fields"));
    assert_eq!(keys(&stored["defaults"]), set("defaults fields"));
    assert_eq!(keys(&stored["assets"][0]), set("asset fields"));
    assert_eq!(keys(&stored["capital"][0]), set("capital fields"));
    let snapshot = policy
        .entry_snapshot(line.listed(&line.symbols.main))
        .expect("a snapshot");
    let stored = serde_json::to_value(&snapshot).expect("json");
    assert_eq!(keys(&stored), set("snapshot fields"));
}

#[test]
fn the_worked_example_is_the_policy_a_shared_case_ends_with_and_the_snapshot_it_gives() {
    let fixture = fixture();
    let (case, line) = worked(&fixture);
    let document = last_document(case);
    let shown = example("policy example");
    let held: Value = serde_json::from_str(document).expect("json");
    assert_eq!(shown, held, "the policy example");
    // The doc shows it indented; a store keeps it on one line, and reading
    // either gives the same policy.
    let policy = ProfilePolicy::from_stored(&shown.to_string()).expect("the example reads");
    assert_eq!(policy.to_stored().expect("json"), document);

    let snapshot: PolicySnapshot =
        serde_json::from_value(example("snapshot example")).expect("the snapshot reads");
    let given = policy
        .entry_snapshot(line.listed(&line.symbols.main))
        .expect("a snapshot");
    assert_eq!(snapshot, given, "the snapshot the policy above gives");
    let last = case
        .steps
        .iter()
        .rev()
        .find_map(|step| step.snapshot.as_deref())
        .expect("a snapshot");
    assert_eq!(serde_json::to_string(&given).expect("json"), last);
}

#[test]
fn the_doc_lists_every_change_an_operator_makes() {
    let fixture = fixture();
    let made: BTreeSet<String> = fixture
        .cases
        .iter()
        .flat_map(|case| &case.steps)
        .filter_map(|step| step.change.as_ref())
        .filter_map(|asked| asked.as_object()?.keys().next().cloned())
        .collect();
    assert_eq!(set("policy changes"), made);
}

/// `listed` with something of it changed, as the venue might list it.
fn relisted(listed: &FuturesInstrument, alter: fn(&mut Value)) -> FuturesInstrument {
    let mut spec = serde_json::to_value(listed).expect("json");
    alter(&mut spec);
    serde_json::from_value(spec).expect("an instrument")
}

/// One of everything the doc's list names is wrong at first, and each is
/// put right in turn: what an entry is refused for, step by step, is the
/// list in its order.
#[test]
fn the_doc_lists_an_entrys_refusals_in_the_order_they_are_looked_for() {
    let fixture = fixture();
    let line = fixture.line("bybit linear");
    let main = &line.symbols.main;
    let listed = line.listed(main);
    let dated: fn(&mut Value) = |spec| spec["contract"] = json!({"Dated": {"delivery_ms": 1}});
    let after = |policy: &ProfilePolicy, asked: Value| {
        policy
            .apply(&change(line, &asked))
            .expect("a change the policy takes")
            .expect("a change that changes it")
    };
    let refused = |policy: &ProfilePolicy, listed: &FuturesInstrument| {
        let refusal = policy.entry_snapshot(listed).expect_err("refused");
        refusal.refusal.as_str().to_string()
    };
    // The asset is held disabled, with no capital and no fallback stop, and
    // the venue lists it dated and settled in another asset.
    let empty = ProfilePolicy::new(line.identity.clone());
    let held = after(
        &empty,
        json!({"add_asset": {"symbol": main, "settle": "USDT", "enabled": false,
                             "risk_percent": "1", "leverage": "5", "stop": "off"}}),
    );
    let elsewhere = relisted(listed, |spec| {
        spec["contract"] = json!({"Dated": {"delivery_ms": 1}});
        spec["id"]["settle"] = json!("USDC");
    });
    let mut met = vec![refused(&held, &elsewhere)];
    // Settled as the profile holds it: now it is the switch.
    let dated = relisted(listed, dated);
    met.push(refused(&held, &dated));
    // An asset the profile has no settings for meets the defaults' switch.
    met.push(refused(&empty, &dated));
    let enabled = after(
        &held,
        json!({"edit_asset": {"symbol": main, "enabled": true}}),
    );
    met.push(refused(&enabled, &dated));
    // A perpetual again: now it is the capital.
    met.push(refused(&enabled, listed));
    let funded = after(
        &enabled,
        json!({"set_capital": {"amount": "100", "asset": "USDT"}}),
    );
    let snapshot = funded.entry_snapshot(listed).expect("admitted");
    let unstopped = snapshot.stop_for(None).expect_err("no stop");
    met.push(unstopped.refusal.as_str().to_string());

    assert_eq!(names("entry refusals"), met);
}

#[test]
fn the_doc_lists_every_problem_the_venues_list_can_show_in_the_order_it_looks() {
    let all: Vec<String> = PolicyProblemKind::ALL
        .iter()
        .map(|kind| kind.as_str().to_string())
        .collect();
    assert_eq!(names("policy problems"), all);
    // And the shared cases meet every one of them.
    let fixture = fixture();
    let met: BTreeSet<String> = fixture
        .cases
        .iter()
        .flat_map(|case| &case.steps)
        .filter_map(|step| step.problems.as_ref())
        .flatten()
        .filter_map(|problem| problem["kind"].as_str().map(str::to_string))
        .collect();
    assert_eq!(met, all.into_iter().collect::<BTreeSet<String>>());
}

#[test]
fn the_doc_lists_every_shared_case_with_its_statement() {
    let fixture = fixture();
    let name = |id: &str| id.split('/').next().unwrap_or(id).to_string();
    let mut expected: Vec<(String, String)> = Vec::new();
    let cases = fixture.cases.iter().map(|case| (&case.id, &case.doc));
    let races = fixture.races.iter().map(|race| (&race.id, &race.doc));
    for (id, statement) in cases.chain(races) {
        let listed = (name(id), statement.clone());
        if !expected.contains(&listed) {
            expected.push(listed);
        }
    }
    assert_eq!(items("policy cases"), expected);
    // Every one of them on every product line.
    for (case, _) in &expected {
        for line in &fixture.lines {
            let id = format!("{case}/{}", line.name.replace(' ', "_"));
            let held = fixture.cases.iter().any(|case| case.id == id)
                || fixture.races.iter().any(|race| race.id == id);
            assert!(held, "{id}");
        }
    }
}

#[test]
fn the_doc_lists_every_call_the_service_has() {
    let source = read("src/automation/policy.rs");
    let service = source
        .split_once("impl<'a> PolicyService<'a> {")
        .expect("the service's methods")
        .1;
    let calls: BTreeSet<String> = service
        .lines()
        .filter_map(|line| {
            let line = line.strip_prefix("    pub ")?;
            let line = line.strip_prefix("async ").unwrap_or(line);
            let name = line.strip_prefix("fn ")?.split('(').next()?;
            Some(name.to_string())
        })
        .filter(|name| name != "new")
        .collect();
    let doc = read("docs/automation-policy.md");
    let documented: BTreeSet<String> = doc
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|row| row.split('(').next().map(str::to_string))
        .collect();
    assert_eq!(documented, calls);
    assert_eq!(calls.len(), 10, "{calls:?}");
}
