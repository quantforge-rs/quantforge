//! A result table's bytes: one Arrow IPC file with one record batch, laid
//! out the same way every time, so that a table has exactly one file and
//! the file's hash names the table. The [result spec] fixes every byte;
//! this module writes them, and `reader` reads them back.
//!
//! [result spec]: <https://github.com/quantforge-rs/quantforge/blob/main/docs/result-spec.md>

use super::RESULT_FORMAT;
use super::flatbuf::Builder;

/// What an Arrow IPC file starts and ends with.
pub(super) const MAGIC: &[u8; 6] = b"ARROW1";

/// What every message in a file starts with.
const CONTINUATION: [u8; 4] = [0xff; 4];

/// Arrow's metadata version 5, as its `MetadataVersion` numbers it.
const VERSION_5: i16 = 4;

/// A message whose header is a `Schema`, in Arrow's `MessageHeader`.
const SCHEMA_MESSAGE: u8 = 1;

/// A message whose header is a `RecordBatch`.
pub(super) const BATCH_MESSAGE: u8 = 3;

/// `Int` and `Utf8` in Arrow's `Type`.
const INT_TYPE: u8 = 2;
const UTF8_TYPE: u8 = 5;

/// The schema metadata key that names the table: `trades` or `equity`.
pub(super) const TABLE_KEY: &str = "quantforge-result";

/// The schema metadata key that holds the result format.
pub(super) const FORMAT_KEY: &str = "quantforge-result-format";

/// The most bytes of text one column holds: its offsets are 32-bit.
const MOST_TEXT: usize = i32::MAX as usize;

/// The Arrow type of a result table's column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnType {
    /// Signed 64-bit integers: Arrow's `Int(64, true)`, which pyarrow calls
    /// `int64`.
    Int64,
    /// UTF-8 text with 32-bit offsets: Arrow's `Utf8`, which pyarrow calls
    /// `string`.
    Utf8,
}

/// A table's name and its columns, in order.
#[derive(Debug)]
pub(super) struct Layout {
    pub(super) name: &'static str,
    pub(super) columns: &'static [(&'static str, ColumnType)],
}

/// One column's values.
#[derive(Debug)]
pub(super) enum Column {
    Int64(Vec<i64>),
    Utf8(Text),
}

/// A text column: every value's text one after another, and where each
/// ends.
#[derive(Debug, Default)]
pub(super) struct Text {
    pub(super) text: String,
    pub(super) ends: Vec<i32>,
}

impl Text {
    /// Adds a value. `false`, adding nothing, when the column would hold
    /// more text than its offsets can count.
    pub(super) fn push(&mut self, value: &str) -> bool {
        self.push_within(value, MOST_TEXT)
    }

    fn push_within(&mut self, value: &str, most: usize) -> bool {
        let end = self.text.len() + value.len();
        if end > most {
            return false;
        }
        self.text.push_str(value);
        self.ends.push(end as i32);
        true
    }

    /// The value in `row`.
    pub(super) fn get(&self, row: usize) -> Option<&str> {
        let end = usize::try_from(*self.ends.get(row)?).ok()?;
        let start = match row.checked_sub(1) {
            Some(before) => usize::try_from(*self.ends.get(before)?).ok()?,
            None => 0,
        };
        self.text.get(start..end)
    }
}

/// The file of a table with `rows` rows, whose `columns` are the layout's,
/// each with `rows` values.
pub(super) fn write(layout: &Layout, rows: usize, columns: &[Column]) -> Vec<u8> {
    let body = Body::of(columns);
    let mut file = Vec::with_capacity(body.bytes.len() + 2048);
    file.extend(MAGIC);
    file.extend([0, 0]);
    encapsulate(&mut file, &schema_message(layout));
    let batch_at = file.len();
    let batch = batch_message(rows, columns.len(), &body);
    let metadata_len = encapsulate(&mut file, &batch);
    file.extend(&body.bytes);
    file.extend(CONTINUATION);
    file.extend([0; 4]);
    let footer = footer(layout, batch_at, metadata_len, body.bytes.len());
    file.extend(&footer);
    file.extend((footer.len() as i32).to_le_bytes());
    file.extend(MAGIC);
    file
}

/// Appends a message's metadata as the file holds it: the continuation
/// mark, the metadata's length, and the metadata padded to 8 bytes. Answers
/// how many bytes that is.
pub(super) fn encapsulate(file: &mut Vec<u8>, metadata: &[u8]) -> usize {
    let padded = metadata.len().next_multiple_of(8);
    file.extend(CONTINUATION);
    file.extend((padded as i32).to_le_bytes());
    file.extend(metadata);
    file.resize(file.len() + padded - metadata.len(), 0);
    8 + padded
}

