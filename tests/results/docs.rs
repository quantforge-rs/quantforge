//! `docs/result-spec.md`, held to the code and to the shared cases: the
//! format's number, the three files, the metrics' fields, each table's
//! columns, the limits, and the worked example, byte for byte.

use quantforge::protocol::results::{
    ColumnType, EQUITY_COLUMNS, EquityRow, MAX_ROWS, Metrics, RESULT_FORMAT, ResultKind,
    TRADES_COLUMNS, encode_equity,
};

use crate::cases::fixture;

const DOC: &str = include_str!("../../docs/result-spec.md");

/// The lines between a section's markers, without code fences.
fn section(name: &str) -> Vec<&'static str> {
    let lines: Vec<&str> = DOC.lines().collect();
    let at = |marker: String| {
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("docs/result-spec.md has no {marker}"))
    };
    let (begin, end) = (
        at(format!("<!-- begin: {name} -->")),
        at(format!("<!-- end: {name} -->")),
    );
    lines[begin + 1..end]
        .iter()
        .copied()
        .filter(|line| !line.starts_with("```"))
        .collect()
}

/// A list's items as their backticked name and what follows the colon.
fn items(name: &str) -> Vec<(String, String)> {
    section(name)
        .into_iter()
        .map(|line| {
            let item = line
                .strip_prefix("- ")
                .unwrap_or_else(|| panic!("{name}: {line:?} is no list item"));
            let (key, value) = item
                .split_once(": ")
                .unwrap_or_else(|| panic!("{name}: {line:?} has no colon"));
            (key.trim_matches('`').to_string(), value.to_string())
        })
        .collect()
}

#[test]
fn the_doc_states_the_format_the_code_writes() {
    assert_eq!(section("result format"), [RESULT_FORMAT.to_string()]);
    assert!(DOC.contains(&format!("**Result format {RESULT_FORMAT}.**")));
    assert_eq!(fixture().format, RESULT_FORMAT);
}

#[test]
fn the_doc_lists_the_three_files_with_their_names_and_media_types() {
    let rows: Vec<Vec<String>> = section("files")
        .into_iter()
        .skip(2)
        .map(|row| {
            row.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().trim_matches('`').to_string())
                .collect()
        })
        .collect();
    let expected: Vec<Vec<String>> = ResultKind::ALL
        .iter()
        .map(|kind| {
            [kind.name(), kind.file_name(), kind.media_type()]
                .map(str::to_string)
                .to_vec()
        })
        .collect();
    assert_eq!(rows, expected);
}

#[test]
fn the_doc_lists_the_metrics_fields_in_the_order_they_are_written() {
    let metrics: Metrics = serde_json::from_str(
        r#"{"initial_cash":"1","final_equity":"2","total_return_pct":"3","max_drawdown_pct":"4","trade_count":5}"#,
    )
    .expect("metrics");
    let file = String::from_utf8(metrics.encode().expect("a file")).expect("JSON");
    // Each field as the file writes it: `"name":` and a value that is a
    // string for a decimal and a bare number for an integer.
    let written: Vec<(String, String)> = file
        .trim_matches(['{', '}'])
        .split(',')
        .map(|field| {
            let (name, value) = field.split_once(':').expect("a field");
            let kind = if value.starts_with('"') {
                "a decimal"
            } else {
                "an integer"
            };
            (name.trim_matches('"').to_string(), kind.to_string())
        })
        .collect();
    assert_eq!(items("metrics fields"), written);
}

fn columns(listed: &[(&str, ColumnType)]) -> Vec<(String, String)> {
    listed
        .iter()
        .map(|(name, kind)| {
            let kind = match kind {
                ColumnType::Int64 => "`Int64`",
                ColumnType::Utf8 => "`Utf8`",
            };
            (name.to_string(), kind.to_string())
        })
        .collect()
}

#[test]
fn the_doc_lists_each_tables_columns_and_types_in_order() {
    assert_eq!(items("trades columns"), columns(&TRADES_COLUMNS));
    assert_eq!(items("equity columns"), columns(&EQUITY_COLUMNS));
}

#[test]
fn the_doc_states_the_limits_the_code_enforces() {
    assert_eq!(
        items("limits"),
        [
            ("rows per table".to_string(), MAX_ROWS.to_string()),
            (
                "text per column, in bytes".to_string(),
                i32::MAX.to_string()
            ),
        ]
    );
}

#[test]
fn the_worked_example_is_the_file_the_code_writes_and_the_shared_case() {
    let shown: String = section("worked example").concat();
    let rows = [
        EquityRow {
            open_time_ms: 1_792_454_400_000,
            equity: "10000".parse().expect("an amount"),
        },
        EquityRow {
            open_time_ms: 1_792_454_460_000,
            equity: "10012.50".parse().expect("an amount"),
        },
    ];
    let file = encode_equity(&rows).expect("a file");
    assert_eq!(shown, hex::encode(&file));

    let fixture = fixture();
    let case = fixture
        .cases
        .iter()
        .find(|case| case.name == "equity.two_bars")
        .expect("the shared case");
    assert_eq!(case.file_hex.as_deref(), Some(shown.as_str()));
    assert!(
        DOC.contains(&format!("`{}`", case.hash)),
        "the example's hash"
    );
    assert!(
        DOC.contains(&format!("The file is {} bytes", case.size_bytes)),
        "the example's size"
    );
    for (open_time_ms, equity) in [
        (1_792_454_400_000_i64, "10000"),
        (1_792_454_460_000, "10012.50"),
    ] {
        assert!(
            DOC.contains(&format!("| {open_time_ms} | `{equity}` |")),
            "the example's rows"
        );
    }
    // The three body lines the page prints are the file's bytes there.
    for (at, line) in [(536, 0), (552, 1), (568, 2)] {
        let bytes: Vec<String> = file[at..at + 16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(
            DOC.contains(&format!("{at}  {}", bytes.join(" "))),
            "body line {line}"
        );
    }
}

#[test]
fn the_metrics_example_is_a_shared_cases_file() {
    let fixture = fixture();
    let case = fixture
        .cases
        .iter()
        .find(|case| case.name == "metrics.sma_cross")
        .expect("the shared case");
    let file = hex::decode(case.file_hex.as_deref().expect("a file")).expect("hex");
    let text = String::from_utf8(file).expect("JSON");
    assert!(DOC.contains(&format!("```json\n{text}\n```")), "{text}");
}
