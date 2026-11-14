//! `docs/signal-schema.json` held to the format. The Python model runs the
//! schema against every shared case, with a validator of its own, and
//! records what it says of each; these tests hold the file to the one the
//! model read, and its shape to the format's fields and words.

use quantforge::protocol::signal::{
    MAX_TICKER_CHARS, SIGNAL_FIELDS, SIGNAL_VERSION, SignalCategory, SignalKind, SignalSide,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cases::{fixture, read};

const SCHEMA: &str = "docs/signal-schema.json";

fn schema() -> Value {
    serde_json::from_str(&read(SCHEMA)).expect("the schema is JSON")
}

/// The names of an object's members, in the order the file writes them.
fn written_keys(text: &str, object: &Value) -> Vec<String> {
    let members = object.as_object().expect("an object");
    let mut keys: Vec<(usize, &String)> = members
        .keys()
        .map(|key| {
            let at = text
                .find(&format!("\"{key}\":"))
                .unwrap_or_else(|| panic!("{SCHEMA} has no {key}"));
            (at, key)
        })
        .collect();
    keys.sort();
    keys.into_iter().map(|(_, key)| key.clone()).collect()
}

fn strings(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("a list")
        .iter()
        .map(|item| item.as_str().expect("text"))
        .collect()
}

#[test]
fn the_schema_is_the_file_the_cases_were_checked_against() {
    let fixture = fixture();
    let digest = Sha256::digest(read(SCHEMA).as_bytes());
    assert_eq!(
        hex::encode(digest),
        fixture.schema_sha256,
        "{SCHEMA} changed: run the Python model again, so that what it records of the schema \
         is about this file"
    );
}

#[test]
fn the_schema_takes_every_signal_and_nothing_that_is_no_json() {
    let fixture = fixture();
    let mut loose = 0;
    for payload in &fixture.payloads {
        let Some(problems) = &payload.problems else {
            assert_eq!(payload.schema_accepts, Some(true), "{}", payload.id);
            continue;
        };
        let not_json = problems.iter().any(|problem| problem.code == "not_json");
        match payload.schema_accepts {
            // A body no JSON reader takes was never the schema's to judge;
            // one only this format calls too deep, or too large a number
            // for, still is.
            None => assert!(not_json, "{}", payload.id),
            Some(false) => {}
            Some(true) => {
                loose += 1;
                // What it lets through is what no schema can see.
                let unseen = [
                    "too_large",
                    "duplicate_field",
                    "wrong_type",
                    "invalid_value",
                    "bar_after_fire",
                ];
                assert!(
                    problems
                        .iter()
                        .all(|problem| unseen.contains(&problem.code.as_str())),
                    "{}: {problems:?}",
                    payload.id
                );
            }
        }
    }
    // Some do: the cases show what a schema cannot see, and the page lists
    // each of them.
    assert!(
        loose >= 10,
        "{loose} bodies pass the schema and are no signals"
    );
}

#[test]
fn the_schema_has_the_formats_fields_for_an_entry_and_for_an_exit() {
    let fixture = fixture();
    let text = read(SCHEMA);
    let schema = schema();
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    let kinds: Vec<&str> = SignalKind::ALL.iter().map(|kind| kind.as_str()).collect();
    let offered: Vec<&str> = schema["oneOf"]
        .as_array()
        .expect("oneOf")
        .iter()
        .map(|one| one["$ref"].as_str().expect("a reference"))
        .collect();
    assert_eq!(offered, ["#/$defs/entry", "#/$defs/exit"]);

    for kind in kinds {
        let shape = &schema["$defs"][kind];
        assert_eq!(shape["type"], "object", "{kind}");
        assert_eq!(shape["additionalProperties"], false, "{kind}");
        assert_eq!(shape["properties"]["kind"]["const"], kind);
        assert_eq!(
            strings(&shape["required"]),
            fixture.required[kind],
            "{kind}"
        );
        // The properties are the format's fields, in its order; an exit has
        // none of an entry's own.
        let start = text.find(&format!("\"{kind}\": {{")).expect("the shape");
        let expected: Vec<&str> = SIGNAL_FIELDS
            .into_iter()
            .filter(|field| kind == "entry" || !fixture.entry_only.iter().any(|only| only == field))
            .collect();
        assert_eq!(
            written_keys(&text[start..], &shape["properties"]),
            expected,
            "{kind}"
        );
    }
}

#[test]
fn the_schema_holds_each_value_to_the_formats_words_and_limits() {
    let fixture = fixture();
    let schema = schema();
    let defs = &schema["$defs"];
    assert_eq!(defs["version"]["const"], SIGNAL_VERSION);
    assert_eq!(defs["version"]["type"], "integer");
    let sides: Vec<&str> = SignalSide::ALL.iter().map(|side| side.as_str()).collect();
    assert_eq!(strings(&defs["side"]["enum"]), sides);
    let families: Vec<&str> = SignalCategory::ALL
        .iter()
        .map(|family| family.as_str())
        .collect();
    assert_eq!(strings(&defs["category"]["enum"]), families);
    assert_eq!(defs["ticker"]["minLength"], 1);
    assert_eq!(defs["ticker"]["maxLength"], MAX_TICKER_CHARS);
    for name in ["profile", "source"] {
        assert_eq!(
            defs[name]["pattern"], "^[a-z0-9][a-z0-9_-]{0,31}$",
            "{name}"
        );
    }
    assert_eq!(defs["exchange"]["pattern"], "^[A-Z0-9_]{1,32}$");
    for time in ["bar_time", "fire_time"] {
        assert_eq!(defs[time]["$ref"], "#/$defs/time", "{time}");
    }
    for kind in ["entry", "exit"] {
        for (field, shape) in defs[kind]["properties"].as_object().expect("properties") {
            // Every field is held to a rule of its own, by reference or in place.
            let held = shape.get("$ref").is_some() || shape.get("const").is_some();
            assert!(held, "{kind} {field}");
            if let Some(reference) = shape.get("$ref").and_then(Value::as_str) {
                let named = reference
                    .strip_prefix("#/$defs/")
                    .expect("a local reference");
                assert!(defs.get(named).is_some(), "{kind} {field}: {reference}");
            }
        }
    }

    // The ticker's pattern refuses beyond ASCII exactly what the format
    // lists, with everything up to U+007F left to the ASCII it names.
    let pattern = defs["ticker"]["pattern"].as_str().expect("a pattern");
    assert!(
        pattern.starts_with("^([A-Z0-9._-]|[^") && pattern.ends_with("])+$"),
        "{pattern}"
    );
    // Read back as ranges: a point followed by `-` opens one.
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut open: Option<u32> = None;
    for part in pattern.split("\\u").skip(1) {
        let point = u32::from_str_radix(&part[..4], 16).expect("a code point");
        match (open.take(), part[4..].starts_with('-')) {
            (Some(low), _) => ranges.push((low, point)),
            (None, true) => open = Some(point),
            (None, false) => ranges.push((point, point)),
        }
    }
    let mut listed = fixture.ticker_refused.clone();
    // The schema's first range starts at U+0000: below U+0080 the pattern's
    // other half decides, so the two say the same of every character.
    listed[0].0 = 0;
    assert_eq!(ranges, listed);
}
