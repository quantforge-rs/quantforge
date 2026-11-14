//! Every body of the shared cases, read by the protocol and held to what
//! the Python model found: the signal it is, with its ID, its canonical
//! payload and its hash, or every problem it has, in the format's order.
//! Then what the server makes of a body: the model's signal, or the answer
//! for a body that is none.

use quantforge::model::futures::{SIGNAL_FUTURE_ALLOWANCE_MS, SignalHash};
use quantforge::model::futures::{SignalAction, SignalAgeLimit, TickerSpelling};
use quantforge::model::{Category, PositionSide};
use quantforge::protocol::ErrorCode;
use quantforge::protocol::signal::{
    MAX_SIGNAL_BYTES, MAX_SIGNAL_DEPTH, MAX_TICKER_CHARS, ReceiptState, SIGNAL_FIELDS,
    SIGNAL_VERSION, Signal, SignalProblemCode,
};
use quantforge::server::{external_signal, read_signal};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::cases::{Payload, ProblemAnswer, fixture};

/// The problems of a body, as the fixture writes them.
fn problems_of(body: &[u8]) -> Option<Vec<ProblemAnswer>> {
    let refused = Signal::read(body).err()?;
    Some(
        refused
            .problems()
            .iter()
            .map(|problem| ProblemAnswer {
                code: problem.code().as_str().to_string(),
                field: problem.field_name().map(str::to_string),
            })
            .collect(),
    )
}

/// A signal's fields as a JSON object, as the fixture writes them.
fn fields_of(signal: &Signal) -> Map<String, Value> {
    let mut fields = Map::new();
    let mut put = |name: &str, value: Option<String>| {
        if let Some(value) = value {
            fields.insert(name.to_string(), Value::String(value));
        }
    };
    put("kind", Some(signal.kind().to_string()));
    put("profile", Some(signal.profile().to_string()));
    put("source", Some(signal.source().to_string()));
    put("ticker", Some(signal.ticker().to_string()));
    put("exchange", signal.exchange().map(str::to_string));
    put(
        "category",
        signal.category().map(|category| category.to_string()),
    );
    put("side", Some(signal.side().to_string()));
    put("bar_time", Some(signal.bar_time().to_string()));
    put("fire_time", Some(signal.fire_time().to_string()));
    put("price", signal.price().map(|price| price.to_string()));
    put("stop", signal.stop().map(|stop| stop.to_string()));
    fields.insert("version".to_string(), Value::from(SIGNAL_VERSION));
    fields
}

#[test]
fn the_formats_constants_are_the_cases() {
    let fixture = fixture();
    assert_eq!(fixture.format, 1);
    assert_eq!(
        fixture.generated_by,
        "spikes/futures-contract/signal_cases.py"
    );
    assert_eq!(fixture.signal_version, SIGNAL_VERSION);
    assert_eq!(fixture.max_signal_bytes, MAX_SIGNAL_BYTES);
    assert_eq!(fixture.max_depth, MAX_SIGNAL_DEPTH);
    assert_eq!(fixture.ticker_max, MAX_TICKER_CHARS);
    assert_eq!(fixture.fields, SIGNAL_FIELDS);
    let codes: Vec<&str> = SignalProblemCode::ALL
        .iter()
        .map(|code| code.as_str())
        .collect();
    assert_eq!(fixture.problem_codes, codes);
    let states: Vec<&str> = ReceiptState::ALL
        .iter()
        .map(|state| state.as_str())
        .collect();
    assert_eq!(fixture.receipt_states, states);
    let spellings: Vec<&str> = TickerSpelling::ALL
        .iter()
        .map(|spelling| spelling.as_str())
        .collect();
    assert_eq!(fixture.spellings, spellings);
    assert_eq!(fixture.age_limit_s.default, SignalAgeLimit::DEFAULT.secs());
    assert_eq!(fixture.age_limit_s.min, SignalAgeLimit::MIN_SECS);
    assert_eq!(fixture.age_limit_s.max, SignalAgeLimit::MAX_SECS);
    assert_eq!(fixture.future_allowance_ms, SIGNAL_FUTURE_ALLOWANCE_MS);
}

