//! `src/protocol/` is the wire format, meant to become a crate of its own,
//! so it may use nothing else from QuantForge. Checked twice:
//!
//! - This test crate compiles the module as its only content, under another
//!   name. A path into the rest of the crate — `crate::model`, `super::` out
//!   of the module root, `crate::protocol::…`, a macro defined elsewhere —
//!   resolves in the library but fails to build here, so `cargo test` cannot
//!   pass. The module's own unit tests run again in this build. Nothing else
//!   may be declared or imported at this file's root, or the module could
//!   reach it through `super::`.
//! - The scan below reads the sources for what that build cannot see: doc
//!   links, code behind a `cfg` the test build leaves out, files pulled in by
//!   path, the package's other dependencies, and decimals that bypass the
//!   strict wire form.

// `pub` gives the items the visibility they have in the library, so that the
// lints treat them alike.
#[path = "../src/protocol/mod.rs"]
pub mod standalone;

mod scan {
    use std::collections::BTreeMap;
    use std::path::Path;

    /// The dependencies the protocol may use besides the standard library.
    const ALLOWED_DEPENDENCIES: [&str; 3] = ["rust_decimal", "serde", "thiserror"];

    /// The only file whose code may name `rust_decimal`'s `Decimal`.
    const DECIMAL_FILE: &str = "src/protocol/decimal.rs";

