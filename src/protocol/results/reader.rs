//! Reading a result table's file back: its one record batch found after
//! the schema its table's files start with, and each column taken from the
//! buffers the batch names. Nothing here checks more than it needs to read
//! safely: the caller writes the rows again and refuses a file that is not
//! byte for byte what it wrote.

use super::MAX_ROWS;
use super::arrow::{
    BATCH_MESSAGE, Column, ColumnType, Layout, MAGIC, Text, encapsulate, schema_message,
};
use super::flatbuf::{Table, read_i64, read_u32};

/// The rows and columns of a file that starts as the layout's files do.
pub(super) fn read(layout: &Layout, file: &[u8]) -> Result<(usize, Vec<Column>), String> {
    let mut start = Vec::with_capacity(512);
    start.extend(MAGIC);
    start.extend([0, 0]);
    encapsulate(&mut start, &schema_message(layout));
    if !file.starts_with(&start) {
        return Err(format!("it does not start with the {} schema", layout.name));
    }
    let at = start.len();
    let cut_short = || "it ends before its record batch does".to_string();
    let metadata_len = read_u32(file, at + 4)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(cut_short)?;
    let metadata_end = (at + 8).checked_add(metadata_len).ok_or_else(cut_short)?;
    let metadata = file.get(at + 8..metadata_end).ok_or_else(cut_short)?;
    let unreadable = || "its record batch cannot be read".to_string();
    let message = Table::root(metadata).ok_or_else(unreadable)?;
    if message.byte(1) != Some(BATCH_MESSAGE) {
        return Err("its second message is not a record batch".to_string());
    }
    let batch = message.table(2).ok_or_else(unreadable)?;
    let rows = batch.long(0).ok_or_else(unreadable)?;
    let rows = usize::try_from(rows)
        .ok()
        .filter(|rows| *rows <= MAX_ROWS)
        .ok_or_else(|| format!("it holds {rows} rows, and a result at most {MAX_ROWS}"))?;
    let body_len = message
        .long(3)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or_else(unreadable)?;
    let body = metadata_end
        .checked_add(body_len)
        .and_then(|end| file.get(metadata_end..end))
        .ok_or_else(cut_short)?;
    let mut buffers = batch
        .structs(2, 16)
        .ok_or_else(unreadable)?
        .chunks_exact(16)
        .map(|buffer| {
            let offset = usize::try_from(read_i64(buffer, 0)?).ok()?;
            let length = usize::try_from(read_i64(buffer, 8)?).ok()?;
            body.get(offset..offset.checked_add(length)?)
        });
    let mut next = || buffers.next().flatten().ok_or_else(unreadable);
    let mut columns = Vec::with_capacity(layout.columns.len());
    for (name, kind) in layout.columns {
        let wrong = || format!("its {name} column does not hold {rows} values");
        next()?;
        columns.push(match kind {
            ColumnType::Int64 => {
                let values = next()?;
                if values.len() != 8 * rows {
                    return Err(wrong());
                }
                Column::Int64(
                    values
                        .chunks_exact(8)
                        .filter_map(|value| read_i64(value, 0))
                        .collect(),
                )
            }
            ColumnType::Utf8 => {
                let (offsets, text) = (next()?, next()?);
                if offsets.len() != 4 * (rows + 1) {
                    return Err(wrong());
                }
                Column::Utf8(read_text(offsets, text).ok_or_else(wrong)?)
            }
        });
    }
    Ok((rows, columns))
}

/// A text column from its offsets and its text, when the text is UTF-8 and
/// the offsets cut it into values from its start to its end.
fn read_text(offsets: &[u8], text: &[u8]) -> Option<Text> {
    let text = std::str::from_utf8(text).ok()?;
    let mut offsets = offsets
        .chunks_exact(4)
        .filter_map(|offset| read_u32(offset, 0))
        .map(|offset| offset as i32);
    let mut last = offsets.next().filter(|first| *first == 0)?;
    let ends: Vec<i32> = offsets.collect();
    for &end in &ends {
        if end < last || !text.is_char_boundary(usize::try_from(end).ok()?) {
            return None;
        }
        last = end;
    }
    (usize::try_from(last).ok()? == text.len()).then(|| Text {
        text: text.to_string(),
        ends,
    })
}

