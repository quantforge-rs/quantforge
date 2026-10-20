//! The fixture files: one transcript per exchange and check, at
//! `tests/conformance/fixtures/<exchange>/<check>.json`, recorded from a
//! testnet run or, for the faults no testnet produces, written by hand.

use quantforge::ExchangeId;
use quantforge::exchange;
use quantforge::exchange::conformance::Check;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::profiles::Profile;
use crate::server::{Answer, Captured};

/// What a recording keeps of a volatile query parameter's value.
pub(crate) const VOLATILE: &str = "*";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Fixture {
    pub(crate) check: String,
    pub(crate) exchange: String,
    pub(crate) source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recorded_on: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) recorded_from: Option<String>,
    /// Where a hand-written transcript's responses come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
    pub(crate) interactions: Vec<Interaction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    Recorded,
    Authored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Interaction {
    pub(crate) request: Request,
    pub(crate) response: Response,
}

/// A request as a transcript keeps it. A recorded one is matched whole; a
/// hand-written one may leave out everything after its path, and is then
/// matched on its method and path alone.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) path: String,
    /// The query's parameters in order, each written `name=value`, a
    /// volatile value kept as [`VOLATILE`].
    #[serde(default, skip_serializing_if = "Option::is_none", with = "pairs")]
    pub(crate) query: Option<Vec<(String, String)>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) body: Option<String>,
    /// Whether the request carried credentials; never what they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) credentials: Option<bool>,
}

/// A response: its status, and its body as JSON kept verbatim, or as text
/// when it is not JSON.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Response {
    pub(crate) status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) json: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
}