#[test]
fn every_body_is_what_the_cases_say() {
    let fixture = fixture();
    let mut wrong = Vec::new();
    let (mut signals, mut refused) = (0, 0);
    for payload in &fixture.payloads {
        let body = payload.bytes();
        match (&payload.signal, &payload.problems) {
            (Some(expected), None) => {
                signals += 1;
                let signal = match Signal::read(&body) {
                    Ok(signal) => signal,
                    Err(problems) => {
                        wrong.push(format!("{}: a signal, refused: {problems}", payload.id));
                        continue;
                    }
                };
                let got = (
                    fields_of(&signal),
                    signal.signal_id(),
                    signal.canonical(),
                    signal.payload_hash().to_string(),
                    signal.bar_time().ms(),
                    signal.fire_time().ms(),
                );
                let recorded = (
                    expected.fields.clone(),
                    expected.signal_id.clone(),
                    expected.canonical.clone(),
                    expected.hash.clone(),
                    expected.bar_time_ms,
                    expected.fire_time_ms,
                );
                if got != recorded {
                    wrong.push(format!(
                        "{}:\n  read     {got:?}\n  recorded {recorded:?}",
                        payload.id
                    ));
                }
            }
            (None, Some(expected)) => {
                refused += 1;
                let got = problems_of(&body);
                if got.as_ref() != Some(expected) {
                    wrong.push(format!(
                        "{}:\n  found    {got:?}\n  recorded {expected:?}",
                        payload.id
                    ));
                }
            }
            _ => panic!(
                "{}: a body is a signal or has problems, one of them",
                payload.id
            ),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    // The cases are there: a fixture that lost them would pass the loop.
    assert!(
        signals >= 60 && refused >= 120,
        "{signals} signals, {refused} refused"
    );
}

#[test]
fn the_cases_show_every_problem_and_every_field() {
    let fixture = fixture();
    let found: Vec<&ProblemAnswer> = fixture
        .payloads
        .iter()
        .filter_map(|payload| payload.problems.as_ref())
        .flatten()
        .collect();
    for code in SignalProblemCode::ALL {
        assert!(
            found.iter().any(|problem| problem.code == code.as_str()),
            "no case has the problem {code}"
        );
    }
    for field in SIGNAL_FIELDS {
        for code in ["wrong_type", "invalid_value", "missing_field"] {
            let optional = ["exchange", "category", "stop"].contains(&field);
            let applies = match code {
                "invalid_value" => field != "version",
                "missing_field" => !optional,
                _ => true,
            };
            let shown = found
                .iter()
                .any(|problem| problem.code == code && problem.field.as_deref() == Some(field));
            assert_eq!(shown, applies, "{code} {field}");
        }
    }
}

#[test]
fn an_id_is_the_fields_the_format_names_and_a_hash_is_of_the_canonical_bytes() {
    let fixture = fixture();
    assert_eq!(
        fixture.id_fields,
        ["source", "kind", "side", "ticker", "bar_time"]
    );
    assert_eq!(fixture.not_in_canonical, ["fire_time"]);
    for payload in fixture
        .payloads
        .iter()
        .filter(|payload| payload.signal.is_some())
    {
        let signal = Signal::read(&payload.bytes()).expect("a signal");
        let fields = fields_of(&signal);
        let parts: Vec<&str> = fixture
            .id_fields
            .iter()
            .map(|field| fields[field].as_str().expect("text"))
            .collect();
        assert_eq!(signal.signal_id(), parts.join(":"), "{}", payload.id);
        assert!(signal.signal_id().chars().count() <= 106, "{}", payload.id);

        // The canonical payload is the signal's own fields, in the format's
        // order, without the one the format leaves out.
        let canonical: Map<String, Value> =
            serde_json::from_str(&signal.canonical()).expect("canonical JSON");
        let mut kept = fields.clone();
        for left_out in &fixture.not_in_canonical {
            kept.remove(left_out);
        }
        assert_eq!(canonical, kept, "{}", payload.id);
        // The members are written in the format's order.
        let written: Vec<usize> = SIGNAL_FIELDS
            .iter()
            .filter_map(|field| signal.canonical().find(&format!("\"{field}\":")))
            .collect();
        assert_eq!(written.len(), canonical.len(), "{}", payload.id);
        assert!(
            written.windows(2).all(|pair| pair[0] < pair[1]),
            "{}",
            payload.id
        );
        assert!(
            !signal.canonical().contains(char::is_whitespace),
            "{}",
            payload.id
        );

        let digest: [u8; 32] = Sha256::digest(signal.canonical().as_bytes()).into();
        assert_eq!(signal.payload_hash().digest(), &digest, "{}", payload.id);
    }
}

#[test]
fn a_signal_is_the_same_however_its_body_is_written() {
    let fixture = fixture();
    for payload in fixture
        .payloads
        .iter()
        .filter(|payload| payload.signal.is_some())
    {
        let signal = Signal::read(&payload.bytes()).expect("a signal");
        let fields = fields_of(&signal);
        let reversed: Vec<String> = fields
            .iter()
            .rev()
            .map(|(name, value)| format!("\"{name}\" :\t{value}"))
            .collect();
        for respelled in [
            serde_json::to_string_pretty(&fields).expect("json"),
            format!("\r\n {{ {} }}\n", reversed.join(" ,\n")),
        ] {
            let again = Signal::read(respelled.as_bytes())
                .unwrap_or_else(|problems| panic!("{}: {respelled}: {problems}", payload.id));
            assert_eq!(again, signal, "{}", payload.id);
            assert_eq!(
                again.payload_hash(),
                signal.payload_hash(),
                "{}",
                payload.id
            );
        }
    }
}

/// A body like the base case's, with one member's raw JSON replaced.
fn with_member(base: &Payload, name: &str, json: &str) -> Vec<u8> {
    let fields = &base.signal.as_ref().expect("a signal").fields;
    let members: Vec<String> = fields
        .iter()
        .map(|(field, value)| {
            let value = if field == name {
                json.to_string()
            } else {
                value.to_string()
            };
            format!("\"{field}\":{value}")
        })
        .collect();
    format!("{{{}}}", members.join(",")).into_bytes()
}

#[test]
fn a_ticker_refuses_exactly_the_characters_the_cases_list() {
    let fixture = fixture();
    let base = fixture.payload("by-linear-long-entry");
    let taken = |character: char| {
        let ticker = Value::String(format!("SOL{character}USDT")).to_string();
        problems_of(&with_member(base, "ticker", &ticker)).is_none()
    };
    let valid = |point: u32| char::from_u32(point).filter(|character| !character.is_ascii());
    let mut edges = 0;
    for (low, high) in &fixture.ticker_refused {
        for point in [*low, *high, (*low + *high) / 2] {
            let character = valid(point).expect("a character");
            assert!(!taken(character), "U+{point:04X} is taken");
        }
        // Either side of a range is taken, unless another range starts there.
        for point in [low.wrapping_sub(1), *high + 1] {
            let refused = |(low, high): &(u32, u32)| (*low..=*high).contains(&point);
            if let Some(character) =
                valid(point).filter(|_| !fixture.ticker_refused.iter().any(refused))
            {
                assert!(taken(character), "U+{point:04X} is refused");
                edges += 1;
            }
        }
    }
    assert!(edges >= 10, "{edges}");
    // Of ASCII, a ticker takes only what the format lists.
    for byte in 0x20u8..0x7f {
        let character = char::from(byte);
        let listed = character.is_ascii_uppercase()
            || character.is_ascii_digit()
            || "._-".contains(character);
        assert_eq!(taken(character), listed, "{character:?}");
    }
}

#[test]
fn what_is_wrong_with_a_body_is_said_in_words_an_operator_can_act_on() {
    let fixture = fixture();
    let said = |id: &str| {
        Signal::read(&fixture.payload(id).bytes())
            .expect_err("not a signal")
            .to_string()
    };
    let prefix = "the body is not a version 1 signal: ";
    for (id, message) in [
        (
            "an-array",
            "the body is a list, and a signal is one JSON object",
        ),
        (
            "nested-too-deep",
            "the body nests lists or objects more than 8 deep",
        ),
        ("not-utf8", "the body is not UTF-8 text"),
        ("duplicate-side", "\"side\" is written more than once"),
        (
            "asks-for-leverage",
            "\"leverage\" is not a field of a version 1 signal",
        ),
        (
            "exit-with-a-price",
            "\"price\" belongs to an entry, and this is an exit",
        ),
        ("no-profile", "\"profile\" is missing"),
        (
            "price-as-a-number",
            "\"price\" is a number, and must be a string",
        ),
        ("stop-is-null", "\"stop\" is null, and must be a string"),
        (
            "version-as-text",
            "\"version\" is a string, and must be the number 1",
        ),
        (
            "a-later-version",
            "version 2 is not supported: this is version 1 of the format",
        ),
        (
            "side-buy",
            "\"side\" is \"buy\", and must be \"long\" or \"short\"",
        ),
        (
            "kind-buy",
            "\"kind\" is \"buy\", and must be \"entry\" or \"exit\"",
        ),
        (
            "stop-not-a-number",
            "\"stop\" is \"NaN\", and must be a price above zero as its digits, such as \
             \"234.23\": no sign, no exponent, at most 28 decimal places",
        ),
        (
            "time-with-an-offset",
            "\"fire_time\" is \"2026-10-20T02:15:00+02:00\", and must be a UTC time, written \
             as TradingView renders it, such as \"2026-10-20T00:15:00Z\"",
        ),
        (
            "ticker-unrendered",
            "\"ticker\" is \"{{ticker}}\", and must be a ticker as TradingView renders it, such \
             as \"SOLUSDT.P\": 1 to 40 characters, its letters uppercase, with no exchange \
             before it and no spaces",
        ),
        (
            "bar-after-fire",
            "\"bar_time\" 2026-10-20T00:15:00Z is after \"fire_time\" 2026-10-20T00:00:00Z: a \
             bar opens before its alert fires",
        ),
    ] {
        assert_eq!(said(id), format!("{prefix}{message}"), "{id}");
    }
    let too_large = said("too-large");
    assert!(
        too_large.starts_with(&format!("{prefix}the body is "))
            && too_large.ends_with(" bytes, and a signal is at most 4096"),
        "{too_large}"
    );
    assert!(said("truncated").starts_with(&format!("{prefix}the body is not JSON: ")));
    // Every problem is said, in the format's order, on one line.
    assert_eq!(
        said("several-problems"),
        format!(
            "{prefix}version 3 is not supported: this is version 1 of the format; \"side\" is \
             \"buy\", and must be \"long\" or \"short\"; \"price\" belongs to an entry, and this \
             is an exit; \"stop\" belongs to an entry, and this is an exit; \"note\" is not a \
             field of a version 1 signal"
        )
    );
}

#[test]
fn the_server_reads_a_body_as_the_models_signal_or_answers_what_is_wrong() {
    let fixture = fixture();
    for payload in &fixture.payloads {
        let body = payload.bytes();
        match Signal::read(&body) {
            Ok(wire) => {
                let signal = read_signal(&body)
                    .unwrap_or_else(|refused| panic!("{}: {refused:?}", payload.id));
                assert_eq!(signal, external_signal(&wire).expect("the model's signal"));
                assert_eq!(signal.id.as_str(), wire.signal_id(), "{}", payload.id);
                assert_eq!(
                    signal.payload_hash,
                    SignalHash::from_digest(*wire.payload_hash().digest())
                );
                assert_eq!(
                    signal.payload_hash.to_string(),
                    wire.payload_hash().to_string()
                );
                assert_eq!(signal.profile.as_str(), wire.profile());
                assert_eq!(signal.ticker, wire.ticker());
                assert_eq!(signal.exchange.as_deref(), wire.exchange());
                assert_eq!(
                    signal.category.map(Category::as_str),
                    wire.category().map(|category| category.as_str())
                );
                assert!(signal.category != Some(Category::Spot));
                assert_eq!(signal.side.as_str(), wire.side().as_str());
                assert!(PositionSide::ALL.contains(&signal.side));
                assert_eq!(signal.bar_time_ms, wire.bar_time().ms());
                assert_eq!(signal.fire_time_ms, wire.fire_time().ms());
                let text = |price: quantforge::model::Price| price.to_string();
                match signal.action {
                    SignalAction::Entry {
                        expected_price,
                        stop,
                    } => {
                        // A price keeps the digits it was sent with.
                        assert_eq!(
                            Some(text(expected_price)),
                            wire.price().map(|price| price.to_string())
                        );
                        assert_eq!(stop.map(text), wire.stop().map(|stop| stop.to_string()));
                    }
                    SignalAction::Exit => {
                        assert_eq!((wire.price(), wire.stop()), (None, None), "{}", payload.id);
                    }
                }
            }
            Err(problems) => {
                let refused = read_signal(&body).expect_err("not a signal");
                assert_eq!(
                    refused.error.code,
                    ErrorCode::InvalidRequest,
                    "{}",
                    payload.id
                );
                assert_eq!(refused.error.code.http_status(), Some(400));
                assert_eq!(
                    refused.error.message,
                    problems.to_string(),
                    "{}",
                    payload.id
                );
                assert_eq!(refused.error.supported, None);
            }
        }
    }
}

#[test]
fn no_body_makes_the_reader_fail_and_what_it_takes_holds_the_formats_promises() {
    // One byte of a signal's body at a time replaced by each of a set of
    // bytes a hostile or broken sender might put there.
    let fixture = fixture();
    let base = fixture.payload("by-linear-short-entry").bytes();
    let (mut taken, mut refused) = (0, 0);
    for at in 0..base.len() {
        for byte in [
            b'"', b'\\', b'{', b'}', b'[', b',', b':', b'0', b'9', b'Z', b'a', b' ', b'\n', 0,
            0x7f, 0x80, 0xff,
        ] {
            let mut body = base.clone();
            body[at] = byte;
            match Signal::read(&body) {
                Ok(signal) => {
                    taken += 1;
                    let canonical = signal.canonical();
                    let members: Map<String, Value> =
                        serde_json::from_str(&canonical).expect("canonical JSON");
                    assert!(
                        members
                            .keys()
                            .all(|key| SIGNAL_FIELDS.contains(&key.as_str()))
                    );
                    assert!(signal.signal_id().chars().count() <= 106);
                    assert!(
                        !signal
                            .signal_id()
                            .chars()
                            .any(|character| character.is_control() || character == ' ')
                    );
                    assert!(signal.bar_time() <= signal.fire_time());
                }
                Err(problems) => {
                    refused += 1;
                    assert!(!problems.problems().is_empty());
                }
            }
        }
    }
    // Most single-byte changes break a body; a digit in a price or a time
    // gives another signal.
    assert!(
        taken > 50 && refused > 2_000,
        "{taken} taken, {refused} refused"
    );
}
