//! The boundary around `src/exchange/`, checked three times.
//!
//! - Only the exchange module may name a concrete adapter. Everything else —
//!   the engines, the CLI, the model, the ports, and any module added later
//!   — reaches an exchange through the `ports` connector traits, and the CLI
//!   gets its connectors from `exchange::connect`.
//! - Outside the Binance adapter, nothing hardcodes Binance at all. The code
//!   that may name it is listed below: the registry's entries and default,
//!   the `Venue` that names it with its product lines in `ExchangeId` and
//!   their storage names, and the two renamed settings the CLI still
//!   refuses. The CLI's `--help` names it only as `--exchange`'s default and
//!   only value.
//! - A venue's own position fields, Binance's `positionSide` and Bybit's
//!   `positionIdx`, stay inside the adapters, which map them onto the
//!   model's `PositionSide`.
//!
//! The source scans are textual. Every line is checked except comments: a
//! line whose trimmed text starts with `//`, which covers `///` and `//!`,
//! and with them the doctests inside doc comments. The Binance scan also
//! skips test modules, whose fixtures name the one exchange there is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Names only an exchange adapter's own module may use.
const FORBIDDEN: [&str; 4] = [
    "exchange::binance",
    "binance::",
    "BinanceSpotClient",
    "BinanceCredentials",
];

/// The only code outside `src/exchange/binance/` that may name Binance, as
/// `(file, trimmed line)`. Each entry must match exactly once, so a stale
/// entry fails as well as a new line.
const NAMES_BINANCE: [(&str, &str); 20] = [
    // The registry: the adapter module, its entry, the lines still waiting
    // for an adapter, and the default exchange.
    ("src/exchange/mod.rs", "pub mod binance;"),
    ("src/exchange/registry.rs", "use super::binance;"),
    (
        "src/exchange/registry.rs",
        "ExchangeId::BinanceSpot => Some(binance::REGISTRATION),",
    ),
    (
        "src/exchange/registry.rs",
        "ExchangeId::BinanceLinear | ExchangeId::BinanceInverse => None,",
    ),
    (
        "src/exchange/registry.rs",
        "pub const DEFAULT_EXCHANGE: ExchangeId = ExchangeId::BinanceSpot;",
    ),
    (
        "src/exchange/registry.rs",
        "const EXCHANGES: [ExchangeId; 1] = [ExchangeId::BinanceSpot];",
    ),
    // The venue, its name, and its product lines with the venue, category
    // and storage name of each.
    ("src/model/market.rs", "Binance,"),
    (
        "src/model/market.rs",
        "pub const ALL: [Self; 2] = [Self::Binance, Self::Bybit];",
    ),
    ("src/model/market.rs", "Self::Binance => \"binance\","),
    ("src/model/market.rs", "BinanceSpot,"),
    ("src/model/market.rs", "BinanceLinear,"),
    ("src/model/market.rs", "BinanceInverse,"),
    (
        "src/model/market.rs",
        "Self::BinanceSpot => (Venue::Binance, Category::Spot, \"binance_spot\"),",
    ),
    (
        "src/model/market.rs",
        "Self::BinanceLinear => (Venue::Binance, Category::Linear, \"binance_linear\"),",
    ),
    (
        "src/model/market.rs",
        "Self::BinanceInverse => (Venue::Binance, Category::Inverse, \"binance_inverse\"),",
    ),
    (
        "src/model/market.rs",
        "(Venue::Binance, Category::Spot) => Self::BinanceSpot,",
    ),
    (
        "src/model/market.rs",
        "(Venue::Binance, Category::Linear) => Self::BinanceLinear,",
    ),
    (
        "src/model/market.rs",
        "(Venue::Binance, Category::Inverse) => Self::BinanceInverse,",
    ),
    // The released names of `--base-url`, recognized only to be refused.
    (
        "src/cli/mod.rs",
        "const LEGACY_BASE_URL_FLAG: &str = \"binance-base-url\";",
    ),
    (
        "src/cli/mod.rs",
        "const LEGACY_BASE_URL_ENV: &str = \"QF_BINANCE_BASE_URL\";",
    ),
];

