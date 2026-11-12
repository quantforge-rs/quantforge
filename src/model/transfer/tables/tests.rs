//! The tables a transfer copies: their headers, their keys and their
//! batches.

use super::*;

#[test]
fn a_header_names_the_format_the_table_and_its_columns() {
    assert_eq!(
        String::from_utf8(SEGMENT_DIRTY.canonical_header()).expect("utf-8"),
        "quantforge transfer 1\nsegment_dirty\nid,exchange,symbol,interval,window_start_ms\n"
    );
}

#[test]
fn every_table_is_keyed_by_one_of_its_columns_and_a_market_table_starts_with_its_market() {
    for table in TRANSFER_TABLES {
        assert!(table.column_index(table.key).is_some(), "{}", table.name);
        if table.numbered {
            assert_eq!(table.key_kind(), ColumnKind::Integer, "{}", table.name);
        }
        if table.by_market {
            let names: Vec<&str> = table.columns[..3].iter().map(|c| c.name).collect();
            assert_eq!(names, ["exchange", "symbol", "interval"], "{}", table.name);
            assert_eq!(table.key_kind(), ColumnKind::Integer, "{}", table.name);
        }
    }
}

#[test]
fn a_batch_of_large_rows_is_capped_and_never_empty() {
    assert_eq!(SEGMENTS.batch_rows(5000), 64);
    assert_eq!(SEGMENTS.batch_rows(10), 10);
    assert_eq!(BUNDLES.batch_rows(5000), 2);
    assert_eq!(JOB_RESULTS.batch_rows(5000), 2);
    assert_eq!(JOB_LOGS.batch_rows(5000), 4);
    assert_eq!(CANDLES.batch_rows(5000), 5000);
    assert_eq!(CANDLES.batch_rows(0), 1);
}
