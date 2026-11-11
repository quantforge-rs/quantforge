//! The metadata of every Arrow file of `tests/fixtures/result_cases.json`,
//! built again with the reference FlatBuffers builder (the `flatbuffers`
//! crate) through arrow-rs's generated code, making the calls the result
//! spec lists in the order it lists them: the bytes must be the file's.

use arrow_ipc::{
    Block, Buffer, Field, FieldBuilder, FieldNode, FooterBuilder, IntBuilder, KeyValueBuilder,
    MessageBuilder, MessageHeader, MetadataVersion, RecordBatchBuilder, Schema, SchemaBuilder,
    Type, Utf8Builder,
};
use flatbuffers::{FlatBufferBuilder, ForwardsUOffset, WIPOffset};
use serde_json::Value;

const CASES: &str = include_str!("../../../tests/fixtures/result_cases.json");

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

/// A table's columns: each one's name, and whether it holds 64-bit
/// integers rather than text.
fn columns(table: &str) -> Vec<(&'static str, bool)> {
    match table {
        "trades" => vec![
            ("symbol", false),
            ("entry_time_ms", true),
            ("exit_time_ms", true),
            ("entry_price", false),
            ("exit_price", false),
            ("qty", false),
            ("gross_quote_pnl", false),
        ],
        "equity" => vec![("open_time_ms", true), ("equity", false)],
        other => panic!("no table {other}"),
    }
}

fn schema<'a>(fbb: &mut FlatBufferBuilder<'a>, table: &str) -> WIPOffset<Schema<'a>> {
    let mut fields = Vec::new();
    for (name, integer) in columns(table) {
        let name = fbb.create_string(name);
        let (tag, of) = if integer {
            let mut int = IntBuilder::new(fbb);
            int.add_bitWidth(64);
            int.add_is_signed(true);
            (Type::Int, int.finish().as_union_value())
        } else {
            (Type::Utf8, Utf8Builder::new(fbb).finish().as_union_value())
        };
        let children = fbb.create_vector::<ForwardsUOffset<Field>>(&[]);
        let mut field = FieldBuilder::new(fbb);
        field.add_children(children);
        field.add_type_(of);
        field.add_name(name);
        field.add_type_type(tag);
        fields.push(field.finish());
    }
    let fields = fbb.create_vector(&fields);
    let mut notes = Vec::new();
    for (key, value) in [("quantforge-result", table), ("quantforge-result-format", "1")] {
        let key = fbb.create_string(key);
        let value = fbb.create_string(value);
        let mut note = KeyValueBuilder::new(fbb);
        note.add_value(value);
        note.add_key(key);
        notes.push(note.finish());
    }
    let notes = fbb.create_vector(&notes);
    let mut schema = SchemaBuilder::new(fbb);
    schema.add_custom_metadata(notes);
    schema.add_fields(fields);
    schema.finish()
}

fn schema_message(table: &str) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let header = schema(&mut fbb, table).as_union_value();
    let mut message = MessageBuilder::new(&mut fbb);
    message.add_header(header);
    message.add_version(MetadataVersion::V5);
    message.add_header_type(MessageHeader::Schema);
    let root = message.finish();
    fbb.finish(root, None);
    fbb.finished_data().to_vec()
}

fn batch_message(rows: i64, columns: usize, buffers: &[(i64, i64)], body_len: i64) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let nodes: Vec<FieldNode> = (0..columns).map(|_| FieldNode::new(rows, 0)).collect();
    let nodes = fbb.create_vector(&nodes);
    let buffers: Vec<Buffer> = buffers
        .iter()
        .map(|&(offset, length)| Buffer::new(offset, length))
        .collect();
    let buffers = fbb.create_vector(&buffers);
    let mut batch = RecordBatchBuilder::new(&mut fbb);
    batch.add_length(rows);
    batch.add_buffers(buffers);
    batch.add_nodes(nodes);
    let header = batch.finish().as_union_value();
    let mut message = MessageBuilder::new(&mut fbb);
    message.add_bodyLength(body_len);
    message.add_header(header);
    message.add_version(MetadataVersion::V5);
    message.add_header_type(MessageHeader::RecordBatch);
    let root = message.finish();
    fbb.finish(root, None);
    fbb.finished_data().to_vec()
}

fn footer(table: &str, block: Block) -> Vec<u8> {
    let mut fbb = FlatBufferBuilder::new();
    let schema = schema(&mut fbb, table);
    let dictionaries = fbb.create_vector::<Block>(&[]);
    let batches = fbb.create_vector(&[block]);
    let mut footer = FooterBuilder::new(&mut fbb);
    footer.add_recordBatches(batches);
    footer.add_dictionaries(dictionaries);
    footer.add_schema(schema);
    footer.add_version(MetadataVersion::V5);
    let root = footer.finish();
    fbb.finish(root, None);
    fbb.finished_data().to_vec()
}

fn length_at(file: &[u8], at: usize) -> usize {
    let bytes: [u8; 4] = file[at..at + 4].try_into().expect("four bytes");
    usize::try_from(i32::from_le_bytes(bytes)).expect("a length")
}

/// `metadata` as a file holds it: padded with zeros to eight bytes.
fn padded(mut metadata: Vec<u8>) -> Vec<u8> {
    metadata.resize(metadata.len().next_multiple_of(8), 0);
    metadata
}

#[test]
fn the_reference_builder_writes_every_files_metadata() {
    let fixture: Value = serde_json::from_str(CASES).expect("the fixture");
    let mut checked = 0;
    for case in fixture["cases"].as_array().expect("cases") {
        let (Some(table), Some(file_hex)) = (case["result"].as_str(), case["file_hex"].as_str())
        else {
            continue;
        };
        if table == "metrics" {
            continue;
        }
        let name = case["name"].as_str().expect("a name");
        let file = unhex(file_hex);

        let schema_len = length_at(&file, 12);
        assert_eq!(
            &file[16..16 + schema_len],
            padded(schema_message(table)).as_slice(),
            "{name}: the schema message"
        );

        // What the file's own record batch says, read with arrow-rs, and
        // the same message built from it.
        let batch_at = 16 + schema_len;
        let batch_len = length_at(&file, batch_at + 4);
        let metadata = &file[batch_at + 8..batch_at + 8 + batch_len];
        let message = arrow_ipc::root_as_message(metadata).expect("a message");
        let batch = message.header_as_record_batch().expect("a record batch");
        let buffers: Vec<(i64, i64)> = batch
            .buffers()
            .expect("buffers")
            .iter()
            .map(|buffer| (buffer.offset(), buffer.length()))
            .collect();
        let rebuilt = batch_message(
            batch.length(),
            columns(table).len(),
            &buffers,
            message.bodyLength(),
        );
        assert_eq!(metadata, padded(rebuilt).as_slice(), "{name}: the record batch");

        let footer_len = length_at(&file, file.len() - 10);
        let stored = &file[file.len() - 10 - footer_len..file.len() - 10];
        let body_len = message.bodyLength();
        let block = Block::new(
            i64::try_from(batch_at).expect("an offset"),
            i32::try_from(8 + batch_len).expect("a length"),
            body_len,
        );
        assert_eq!(stored, footer(table, block).as_slice(), "{name}: the footer");
        checked += 1;
    }
    assert!(checked >= 7, "{checked} files");
}
