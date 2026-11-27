//! The manifest a client writes for a strategy whose files hold none: one
//! text for the same strategy, whichever client wrote it.

use std::collections::BTreeMap;

use super::BUNDLE_FORMAT;

/// Writes the manifest of a strategy whose files hold none, in its one
/// canonical form: the keys in the spec's order, the strategy's parameters
/// and then its packages, each sorted by name. The same entry, parameters
/// and packages make the same text, and so, beside the same files, the same
/// bundle and the same code hash, whichever client wrote it and on whatever
/// machine.
///
/// - `entry`, `engine` and `python` are written as TOML strings, as they
///   are given.
/// - `params` is the `[params]` table: a string as a basic string, a float
///   with its point or its exponent, so that it reads back as a float, and
///   an array or a table inline.
/// - `deps` is the `[deps]` table: each package with the version it pins.
///
/// Nothing is checked here. A value no manifest holds, such as a date or an
/// infinity, is written as TOML has it, for the bundle's own check to
/// refuse in its own words.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use quantforge::protocol::bundle::written_manifest;
///
/// let params: toml::Table = "slow = 50\nfast = 20\nlevels = [1, 2.5]".parse()?;
/// let deps = BTreeMap::from([("numpy".to_string(), "2.1.3".to_string())]);
/// let text = written_manifest("strategy.py:SmaCross", "==0.3.0", "3.12", &params, &deps);
/// assert_eq!(
///     text,
///     "format = 1\nentry = \"strategy.py:SmaCross\"\nengine = \"==0.3.0\"\npython = \"3.12\"\n\n\
///      [params]\nfast = 20\nlevels = [1, 2.5]\nslow = 50\n\n[deps]\nnumpy = \"2.1.3\"\n"
/// );
/// # Ok::<(), toml::de::Error>(())
/// ```
pub fn written_manifest(
    entry: &str,
    engine: &str,
    python: &str,
    params: &toml::Table,
    deps: &BTreeMap<String, String>,
) -> String {
    let mut text = format!(
        "format = {BUNDLE_FORMAT}\nentry = {}\nengine = {}\npython = {}\n",
        string(entry),
        string(engine),
        string(python)
    );
    if !params.is_empty() {
        text.push_str("\n[params]\n");
        for (key, value) in params {
            text.push_str(&format!("{} = {}\n", key_text(key), value_text(value)));
        }
    }
    if !deps.is_empty() {
        text.push_str("\n[deps]\n");
        for (name, version) in deps {
            text.push_str(&format!("{} = {}\n", key_text(name), string(version)));
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
    use super::super::{Bundle, BundleLimits};
    use super::*;

    fn table(text: &str) -> toml::Table {
        text.parse().expect("a table")
    }

    fn pins(deps: &[(&str, &str)]) -> BTreeMap<String, String> {
        deps.iter()
            .map(|&(name, version)| (name.to_string(), version.to_string()))
            .collect()
    }

    #[test]
    fn a_manifest_without_params_or_deps_is_its_four_keys() {
        let text = written_manifest("sma.py:Sma", "==0.3.0", "3.12", &table(""), &pins(&[]));
        assert_eq!(
            text,
            "format = 1\nentry = \"sma.py:Sma\"\nengine = \"==0.3.0\"\npython = \"3.12\"\n"
        );
    }

    #[test]
    fn params_and_deps_are_written_sorted_by_name_whatever_order_they_came_in() {
        let one = written_manifest(
            "sma.py:Sma",
            "==0.3.0",
            "3.12",
            &table("slow = 50\nfast = 20"),
            &pins(&[("pandas", "2.2.3"), ("numpy", "2.1.3")]),
        );
        let two = written_manifest(
            "sma.py:Sma",
            "==0.3.0",
            "3.12",
            &table("fast = 20\nslow = 50"),
            &pins(&[("numpy", "2.1.3"), ("pandas", "2.2.3")]),
        );
        assert_eq!(one, two);
        assert_eq!(
            one,
            "format = 1\nentry = \"sma.py:Sma\"\nengine = \"==0.3.0\"\npython = \"3.12\"\n\n\
             [params]\nfast = 20\nslow = 50\n\n[deps]\nnumpy = \"2.1.3\"\npandas = \"2.2.3\"\n"
        );
    }

    #[test]
    fn deps_alone_are_written_without_a_params_table() {
        let text = written_manifest(
            "sma.py:Sma",
            "==0.3.0",
            "3.12",
            &table(""),
            &pins(&[("scikit-learn", "1.5.0")]),
        );
        assert!(!text.contains("[params]"), "{text}");
        assert!(
            text.ends_with("python = \"3.12\"\n\n[deps]\nscikit-learn = \"1.5.0\"\n"),
            "{text}"
        );
    }

    #[test]
    fn what_is_written_reads_back_as_the_bundle_it_was_written_for() {
        let params = r#"n = -7
ratio = 0.1
big = 1e20
tiny = 1e-7
whole = 2.0
on = true
name = "a \"q\" \\ é	tab"
levels = [1, 2.5, "x"]
window = { size = 5, kind = "ema" }
"odd key" = 1
empty = {}"#;
        let text = written_manifest(
            "pkg/sma.py:Sma",
            ">=0.3.0, <0.4.0",
            "3.12",
            &table(params),
            &pins(&[("numpy", "2.1.3"), ("pandas", "2.2.3")]),
        );
        let bundle = Bundle::from_files(
            [
                ("pkg/sma.py", "class Sma:\n    pass\n"),
                ("quantforge.toml", text.as_str()),
            ],
            &BundleLimits::default(),
        )
        .expect("a bundle");
        let manifest = bundle.manifest();
        assert_eq!(manifest.entry().to_string(), "pkg/sma.py:Sma");
        assert_eq!(manifest.engine().to_string(), ">=0.3.0, <0.4.0");
        assert_eq!(manifest.python().to_string(), "3.12");
        assert_eq!(
            serde_json::to_value(manifest.params()).expect("json"),
            serde_json::json!({
                "n": -7, "ratio": 0.1, "big": 1e20, "tiny": 1e-7, "whole": 2.0, "on": true,
                "name": "a \"q\" \\ é\ttab", "levels": [1, 2.5, "x"],
                "window": {"size": 5, "kind": "ema"}, "odd key": 1, "empty": {}
            })
        );
        let deps: Vec<(&str, &str)> = manifest
            .deps()
            .iter()
            .map(|(name, version)| (name.as_str(), version.as_str()))
            .collect();
        assert_eq!(deps, [("numpy", "2.1.3"), ("pandas", "2.2.3")]);
    }

    #[test]
    fn a_control_character_and_a_quote_are_escaped_wherever_text_is_written() {
        let text = written_manifest(
            "sma.py:Sma",
            "==0.3.0",
            "3.12",
            &table("\"a\\u0001\\\"b\" = \"line\\nnext\\u007F\\u0085\""),
            &pins(&[("odd name", "1\"0")]),
        );
        assert!(
            text.contains("\"a\\u0001\\\"b\" = \"line\\nnext\\u007F\\u0085\"\n"),
            "{text}"
        );
        assert!(text.contains("\"odd name\" = \"1\\\"0\"\n"), "{text}");
    }

    #[test]
    fn a_value_no_manifest_holds_is_written_for_the_check_to_refuse() {
        let text = written_manifest(
            "sma.py:Sma",
            "==0.3.0",
            "3.12",
            &table("x = inf\ny = -inf\nz = nan\nat = 1979-05-27"),
            &pins(&[]),
        );
        assert!(
            text.ends_with("[params]\nat = 1979-05-27\nx = inf\ny = -inf\nz = nan\n"),
            "{text}"
        );
    }
}
