//! Every Arrow file of `tests/fixtures/result_cases.json`, read with
//! arrow-rs and held to its case.

use std::collections::HashMap;
use std::io::Cursor;

use arrow_array::{Array, Int64Array, StringArray};
use arrow_ipc::reader::FileReader;
use arrow_schema::DataType;
use serde_json::Value;

const CASES: &str = include_str!("../../../tests/fixtures/result_cases.json");

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

/// The fixture's Arrow cases that list their files: name, table, file, rows.
pub fn listed() -> Vec<(String, String, Vec<u8>, Vec<Value>)> {
    let fixture: Value = serde_json::from_str(CASES).expect("the fixture");
    fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["result"] != "metrics" && case.get("file_hex").is_some())
        .map(|case| {
            (
                case["name"].as_str().expect("a name").to_string(),
                case["result"].as_str().expect("a result").to_string(),
                unhex(case["file_hex"].as_str().expect("hex")),
                case["rows"].as_array().expect("rows").clone(),
            )
        })
        .collect()
}

#[test]
fn arrow_rs_reads_every_file_as_its_case_lists_it() {
    let cases = listed();
    assert!(cases.len() >= 7, "{} cases", cases.len());
    for (name, table, file, rows) in cases {
        let reader = FileReader::try_new(Cursor::new(file), None).expect(&name);
        let schema = reader.schema();
        assert_eq!(
            schema.metadata(),
            &HashMap::from([
                ("quantforge-result".to_string(), table.clone()),
                ("quantforge-result-format".to_string(), "1".to_string()),
            ]),
            "{name}"
        );
        assert_eq!(reader.num_batches(), 1, "{name}");
        let batches: Vec<_> = reader.map(|batch| batch.expect("a batch")).collect();
        let [batch] = batches.as_slice() else {
            panic!("{name}: one batch");
        };
        assert_eq!(batch.num_rows(), rows.len(), "{name}");
        for (field, column) in schema.fields().iter().zip(batch.columns()) {
            assert!(!field.is_nullable(), "{name}: {field}");
            assert_eq!(column.null_count(), 0, "{name}: {field}");
            let read: Vec<Value> = match field.data_type() {
                DataType::Int64 => {
                    let values = column.as_any().downcast_ref::<Int64Array>().expect("int64");
                    values.values().iter().map(|value| Value::from(*value)).collect()
                }
                DataType::Utf8 => {
                    let values = column.as_any().downcast_ref::<StringArray>().expect("utf8");
                    (0..values.len()).map(|row| Value::from(values.value(row))).collect()
                }
                other => panic!("{name}: {field} is {other}"),
            };
            let listed: Vec<Value> = rows.iter().map(|row| row[field.name()].clone()).collect();
            assert_eq!(read, listed, "{name}: {field}");
        }
        let columns: Vec<&str> = schema.fields().iter().map(|field| field.name().as_str()).collect();
        let expected: Vec<&str> = match table.as_str() {
            "trades" => vec![
                "symbol",
                "entry_time_ms",
                "exit_time_ms",
                "entry_price",
                "exit_price",
                "qty",
                "gross_quote_pnl",
            ],
            _ => vec!["open_time_ms", "equity"],
        };
        assert_eq!(columns, expected, "{name}");
    }
}
