//! What a transfer between two stores carries (`docs/storage-migration.md`):
//! each table as rows of the values both backends store, read in the same
//! order on both, the canonical text a checksum is taken over, and the
//! record a target keeps of the copy it holds.
//!
//! [`TRANSFER_TABLES`] lists the tables of schema [`TRANSFER_SCHEMA`] that a
//! transfer copies; every other table is the store's own and stays behind.
//! A story that adds a table adds it to one list or the other, and the
//! storage tests fail until it does.

mod tables;

pub use tables::{
    BOT_RUNS, BUNDLES, CANDLES, CLOSED_TRADES, ColumnKind, JOBS, KEPT_BY_EACH_STORE, ORDER_EVENTS,
    SEGMENT_DIRTY, SEGMENT_INDEX, SEGMENTS, TRANSFER_TABLES, TransferColumn, TransferTable,
};

use super::{ModelError, StoreId, TimestampMs};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The format of the canonical text and of [`TransferRecord`]. A change to
/// either is a new format, and a copy begun under another is refused.
pub const TRANSFER_FORMAT: u32 = 1;

/// The schema whose tables [`TRANSFER_TABLES`] lists. A source and a target
/// are both at it, and a schema step that changes a listed table changes it.
pub const TRANSFER_SCHEMA: u32 = 9;

/// A stored value as both backends hold it: no decimal parsed, no text
/// normalized. Values of a kind compare as SQLite compares them: integers
/// by value, text by its UTF-8 bytes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TransferValue {
    Null,
    Integer(i64),
    Text(String),
    Blob(Vec<u8>),
}

impl TransferValue {
    /// Appends the value's canonical text: `n` for NULL, otherwise its tag
    /// (`i`, `t` or `b`), its length in bytes in decimal, a colon, and its
    /// bytes: an integer's decimal digits, a text's UTF-8, a blob's own.
    pub fn write_canonical(&self, out: &mut Vec<u8>) {
        let (tag, bytes): (u8, &[u8]) = match self {
            Self::Null => {
                out.push(b'n');
                return;
            }
            Self::Integer(value) => {
                let digits = value.to_string();
                out.push(b'i');
                out.extend_from_slice(digits.len().to_string().as_bytes());
                out.push(b':');
                out.extend_from_slice(digits.as_bytes());
                return;
            }
            Self::Text(text) => (b't', text.as_bytes()),
            Self::Blob(bytes) => (b'b', bytes),
        };
        out.push(tag);
        out.extend_from_slice(bytes.len().to_string().as_bytes());
        out.push(b':');
        out.extend_from_slice(bytes);
    }

    /// The value's kind, as a report names it.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Integer(_) => "integer",
            Self::Text(_) => "text",
            Self::Blob(_) => "blob",
        }
    }
}

/// How a report shows a value: text quoted, a blob by its length.
impl fmt::Display for TransferValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("NULL"),
            Self::Integer(value) => write!(f, "{value}"),
            Self::Text(text) => write!(f, "{text:?}"),
            Self::Blob(bytes) => write!(f, "<{} bytes>", bytes.len()),
        }
    }
}

/// A row: one value per column of its table, in the table's order.
pub type TransferRow = Vec<TransferValue>;

/// Appends a row's canonical text: its values, then a newline.
pub fn write_canonical_row(row: &[TransferValue], out: &mut Vec<u8>) {
    for value in row {
        value.write_canonical(out);
    }
    out.push(b'\n');
}

/// A market as a market table's first three columns hold it, unparsed.
/// Markets order as their three texts do, byte by byte.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferMarket {
    pub exchange: String,
    pub symbol: String,
    pub interval: String,
}

impl fmt::Display for TransferMarket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.exchange, self.symbol, self.interval)
    }
}

/// How far the copy a target holds has come.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Begun and not finished: what the target holds is part of its source.
    Copying,
    /// The target holds one snapshot of the source, row for row.
    Copied,
}

impl TransferState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Copying => "copying",
            Self::Copied => "copied",
        }
    }
}

