//! The threat model, `docs/security/remote-execution.md`, held to what it
//! rests on: every test its table of evidence names is a test that exists,
//! and its table of guards says, in the column of the system the tests run
//! on, what a worker started here says it enforces. Each table sits between
//! `<!-- begin: name -->` and `<!-- end: name -->` markers.

use std::path::Path;

use quantforge::server::Guard;

use crate::support::{Pool, roomy};

const PAGE: &str = include_str!("../../docs/security/remote-execution.md");

/// A marked table's rows after its header and separator, cell by cell. No
/// cell of these tables holds a `|`.
fn rows(name: &str) -> Vec<Vec<String>> {
    let lines: Vec<&str> = PAGE.lines().collect();
    let marker = |kind: &str| {
        let marker = format!("<!-- {kind}: {name} -->");
        lines
            .iter()
            .position(|line| line.trim() == marker)
            .unwrap_or_else(|| panic!("the threat model has no {marker}"))
    };
    lines[marker("begin") + 3..marker("end")]
        .iter()
        .map(|row| {
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn every_test_the_threat_model_names_is_a_test_that_exists() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let evidence = rows("evidence");
    assert!(evidence.len() >= 30, "{} rows of evidence", evidence.len());
    for row in evidence {
        let [_, claim, test] = row.as_slice() else {
            panic!("a row of a layer, a claim and a test, not {row:?}");
        };
        let (file, function) = test
            .trim_matches('`')
            .split_once("::")
            .unwrap_or_else(|| panic!("{claim:?} names no test as file::function: {test}"));
        // The Python package is no part of the crate as it is published.
        if file.starts_with("quantforge-py/") && !root.join("quantforge-py").exists() {
            continue;
        }
        let path = file
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{claim:?} names {file}: {error}"));
        let declared = if file.ends_with(".py") {
            format!("def {function}(")
        } else {
            format!("fn {function}(")
        };
        assert!(
            source.contains(&declared),
            "{claim:?} names {function}, and {file} has no such test"
        );
    }
}

/// What a cell of the guards' table says enforces a guard: its first word,
/// `none` where nothing does.
fn mechanism(cell: &str) -> &str {
    cell.split(',')
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches('`')
}

#[test]
fn the_threat_model_tells_this_systems_guards_as_a_worker_started_here_reports_them() {
    let column = match std::env::consts::OS {
        "linux" => 1,
        "macos" => 2,
        "windows" => 3,
        other => panic!("the threat model has no column for {other}"),
    };
    let pool = Pool::start(1, roomy());
    let posture = pool.pool.posture();
    let table = rows("guards");
    let told = |guard: &str| -> String {
        let row = table
            .iter()
            .find(|row| row[0] == guard)
            .unwrap_or_else(|| panic!("the threat model's guards have no {guard} row"));
        row[column].clone()
    };
    for (guard, held) in [
        ("CPU time", &posture.cpu),
        ("Memory", &posture.memory),
        ("Network", &posture.network),
        ("Files", &posture.files),
    ] {
        let cell = told(guard);
        // A kernel without Landlock is the one guard the page lets go.
        let without =
            matches!(held, Guard::Unenforced { .. }) && cell.contains("where the kernel has it");
        assert!(
            without || held.to_string() == mechanism(&cell),
            "{guard}: the page says {cell:?}, and a worker here {held:?}"
        );
    }
    // The filter that refuses a socket refuses a process and a signal.
    assert_eq!(
        mechanism(&told("Processes and signals")),
        posture.network.to_string()
    );
    assert_eq!(told("Wall-clock time"), "the server kills the worker");
    assert_eq!(table.len(), 6, "{table:?}");
}
