//! A Python strategy as `--strategy`, `--entry` and `--params` name it,
//! made into a bundle by the bundle spec: a directory with a
//! `quantforge.toml` of its own as it is, or a `.py` file or a directory
//! without one, beside the manifest written for it here. Everything is
//! read and checked before any server is asked, and the manifest takes its
//! Python version from the server's runtime once it is known.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use quantforge::ENGINE_VERSION;
use quantforge::protocol::bundle::{Bundle, BundleLimits, Entry, MANIFEST_FILE};

/// The Python version a written manifest is checked with before the
/// server's is known: any does, since only admission reads it.
const PROVISIONAL_PYTHON: &str = "3.11";

/// A strategy read from disk, ready to be bundled for a runtime.
#[derive(Debug)]
pub(super) enum Draft {
    /// One Python file, at the bundle's root under its own name, beside the
    /// manifest written for it.
    File {
        name: String,
        bytes: Vec<u8>,
        written: Written,
    },
    /// A directory, collected as the bundle spec says, beside the manifest
    /// written for it, or with its own when `written` is `None`.
    Dir {
        root: PathBuf,
        written: Option<Written>,
    },
}

/// Where a strategy is read from, before its manifest is settled.
enum Source {
    File { name: String, bytes: Vec<u8> },
    Dir(PathBuf),
}

#[derive(Debug)]
pub(super) struct Written {
    entry: Entry,
    params: toml::Table,
}

impl Draft {
    /// Reads `--strategy`, with `--entry` and `--params`, and checks the
    /// bundle they make against the spec.
    pub(super) fn read(path: &Path, entry: Option<&str>, params: Option<&str>) -> Result<Self> {
        let shown = path.display();
        let metadata =
            fs::metadata(path).with_context(|| format!("cannot read --strategy {shown}"))?;
        let (source, own_manifest) = if metadata.is_dir() {
            let own = fs::symlink_metadata(path.join(MANIFEST_FILE)).is_ok();
            (Source::Dir(path.to_path_buf()), own)
        } else {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .with_context(|| format!("--strategy {shown} has no file name a bundle can hold"))?
                .to_string();
            let bytes =
                fs::read(path).with_context(|| format!("cannot read --strategy {shown}"))?;
            (Source::File { name, bytes }, false)
        };
        let written = if own_manifest {
            if entry.is_some() || params.is_some() {
                bail!(
                    "{} names the strategy's entry and params; leave out --entry and --params, \
                     or remove the file to have it written from them",
                    path.join(MANIFEST_FILE).display()
                );
            }
            None
        } else {
            let example = match &source {
                Source::File { name, .. } => format!("{name}:MyStrategy"),
                Source::Dir(_) => "strategy.py:MyStrategy".to_string(),
            };
            let Some(entry) = entry else {
                bail!(
                    "--entry must name the strategy, as in --entry {example}: {shown} holds no \
                     {MANIFEST_FILE} that names it"
                );
            };
            let entry: Entry = entry
                .parse()
                .map_err(|error| anyhow::anyhow!("--entry: {error}"))?;
            let params = params.map(parse_params).transpose()?.unwrap_or_default();
            Some(Written { entry, params })
        };
        let draft = match (source, written) {
            (Source::File { name, bytes }, Some(written)) => Self::File {
                name,
                bytes,
                written,
            },
            (Source::Dir(root), written) => Self::Dir { root, written },
            (Source::File { .. }, None) => bail!("--strategy {shown} has no manifest to write"),
        };
        draft.bundle(PROVISIONAL_PYTHON)?;
        Ok(draft)
    }

    /// The bundle, a written manifest naming `python`, which must be the
    /// server's.
    pub(super) fn bundle(&self, python: &str) -> Result<Bundle> {
        let limits = BundleLimits::default();
        let bundle = match self {
            Self::File {
                name,
                bytes,
                written,
            } => Bundle::from_files(
                [
                    (name.clone(), bytes.clone()),
                    (
                        MANIFEST_FILE.to_string(),
                        manifest(written, python).into_bytes(),
                    ),
                ],
                &limits,
            ),
            Self::Dir {
                root,
                written: Some(written),
            } => Bundle::from_dir_with(root, [(MANIFEST_FILE, manifest(written, python))], &limits),
            Self::Dir {
                root,
                written: None,
            } => Bundle::from_dir(root, &limits),
        };
        Ok(bundle?)
    }

