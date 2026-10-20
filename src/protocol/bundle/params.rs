//! A strategy's parameters, as its manifest gives them and JSON carries
//! them.

use serde::Serialize;
use serde_json::{Map, Number, Value};

use super::problems::{BundleProblem, ProblemCode};

/// The largest integer a param may hold, so that every JSON reader,
/// JavaScript's included, reads it exactly.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// A strategy's parameters: the manifest's `[params]` table, as JSON.
///
/// A value is a string, a boolean, an integer within ±(2^53 − 1), a finite
/// float, an array, or a table of them. A float is an IEEE 754 binary64, as
/// Python reads it; nothing in QuantForge computes with it. An amount that
/// must stay exact belongs in a string. TOML dates and times are refused,
/// since JSON has none, and so are keys starting with `$`.
///
/// It serializes as the JSON object, and is built only from a checked
/// manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Params(Map<String, Value>);

impl Params {
    /// The parameters, by name.
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.0
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The `[params]` table as JSON, with a `param_not_json` problem for every
/// value JSON cannot hold.
pub(super) fn from_toml(table: &toml::Table) -> (Params, Vec<BundleProblem>) {
    let mut problems = Vec::new();
    let map = convert_table(table, "params", &mut problems);
    (Params(map), problems)
}

fn convert_table(
    table: &toml::Table,
    at: &str,
    problems: &mut Vec<BundleProblem>,
) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in table {
        let path = format!("{at}.{}", shown_key(key));
        if key.starts_with('$') {
            problems.push(not_json(&path, "starts with $, which is reserved"));
            continue;
        }
        if let Some(value) = convert(value, &path, problems) {
            map.insert(key.clone(), value);
        }
    }
    map
}

fn convert(value: &toml::Value, at: &str, problems: &mut Vec<BundleProblem>) -> Option<Value> {
    Some(match value {
        toml::Value::String(text) => Value::String(text.clone()),
        toml::Value::Boolean(flag) => Value::Bool(*flag),
        toml::Value::Integer(integer)
            if integer.unsigned_abs() <= MAX_SAFE_INTEGER.unsigned_abs() =>
        {
            Value::Number(Number::from(*integer))
        }
        toml::Value::Integer(_) => {
            problems.push(not_json(
                at,
                "is an integer beyond 2^53 - 1, which not every JSON reader holds exactly; write \
                 it as a string",
            ));
            return None;
        }
        toml::Value::Float(float) => match Number::from_f64(*float) {
            Some(number) => Value::Number(number),
            None => {
                problems.push(not_json(at, "is not a finite number"));
                return None;
            }
        },
        toml::Value::Datetime(_) => {
            problems.push(not_json(
                at,
                "is a TOML date or time, which JSON cannot hold; write it as a string",
            ));
            return None;
        }
        toml::Value::Array(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| convert(item, &format!("{at}[{index}]"), problems))
                .collect(),
        ),
        toml::Value::Table(table) => Value::Object(convert_table(table, at, problems)),
    })
}

/// A key as TOML would write it in a dotted path: bare when it can be.
pub(super) fn shown_key(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    if bare {
        key.to_string()
    } else {
        format!("{key:?}")
    }
}

fn not_json(at: &str, reason: &str) -> BundleProblem {
    BundleProblem::new(ProblemCode::ParamNotJson, format!("{at} {reason}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(toml: &str) -> (Params, Vec<String>) {
        let table: toml::Table = toml.parse().expect("toml");
        let (params, problems) = from_toml(&table);
        (
            params,
            problems.iter().map(|p| p.message().to_string()).collect(),
        )
    }

    #[test]
    fn json_compatible_values_become_their_json() {
        let (params, problems) = params(
            r#"
            fast = 20
            threshold = 0.25
            name = "sma"
            enabled = true
            levels = [1, 2.5, "x"]
            window = { size = 5, kind = "ema" }
            largest = 9007199254740991
            smallest = -9007199254740991
            "#,
        );
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            serde_json::to_string(&params).expect("serialize"),
            r#"{"enabled":true,"fast":20,"largest":9007199254740991,"levels":[1,2.5,"x"],"name":"sma","smallest":-9007199254740991,"threshold":0.25,"window":{"kind":"ema","size":5}}"#
        );
    }

    #[test]
    fn values_json_cannot_hold_are_named_by_their_path() {
        let (params, problems) = params(
            r#"
            start = 2024-01-01
            at = 07:30:00
            big = 9007199254740992
            "$ref" = 1
            nested = { when = 1979-05-27T07:32:00Z }
            list = [1, 1979-05-27]
            "odd key" = 2026-01-01
            fine = 1
            "#,
        );
        assert_eq!(
            problems,
            [
                "params.\"$ref\" starts with $, which is reserved",
                "params.at is a TOML date or time, which JSON cannot hold; write it as a string",
                "params.big is an integer beyond 2^53 - 1, which not every JSON reader holds \
                 exactly; write it as a string",
                "params.list[1] is a TOML date or time, which JSON cannot hold; write it as a string",
                "params.nested.when is a TOML date or time, which JSON cannot hold; write it as a \
                 string",
                "params.\"odd key\" is a TOML date or time, which JSON cannot hold; write it as a \
                 string",
                "params.start is a TOML date or time, which JSON cannot hold; write it as a string",
            ]
        );
        assert_eq!(params.as_map().len(), 3);
        assert!(params.as_map().contains_key("fine"));
    }
}
