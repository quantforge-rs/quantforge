# The result spec

**Result format 1.** A backtest job that succeeds produces three files: its
metrics, its closed trades and its equity curve. This page fixes every byte
of each, so that the same result is the same three files on every server,
every store and every release, and a file's SHA-256 names what the job
produced. `quantforge::protocol::results` is the reference implementation
and the contract a client decodes against;
`tests/results/docs.rs` holds this page to it.

<!-- begin: result format -->
1
<!-- end: result format -->

## Contents

- [The files](#the-files)
- [metrics.json](#metricsjson)
- [The tables](#the-tables)
- [Values](#values)
- [An Arrow file](#an-arrow-file)
- [The body](#the-body)
- [The metadata](#the-metadata)
- [A worked example](#a-worked-example)
- [Reading a file](#reading-a-file)
- [The shared cases](#the-shared-cases)
- [What was checked, and how](#what-was-checked-and-how)

## The files

<!-- begin: files -->
| Result | File | Media type |
| --- | --- | --- |
| `metrics` | `metrics.json` | `application/json` |
| `trades` | `trades.arrow` | `application/vnd.apache.arrow.file` |
| `equity` | `equity.arrow` | `application/vnd.apache.arrow.file` |
<!-- end: files -->

A job lists them in that order, each with the SHA-256 of its bytes, written
`sha256:` and 64 lowercase hex digits, and its size. A server answers each
at `GET /v1/backtests/{job_id}/result/<file>`; the routes are specified
with the job surface, in `quantforge::protocol::jobs`.

## metrics.json

One JSON object in UTF-8, with these fields in this order and no
whitespace anywhere:

<!-- begin: metrics fields -->
- `initial_cash`: a decimal
- `final_equity`: a decimal
- `total_return_pct`: a decimal
- `max_drawdown_pct`: a decimal
- `trade_count`: an integer
<!-- end: metrics fields -->

A decimal is a JSON string holding the text [below](#values); `trade_count`
is a JSON number without sign, fraction or exponent, the number of rows of
the `trades` table. Amounts are in the quote asset. `final_equity` is cash
plus any position still open, valued at the last bar's close, after a
close-out at the end and its fee; `total_return_pct` is
`(final_equity - initial_cash) / initial_cash` in percent; and
`max_drawdown_pct` is the largest fall of the equity curve from an earlier
peak, in percent.

```json
{"initial_cash":"10010","final_equity":"10999.989","total_return_pct":"9.89","max_drawdown_pct":"10","trade_count":2}
```

## The tables

`trades.arrow` holds one row per closed trade, in the order the trades
closed:

<!-- begin: trades columns -->
- `symbol`: `Utf8`
- `entry_time_ms`: `Int64`
- `exit_time_ms`: `Int64`
- `entry_price`: `Utf8`
- `exit_price`: `Utf8`
- `qty`: `Utf8`
- `gross_quote_pnl`: `Utf8`
<!-- end: trades columns -->

`symbol` is the exchange's symbol in uppercase. `entry_time_ms` and
`exit_time_ms` are the open times of the bars the entry and the exit filled
at; a position closed out at the end exits at the last bar's close time.
`entry_price`, `exit_price`, `qty` and `gross_quote_pnl` are decimals:
the two fill prices, the quantity in the base asset, and the change in cash
from before the entry to after the exit in the quote asset, with both fees
already deducted.

`equity.arrow` holds one row per bar the job ran on, in open-time order:

<!-- begin: equity columns -->
- `open_time_ms`: `Int64`
- `equity`: `Utf8`
<!-- end: equity columns -->

`open_time_ms` is the bar's open time, which names the bar, and `equity` a
decimal: cash plus the position valued at that bar's close, after that
bar's fill. It is the series `max_drawdown_pct` is measured on. It does not
include the fee of a close-out at the end, which `final_equity` does, so the
last row can lie above the final equity by that fee.

`Int64` is Arrow's `Int` of 64 bits, signed; `Utf8` is Arrow's `Utf8`, with
32-bit offsets. No column is nullable, and no value is null.

<!-- begin: limits -->
- rows per table: 16777216
- text per column, in bytes: 2147483647
<!-- end: limits -->

## Values

- **A time** is a count of milliseconds since the Unix epoch, UTC. It is a
  plain 64-bit integer, not an Arrow timestamp.
- **A decimal** is its exact text: an optional minus sign, the integer
  digits without leading zeros, and an optional fraction,
  `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, with no minus sign on zero, no exponent,
  and at most 28 decimal places and 96 bits of digits. Trailing zeros are
  part of the value as the engine computed it and are kept: `10012.50` and
  `10012.5` are different files. It is never an Arrow decimal, which would
  fix one scale for a whole column, and never a float.
- **Text** is UTF-8. A symbol may hold any characters an exchange lists.

In Python, `decimal.Decimal(text)` reads a decimal exactly and
`float(text)` gives the nearest float64, as the
[decimal and float64 policy](decimal-float-policy.md) has it.

## An Arrow file

A table is one Arrow IPC file (the file format, also called Feather V2),
written little-endian. Every length and offset below is in bytes, and
every multi-byte integer is little-endian.

| Part | Bytes |
| --- | --- |
| Magic | `ARROW1`, then two zero bytes |
| Schema message | `ff ff ff ff`; the metadata's length as a 32-bit integer; the metadata, padded with zeros to a multiple of 8 |
| Record batch message | The same three, then the [body](#the-body) |
| End of stream | `ff ff ff ff 00 00 00 00` |
| Footer | The footer's metadata, as it is |
| Footer length | A 32-bit integer |
| Magic | `ARROW1` |

There is exactly one record batch, even for a table without rows. Nothing
is compressed, there are no dictionaries, and nothing follows the last
magic. A metadata length counts the padding.

## The body

The record batch's buffers, one after another in column order. Each buffer
starts at a multiple of 8 from the start of the body and is padded with
zeros to the next one; its recorded length leaves the padding out.

- Every column starts with its validity bitmap, which is empty, since
  nothing is null: a buffer of length 0 at the offset where the next one
  starts.
- An `Int64` column then has one buffer: each value as 8 bytes.
- A `Utf8` column then has two: the offsets, `rows + 1` 32-bit integers
  starting at 0, each the end of one value in the text; and the text, every
  value's bytes one after another. A table without rows still has the one
  offset, 0.

## The metadata

The three pieces of metadata are FlatBuffers, of the tables Arrow's
`Schema.fbs`, `Message.fbs` and `File.fbs` define. Each is laid out as the
reference FlatBuffers builder lays it out when it is given the calls below,
in this order, without forced defaults: so a field whose value is its
default is left out, and equal vtables are written once. Any FlatBuffers
library reproduces the bytes by making the same calls; the worked example
shows them.

**The schema**, which the schema message and the footer both hold:

1. For each column, in order: the string of its name; its type table, an
   `Int` with `bitWidth` 64 and then `is_signed` true, or an empty `Utf8`;
   an empty `children` vector; then the `Field`, adding `children`, `type`,
   `name` and `type_type`, in that order. `nullable` is false, the default.
2. The vector of the fields.
3. Two `KeyValue` tables: `quantforge-result` with the table's name,
   `trades` or `equity`, then `quantforge-result-format` with the format's
   number in decimal digits. For each: the key's string, the value's
   string, then the table, adding `value` and then `key`.
4. The vector of the two.
5. The `Schema`, adding `custom_metadata` and then `fields`. `endianness`
   is little, the default.

**A message**: its header table first, then the `Message`, adding
`bodyLength` when it is not 0, `header`, `version` (`V5`), and
`header_type`. The schema message's header is the schema, and its body
length 0.

**The record batch**, the second message's header: the `nodes` vector, one
`FieldNode` per column with the row count and a null count of 0; the
`buffers` vector, one `Buffer` per buffer of the body with its offset and
length; then the `RecordBatch`, adding `length` when the table has rows,
`buffers`, and `nodes`.

**The footer**: the schema, as above; an empty `dictionaries` vector; the
`recordBatches` vector with one `Block`: the offset of the record batch
message in the file, the length of its metadata with the 8 bytes before
it and its padding, and the body's length; then the `Footer`, adding
`recordBatches`, `dictionaries`, `schema`, and `version` (`V5`).

## A worked example

Two bars, the second with two decimal places:

| `open_time_ms` | `equity` |
| --- | --- |
| 1792454400000 | `10000` |
| 1792454460000 | `10012.50` |

The file is 954 bytes, named
`sha256:8ae7aa16fdebaa2e348dbcf9229df71c8ddaf7968349accea90683e66c729f4f`.
Bytes 0 to 7 are the magic; 8 to 327 the schema message, whose metadata is
312 bytes; 328 to 535 the record batch message's 8 bytes and 200 bytes of
metadata; 536 to 583 the body; 584 to 591 the end of the stream; 592 to 943
the footer, 352 bytes; 944 to 947 its length; and 948 to 953 the magic.

The body is three buffers after two empty bitmaps: the two times at 0, the
offsets 0, 5 and 13 at 16, and the 13 bytes of `1000010012.50` at 32:

```text
536  00 98 9b 56 a1 01 00 00 60 82 9c 56 a1 01 00 00
552  00 00 00 00 05 00 00 00 0d 00 00 00 00 00 00 00
568  31 30 30 30 30 31 30 30 31 32 2e 35 30 00 00 00
```

The whole file, as hex:

<!-- begin: worked example -->
```text
4152524f57310000ffffffff380100001000000000000a000e000600050008000a00000000010400
1000000000000a000c000000040008000a0000008000000004000000020000004400000004000000
ccffffff10000000040000000100000031000000180000007175616e74666f7267652d726573756c
742d666f726d61740000000008000c00040008000800000014000000040000000600000065717569
74790000110000007175616e74666f7267652d726573756c74000000020000004400000004000000
d4ffffff000000051800000010000000040000000000000004000400040000000600000065717569
74790000100014000800000007000c00000010001000000000000002240000001400000004000000
0000000008000c00080007000800000000000001400000000c0000006f70656e5f74696d655f6d73
0000000000000000ffffffffc800000014000000000000000c0016000600050008000c000c000000
0003040018000000300000000000000000000a0018000c00040008000a0000006c00000010000000
02000000000000000000000005000000000000000000000000000000000000000000000000000000
10000000000000001000000000000000000000000000000010000000000000000c00000000000000
20000000000000000d00000000000000000000000200000002000000000000000000000000000000
0200000000000000000000000000000000989b56a101000060829c56a10100000000000005000000
0d00000000000000313030303031303031322e3530000000ffffffff00000000100000000c001400
060008000c0010000c000000000004003c0000002800000004000000010000004801000000000000
d0000000000000003000000000000000000000000000000000000a000c000000040008000a000000
8000000004000000020000004400000004000000ccffffff10000000040000000100000031000000
180000007175616e74666f7267652d726573756c742d666f726d61740000000008000c0004000800
080000001400000004000000060000006571756974790000110000007175616e74666f7267652d72
6573756c74000000020000004400000004000000d4ffffff00000005180000001000000004000000
000000000400040004000000060000006571756974790000100014000800000007000c0000001000
10000000000000022400000014000000040000000000000008000c00080007000800000000000001
400000000c0000006f70656e5f74696d655f6d7300000000600100004152524f5731
```
<!-- end: worked example -->

## Reading a file

Any Arrow library reads a table as it is: `pyarrow.ipc.open_file`,
`pyarrow.feather.read_table`, `pandas.read_feather`, Polars' `read_ipc`,
arrow-rs's `FileReader`. A reader should check the bytes it fetched against
the file's hash and size first.

A reader that must know a file is exactly a result of this format, such as
`decode_trades` and `decode_equity`, reads the rows, writes them again as
this page says, and refuses a file that is not byte for byte what it wrote.
`Metrics::decode` does the same for `metrics.json`.

The format's number is in every table's schema, under
`quantforge-result-format`, and in a server's answer for the job. A change
to any byte this page fixes raises it.

## The shared cases

[`tests/fixtures/result_cases.json`](../tests/fixtures/result_cases.json)
lists, for each case, a metrics object or a table's rows, and the file they
are written as, as hex, with its hash and size; one long curve is given by
its rule and its hash alone. An implementation conforms when it writes every
case's file byte for byte and reads it back to the rows. The files were
written by
[`spikes/result-format/result_cases.py`](../spikes/result-format/result_cases.py),
a standard-library Python implementation of this page with a FlatBuffers
builder of its own, never by the Rust code under test, and
`tests/results/cases.rs` holds the Rust code to them.

## What was checked, and how

On 2026-11-11, with every listed file of the shared cases:

- **pyarrow 26.0.1** (Arrow C++) read each through `ipc.open_file`,
  `feather.read_table` and, for the stream inside the file,
  `ipc.open_stream`: one record batch, the columns and types above, none
  nullable, the two metadata entries, full validation, and the rows as
  listed (`spikes/result-format/verify_pyarrow.py`).
- **arrow-rs 60.1.0** read each through `FileReader`, with the same checks
  (`spikes/result-format/tests/arrow_rs.rs`).
- **The reference FlatBuffers builder**, the `flatbuffers` crate 25.12.19
  driven through arrow-rs's generated builders with the calls
  [above](#the-metadata), wrote metadata equal byte for byte to each file's
  schema message, record batch message and footer
  (`spikes/result-format/tests/reference_builder.rs`).

The spike is a standalone project that CI does not build; QuantForge itself
depends on no Arrow or FlatBuffers library. Run it again after any change
to the cases.