    /// What a report names the strategy by: its bundle's entry.
    pub(super) fn label(&self) -> Result<String> {
        Ok(match self {
            Self::File { written, .. }
            | Self::Dir {
                written: Some(written),
                ..
            } => written.entry.to_string(),
            Self::Dir { written: None, .. } => self
                .bundle(PROVISIONAL_PYTHON)?
                .manifest()
                .entry()
                .to_string(),
        })
    }
}

/// `--params`: the body of a TOML inline table, such as `fast=20,slow=50`
/// or `name="sma", levels=[1, 2]`, read as TOML 1.0 reads one.
pub(super) fn parse_params(text: &str) -> Result<toml::Table> {
    let refuse = |reason: String| {
        anyhow::anyhow!(
            "--params must be TOML key = value pairs separated by commas, as in fast=20,slow=50 \
             or name=\"sma\": {reason}"
        )
    };
    let document: toml::Table = format!("params = {{{text}}}")
        .parse()
        .map_err(|error: toml::de::Error| refuse(error.message().trim().replace('\n', "; ")))?;
    match (document.len(), document.get("params")) {
        (1, Some(toml::Value::Table(params))) => Ok(params.clone()),
        _ => Err(refuse("it closes the table it belongs in".to_string())),
    }
}

/// The manifest written for `written`, in one canonical form, so that the
/// same flags make the same bundle: its keys in the spec's order, the
/// params sorted by name. Its engine is this build's own version, so a
/// server runs the engine a local run would.
fn manifest(written: &Written, python: &str) -> String {
    let mut text = format!(
        "format = 1\nentry = {}\nengine = {}\npython = {}\n",
        string(&written.entry.to_string()),
        string(&format!("=={ENGINE_VERSION}")),
        string(python)
    );
    if !written.params.is_empty() {
        text.push_str("\n[params]\n");
        for (key, value) in &written.params {
            text.push_str(&format!("{} = {}\n", key_text(key), value_text(value)));
        }
    }
    text
}

/// A TOML basic string.
fn string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// A key, bare where TOML allows.
fn key_text(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if bare { key.to_string() } else { string(key) }
}