/// The venues' own position fields: one venue's position side in hedge
/// mode, and the other's position index. Matched case-sensitively, so the
/// model's `PositionSide` is not one of them.
const POSITION_FIELDS: [&str; 2] = ["positionIdx", "positionSide"];

/// Where the venue adapters live, the only code that may name those fields.
const ADAPTERS: [&str; 2] = ["src/exchange/binance/", "src/exchange/bybit/"];

fn crate_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Every `.rs` file under `dir` except those under `skip`, keyed by its
/// `/`-joined path from the crate root on every platform.
fn source_files(dir: &Path, relative: &str, skip: &str, files: &mut BTreeMap<String, PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().into_string().expect("utf-8 file name");
        let path = format!("{relative}/{name}");
        if entry.file_type().expect("file type").is_dir() {
            if path != skip {
                source_files(&entry.path(), &path, skip, files);
            }
        } else if name.ends_with(".rs") {
            files.insert(path, entry.path());
        }
    }
}

/// The source files of the crate outside `skip`.
fn crate_sources(skip: &str) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    source_files(&crate_root().join("src"), "src", skip, &mut files);
    assert!(
        files.len() > 40,
        "expected to scan the whole crate, found {} files",
        files.len()
    );
    files
        .into_iter()
        .map(|(relative, path)| {
            let text = std::fs::read_to_string(&path).expect("read source file");
            (relative, text)
        })
        .collect()
}

/// The non-comment lines among `lines` that `names` matches, numbered from
/// 1.
fn matching_lines<'a>(
    lines: impl Iterator<Item = &'a str>,
    names: impl Fn(&str) -> bool,
) -> Vec<(usize, &'a str)> {
    lines
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| names(line))
        .map(|(index, line)| (index + 1, line.trim()))
        .collect()
}

/// The non-comment lines of `text` that name a forbidden item, numbered
/// from 1.
fn violations(text: &str) -> Vec<(usize, &str)> {
    matching_lines(text.lines(), |line| {
        FORBIDDEN.iter().any(|name| line.contains(name))
    })
}

fn names_binance(line: &str) -> bool {
    line.to_ascii_lowercase().contains("binance")
}

fn names_a_position_field(line: &str) -> bool {
    POSITION_FIELDS.iter().any(|field| line.contains(field))
}

/// Where the test module that `declaring` declares as `mod name;` lives:
/// beside a `mod.rs`, `lib.rs`, or `main.rs`, and in a directory named
/// after any other file.
fn out_of_line_module(declaring: &str, name: &str) -> [String; 2] {
    let (dir, file) = declaring.rsplit_once('/').expect("a path under src");
    let stem = file.strip_suffix(".rs").expect("a .rs file");
    let dir = if matches!(stem, "mod" | "lib" | "main") {
        dir.to_string()
    } else {
        format!("{dir}/{stem}")
    };
    [format!("{dir}/{name}.rs"), format!("{dir}/{name}/mod.rs")]
}

/// How many leading lines of each file are code rather than tests, for
/// every file that is not itself a test module. A file's tests sit in one
/// module that opens with `#[cfg(test)]`: either in the file, where it is
/// the last item, so the code ends above that line, or out of line, where
/// the module's own file is all test code.
fn code_lines(files: &BTreeMap<String, String>) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    let mut test_files = Vec::new();
    for (path, text) in files {
        let lines: Vec<&str> = text.lines().collect();
        let marks: Vec<usize> = (0..lines.len())
            .filter(|&index| lines[index].trim() == "#[cfg(test)]")
            .collect();
        assert!(marks.len() <= 1, "{path}: more than one #[cfg(test)]");
        let Some(&mark) = marks.first() else {
            counts.insert(path.as_str(), lines.len());
            continue;
        };
        let item = lines.get(mark + 1).map_or("", |line| line.trim());
        match item.strip_prefix("mod ") {
            Some(module) if module.ends_with(" {") => {
                counts.insert(path.as_str(), mark);
            }
            Some(module) if module.ends_with(';') => {
                let name = module.trim_end_matches(';');
                let candidates = out_of_line_module(path, name);
                let file = candidates
                    .iter()
                    .find(|candidate| files.contains_key(candidate.as_str()))
                    .unwrap_or_else(|| {
                        panic!("{path}: test module {name} is in none of {candidates:?}")
                    });
                test_files.push(file.clone());
                counts.insert(path.as_str(), lines.len());
            }
            _ => panic!("{path}:{}: #[cfg(test)] must open a module", mark + 2),
        }
    }
    for file in &test_files {
        counts.remove(file.as_str());
    }
    counts
}

