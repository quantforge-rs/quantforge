//! Reading a body down to its members: its size, its encoding, how deep it
//! nests, and its JSON. Members are handed on as they were written, each
//! one that was written twice included, so that the format's own rules
//! decide what they mean.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};

use super::problems::{SignalProblem, SignalProblemCode};
use super::{MAX_SIGNAL_BYTES, MAX_SIGNAL_DEPTH};

/// One member's value, as far as the format tells values apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Raw {
    /// A JSON string.
    Text(String),
    /// A JSON number written as a whole number, without a sign, a fraction
    /// or an exponent, that 64 bits hold.
    Whole(u64),
    /// Anything else, by what to call it.
    Other(&'static str),
}

impl Raw {
    /// What the value is, for a message.
    pub(super) fn described(&self) -> &'static str {
        match self {
            Self::Text(_) => "a string",
            Self::Whole(_) => "a number",
            Self::Other(what) => what,
        }
    }
}

struct RawVisitor;

impl<'de> Visitor<'de> for RawVisitor {
    type Value = Raw;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Raw, E> {
        Ok(Raw::Text(text.to_string()))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Raw, E> {
        Ok(Raw::Whole(value))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Raw, E> {
        Ok(Raw::Other("a number"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Raw, E> {
        Ok(Raw::Other("a number"))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Raw, E> {
        Ok(Raw::Other("a boolean"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Raw, E> {
        Ok(Raw::Other("null"))
    }

    // A nested value is read to its end, so that what it holds is checked
    // as JSON, and then counts only as what it is.
    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Raw, A::Error> {
        while items.next_element::<Raw>()?.is_some() {}
        Ok(Raw::Other("a list"))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Raw, A::Error> {
        while members.next_entry::<String, Raw>()?.is_some() {}
        Ok(Raw::Other("an object"))
    }
}

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RawVisitor)
    }
}

/// A whole body: an object's members in the order they were written, or
/// what else it is.
enum Top {
    Object(Vec<(String, Raw)>),
    Other(&'static str),
}

struct TopVisitor;

impl<'de> Visitor<'de> for TopVisitor {
    type Value = Top;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut members: A) -> Result<Top, A::Error> {
        let mut read = Vec::new();
        while let Some(member) = members.next_entry::<String, Raw>()? {
            read.push(member);
        }
        Ok(Top::Object(read))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, items: A) -> Result<Top, A::Error> {
        RawVisitor.visit_seq(items).map(|_| Top::Other("a list"))
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<Top, E> {
        Ok(Top::Other("a string"))
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Top, E> {
        Ok(Top::Other("a number"))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Top, E> {
        Ok(Top::Other("a number"))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Top, E> {
        Ok(Top::Other("a number"))
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Top, E> {
        Ok(Top::Other("a boolean"))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Top, E> {
        Ok(Top::Other("null"))
    }
}

impl<'de> Deserialize<'de> for Top {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TopVisitor)
    }
}

/// Whether the text opens more lists and objects, one inside the other,
/// than [`MAX_SIGNAL_DEPTH`]. Brackets inside strings do not count.
fn too_deep(text: &str) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'[' | b'{' => {
                    depth += 1;
                    if depth > MAX_SIGNAL_DEPTH {
                        return true;
                    }
                }
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    false
}

/// The members of the object a body is, in the order they were written, or
/// the one problem that keeps it from being read at all.
pub(super) fn members(body: &[u8]) -> Result<Vec<(String, Raw)>, SignalProblem> {
    if body.len() > MAX_SIGNAL_BYTES {
        return Err(SignalProblem::body(
            SignalProblemCode::TooLarge,
            format!(
                "the body is {} bytes, and a signal is at most {MAX_SIGNAL_BYTES}",
                body.len()
            ),
        ));
    }
    let not_json = |why: String| SignalProblem::body(SignalProblemCode::NotJson, why);
    let text = std::str::from_utf8(body)
        .map_err(|_| not_json("the body is not UTF-8 text".to_string()))?;
    if too_deep(text) {
        return Err(not_json(format!(
            "the body nests lists or objects more than {MAX_SIGNAL_DEPTH} deep"
        )));
    }
    match serde_json::from_str::<Top>(text) {
        Ok(Top::Object(members)) => Ok(members),
        Ok(Top::Other(what)) => Err(SignalProblem::body(
            SignalProblemCode::NotAnObject,
            format!("the body is {what}, and a signal is one JSON object"),
        )),
        Err(error) => Err(not_json(format!("the body is not JSON: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(body: &str) -> Result<Vec<(String, Raw)>, SignalProblemCode> {
        members(body.as_bytes()).map_err(|problem| problem.code())
    }

    fn text(value: &str) -> Raw {
        Raw::Text(value.to_string())
    }

    #[test]
    fn members_come_as_written_duplicates_included() {
        let read = read(r#"{"a":"x","b":7,"a":"y","c":null}"#).expect("members");
        assert_eq!(
            read,
            [
                ("a".to_string(), text("x")),
                ("b".to_string(), Raw::Whole(7)),
                ("a".to_string(), text("y")),
                ("c".to_string(), Raw::Other("null")),
            ]
        );
    }

    #[test]
    fn only_a_plain_whole_number_counts_as_one() {
        let value = |json: &str| {
            let body = format!("{{\"v\":{json}}}");
            read(&body).expect("members").remove(0).1
        };
        assert_eq!(value("1"), Raw::Whole(1));
        assert_eq!(value("0"), Raw::Whole(0));
        assert_eq!(value("18446744073709551615"), Raw::Whole(u64::MAX));
        for other in ["-0", "-1", "1.0", "1e0", "1E2", "18446744073709551616"] {
            assert_eq!(value(other), Raw::Other("a number"), "{other}");
        }
        assert_eq!(value("true"), Raw::Other("a boolean"));
        assert_eq!(value("[1,[2]]"), Raw::Other("a list"));
        assert_eq!(value(r#"{"a":{"b":1}}"#), Raw::Other("an object"));
        // Escapes spell the text they stand for.
        assert_eq!(value("\"\\u0041\\n\\\"\""), text("A\n\""));
    }

    #[test]
    fn what_is_not_json_is_refused_wherever_it_sits() {
        for body in [
            "",
            "{",
            r#"{"a":1,}"#,
            r#"{"a":1}{"a":1}"#,
            r#"{"a":01}"#,
            r#"{"a":NaN}"#,
            r#"{'a':1}"#,
            // A number no double holds, at the top and nested.
            r#"{"a":1e999}"#,
            r#"{"a":[[1e999]]}"#,
            r#"{"a":-1e999}"#,
            // Half a character, in a value, in a name and nested.
            r#"{"a":"\ud800"}"#,
            r#"{"\ud800":1}"#,
            r#"{"a":["\udc00"]}"#,
            r#"{"a":{"b":"\ud800"}}"#,
            "{\"a\":\"line\nbreak\"}",
            "\u{feff}{}",
        ] {
            assert_eq!(read(body), Err(SignalProblemCode::NotJson), "{body:?}");
        }
        let long = format!("{{\"a\":1{}}}", "0".repeat(400));
        assert_eq!(read(&long), Err(SignalProblemCode::NotJson));
        assert_eq!(
            members(b"{\"a\":\"\xff\"}").map_err(|problem| problem.code()),
            Err(SignalProblemCode::NotJson)
        );
        // A number that underflows a double is still a number.
        assert_eq!(
            read(r#"{"a":1e-999}"#),
            Ok(vec![("a".to_string(), Raw::Other("a number"))])
        );
    }

    #[test]
    fn json_that_is_no_object_is_told_apart_from_no_json() {
        for body in [
            "[]",
            r#"[{"a":1}]"#,
            "\"text\"",
            "1",
            "-1",
            "1.5",
            "true",
            "null",
        ] {
            assert_eq!(read(body), Err(SignalProblemCode::NotAnObject), "{body}");
        }
        assert_eq!(read(" {} "), Ok(Vec::new()));
    }

    #[test]
    fn nesting_is_read_up_to_the_limit_and_no_further() {
        let nested = |lists: usize| format!("{{\"a\":{}{}}}", "[".repeat(lists), "]".repeat(lists));
        // The object is one level, so seven lists reach the limit.
        assert_eq!(
            read(&nested(MAX_SIGNAL_DEPTH - 1)),
            Ok(vec![("a".to_string(), Raw::Other("a list"))])
        );
        assert_eq!(
            read(&nested(MAX_SIGNAL_DEPTH)),
            Err(SignalProblemCode::NotJson)
        );
        // Brackets inside a string open nothing, before an escaped quote
        // or after it.
        let brackets = "[{".repeat(20);
        let quoted = format!("{{\"a\":\"{brackets}\\\"{brackets}\"}}");
        assert!(read(&quoted).is_ok(), "{quoted}");
        let backslash = format!("{{\"a\":\"{brackets}\\\\\",\"b\":\"{brackets}\"}}");
        assert!(read(&backslash).is_ok(), "{backslash}");
    }

    #[test]
    fn a_body_over_the_limit_is_not_read_at_all() {
        let padded = |len: usize| {
            let mut body = String::from("{}");
            body.push_str(&" ".repeat(len - 2));
            body
        };
        assert_eq!(read(&padded(MAX_SIGNAL_BYTES)), Ok(Vec::new()));
        assert_eq!(
            read(&padded(MAX_SIGNAL_BYTES + 1)),
            Err(SignalProblemCode::TooLarge)
        );
        // Its size is said before whether it is JSON.
        assert_eq!(
            read(&"x".repeat(MAX_SIGNAL_BYTES + 1)),
            Err(SignalProblemCode::TooLarge)
        );
    }
}
