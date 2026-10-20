//! The import boundary around `src/exchange/`: only the exchange module may
//! name a concrete adapter. Everything else — the engines, the CLI, the
//! model, the ports, and any module added later — reaches an exchange
//! through the `ports` connector traits, and the CLI gets its connectors
//! from `exchange::connect`, so the boundary needs no exemptions.
//!
//! The scan is textual. Every line is checked except comments: a line whose
//! trimmed text starts with `//`, which covers `///` and `//!`, and with them
//! the doctests inside doc comments.

use std::path::{Path, PathBuf};

/// Names only an exchange adapter's own module may use.
const FORBIDDEN: [&str; 4] = [
    "exchange::binance",
    "binance::",
    "BinanceSpotClient",
    "BinanceCredentials",
];

/// Every `.rs` file under `dir` except those under `src/exchange`, keyed by
/// its `/`-joined path from the crate root on every platform.
fn source_files(dir: &Path, relative: &str, files: &mut Vec<(String, PathBuf)>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().into_string().expect("utf-8 file name");
        let path = format!("{relative}/{name}");
        if entry.file_type().expect("file type").is_dir() {
            if path != "src/exchange" {
                source_files(&entry.path(), &path, files);
            }
        } else if name.ends_with(".rs") {
            files.push((path, entry.path()));
        }
    }
}

/// The non-comment lines of `text` that name a forbidden item, numbered
/// from 1.
fn violations(text: &str) -> Vec<(usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| FORBIDDEN.iter().any(|name| line.contains(name)))
        .map(|(index, line)| (index + 1, line.trim()))
        .collect()
}

#[test]
fn nothing_outside_src_exchange_names_a_concrete_exchange_adapter() {
    let mut files = Vec::new();
    source_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        "src",
        &mut files,
    );
    assert!(
        files.len() > 40,
        "expected to scan the whole crate, found {} files",
        files.len()
    );

    let mut found = Vec::new();
    for (relative, path) in &files {
        let text = std::fs::read_to_string(path).expect("read source file");
        for (line, code) in violations(&text) {
            found.push(format!("{relative}:{line}: {code}"));
        }
    }
    assert!(
        found.is_empty(),
        "these lines name a concrete exchange adapter outside src/exchange/; use the \
         ports connector traits, and let exchange::connect build the connectors:\n  {}",
        found.join("\n  ")
    );
}

// Without this, a matcher that could never match would pass the test above.
#[test]
fn the_matcher_finds_the_adapter_where_it_belongs() {
    let client = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("exchange")
        .join("binance")
        .join("client.rs");
    let text = std::fs::read_to_string(client).expect("read the binance client");
    assert!(
        !violations(&text).is_empty(),
        "the adapter's own file names it, so the matcher must find it"
    );
}