#[test]
fn nothing_outside_src_exchange_names_a_concrete_exchange_adapter() {
    let mut found = Vec::new();
    for (relative, text) in &crate_sources("src/exchange") {
        for (line, code) in violations(text) {
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
    let client = crate_root()
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

// The allowlist doubles as the positive control: its entries are real
// lines the scan has to find.
#[test]
fn nothing_outside_the_binance_adapter_hardcodes_binance() {
    let files = crate_sources("src/exchange/binance");
    let mut allowed: BTreeMap<(&str, &str), usize> =
        NAMES_BINANCE.iter().map(|&entry| (entry, 0)).collect();
    let mut found = Vec::new();
    for (path, count) in code_lines(&files) {
        for (line, text) in matching_lines(files[path].lines().take(count), names_binance) {
            match allowed.get_mut(&(path, text)) {
                Some(count) => *count += 1,
                None => found.push(format!("{path}:{line}: {text}")),
            }
        }
    }
    assert!(
        found.is_empty(),
        "these lines hardcode Binance outside src/exchange/binance/; name the exchange \
         through the registry (exchange::name, exchange::lookup), or branch on its \
         capabilities:\n  {}",
        found.join("\n  ")
    );
    let miscounted: Vec<_> = allowed.iter().filter(|(_, count)| **count != 1).collect();
    assert!(
        miscounted.is_empty(),
        "NAMES_BINANCE entries must each match exactly one line: {miscounted:#?}"
    );
}

#[test]
fn venue_position_fields_stay_inside_the_adapters() {
    let files = crate_sources("src/exchange/binance");
    let mut found = Vec::new();
    for (path, count) in code_lines(&files) {
        if ADAPTERS.iter().any(|adapter| path.starts_with(adapter)) {
            continue;
        }
        let lines = files[path].lines().take(count);
        for (line, text) in matching_lines(lines, names_a_position_field) {
            found.push(format!("{path}:{line}: {text}"));
        }
    }
    assert!(
        found.is_empty(),
        "these lines name a venue's own position field outside its adapter; map it onto \
         PositionSide inside the adapter:\n  {}",
        found.join("\n  ")
    );
}

// Without this, a field list the matcher could never find would pass the
// test above. The venue notes name both fields.
#[test]
fn the_position_field_matcher_finds_both_fields_in_the_venue_notes() {
    let notes = crate_root().join("docs").join("futures-venues.md");
    let text = std::fs::read_to_string(notes).expect("read the venue notes");
    for field in POSITION_FIELDS {
        let named = text
            .lines()
            .any(|line| names_a_position_field(line) && line.contains(field));
        assert!(named, "docs/futures-venues.md no longer names {field}");
    }
}

// The help a user sees is the rendered surface of those sources; the
// committed snapshots are that output, byte for byte.
#[test]
fn help_names_binance_only_as_the_exchange_default() {
    let dir = crate_root().join("tests").join("snapshots");
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("read snapshot dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|ext| ext != "txt") {
            continue;
        }
        let name = path.file_name().expect("file name").to_string_lossy();
        let text = std::fs::read_to_string(&path).expect("read snapshot");
        let naming: Vec<&str> = text.lines().filter(|line| names_binance(line)).collect();
        // Every command's help lists the global --exchange; --version does not.
        let expected = usize::from(name != "version.txt");
        assert!(
            naming.len() == expected
                && naming
                    .iter()
                    .all(|line| line.contains("[env: QF_EXCHANGE=]")),
            "{name} must name Binance exactly {expected} time(s), in --exchange's help line; \
             found {naming:#?}"
        );
        checked += 1;
    }
    assert!(checked > 15, "expected every snapshot, read {checked}");
}
