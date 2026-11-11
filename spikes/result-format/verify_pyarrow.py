#!/usr/bin/env python3
"""Reads every Arrow file of tests/fixtures/result_cases.json with pyarrow,
the reference C++ implementation's bindings, and holds each to its case: one
record batch, the spec's columns and types, none nullable, the schema's two
metadata entries, and the rows, every decimal as its exact text.

Run on demand, from the repository root, wherever pyarrow can be imported:

    python3 spikes/result-format/verify_pyarrow.py [--site <directory holding pyarrow>]

It is not part of the test suite: the suite holds the Rust code to the
fixture's bytes, and this holds those bytes to Arrow.
"""

import hashlib
import json
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
if "--site" in sys.argv:
    sys.path.insert(0, sys.argv[sys.argv.index("--site") + 1])

import pyarrow as pa  # noqa: E402
import pyarrow.feather  # noqa: E402
import pyarrow.ipc  # noqa: E402

import result_cases  # noqa: E402

TYPES = {result_cases.INT64: pa.int64(), result_cases.UTF8: pa.string()}


def check(name, table_name, raw, rows):
    reader = pa.ipc.open_file(pa.BufferReader(raw))
    assert reader.num_record_batches == 1, name
    schema = reader.schema
    expected = result_cases.TABLES[table_name]
    assert schema.names == [column for column, _ in expected], (name, schema.names)
    for field, (_, kind) in zip(schema, expected):
        assert field.type == TYPES[kind], (name, field)
        assert not field.nullable, (name, field)
    assert schema.metadata == {
        b"quantforge-result": table_name.encode(),
        b"quantforge-result-format": str(result_cases.FORMAT).encode(),
    }, (name, schema.metadata)
    table = reader.read_all()
    table.validate(full=True)
    assert table.num_rows == len(rows), name
    assert table.to_pylist() == rows, name
    for column in table.columns:
        assert column.null_count == 0, name

    # The same file through the Feather reader, and the stream inside it
    # through the stream reader.
    assert pa.feather.read_table(pa.BufferReader(raw)).to_pylist() == rows, name
    stream = pa.ipc.open_stream(pa.BufferReader(raw[8:]))
    assert stream.read_all().to_pylist() == rows, name


def main():
    root = HERE.parents[1]
    fixture = json.loads((root / "tests" / "fixtures" / "result_cases.json").read_text("utf-8"))
    checked = 0
    for case in fixture["cases"]:
        if case["result"] == "metrics":
            continue
        if "series" in case:
            rows = result_cases.series(case["series"])
            raw = result_cases.arrow_file(case["result"], rows)
        else:
            rows = case["rows"]
            raw = bytes.fromhex(case["file_hex"])
        assert "sha256:" + hashlib.sha256(raw).hexdigest() == case["hash"], case["name"]
        assert len(raw) == case["size_bytes"], case["name"]
        check(case["name"], case["result"], raw, rows)
        checked += 1
    print(f"pyarrow {pa.__version__} reads all {checked} Arrow files as their cases list them")


if __name__ == "__main__":
    main()
