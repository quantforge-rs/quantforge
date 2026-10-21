//! The shared cases in `tests/fixtures/segment_cases.json`, which every
//! implementation of the spec must pass. Their payloads and names come from
//! an independent Python implementation, never from the code under test.
//! Then every payload, and every stored form, a reader refuses.

use quantforge::storage::segments::{self, FORMAT, MAX_PAYLOAD_BYTES, SegmentError, WINDOW_BARS};
use quantforge::{Candle, SegmentHash};
use rust_decimal::Decimal;
use serde::Deserialize;

const CASES: &str = include_str!("../fixtures/segment_cases.json");

#[derive(Deserialize)]
struct Fixture {
    format: u32,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    candles: Vec<TextCandle>,
    payload_hex: String,
    hash: String,
}

/// A candle as the spec writes one: every decimal its exact text.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct TextCandle {
    pub open_time_ms: i64,
    pub close_time_ms: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    pub trades: Option<u64>,
}

impl TextCandle {
    pub fn candle(&self) -> Candle {
        let exact = |text: &str| Decimal::from_str_exact(text).expect("an exact decimal");
        Candle {
            open_time_ms: self.open_time_ms,
            close_time_ms: self.close_time_ms,
            open: exact(&self.open),
            high: exact(&self.high),
            low: exact(&self.low),
            close: exact(&self.close),
            volume: exact(&self.volume),
            trades: self.trades,
        }
    }

    fn of(candle: &Candle) -> Self {
        Self {
            open_time_ms: candle.open_time_ms,
            close_time_ms: candle.close_time_ms,
            open: candle.open.to_string(),
            high: candle.high.to_string(),
            low: candle.low.to_string(),
            close: candle.close.to_string(),
            volume: candle.volume.to_string(),
            trades: candle.trades,
        }
    }
}

fn fixture() -> Fixture {
    let fixture: Fixture = serde_json::from_str(CASES).expect("segment cases");
    assert_eq!(fixture.format, FORMAT);
    assert!(fixture.cases.len() >= 8, "the fixture lost cases");
    fixture
}

#[test]
fn every_case_encodes_to_its_payload_and_name() {
    for case in fixture().cases {
        let candles: Vec<Candle> = case.candles.iter().map(TextCandle::candle).collect();
        let payload = segments::encode(&candles).expect("payload");
        assert_eq!(hex::encode(&payload), case.payload_hex, "{}", case.name);
        assert_eq!(
            segments::hash(&payload).to_string(),
            case.hash,
            "{}",
            case.name
        );
    }
}

#[test]
fn every_payload_decodes_to_its_candles_with_the_same_text() {
    for case in fixture().cases {
        let payload = hex::decode(&case.payload_hex).expect("hex");
        let candles = segments::decode(&payload).expect("candles");
        let texts: Vec<TextCandle> = candles.iter().map(TextCandle::of).collect();
        assert_eq!(texts, case.candles, "{}", case.name);
    }
}

#[test]
fn a_stored_case_opens_under_its_name_and_under_no_other() {
    let cases = fixture().cases;
    for (case, other) in cases.iter().zip(cases.iter().cycle().skip(1)) {
        let payload = hex::decode(&case.payload_hex).expect("hex");
        let stored = segments::compress(&payload).expect("compress");
        let name: SegmentHash = case.hash.parse().expect("name");
        let candles: Vec<Candle> = case.candles.iter().map(TextCandle::candle).collect();
        assert_eq!(segments::open(&stored, &name).expect("open"), candles);

        let wrong: SegmentHash = other.hash.parse().expect("name");
        assert!(
            matches!(
                segments::open(&stored, &wrong),
                Err(SegmentError::HashMismatch { expected, found }) if expected == wrong && found == name
            ),
            "{}",
            case.name
        );
    }
}