/// The buffers of a record batch, one after another, each padded to 8
/// bytes, and where each is.
struct Body {
    bytes: Vec<u8>,
    /// Each buffer's offset and length.
    buffers: Vec<(usize, usize)>,
}

impl Body {
    fn of(columns: &[Column]) -> Self {
        let mut body = Self {
            bytes: Vec::new(),
            buffers: Vec::new(),
        };
        for column in columns {
            // No value is null, so no column has a validity bitmap.
            body.buffers.push((body.bytes.len(), 0));
            match column {
                Column::Int64(values) => {
                    body.buffer(values.iter().flat_map(|value| value.to_le_bytes()));
                }
                Column::Utf8(text) => {
                    let offsets = std::iter::once(&0).chain(&text.ends);
                    body.buffer(offsets.flat_map(|offset| offset.to_le_bytes()));
                    body.buffer(text.text.bytes());
                }
            }
        }
        body
    }

    fn buffer(&mut self, bytes: impl Iterator<Item = u8>) {
        let start = self.bytes.len();
        self.bytes.extend(bytes);
        self.buffers.push((start, self.bytes.len() - start));
        self.bytes.resize(self.bytes.len().next_multiple_of(8), 0);
    }
}

/// A `Message` holding the `header` that `build` writes, before a body of
/// `body_len` bytes.
fn message(kind: u8, body_len: usize, build: impl FnOnce(&mut Builder) -> usize) -> Vec<u8> {
    let mut builder = Builder::new();
    let header = build(&mut builder);
    builder.start_table(5);
    if body_len != 0 {
        builder.scalar(3, &(body_len as i64).to_le_bytes());
    }
    builder.reference(2, header);
    builder.scalar(0, &VERSION_5.to_le_bytes());
    builder.scalar(1, &[kind]);
    let root = builder.end_table();
    builder.finish(root)
}

pub(super) fn schema_message(layout: &Layout) -> Vec<u8> {
    message(SCHEMA_MESSAGE, 0, |builder| schema(builder, layout))
}

/// The `Schema`: every column a `Field` that is never null, and the
/// table's name and the format as its metadata.
fn schema(builder: &mut Builder, layout: &Layout) -> usize {
    let fields: Vec<usize> = layout
        .columns
        .iter()
        .map(|(name, kind)| {
            let name = builder.string(name);
            let (tag, of) = match kind {
                ColumnType::Int64 => {
                    builder.start_table(2);
                    builder.scalar(0, &64_i32.to_le_bytes());
                    builder.scalar(1, &[1]);
                    (INT_TYPE, builder.end_table())
                }
                ColumnType::Utf8 => {
                    builder.start_table(0);
                    (UTF8_TYPE, builder.end_table())
                }
            };
            let children = builder.offsets(&[]);
            builder.start_table(7);
            builder.reference(5, children);
            builder.reference(3, of);
            builder.reference(0, name);
            builder.scalar(2, &[tag]);
            builder.end_table()
        })
        .collect();
    let fields = builder.offsets(&fields);
    let format = RESULT_FORMAT.to_string();
    let notes = [(TABLE_KEY, layout.name), (FORMAT_KEY, format.as_str())].map(|(key, value)| {
        let key = builder.string(key);
        let value = builder.string(value);
        builder.start_table(2);
        builder.reference(1, value);
        builder.reference(0, key);
        builder.end_table()
    });
    let notes = builder.offsets(&notes);
    builder.start_table(4);
    builder.reference(2, notes);
    builder.reference(1, fields);
    builder.end_table()
}

/// The `RecordBatch` message: `rows` rows, a node per column with no
/// nulls, and where each buffer is in the body.
fn batch_message(rows: usize, columns: usize, body: &Body) -> Vec<u8> {
    message(BATCH_MESSAGE, body.bytes.len(), |builder| {
        let node = [rows as i64, 0].map(i64::to_le_bytes).concat();
        let nodes = builder.structs(16, &node.repeat(columns));
        let buffers: Vec<u8> = body
            .buffers
            .iter()
            .flat_map(|&(offset, length)| [offset as i64, length as i64])
            .flat_map(i64::to_le_bytes)
            .collect();
        let buffers = builder.structs(16, &buffers);
        builder.start_table(5);
        if rows != 0 {
            builder.scalar(0, &(rows as i64).to_le_bytes());
        }
        builder.reference(2, buffers);
        builder.reference(1, nodes);
        builder.end_table()
    })
}