/// A value as TOML writes it. A float keeps its point or exponent, so it
/// reads back as a float; a value no manifest holds is written as TOML has
/// it, for the bundle check to refuse in its own words.
fn value_text(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => string(text),
        toml::Value::Integer(number) => number.to_string(),
        toml::Value::Float(number) if number.is_nan() => "nan".to_string(),
        toml::Value::Float(number) if number.is_infinite() => {
            if *number > 0.0 { "inf" } else { "-inf" }.to_string()
        }
        toml::Value::Float(number) => format!("{number:?}"),
        toml::Value::Boolean(flag) => flag.to_string(),
        toml::Value::Datetime(moment) => moment.to_string(),
        toml::Value::Array(items) => format!(
            "[{}]",
            items.iter().map(value_text).collect::<Vec<_>>().join(", ")
        ),
        toml::Value::Table(table) if table.is_empty() => "{}".to_string(),
        toml::Value::Table(table) => format!(
            "{{ {} }}",
            table
                .iter()
                .map(|(key, value)| format!("{} = {}", key_text(key), value_text(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn written(entry: &str, params: &str) -> Written {
        Written {
            entry: entry.parse().expect("an entry"),
            params: parse_params(params).expect("params"),
        }
    }

    #[test]
    fn the_same_params_in_any_spelling_write_the_same_manifest() {
        let one = manifest(&written("sma.py:Sma", "fast=20,slow=50"), "3.11");
        let two = manifest(&written("sma.py:Sma", " slow = 50 , fast=20 "), "3.11");
        assert_eq!(one, two);
        assert_eq!(
            one,
            format!(
                "format = 1\nentry = \"sma.py:Sma\"\nengine = \"=={ENGINE_VERSION}\"\npython = \
                 \"3.11\"\n\n[params]\nfast = 20\nslow = 50\n"
            )
        );
        assert!(!manifest(&written("sma.py:Sma", ""), "3.11").contains("[params]"));
    }

    #[test]
    fn every_value_a_manifest_holds_reads_back_as_the_bundle_takes_it() {
        let params = r#"n=-7, ratio=0.1, big=1e20, tiny=1e-7, whole=2.0, on=true, name="a \"q\" \\ é	tab", levels=[1, 2.5, "x"], window={size=5, kind="ema"}, "odd key"=1, empty={}"#;
        let text = manifest(&written("sma.py:Sma", params), "3.11");
        let bundle = Bundle::from_files(
            [
                ("sma.py", "class Sma:\n    pass\n"),
                ("quantforge.toml", &text),
            ],
            &BundleLimits::default(),
        )
        .expect("a bundle");
        let read = serde_json::to_value(bundle.manifest().params()).expect("json");
        assert_eq!(
            read,
            json!({
                "n": -7, "ratio": 0.1, "big": 1e20, "tiny": 1e-7, "whole": 2.0, "on": true,
                "name": "a \"q\" \\ é\ttab", "levels": [1, 2.5, "x"],
                "window": {"size": 5, "kind": "ema"}, "odd key": 1, "empty": {}
            })
        );
    }

    #[test]
    fn params_that_are_no_table_are_refused_saying_how_to_write_them() {
        for (text, reason) in [
            ("fast", "fast=20,slow=50"),
            ("fast=20,", "fast=20,slow=50"),
            ("a=1}\nb={", "it closes the table it belongs in"),
            ("name=sma", "fast=20,slow=50"),
            ("a=1,a=2", "fast=20,slow=50"),
        ] {
            let error = parse_params(text).expect_err("refused");
            assert!(error.to_string().contains(reason), "{text}: {error}");
        }
    }

    #[test]
    fn a_value_no_manifest_holds_is_refused_by_the_bundle_check() {
        let root = tempfile::tempdir().expect("a directory");
        let file = root.path().join("sma.py");
        fs::write(&file, "class Sma:\n    pass\n").expect("a file");
        let error = Draft::read(&file, Some("sma.py:Sma"), Some("x=inf")).expect_err("refused");
        assert!(
            error
                .to_string()
                .contains("a manifest's numbers are finite"),
            "{error}"
        );
        let error =
            Draft::read(&file, Some("sma.py:Sma"), Some("at=1979-05-27")).expect_err("refused");
        assert!(error.to_string().contains("TOML date or time"), "{error}");
    }

    #[test]
    fn a_directory_with_a_manifest_is_bundled_as_it_is_and_takes_no_flags() {
        let root = tempfile::tempdir().expect("a directory");
        fs::write(root.path().join("sma.py"), "class Sma:\n    pass\n").expect("a file");
        let own = "format = 1\nentry = \"sma.py:Sma\"\nengine = \">=0.1.0\"\npython = \"3.12\"\n";
        fs::write(root.path().join(MANIFEST_FILE), own).expect("a manifest");
        let draft = Draft::read(root.path(), None, None).expect("a draft");
        assert_eq!(draft.label().expect("a label"), "sma.py:Sma");
        let bundle = draft.bundle("3.99").expect("a bundle");
        assert_eq!(bundle.manifest().python().to_string(), "3.12");
        let error =
            Draft::read(root.path(), Some("sma.py:Sma"), None).expect_err("an entry beside it");
        assert!(
            error.to_string().contains("leave out --entry and --params"),
            "{error}"
        );
    }

    #[test]
    fn a_strategy_without_a_manifest_needs_an_entry_that_names_its_file() {
        let root = tempfile::tempdir().expect("a directory");
        let file = root.path().join("sma.py");
        fs::write(&file, "class Sma:\n    pass\n").expect("a file");
        let error = Draft::read(&file, None, None).expect_err("no entry");
        assert!(
            error
                .to_string()
                .starts_with("--entry must name the strategy, as in --entry sma.py:MyStrategy"),
            "{error}"
        );
        let error = Draft::read(&file, Some("other.py:Sma"), None).expect_err("another file");
        assert!(error.to_string().contains("other.py"), "{error}");
        let error = Draft::read(&file, Some("sma:Sma"), None).expect_err("no file");
        assert!(
            error.to_string().starts_with("--entry: invalid entry"),
            "{error}"
        );
        let draft = Draft::read(&file, Some("sma.py:Sma"), Some("fast=2")).expect("a draft");
        let bundle = draft.bundle("3.12").expect("a bundle");
        assert_eq!(bundle.manifest().python().to_string(), "3.12");
        assert_eq!(
            bundle.manifest().engine().to_string(),
            format!("=={ENGINE_VERSION}")
        );
    }
}