    fn crate_root() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
    }

    /// Every `.rs` file under `src/protocol/`, keyed by its `/`-joined path
    /// from the crate root on every platform.
    fn protocol_sources() -> BTreeMap<String, String> {
        let mut files = BTreeMap::new();
        collect(
            &crate_root().join("src").join("protocol"),
            "src/protocol",
            &mut files,
        );
        files
    }

    fn collect(dir: &Path, relative: &str, files: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(dir).expect("read protocol dir") {
            let entry = entry.expect("dir entry");
            let name = entry.file_name().into_string().expect("utf-8 file name");
            let path = format!("{relative}/{name}");
            if entry.file_type().expect("file type").is_dir() {
                collect(&entry.path(), &path, files);
            } else if name.ends_with(".rs") {
                let text = std::fs::read_to_string(entry.path()).expect("read source file");
                files.insert(path, text);
            }
        }
    }

    fn is_comment(line: &str) -> bool {
        line.trim_start().starts_with("//")
    }

    /// The code lines of a file above its test module, which is its last
    /// item, numbered from 1.
    fn code_before_tests(text: &str) -> Vec<(usize, &str)> {
        text.lines()
            .enumerate()
            .take_while(|(_, line)| line.trim() != "#[cfg(test)]")
            .filter(|(_, line)| !is_comment(line))
            .map(|(index, line)| (index + 1, line))
            .collect()
    }

    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// Whether the `len` bytes of `line` at `at` are a whole word, with no
    /// identifier character on either side: `hex` in `hex::encode`, but not
    /// in `hexdigit`.
    fn whole_word_at(line: &str, at: usize, len: usize) -> bool {
        let before = line[..at].chars().next_back();
        let after = line[at + len..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    }

    fn names_the_crate(line: &str) -> bool {
        line.contains("crate::")
    }

    /// `quantforge` other than as `quantforge::protocol` or a hyphenated name
    /// such as `quantforge-protocol`.
    fn names_quantforge_beyond_the_protocol(line: &str) -> bool {
        line.match_indices("quantforge").any(|(at, word)| {
            let rest = &line[at + word.len()..];
            !(rest.starts_with("::protocol") || rest.starts_with('-'))
        })
    }

    /// A `cfg` other than the test module's, which could hide code from the
    /// standalone build.
    fn gates_code(line: &str) -> bool {
        (line.contains("cfg(") || line.contains("cfg_attr(")) && line.trim() != "#[cfg(test)]"
    }

    fn pulls_in_a_file(line: &str) -> bool {
        ["#[path", "include!", "include_str!", "include_bytes!"]
            .iter()
            .any(|marker| line.contains(marker))
    }

    /// `dependency` as the root of a path, a macro, or a `use`: `hex::`, but
    /// not the `time` of `std::time::`.
    fn uses_dependency(line: &str, dependency: &str) -> bool {
        line.match_indices(dependency).any(|(at, _)| {
            let rest = &line[at + dependency.len()..];
            whole_word_at(line, at, dependency.len())
                && !line[..at].ends_with(':')
                && ["::", "!", ";", " as "]
                    .iter()
                    .any(|tail| rest.starts_with(tail))
        })
    }

    fn names_a_raw_decimal(line: &str) -> bool {
        line.match_indices("Decimal")
            .any(|(at, word)| whole_word_at(line, at, word.len()))
    }

    /// The package's `[dependencies]`, as the names code uses for them.
    fn package_dependencies() -> Vec<String> {
        let manifest =
            std::fs::read_to_string(crate_root().join("Cargo.toml")).expect("read Cargo.toml");
        let mut in_dependencies = false;
        let mut names = Vec::new();
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                in_dependencies = line == "[dependencies]";
            } else if in_dependencies && !line.is_empty() && !line.starts_with('#') {
                if let Some((name, _)) = line.split_once('=') {
                    names.push(name.trim().replace('-', "_"));
                }
            }
        }
        names
    }

    #[test]
    fn the_protocol_names_nothing_else_in_the_crate() {
        let mut found = Vec::new();
        for (path, text) in &protocol_sources() {
            for (index, line) in text.lines().enumerate() {
                let code = !is_comment(line);
                let problem = if names_the_crate(line) {
                    "names the crate; use a path relative to the module"
                } else if names_quantforge_beyond_the_protocol(line) {
                    "names quantforge beyond quantforge::protocol"
                } else if code && gates_code(line) {
                    "gates code with a cfg the standalone build may not see"
                } else if code && pulls_in_a_file(line) {
                    "pulls in another file"
                } else {
                    continue;
                };
                found.push(format!("{path}:{}: {problem}: {}", index + 1, line.trim()));
            }
        }
        assert!(
            found.is_empty(),
            "src/protocol/ must stand alone, so that it can become a crate of its own:\n  {}",
            found.join("\n  ")
        );
    }

    #[test]
    fn the_protocol_uses_no_other_dependency() {
        let forbidden: Vec<String> = package_dependencies()
            .into_iter()
            .filter(|name| !ALLOWED_DEPENDENCIES.contains(&name.as_str()))
            .collect();
        let mut found = Vec::new();
        for (path, text) in &protocol_sources() {
            for (line, code) in code_before_tests(text) {
                for dependency in forbidden.iter().filter(|name| uses_dependency(code, name)) {
                    found.push(format!("{path}:{line}: uses {dependency}: {}", code.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "src/protocol/ may depend on {ALLOWED_DEPENDENCIES:?} only, so that the crate it \
             becomes stays small:\n  {}",
            found.join("\n  ")
        );
    }

    // rust_decimal's own `Deserialize` takes JSON numbers, which a float may
    // already have rounded; the wire form is `ExactDecimal`'s.
    #[test]
    fn every_protocol_decimal_is_an_exact_decimal() {
        let mut found = Vec::new();
        for (path, text) in &protocol_sources() {
            if path == DECIMAL_FILE {
                continue;
            }
            for (line, code) in code_before_tests(text) {
                if names_a_raw_decimal(code) {
                    found.push(format!("{path}:{line}: {}", code.trim()));
                }
            }
        }
        assert!(
            found.is_empty(),
            "these lines use a raw Decimal; use ExactDecimal, which only reads decimal \
             strings:\n  {}",
            found.join("\n  ")
        );
    }

    // Without these, a scan that read nothing, or matchers that could never
    // match, would pass the tests above.
    #[test]
    fn the_scan_reads_the_whole_protocol_and_the_manifest() {
        let files = protocol_sources();
        assert!(files.len() >= 12, "scanned only {files:?}");
        assert!(files.contains_key(DECIMAL_FILE));
        assert!(
            files.values().any(|text| text
                .lines()
                .any(|line| is_comment(line) && line.contains("quantforge::protocol"))),
            "the doctests import the module as quantforge::protocol"
        );
        let dependencies = package_dependencies();
        for name in ALLOWED_DEPENDENCIES.iter().chain(&["tokio", "async_trait"]) {
            assert!(
                dependencies.iter().any(|dependency| dependency == name),
                "{name} missing from {dependencies:?}"
            );
        }
    }

    #[test]
    fn the_matchers_flag_what_they_exist_to_catch() {
        assert!(names_the_crate("use crate::model::Candle;"));
        assert!(names_the_crate("/// See [`crate::Candle`]."));
        assert!(!names_the_crate("pub(crate) fn helper() {}"));

        assert!(names_quantforge_beyond_the_protocol(
            "/// use quantforge::Candle;"
        ));
        assert!(names_quantforge_beyond_the_protocol(
            "/// use quantforge as qf;"
        ));
        assert!(!names_quantforge_beyond_the_protocol(
            "/// use quantforge::protocol::stream::Hello;"
        ));
        assert!(!names_quantforge_beyond_the_protocol(
            "//! become the `quantforge-protocol` crate"
        ));

        assert!(gates_code("#[cfg(not(test))]"));
        assert!(gates_code("#[cfg(doc)] use crate::Candle;"));
        assert!(gates_code(r#"#[cfg_attr(test, path = "other.rs")]"#));
        assert!(!gates_code("#[cfg(test)]"));

        assert!(pulls_in_a_file(r#"#[path = "other.rs"]"#));
        assert!(pulls_in_a_file(
            r#"const FIXTURE: &str = include_str!("x.json");"#
        ));

        assert!(uses_dependency("        hex::encode(digest)", "hex"));
        assert!(uses_dependency("use tokio::sync::Mutex;", "tokio"));
        assert!(uses_dependency("use uuid;", "uuid"));
        assert!(uses_dependency("        tracing::warn!(\"x\");", "tracing"));
        assert!(!uses_dependency("    let hexadecimal = 1;", "hex"));
        assert!(!uses_dependency("fn hex_digit(byte: u8) {}", "hex"));
        assert!(!uses_dependency("use std::time::Duration;", "time"));

        assert!(names_a_raw_decimal("    pub open: Decimal,"));
        assert!(names_a_raw_decimal("use rust_decimal::Decimal;"));
        assert!(!names_a_raw_decimal("    pub open: ExactDecimal,"));
        assert!(!names_a_raw_decimal("    pub decimals: u32,"));
    }
}