/// The `Footer`: the schema again, no dictionaries, and the one record
/// batch's place in the file.
fn footer(layout: &Layout, batch_at: usize, metadata_len: usize, body_len: usize) -> Vec<u8> {
    let mut builder = Builder::new();
    let schema = schema(&mut builder, layout);
    let dictionaries = builder.structs(24, &[]);
    let mut block = Vec::with_capacity(24);
    block.extend((batch_at as i64).to_le_bytes());
    block.extend((metadata_len as i32).to_le_bytes());
    block.extend([0; 4]);
    block.extend((body_len as i64).to_le_bytes());
    let batches = builder.structs(24, &block);
    builder.start_table(5);
    builder.reference(3, batches);
    builder.reference(2, dictionaries);
    builder.reference(1, schema);
    builder.scalar(0, &VERSION_5.to_le_bytes());
    let root = builder.end_table();
    builder.finish(root)
}

#[cfg(test)]
mod tests {
    use super::super::flatbuf::{Table, read_i64, read_u32};
    use super::super::reader::read;
    use super::*;

    const PAIRS: Layout = Layout {
        name: "pairs",
        columns: &[("at_ms", ColumnType::Int64), ("label", ColumnType::Utf8)],
    };

    fn pairs(rows: &[(i64, &str)]) -> Vec<Column> {
        let mut labels = Text::default();
        for (_, label) in rows {
            assert!(labels.push(label));
        }
        vec![
            Column::Int64(rows.iter().map(|(at, _)| *at).collect()),
            Column::Utf8(labels),
        ]
    }

    fn i32_at(file: &[u8], at: usize) -> usize {
        usize::try_from(read_u32(file, at).expect("a length")).expect("a size")
    }

    #[test]
    fn a_file_is_the_magic_two_messages_the_end_mark_and_a_footer() {
        let rows = [(1_i64, "a"), (-2, "bcd"), (i64::MAX, "")];
        let file = write(&PAIRS, rows.len(), &pairs(&rows));
        assert_eq!(&file[..8], b"ARROW1\0\0");
        assert_eq!(&file[file.len() - 6..], b"ARROW1");

        // The schema message, with no body.
        assert_eq!(&file[8..12], [0xff; 4]);
        let schema_len = i32_at(&file, 12);
        assert_eq!(schema_len % 8, 0);

        // The record batch message, and its body: a buffer of three
        // 64-bit values, one of four offsets, and one of four bytes of
        // text, each padded to eight bytes.
        let batch_at = 16 + schema_len;
        assert_eq!(&file[batch_at..batch_at + 4], [0xff; 4]);
        let batch_len = i32_at(&file, batch_at + 4);
        let body = batch_at + 8 + batch_len;
        assert_eq!(body % 8, 0);
        let mut expected = Vec::new();
        for (at, _) in rows {
            expected.extend(at.to_le_bytes());
        }
        for offset in [0_i32, 1, 4, 4] {
            expected.extend(offset.to_le_bytes());
        }
        expected.extend(b"abcd\0\0\0\0");
        assert_eq!(&file[body..body + expected.len()], expected.as_slice());

        // The end of the stream, then the footer and its length.
        let end = body + expected.len();
        assert_eq!(&file[end..end + 8], [0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]);
        let footer_len = i32_at(&file, file.len() - 10);
        assert_eq!(end + 8 + footer_len + 10, file.len());

        // The footer's one block names the record batch.
        let footer = Table::root(&file[end + 8..file.len() - 10]).expect("a footer");
        let block = footer.structs(3, 24).expect("the record batches");
        assert_eq!(block.len(), 24);
        assert_eq!(read_i64(block, 0), i64::try_from(batch_at).ok());
        assert_eq!(read_u32(block, 8), u32::try_from(8 + batch_len).ok());
        assert_eq!(read_i64(block, 16), i64::try_from(expected.len()).ok());
        assert_eq!(footer.structs(2, 24), Some(&[][..]), "no dictionaries");
    }

    #[test]
    fn the_schema_names_the_table_its_format_and_each_column() {
        let file = write(&PAIRS, 0, &pairs(&[]));
        let schema = &file[16..16 + i32_at(&file, 12)];
        let holds = |text: &str| {
            schema
                .windows(text.len())
                .any(|found| found == text.as_bytes())
        };
        for text in [
            "quantforge-result",
            "pairs",
            "quantforge-result-format",
            "at_ms",
            "label",
        ] {
            assert!(holds(text), "{text}");
        }
        let other = Layout {
            name: "other",
            columns: PAIRS.columns,
        };
        assert_eq!(
            read(&other, &file).expect_err("another table's file"),
            "it does not start with the other schema"
        );
    }

    #[test]
    fn a_text_column_stops_at_what_its_offsets_can_count() {
        let mut text = Text::default();
        assert!(text.push_within("abc", 4));
        assert!(text.push_within("d", 4));
        assert!(!text.push_within("e", 4), "a fifth byte");
        assert!(text.push_within("", 4));
        assert_eq!(
            (text.get(0), text.get(1), text.get(2), text.get(3)),
            (Some("abc"), Some("d"), Some(""), None)
        );
        assert_eq!(MOST_TEXT, 2_147_483_647);
    }
}
