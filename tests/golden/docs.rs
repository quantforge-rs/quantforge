//! `docs/golden-parity.md`, held to the files it describes: its tables are
//! the frozen files' own figures, its listing is the strategy's file, and
//! the tests it names exist. Each sits between `<!-- begin: name -->` and
//! `<!-- end: name -->` markers.

use super::{
    CANDLES, COLUMNS, Document, EXPECTED, EXPECTED_SHA256, STRATEGY, bar_at, candles, dataset,
    expected, read, sha256,
};
use std::path::PathBuf;

const PAGE: &str = "docs/golden-parity.md";

/// The lines between a section's two markers, as written.
fn section(page: &str, name: &str) -> Vec<String> {
    let marker = |kind: &str| {
        let text = format!("<!-- {kind}: {name} -->");
        let found: Vec<usize> = page
            .lines()
            .enumerate()
            .filter(|(_, line)| line.trim() == text)
            .map(|(at, _)| at)
            .collect();
        assert_eq!(found.len(), 1, "{PAGE} has one {text}");
        found[0]
    };
    let (begin, end) = (marker("begin"), marker("end"));
    page.lines()
        .skip(begin + 1)
        .take(end - begin - 1)
        .map(str::to_string)
        .collect()
}

/// A marked table's rows after its header and its rule, cell by cell. No
/// cell of these tables holds a `|`.
fn rows(page: &str, name: &str) -> Vec<Vec<String>> {
    section(page, name)
        .iter()
        .skip(2)
        .map(|row| {
            row.trim()
                .trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
}

/// A table's two columns as pairs.
fn pairs(page: &str, name: &str) -> Vec<(String, String)> {
    rows(page, name)
        .into_iter()
        .map(|row| {
            let [what, value] = &row[..] else {
                panic!("a row of {name} has {} cells, not 2", row.len());
            };
            (what.clone(), value.clone())
        })
        .collect()
}

/// A cell's text with its one code span's backticks taken off.
fn quoted(cell: &str) -> &str {
    cell.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .filter(|text| !text.contains('`'))
        .unwrap_or_else(|| panic!("{cell:?} is not one code span"))
}

fn owned(pairs: &[(&str, String)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(what, value)| (what.to_string(), value.clone()))
        .collect()
}

fn code(text: impl std::fmt::Display) -> String {
    format!("`{text}`")
}

#[test]
fn the_page_lists_the_files_of_the_fixture() {
    let page = read(PAGE);
    let listed: Vec<String> = rows(&page, "files")
        .iter()
        .map(|row| quoted(&row[0]).to_string())
        .collect();
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden");
    let mut held: Vec<String> = std::fs::read_dir(&directory)
        .expect("the fixture's directory")
        .map(|entry| {
            let name = entry.expect("an entry").file_name();
            name.to_string_lossy().into_owned()
        })
        // What importing the strategy's file leaves beside it, and what a
        // desktop or an editor hides in a directory, are not the fixture's.
        .filter(|name| name != "__pycache__" && !name.starts_with('.'))
        .collect();
    held.sort();
    assert_eq!(listed, held);
    for path in [CANDLES, EXPECTED, STRATEGY] {
        let name = path.rsplit('/').next().expect("a name");
        assert!(listed.iter().any(|file| file == name), "{name}");
    }
}

#[test]
fn the_page_states_the_run_and_its_result_as_the_file_has_them() {
    let page = read(PAGE);
    let Document {
        dataset,
        strategy,
        config,
        result,
        ..
    } = expected();
    let market = &dataset.market;
    assert_eq!(
        pairs(&page, "run"),
        owned(&[
            (
                "Market",
                format!(
                    "`{}` `{}` `{}`",
                    market.exchange, market.symbol, market.interval
                )
            ),
            ("Strategy", code(&strategy.name)),
            ("`fast`", code(strategy.params.fast)),
            ("`slow`", code(strategy.params.slow)),
            ("`initial_cash`", code(&config.initial_cash)),
            ("`fee_bps`", code(&config.fee_bps)),
            ("`close_out_at_end`", code(config.close_out_at_end)),
        ])
    );
    assert_eq!(
        pairs(&page, "result"),
        owned(&[
            ("`initial_cash`", code(&result.initial_cash)),
            ("`final_equity`", code(&result.final_equity)),
            ("`total_return_pct`", code(&result.total_return_pct)),
            ("`trade_count`", code(result.trade_count)),
            ("`max_drawdown_pct`", code(&result.max_drawdown_pct)),
        ])
    );
}

#[test]
fn the_page_lists_every_trade_as_the_file_has_it() {
    let page = read(PAGE);
    let (document, candles) = (expected(), candles());
    let last = candles.last().expect("a last candle");
    let written: Vec<Vec<String>> = document
        .result
        .trades
        .iter()
        .zip(1..)
        .map(|(trade, number)| {
            // A fill is at a bar's open, but for the close-out, which is at
            // the last bar's close.
            let exit = if trade.exit_time_ms == last.close_time_ms {
                format!("{}, at its close", candles.len() - 1)
            } else {
                bar_at(&candles, trade.exit_time_ms).to_string()
            };
            vec![
                number.to_string(),
                bar_at(&candles, trade.entry_time_ms).to_string(),
                exit,
                code(&trade.entry_price),
                code(&trade.exit_price),
                code(&trade.qty),
                code(&trade.gross_quote_pnl),
            ]
        })
        .collect();
    assert_eq!(rows(&page, "trades"), written);
    assert_eq!(written.len(), document.result.trade_count);
}

#[test]
fn the_page_describes_the_dataset_as_it_is() {
    let page = read(PAGE);
    let (document, candles) = (expected(), candles());
    let dataset = &document.dataset;
    let first_day = candles[0].open_time_ms.div_euclid(86_400_000);
    let on_the_first = candles
        .iter()
        .filter(|candle| candle.open_time_ms.div_euclid(86_400_000) == first_day)
        .count();
    let [first_hour, second_hour] = dataset::STILL;
    assert_eq!(
        pairs(&page, "dataset"),
        owned(&[
            ("Candles", code(dataset.candles)),
            (
                "First open",
                format!(
                    "`{}`, which is `{}`",
                    dataset.first_open, dataset.first_open_ms
                )
            ),
            (
                "Last open",
                format!(
                    "`{}`, which is `{}`",
                    dataset.last_open, dataset.last_open_ms
                )
            ),
            ("On the first of the two days, UTC", code(on_the_first)),
            (
                "Still hours",
                format!(
                    "Bars `{}` to `{}`, and `{}` to `{}`",
                    first_hour.start,
                    first_hour.end - 1,
                    second_hour.start,
                    second_hour.end - 1
                )
            ),
            (
                "Quiet stretch",
                format!(
                    "Bars `{}` to `{}`",
                    dataset::QUIET.start,
                    dataset::QUIET.end - 1
                )
            ),
        ])
    );

    // The file's columns, in its first line's order.
    let columns: Vec<String> = rows(&page, "columns")
        .iter()
        .map(|row| quoted(&row[0]).to_string())
        .collect();
    assert_eq!(columns.join(","), COLUMNS);
}

/// The fields of the document, in the file's order: each field of the top
/// level, and under one that holds a record, each of the record's.
fn fields(text: &str) -> Vec<String> {
    // The records the page takes apart; `result` is the engine's own type.
    const RECORDS: [&str; 4] = ["oracle", "dataset", "strategy", "config"];
    let key = |line: &str, indent: usize| -> Option<String> {
        let rest = line.strip_prefix(&" ".repeat(indent))?.strip_prefix('"')?;
        let (name, after) = rest.split_once('"')?;
        after.starts_with(':').then(|| name.to_string())
    };
    let mut found = Vec::new();
    let mut within: Option<String> = None;
    for line in text.lines() {
        if let Some(top) = key(line, 2) {
            if RECORDS.contains(&top.as_str()) {
                within = Some(top);
            } else {
                within = None;
                found.push(top);
            }
        } else if let (Some(top), Some(field)) = (&within, key(line, 4)) {
            found.push(format!("{top}.{field}"));
        }
    }
    found
}

#[test]
fn the_page_lists_every_field_of_the_document() {
    let page = read(PAGE);
    let listed: Vec<String> = rows(&page, "fields")
        .iter()
        .map(|row| quoted(&row[0]).to_string())
        .collect();
    let written = fields(&read(EXPECTED));
    assert_eq!(listed, written);
    assert_eq!(
        (written.first(), written.last()),
        (Some(&"format".to_string()), Some(&"decisions".to_string()))
    );
    assert!(written.contains(&"dataset.sha256".to_string()));
}

#[test]
fn the_page_says_where_the_results_come_from_and_pins_both_files() {
    let page = read(PAGE);
    let document = expected();
    let oracle = &document.oracle;
    assert_eq!(
        pairs(&page, "oracle"),
        owned(&[
            ("Strategy", code(&oracle.strategy)),
            ("Engine", code(&oracle.engine)),
            ("Commit", code(&oracle.commit)),
            ("Decimal arithmetic", code(&oracle.decimal)),
            ("Written on", code(&oracle.written_on)),
        ])
    );
    // The two hashes: the dataset's, which the document names, and the
    // document's own, which the tests pin.
    assert_eq!(
        pairs(&page, "frozen"),
        owned(&[
            ("`candles.csv`", code(sha256(&read(CANDLES)))),
            ("`sma_cross.json`", code(EXPECTED_SHA256)),
        ])
    );
    assert_eq!(sha256(&read(CANDLES)), document.dataset.sha256);
}

#[test]
fn the_page_shows_the_strategy_as_its_file_has_it() {
    let page = read(PAGE);
    let lines = section(&page, "strategy");
    assert_eq!(lines.first().map(String::as_str), Some("```python"));
    assert_eq!(lines.last().map(String::as_str), Some("```"));
    let shown = lines[1..lines.len() - 1].join("\n") + "\n";
    assert!(
        shown == read(STRATEGY),
        "{PAGE} shows another strategy than {STRATEGY}"
    );
}

#[test]
fn the_page_names_the_tests_that_hold_the_goldens() {
    let page = read(PAGE);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let held_by = rows(&page, "held by");
    let tests: Vec<&str> = held_by.iter().map(|row| quoted(&row[0])).collect();
    assert_eq!(
        tests,
        [
            "tests/golden/oracle.rs",
            "tests/golden/main.rs",
            "tests/golden/dataset.rs",
            "tests/golden/docs.rs",
            "quantforge-py/tests/test_golden_parity.py",
        ]
    );
    for row in &held_by {
        // The crate is packaged without the Python package, whose test is
        // there in a checkout alone.
        let path = quoted(&row[0]);
        let packaged_without = path.starts_with("quantforge-py/");
        assert!(
            root.join(path).is_file() || (packaged_without && !root.join("quantforge-py").exists()),
            "{path}"
        );
        // The job that runs it, by the name branch protection requires.
        let job = row[1].split(',').next().expect("a job");
        assert_eq!(
            quoted(job),
            if packaged_without { "python" } else { "checks" }
        );
    }
}