/// A query's parameters as `name=value` strings, one line each in a
/// fixture file.
mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(
        pairs: &Option<Vec<(String, String)>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        pairs
            .as_ref()
            .map(|pairs| {
                pairs
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<_>>()
            })
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<(String, String)>>, D::Error> {
        let Some(lines) = Option::<Vec<String>>::deserialize(deserializer)? else {
            return Ok(None);
        };
        lines
            .into_iter()
            .map(|line| match line.split_once('=') {
                Some((name, value)) => Ok((name.to_string(), value.to_string())),
                None => Err(serde::de::Error::custom(format!(
                    "query parameter {line:?} is not name=value"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }
}

impl Response {
    pub(crate) fn recorded(status: u16, body: String) -> Self {
        match serde_json::from_str::<Box<RawValue>>(&body) {
            Ok(json) => Self {
                status,
                json: Some(json),
                text: None,
            },
            Err(_) => Self {
                status,
                json: None,
                text: Some(body),
            },
        }
    }

    pub(crate) fn body(&self) -> &str {
        match (&self.json, &self.text) {
            (Some(json), _) => json.get(),
            (None, Some(text)) => text,
            (None, None) => "",
        }
    }

    pub(crate) fn answer(&self) -> Answer {
        Answer {
            status: self.status,
            body: self.body().to_string(),
        }
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.method, self.path)?;
        if let Some(query) = self.query.as_ref().filter(|query| !query.is_empty()) {
            let pairs: Vec<String> = query
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            write!(f, "?{}", pairs.join("&"))?;
        }
        if let Some(body) = &self.body {
            write!(f, " with body {body:?}")?;
        }
        if self.credentials == Some(true) {
            f.write_str(" with credentials")?;
        }
        Ok(())
    }
}

/// `request` as a transcript keeps it under `profile`: the query decoded,
/// volatile values replaced, and of the credentials only whether any were
/// sent.
pub(crate) fn observe(request: &Captured, profile: &Profile) -> Request {
    let query = request
        .query
        .as_deref()
        .map(|raw| {
            url::form_urlencoded::parse(raw.as_bytes())
                .map(|(name, value)| {
                    let value = if profile.volatile_params.contains(&name.as_ref()) {
                        VOLATILE.to_string()
                    } else {
                        value.into_owned()
                    };
                    (name.into_owned(), value)
                })
                .collect()
        })
        .unwrap_or_default();
    Request {
        method: request.method.clone(),
        path: request.path.clone(),
        query: Some(query),
        body: (!request.body.is_empty())
            .then(|| String::from_utf8_lossy(&request.body).into_owned()),
        credentials: Some(
            profile
                .credential_headers
                .iter()
                .any(|name| request.headers.contains_key(*name)),
        ),
    }
}

/// Whether `seen` is the request `expected` describes.
pub(crate) fn matches(expected: &Request, seen: &Request) -> bool {
    if expected.method != seen.method || expected.path != seen.path {
        return false;
    }
    // A hand-written request without a query is matched on its method and
    // path alone.
    expected.query.is_none() || expected == seen
}

/// `body` with the value of every `"field":` blanked, a number to `0` and
/// a string to `""`. Text is substituted in place, so everything else
/// stays byte for byte.
pub(crate) fn redact(body: &str, fields: &[&str]) -> String {
    fields
        .iter()
        .fold(body.to_string(), |body, field| redact_field(&body, field))
}

fn redact_field(body: &str, field: &str) -> String {
    let key = format!("\"{field}\"");
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find(&key) {
        let (head, tail) = rest.split_at(at + key.len());
        out.push_str(head);
        let Some(value) = tail.trim_start().strip_prefix(':') else {
            // The name as a value, not as a key.
            rest = tail;
            continue;
        };
        let value = value.trim_start();
        out.push_str(&tail[..tail.len() - value.len()]);
        if let Some(string) = value.strip_prefix('"') {
            let end = closing_quote(string).unwrap_or(string.len());
            out.push_str("\"\"");
            rest = string.get(end + 1..).unwrap_or("");
        } else {
            let end = value
                .find(|c: char| !(c.is_ascii_digit() || c == '-'))
                .unwrap_or(value.len());
            // Anything but a number or a string is left as it is.
            if end > 0 {
                out.push('0');
            }
            rest = &value[end..];
        }
    }
    out.push_str(rest);
    out
}

/// The byte index of the quote that closes a JSON string whose content
/// starts `string`.
fn closing_quote(string: &str) -> Option<usize> {
    let mut escaped = false;
    for (index, c) in string.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some(index),
            _ => escaped = false,
        }
    }
    None
}

pub(crate) fn dir(exchange: ExchangeId) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/conformance/fixtures")
        .join(exchange::name(exchange))
}

/// Every fixture file of `exchange`, by file name without `.json`.
pub(crate) fn load_all(exchange: ExchangeId) -> BTreeMap<String, Fixture> {
    let dir = dir(exchange);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return BTreeMap::new();
    };
    let mut fixtures = BTreeMap::new();
    for entry in entries {
        let path = entry.expect("fixture entry").path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        let text = std::fs::read_to_string(&path).expect("read fixture");
        let fixture: Fixture = serde_json::from_str(&text)
            .unwrap_or_else(|err| panic!("{}: not a fixture: {err}", path.display()));
        assert_eq!(fixture.check, id, "{}: names another check", path.display());
        assert_eq!(
            fixture.exchange,
            exchange::name(exchange),
            "{}: names another exchange",
            path.display()
        );
        for (index, interaction) in fixture.interactions.iter().enumerate() {
            let response = &interaction.response;
            assert!(
                response.json.is_some() != response.text.is_some(),
                "{}: response {} needs exactly one of json and text",
                path.display(),
                index + 1
            );
        }
        fixtures.insert(id.to_string(), fixture);
    }
    fixtures
}

pub(crate) fn save(exchange: ExchangeId, check: Check, fixture: &Fixture) {
    let dir = dir(exchange);
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let text = serde_json::to_string_pretty(fixture).expect("fixture json");
    std::fs::write(dir.join(format!("{}.json", check.id())), text + "\n").expect("write fixture");
}

#[test]
fn redaction_blanks_the_named_fields_and_keeps_every_other_byte() {
    let body = r#"{"makerCommission":0,"uid":354937868,"accountType":"SPOT","note":"uid","nested":{"uid" : -7},"name":"uid","owner":{"uid":"a\"b"}}"#;
    assert_eq!(
        redact(body, &["uid"]),
        r#"{"makerCommission":0,"uid":0,"accountType":"SPOT","note":"uid","nested":{"uid" : 0},"name":"uid","owner":{"uid":""}}"#
    );
    assert_eq!(redact(body, &[]), body);
    assert_eq!(redact("[]", &["uid"]), "[]");
}