#[cfg(test)]
mod tests {
    use super::super::arrow::write;
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

    #[test]
    fn what_was_written_reads_back() {
        let long = "x".repeat(9);
        let tables: [&[(i64, &str)]; 3] = [
            &[],
            &[(0, "")],
            &[(i64::MIN, "é"), (7, long.as_str()), (-1, "")],
        ];
        for rows in tables {
            let file = write(&PAIRS, rows.len(), &pairs(rows));
            let (count, columns) = read(&PAIRS, &file).expect("a file");
            assert_eq!(count, rows.len());
            let [Column::Int64(times), Column::Utf8(labels)] = columns.as_slice() else {
                panic!("two columns, not {columns:?}");
            };
            assert_eq!(*times, rows.iter().map(|(at, _)| *at).collect::<Vec<_>>());
            for (row, (_, label)) in rows.iter().enumerate() {
                assert_eq!(labels.get(row), Some(*label));
            }
            assert_eq!(labels.get(rows.len()), None);
        }
    }

    // A file's own row count is checked against the most a table holds
    // before anything is sized by it. The count is written three times in a
    // table of two columns, in the batch and in each column's node, and
    // seven rows make it a number no offset, length or value here is.
    #[test]
    fn a_file_that_says_it_holds_more_rows_than_a_table_holds_is_refused() {
        let rows: Vec<(i64, &str)> = (100..107).map(|at| (at, "abc")).collect();
        let file = write(&PAIRS, rows.len(), &pairs(&rows));
        let count = 7_i64.to_le_bytes();
        let places: Vec<usize> = (0..file.len() - 8)
            .filter(|at| file[*at..*at + 8] == count)
            .collect();
        assert_eq!(places.len(), 3, "the batch's length and two nodes");
        let claim = |claimed: usize| {
            let mut altered = file.clone();
            for at in &places {
                altered[*at..*at + 8].copy_from_slice(&(claimed as i64).to_le_bytes());
            }
            read(&PAIRS, &altered)
        };
        assert_eq!(
            claim(MAX_ROWS + 1).expect_err("more than a table holds"),
            "it holds 16777217 rows, and a result at most 16777216"
        );
        // The most a table holds passes that check, and fails the next: the
        // file has seven values for each column.
        assert_eq!(
            claim(MAX_ROWS).expect_err("seven values"),
            "its at_ms column does not hold 16777216 values"
        );
        assert_eq!(MAX_ROWS, 16_777_216);
    }

    #[test]
    fn a_file_cut_short_or_altered_is_refused_or_reads_as_other_rows_without_a_panic() {
        let rows = [(1_i64, "ab"), (2, "c")];
        let file = write(&PAIRS, rows.len(), &pairs(&rows));
        for cut in 0..file.len() {
            // A cut may fall after everything a read looks at.
            let _ = read(&PAIRS, &file[..cut]);
        }
        assert!(read(&PAIRS, &file[..file.len() / 2]).is_err());
        for at in 0..file.len() {
            for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut altered = file.clone();
                altered[at] = value;
                let _ = read(&PAIRS, &altered);
            }
        }
    }

    #[test]
    fn text_whose_offsets_do_not_cut_it_into_values_is_refused() {
        let offsets = |values: &[i32]| -> Vec<u8> {
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect()
        };
        assert!(read_text(&offsets(&[0, 1, 3]), b"abc").is_some());
        assert!(read_text(&offsets(&[1, 3]), b"abc").is_none(), "not from 0");
        assert!(
            read_text(&offsets(&[0, 2, 1, 3]), b"abc").is_none(),
            "falling"
        );
        assert!(read_text(&offsets(&[0, 2]), b"abc").is_none(), "short");
        assert!(read_text(&offsets(&[0, 4]), b"abc").is_none(), "long");
        assert!(
            read_text(&offsets(&[0, -1, 3]), b"abc").is_none(),
            "negative"
        );
        assert!(
            read_text(&offsets(&[0, 1, 2]), "é".as_bytes()).is_none(),
            "inside a character"
        );
        assert!(read_text(&offsets(&[0, 1]), &[0xff]).is_none(), "not UTF-8");
    }
}
