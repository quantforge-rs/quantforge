//! The spec document, `docs/segment-spec.md`, held to the code: its
//! constants are the code's, and its worked example encodes, as it says,
//! to its payload and its name. Each is found between
//! `<!-- begin: name -->` and `<!-- end: name -->` markers.

use std::path::Path;

use quantforge::Candle;
use quantforge::storage::segments::{self, FORMAT, MAX_PAYLOAD_BYTES, WINDOW_BARS};

use crate::cases::TextCandle;

fn read(relative: &str) -> String {
    let path = relative.split('/').fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        |path, part| path.join(part),
    );
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// The lines of a marked section, as written.
fn section(name: &str) -> Vec<String> {
    let spec = read("docs/segment-spec.md");
    let lines: Vec<&str> = spec.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/segment-spec.md has no {marker}"))
    };
    let (begin, end) = (marker("begin"), marker("end"));
    lines[begin + 1..end]
        .iter()
        .map(|line| line.to_string())
        .collect()
}

/// A marked section's one line of text.
fn line(name: &str) -> String {
    let lines: Vec<String> = section(name)
        .into_iter()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "{name}: one line");
    lines[0].clone()
}

/// A marked section's fenced block, without its fences.
fn fenced(name: &str) -> Vec<String> {
    let lines = section(name);
    let fences: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("```"))
        .map(|(index, _)| index)
        .collect();
    let [open, close] = fences[..] else {
        panic!("{name}: one fenced block");
    };
    lines[open + 1..close].to_vec()
}

#[test]
fn the_spec_states_the_codes_numbers() {
    assert_eq!(line("window bars"), WINDOW_BARS.to_string());
    assert_eq!(line("max payload bytes"), MAX_PAYLOAD_BYTES.to_string());

    let layout = section("payload layout");
    let fields: Vec<&str> = layout
        .iter()
        .skip(2)
        .filter_map(|row| row.trim().trim_matches('|').split('|').nth(1))
        .map(str::trim)
        .collect();
    assert_eq!(fields.len(), 12, "{layout:?}");
    assert!(fields[0].contains("`QFSEGMNT`"), "{}", fields[0]);
    assert!(fields[1].contains(&format!("`{FORMAT}`")), "{}", fields[1]);
    assert!(fields[2].contains("1 to 1,440"), "{}", fields[2]);
}

#[test]
fn the_worked_example_packs_to_its_payload_and_name() {
    let candles: Vec<TextCandle> =
        serde_json::from_str(&fenced("worked example candles").join("\n")).expect("candles");
    let candles: Vec<Candle> = candles.iter().map(TextCandle::candle).collect();
    // Each line is hex, then what it holds.
    let hex: String = fenced("worked example payload")
        .iter()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let payload = segments::encode(&candles).expect("payload");
    assert_eq!(hex::encode(&payload), hex);
    assert_eq!(
        segments::hash(&payload).to_string(),
        line("worked example hash")
    );
    assert_eq!(segments::decode(&payload).expect("candles"), candles);
}

#[test]
fn the_format_is_the_one_the_protocol_names() {
    assert_eq!(FORMAT, quantforge::protocol::segments::SEGMENT_FORMAT);
}
