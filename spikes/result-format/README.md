# Result format spike

The evidence for [the result spec](../../docs/result-spec.md): the files a
backtest job's trades and equity curve are written as are Arrow IPC files
that Arrow's own libraries read, and their bytes are what the reference
FlatBuffers builder writes. QuantForge writes those files with a few hundred
lines of its own and depends on no Arrow or FlatBuffers library; this is
where the two are held against each other. A standalone Cargo project and
two Python scripts, outside the published crate and outside CI.

- `result_cases.py`: the shared cases,
  `tests/fixtures/result_cases.json`. Standard library only, with a
  FlatBuffers builder and an Arrow file writer of its own, written from the
  spec, and a reader of its own that reads back every file it wrote. The
  Rust tests hold the Rust code to its bytes.
- `verify_pyarrow.py`: every Arrow file of the cases, read with pyarrow.
- `tests/arrow_rs.rs`: the same files, read with arrow-rs.
- `tests/reference_builder.rs`: each file's schema message, record batch
  message and footer, built again with the `flatbuffers` crate through
  arrow-rs's generated builders, making the calls the spec lists: the bytes
  are the file's.

## Running

From the repository root, after any change to the cases or the spec:

```bash
python3 -I spikes/result-format/result_cases.py
cargo test --locked --test results
python3 spikes/result-format/verify_pyarrow.py
(cd spikes/result-format && cargo test --locked)
```

`verify_pyarrow.py` needs pyarrow; `--site <directory>` names a directory
that holds it when it is not installed for the interpreter. The Cargo
project builds with a current stable Rust, newer than QuantForge's own
minimum: arrow-rs 60 needs Rust 1.88.

## Last run

2026-11-11: pyarrow 26.0.1 on Python 3.14, arrow-rs 60.1.0 and flatbuffers
25.12.19 on Rust 1.99, Linux x86_64. All eight Arrow files of the cases read
as listed, and all seven listed in full matched the reference builder byte
for byte.