#[test]
fn candles_that_differ_only_in_scale_or_a_missing_trade_count_pack_differently() {
    let named = |name: &str| {
        fixture()
            .cases
            .into_iter()
            .find(|case| case.name == name)
            .unwrap_or_else(|| panic!("case {name}"))
    };
    let trailing = named("a price with a trailing zero");
    let without = named("the same price without it");
    assert_eq!(
        trailing.candles[0].candle(),
        without.candles[0].candle(),
        "equal as numbers"
    );
    assert_ne!(trailing.hash, without.hash);
    assert_ne!(
        named("trade count unknown").hash,
        named("trade count zero").hash
    );
}

#[test]
fn a_full_window_round_trips_and_one_more_candle_is_refused() {
    let candles: Vec<Candle> = (0..i64::from(WINDOW_BARS + 1))
        .map(|index| {
            TextCandle {
                open_time_ms: index * 60_000,
                close_time_ms: index * 60_000 + 59_999,
                open: format!("{index}.5"),
                high: format!("{}.75", index + 1),
                low: format!("{index}"),
                close: format!("{index}.50"),
                volume: "0.00100000".to_string(),
                trades: (index % 3 != 0).then_some(index as u64),
            }
            .candle()
        })
        .collect();
    let full = &candles[..WINDOW_BARS as usize];
    let payload = segments::encode(full).expect("payload");
    assert!(payload.len() <= MAX_PAYLOAD_BYTES);
    assert_eq!(segments::decode(&payload).expect("candles"), full);

    assert!(matches!(
        segments::encode(&candles),
        Err(SegmentError::BarCount { count }) if count == WINDOW_BARS as usize + 1
    ));
    assert!(matches!(
        segments::encode(&[]),
        Err(SegmentError::BarCount { count: 0 })
    ));
    let mut out_of_order = full[..2].to_vec();
    out_of_order.swap(0, 1);
    assert!(matches!(
        segments::encode(&out_of_order),
        Err(SegmentError::OutOfOrder {
            previous_ms: 60_000,
            open_time_ms: 0
        })
    ));
}

/// Writes the layout of the spec around whatever it is given, with none of
/// the encoder's checks, so that a reader can be shown each kind of bytes
/// it must refuse.
struct Raw {
    magic: Vec<u8>,
    format: u32,
    count: u32,
    open_times: Vec<i64>,
    close_times: Vec<i64>,
    trades: Vec<u64>,
    known: Vec<u8>,
    texts: [Vec<String>; 5],
}

impl Raw {
    /// Two valid candles.
    fn valid() -> Self {
        let text = |values: [&str; 2]| values.map(str::to_string).to_vec();
        Self {
            magic: b"QFSEGMNT".to_vec(),
            format: 1,
            count: 2,
            open_times: vec![0, 60_000],
            close_times: vec![59_999, 119_999],
            trades: vec![3, 0],
            known: vec![1, 0],
            texts: [
                text(["1.5", "2"]),
                text(["2", "2"]),
                text(["1", "1.25"]),
                text(["2", "1.25"]),
                text(["10", "0.5"]),
            ],
        }
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = self.magic.clone();
        out.extend(self.format.to_le_bytes());
        out.extend(self.count.to_le_bytes());
        out.extend(self.open_times.iter().flat_map(|time| time.to_le_bytes()));
        out.extend(self.close_times.iter().flat_map(|time| time.to_le_bytes()));
        out.extend(self.trades.iter().flat_map(|trades| trades.to_le_bytes()));
        out.extend(&self.known);
        for column in &self.texts {
            out.extend(column.iter().map(|text| text.len() as u8));
            out.extend(column.iter().flat_map(|text| text.bytes()));
        }
        out
    }
}

fn refused(raw: &Raw) -> SegmentError {
    segments::decode(&raw.bytes()).expect_err("a refused payload")
}

#[test]
fn the_raw_writer_writes_what_the_encoder_writes() {
    let candles = segments::decode(&Raw::valid().bytes()).expect("candles");
    assert_eq!(
        segments::encode(&candles).expect("payload"),
        Raw::valid().bytes()
    );
}