impl fmt::Display for TransferState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a target records of the copy it holds: which store it came from,
/// when it began, and whether it finished. A store holds at most one, and
/// one that holds a copy takes no other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRecord {
    pub format: u32,
    pub source_store_id: StoreId,
    pub schema_version: u32,
    pub state: TransferState,
    pub started_at_ms: TimestampMs,
    pub copied_at_ms: Option<TimestampMs>,
}

impl TransferRecord {
    /// A copy of `source_store_id` begun at `started_at_ms`.
    pub fn begin(source_store_id: StoreId, started_at_ms: TimestampMs) -> Self {
        Self {
            format: TRANSFER_FORMAT,
            source_store_id,
            schema_version: TRANSFER_SCHEMA,
            state: TransferState::Copying,
            started_at_ms,
            copied_at_ms: None,
        }
    }

    /// The same copy, finished at `copied_at_ms`.
    pub fn copied(&self, copied_at_ms: TimestampMs) -> Self {
        Self {
            state: TransferState::Copied,
            copied_at_ms: Some(copied_at_ms),
            ..self.clone()
        }
    }

    pub fn to_json(&self) -> String {
        // A struct of strings and integers always serializes.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The record from its JSON, or the refusal of one written in another
    /// format, named before anything else is read.
    pub fn from_json(json: &str) -> Result<Self, ModelError> {
        #[derive(Deserialize)]
        struct Format {
            format: u32,
        }
        let invalid = |error: serde_json::Error| ModelError::InvalidRecord {
            record: "transfer record",
            reason: error.to_string(),
        };
        let Format { format } = serde_json::from_str(json).map_err(invalid)?;
        if format != TRANSFER_FORMAT {
            return Err(ModelError::UnsupportedFormat {
                record: "transfer record",
                found: format,
                expected: TRANSFER_FORMAT,
            });
        }
        serde_json::from_str(json).map_err(invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical(value: TransferValue) -> String {
        let mut out = Vec::new();
        value.write_canonical(&mut out);
        String::from_utf8(out).expect("utf-8")
    }

    #[test]
    fn each_kind_of_value_has_its_canonical_text() {
        assert_eq!(canonical(TransferValue::Null), "n");
        assert_eq!(canonical(TransferValue::Integer(0)), "i1:0");
        assert_eq!(
            canonical(TransferValue::Integer(i64::MIN)),
            "i20:-9223372036854775808"
        );
        assert_eq!(canonical(TransferValue::Text(String::new())), "t0:");
        // The length counts bytes, not characters.
        assert_eq!(
            canonical(TransferValue::Text("ü1.10".to_string())),
            "t6:ü1.10"
        );
        let mut out = Vec::new();
        TransferValue::Blob(vec![0, 255]).write_canonical(&mut out);
        assert_eq!(out, b"b2:\x00\xff");
    }

    #[test]
    fn text_values_compare_by_their_bytes() {
        let text = |value: &str| TransferValue::Text(value.to_string());
        assert!(text("run-B") < text("run-a"));
        assert!(text("BTCUSDT") < text("币安人生USDT"));
        assert!(TransferValue::Integer(-1) < TransferValue::Integer(0));
    }

    #[test]
    fn a_record_round_trips_and_one_of_another_format_is_refused_by_its_format() {
        let store_id = StoreId::new("0123456789abcdef0123456789abcdef").expect("id");
        let record = TransferRecord::begin(store_id, 1_000).copied(2_000);
        assert_eq!(
            TransferRecord::from_json(&record.to_json()).expect("read"),
            record
        );

        let newer = record.to_json().replace("\"format\":1", "\"format\":2");
        let newer = format!("{}, \"added\": true}}", newer.trim_end_matches('}'));
        let error = TransferRecord::from_json(&newer).expect_err("newer format");
        assert_eq!(
            error.to_string(),
            "transfer record format 2 is not supported; this build reads format 1"
        );

        let unknown = format!(
            "{}, \"added\": true}}",
            record.to_json().trim_end_matches('}')
        );
        assert!(matches!(
            TransferRecord::from_json(&unknown),
            Err(ModelError::InvalidRecord { .. })
        ));
    }
}
