//! The spec document, `docs/bundle-spec.md`, held to the code: its lists and
//! tables are the code's, and its worked example hashes as it says. Each is
//! found between `<!-- begin: name -->` and `<!-- end: name -->` markers.

use std::collections::BTreeMap;
use std::path::Path;

use quantforge::protocol::bundle::{
    ALLOWED_EXTENSIONS, Bundle, BundleLimits, MANIFEST_KEYS, MAX_MANIFEST_BYTES, ProblemCode,
    RESERVED_NAMES,
};

const SPEC_URL: &str = "https://github.com/quantforge-rs/quantforge/blob/main/docs/bundle-spec.md";

fn read(relative: &str) -> String {
    let path = relative.split('/').fold(
        Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf(),
        |path, part| path.join(part),
    );
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

/// The lines of a marked section, as written.
fn section(name: &str) -> Vec<String> {
    let spec = read("docs/bundle-spec.md");
    let lines: Vec<&str> = spec.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/bundle-spec.md has no {marker}"))
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

/// A marked table's rows, as their cells without backticks, after the header
/// and separator rows.
fn rows(name: &str) -> Vec<Vec<String>> {
    section(name)
        .iter()
        .skip(2)
        .map(|row| {
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_string())
                .collect()
        })
        .collect()
}

/// A marked fenced code block's text, with a final newline.
fn fenced(name: &str) -> String {
    let lines = section(name);
    let fences = lines.first().is_some_and(|line| line.starts_with("```"))
        && lines.last().is_some_and(|line| line == "```");
    assert!(fences, "{name}: one fenced block");
    lines[1..lines.len() - 1]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect()
}

#[test]
fn the_problem_code_table_lists_every_code_in_order() {
    let listed: Vec<String> = rows("problem codes")
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    let codes: Vec<&str> = ProblemCode::ALL.iter().map(|code| code.as_str()).collect();
    assert_eq!(listed, codes);
}

#[test]
fn the_manifest_key_table_lists_every_key_in_order() {
    let listed: Vec<String> = rows("manifest keys")
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    assert_eq!(listed, MANIFEST_KEYS);
}

#[test]
fn the_extensions_and_reserved_names_are_the_codes() {
    let extensions: Vec<String> = ALLOWED_EXTENSIONS
        .iter()
        .map(|ext| format!("`.{ext}`"))
        .collect();
    assert_eq!(line("extensions"), extensions.join(", "));
    assert_eq!(line("reserved names"), RESERVED_NAMES.join(", "));
}

#[test]
fn the_limits_table_gives_the_defaults_by_their_wire_names() {
    let listed: BTreeMap<String, u64> = rows("limits")
        .into_iter()
        .map(|row| {
            (
                row[0].clone(),
                row[1].parse().expect("a number of bytes or files"),
            )
        })
        .collect();
    let defaults = serde_json::to_value(BundleLimits::default()).expect("limits as JSON");
    let mut expected: BTreeMap<String, u64> = defaults
        .as_object()
        .expect("an object")
        .iter()
        .map(|(name, value)| (name.clone(), value.as_u64().expect("a count")))
        .collect();
    expected.insert("MAX_MANIFEST_BYTES".to_string(), MAX_MANIFEST_BYTES);
    assert_eq!(listed, expected);
}

#[test]
fn the_worked_example_hashes_as_the_spec_says() {
    let manifest = fenced("example quantforge.toml");
    let strategy = fenced("example strategy.py");
    let bundle = Bundle::from_files(
        [("quantforge.toml", manifest), ("strategy.py", strategy)],
        &BundleLimits::default(),
    )
    .unwrap_or_else(|error| panic!("the example is a bundle: {error}"));
    assert_eq!(
        format!("`{}`", bundle.code_hash()),
        line("example code hash")
    );
    assert_eq!(
        bundle.canonical_tar().len(),
        3072,
        "the size the spec gives"
    );
    assert_eq!(bundle.manifest().entry().class(), "SmaCross");
}

#[test]
fn the_rustdoc_links_to_the_spec() {
    for source in ["src/protocol/mod.rs", "src/protocol/bundle/mod.rs"] {
        assert!(
            read(source).contains(SPEC_URL),
            "{source} links to the spec"
        );
    }
    let path = SPEC_URL
        .split("/blob/main/")
        .nth(1)
        .expect("a path in the repository");
    assert!(!read(path).is_empty());
}