#[test]
fn a_reader_refuses_a_wrong_header() {
    let mut raw = Raw::valid();
    raw.magic = b"QFSEGMNX".to_vec();
    assert!(matches!(refused(&raw), SegmentError::Malformed { .. }));

    let mut raw = Raw::valid();
    raw.format = 2;
    assert!(matches!(refused(&raw), SegmentError::UnsupportedFormat(2)));

    for count in [0, WINDOW_BARS + 1] {
        let mut raw = Raw::valid();
        raw.count = count;
        assert!(
            matches!(refused(&raw), SegmentError::BarCount { count: found } if found == count as usize),
            "{count}"
        );
    }
}

#[test]
fn a_reader_refuses_open_times_that_do_not_rise() {
    for open_times in [vec![60_000, 0], vec![0, 0]] {
        let mut raw = Raw::valid();
        raw.open_times = open_times;
        assert!(matches!(refused(&raw), SegmentError::OutOfOrder { .. }));
    }
}

#[test]
fn a_reader_refuses_a_trade_count_that_could_be_written_two_ways() {
    let mut raw = Raw::valid();
    raw.known = vec![1, 2];
    assert!(matches!(refused(&raw), SegmentError::Malformed { .. }));

    let mut raw = Raw::valid();
    raw.trades = vec![3, 9];
    assert!(matches!(refused(&raw), SegmentError::Malformed { .. }));
}

#[test]
fn a_reader_refuses_a_decimal_not_in_its_exact_form() {
    for text in [
        "",
        "01",
        "+1",
        "1.",
        ".5",
        "-0",
        "-0.00",
        "1e5",
        "1E5",
        "1_000",
        " 1",
        "1 ",
        "0x10",
        "NaN",
        "0.00000000000000000000000000001",
        "79228162514264337593543950336",
    ] {
        for column in 0..5 {
            let mut raw = Raw::valid();
            raw.texts[column][1] = text.to_string();
            assert!(
                matches!(
                    refused(&raw),
                    SegmentError::InvalidDecimal { column: name, open_time_ms: 60_000, text: found }
                        if name == ["open", "high", "low", "close", "volume"][column] && found == text
                ),
                "{text:?} as column {column}"
            );
        }
    }
}

#[test]
fn a_reader_refuses_a_payload_that_ends_early_or_runs_on() {
    let bytes = Raw::valid().bytes();
    for end in [0, 7, 15, 16, 40, bytes.len() - 1] {
        assert!(
            matches!(
                segments::decode(&bytes[..end]),
                Err(SegmentError::Malformed { .. })
            ),
            "cut at {end}"
        );
    }
    let mut longer = bytes;
    longer.push(b'0');
    assert!(matches!(
        segments::decode(&longer),
        Err(SegmentError::Malformed { .. })
    ));
}

#[test]
fn stored_bytes_are_exactly_one_frame_of_at_most_the_largest_payload() {
    let payload = Raw::valid().bytes();
    let stored = segments::compress(&payload).expect("compress");
    let name = segments::hash(&payload);
    assert!(segments::open(&stored, &name).is_ok());

    let mut twice = stored.clone();
    twice.extend_from_slice(&stored);
    let mut trailing = stored.clone();
    trailing.push(0);
    for bytes in [payload.clone(), twice, trailing] {
        assert!(matches!(
            segments::open(&bytes, &name),
            Err(SegmentError::NotOneFrame)
        ));
    }

    // A stream records no size, so the cap stops it while it decompresses.
    let streamed =
        zstd::stream::encode_all(&vec![0u8; MAX_PAYLOAD_BYTES + 1][..], 3).expect("zstd");
    assert!(matches!(
        segments::decompress(&streamed),
        Err(SegmentError::Zstd(_))
    ));
    let sized = segments::compress(&vec![0u8; MAX_PAYLOAD_BYTES + 1]).expect("compress");
    assert!(matches!(
        segments::decompress(&sized),
        Err(SegmentError::TooLarge)
    ));
}
