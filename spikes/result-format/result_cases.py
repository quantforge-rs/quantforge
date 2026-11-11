#!/usr/bin/env python3
"""The shared cases of the result spec (docs/result-spec.md), written
independently of the Rust code under test: for each case, a job's metrics, its
trades or its equity curve, and the file they are written as, as hex, with its
hash and size.

Run with ``python3 -I spikes/result-format/result_cases.py`` from the
repository root; it writes tests/fixtures/result_cases.json. Stdlib only. Run
again, it writes the same bytes. Before it writes, it reads every Arrow file
it made back with a reader of its own that follows Arrow's published layout,
not this script's writer.
"""

import hashlib
import json
import pathlib
import struct
import sys

FORMAT = 1
MAX_ROWS = 1 << 24

INT64 = "int64"
UTF8 = "utf8"

TABLES = {
    "trades": [
        ("symbol", UTF8),
        ("entry_time_ms", INT64),
        ("exit_time_ms", INT64),
        ("entry_price", UTF8),
        ("exit_price", UTF8),
        ("qty", UTF8),
        ("gross_quote_pnl", UTF8),
    ],
    "equity": [
        ("open_time_ms", INT64),
        ("equity", UTF8),
    ],
}

METRICS_FIELDS = [
    "initial_cash",
    "final_equity",
    "total_return_pct",
    "max_drawdown_pct",
    "trade_count",
]


class Builder:
    """A FlatBuffers buffer laid out as the reference builder does: from its
    end towards its start. Every offset counts bytes from the end."""

    def __init__(self):
        self.data = b""
        self.widest = 1
        self.fields = []
        self.table_end = 0
        self.vtables = []

    def used(self):
        return len(self.data)

    def align(self, size, more):
        self.widest = max(self.widest, size)
        self.data = bytes(-(self.used() + more) % size) + self.data

    def place(self, raw):
        self.data = raw + self.data

    def offset_to(self, target):
        self.align(4, 0)
        self.place(struct.pack("<I", self.used() - target + 4))

    def string(self, text):
        raw = text.encode("utf-8")
        self.align(4, len(raw) + 1)
        self.place(b"\0")
        self.place(raw)
        self.place(struct.pack("<I", len(raw)))
        return self.used()

    def offsets(self, targets):
        self.align(4, 4 * len(targets))
        for target in reversed(targets):
            self.offset_to(target)
        self.place(struct.pack("<I", len(targets)))
        return self.used()

    def structs(self, width, raw):
        self.align(4, len(raw))
        self.align(8, len(raw))
        self.place(raw)
        self.place(struct.pack("<I", len(raw) // width))
        return self.used()

    def start_table(self, fields):
        self.fields = [0] * fields
        self.table_end = self.used()

    def scalar(self, slot, raw):
        self.align(len(raw), 0)
        self.place(raw)
        self.fields[slot] = self.used()

    def reference(self, slot, target):
        self.offset_to(target)
        self.fields[slot] = self.used()

    def end_table(self):
        self.align(4, 0)
        self.place(bytes(4))
        table = self.used()
        fields = list(self.fields)
        while fields and fields[-1] == 0:
            fields.pop()
        vtable = struct.pack("<HH", 2 * len(fields) + 4, table - self.table_end)
        for field in fields:
            vtable += struct.pack("<H", table - field if field else 0)
        start = next((at for at, raw in self.vtables if raw == vtable), None)
        if start is None:
            self.place(vtable)
            start = self.used()
            self.vtables.append((start, vtable))
        at = len(self.data) - table
        self.data = self.data[:at] + struct.pack("<i", start - table) + self.data[at + 4 :]
        return table

    def finish(self, root):
        self.align(self.widest, 4)
        self.offset_to(root)
        return self.data


def pad8(raw):
    return raw + bytes(-len(raw) % 8)


def schema(builder, table):
    fields = []
    for name, kind in TABLES[table]:
        name_at = builder.string(name)
        if kind == INT64:
            builder.start_table(2)
            builder.scalar(0, struct.pack("<i", 64))
            builder.scalar(1, b"\x01")
            tag, type_at = 2, builder.end_table()
        else:
            builder.start_table(0)
            tag, type_at = 5, builder.end_table()
        children = builder.offsets([])
        builder.start_table(7)
        builder.reference(5, children)
        builder.reference(3, type_at)
        builder.reference(0, name_at)
        builder.scalar(2, bytes([tag]))
        fields.append(builder.end_table())
    fields_at = builder.offsets(fields)
    notes = []
    for key, value in [("quantforge-result", table), ("quantforge-result-format", str(FORMAT))]:
        key_at = builder.string(key)
        value_at = builder.string(value)
        builder.start_table(2)
        builder.reference(1, value_at)
        builder.reference(0, key_at)
        notes.append(builder.end_table())
    notes_at = builder.offsets(notes)
    builder.start_table(4)
    builder.reference(2, notes_at)
    builder.reference(1, fields_at)
    return builder.end_table()


def message(kind, body_len, header):
    builder = Builder()
    header_at = header(builder)
    builder.start_table(5)
    if body_len:
        builder.scalar(3, struct.pack("<q", body_len))
    builder.reference(2, header_at)
    builder.scalar(0, struct.pack("<h", 4))
    builder.scalar(1, bytes([kind]))
    return builder.finish(builder.end_table())


def encapsulated(metadata):
    padded = pad8(metadata)
    return b"\xff\xff\xff\xff" + struct.pack("<i", len(padded)) + padded


def arrow_file(table, rows):
    """The file of `table` holding `rows`, a list of dicts by column."""
    if len(rows) > MAX_ROWS:
        raise ValueError("too many rows")
    body = b""
    buffers = []
    for name, kind in TABLES[table]:
        buffers.append((len(body), 0))
        if kind == INT64:
            raw = b"".join(struct.pack("<q", row[name]) for row in rows)
            buffers.append((len(body), len(raw)))
            body += pad8(raw)
        else:
            texts = [row[name].encode("utf-8") for row in rows]
            ends, total = [0], 0
            for text in texts:
                total += len(text)
                ends.append(total)
            raw = b"".join(struct.pack("<i", end) for end in ends)
            buffers.append((len(body), len(raw)))
            body += pad8(raw)
            raw = b"".join(texts)
            buffers.append((len(body), len(raw)))
            body += pad8(raw)

    def batch(builder):
        nodes = builder.structs(16, struct.pack("<qq", len(rows), 0) * len(TABLES[table]))
        raw = b"".join(struct.pack("<qq", offset, length) for offset, length in buffers)
        buffers_at = builder.structs(16, raw)
        builder.start_table(5)
        if rows:
            builder.scalar(0, struct.pack("<q", len(rows)))
        builder.reference(2, buffers_at)
        builder.reference(1, nodes)
        return builder.end_table()

    out = b"ARROW1\0\0"
    out += encapsulated(message(1, 0, lambda builder: schema(builder, table)))
    batch_at = len(out)
    metadata = encapsulated(message(3, len(body), batch))
    out += metadata + body
    out += b"\xff\xff\xff\xff\0\0\0\0"

    builder = Builder()
    schema_at = schema(builder, table)
    dictionaries = builder.structs(24, b"")
    block = struct.pack("<qi4xq", batch_at, len(metadata), len(body))
    batches = builder.structs(24, block)
    builder.start_table(5)
    builder.reference(3, batches)
    builder.reference(2, dictionaries)
    builder.reference(1, schema_at)
    builder.scalar(0, struct.pack("<h", 4))
    footer = builder.finish(builder.end_table())
    return out + footer + struct.pack("<i", len(footer)) + b"ARROW1"


# A reader that follows Arrow's published layout (format/Schema.fbs,
# Message.fbs and File.fbs, and the IPC file format), sharing nothing with
# the writer above: it finds every field through its table's vtable.


class Table:
    def __init__(self, buffer, at):
        self.buffer = buffer
        self.at = at

    @classmethod
    def root(cls, buffer):
        return cls(buffer, struct.unpack_from("<I", buffer, 0)[0])

    def field(self, slot):
        vtable = self.at - struct.unpack_from("<i", self.buffer, self.at)[0]
        length = struct.unpack_from("<H", self.buffer, vtable)[0]
        entry = 4 + 2 * slot
        if entry + 2 > length:
            return None
        within = struct.unpack_from("<H", self.buffer, vtable + entry)[0]
        return self.at + within if within else None

    def scalar(self, slot, code, default):
        at = self.field(slot)
        return default if at is None else struct.unpack_from(code, self.buffer, at)[0]

    def follow(self, slot):
        at = self.field(slot)
        return None if at is None else at + struct.unpack_from("<I", self.buffer, at)[0]

    def table(self, slot):
        at = self.follow(slot)
        return None if at is None else Table(self.buffer, at)

    def string(self, slot):
        at = self.follow(slot)
        length = struct.unpack_from("<I", self.buffer, at)[0]
        return self.buffer[at + 4 : at + 4 + length].decode("utf-8")

    def vector(self, slot):
        """Where the vector's elements start, and how many it holds."""
        at = self.follow(slot)
        return at + 4, struct.unpack_from("<I", self.buffer, at)[0]

    def tables(self, slot):
        start, count = self.vector(slot)
        found = []
        for index in range(count):
            at = start + 4 * index
            found.append(Table(self.buffer, at + struct.unpack_from("<I", self.buffer, at)[0]))
        return found


def read_schema(schema_table):
    columns = []
    for field in schema_table.tables(1):
        tag = field.scalar(2, "<B", 0)
        if tag == 2:
            kind = field.table(3)
            assert (kind.scalar(0, "<i", 0), kind.scalar(1, "<B", 0)) == (64, 1)
            columns.append((field.string(0), INT64))
        else:
            assert tag == 5, tag
            columns.append((field.string(0), UTF8))
        assert field.scalar(1, "<B", 0) == 0, "nullable"
        assert field.vector(5)[1] == 0, "children"
    notes = {note.string(0): note.string(1) for note in schema_table.tables(2)}
    assert schema_table.scalar(0, "<h", 0) == 0, "little-endian"
    return columns, notes


def read_arrow_file(raw):
    """The table's name and its rows, read as any Arrow reader would."""
    assert raw[:8] == b"ARROW1\0\0" and raw[-6:] == b"ARROW1"
    footer_len = struct.unpack_from("<i", raw, len(raw) - 10)[0]
    footer = Table.root(raw[len(raw) - 10 - footer_len : len(raw) - 10])
    assert footer.scalar(0, "<h", 0) == 4, "metadata version 5"
    columns, notes = read_schema(footer.table(1))
    assert footer.vector(2)[1] == 0, "dictionaries"
    blocks_at, count = footer.vector(3)
    assert count == 1
    buffer = footer.buffer
    batch_at, metadata_len, body_len = struct.unpack_from("<qi4xq", buffer, blocks_at)

    # The stream inside the file: the schema message first, and it is the
    # footer's schema.
    assert raw[8:12] == b"\xff\xff\xff\xff"
    first_len = struct.unpack_from("<i", raw, 12)[0]
    first = Table.root(raw[16 : 16 + first_len])
    assert first.scalar(1, "<B", 0) == 1 and first.scalar(3, "<q", 0) == 0
    assert read_schema(first.table(2)) == (columns, notes)
    assert 16 + first_len == batch_at

    assert raw[batch_at : batch_at + 4] == b"\xff\xff\xff\xff"
    assert struct.unpack_from("<i", raw, batch_at + 4)[0] == metadata_len - 8
    message_table = Table.root(raw[batch_at + 8 : batch_at + metadata_len])
    assert message_table.scalar(0, "<h", 0) == 4
    assert message_table.scalar(1, "<B", 0) == 3
    assert message_table.scalar(3, "<q", 0) == body_len
    batch = message_table.table(2)
    count = batch.scalar(0, "<q", 0)
    body = raw[batch_at + metadata_len : batch_at + metadata_len + body_len]
    end = batch_at + metadata_len + body_len
    assert raw[end : end + 8] == b"\xff\xff\xff\xff\0\0\0\0"
    assert end + 8 + footer_len + 10 == len(raw)

    nodes_at, node_count = batch.vector(1)
    assert node_count == len(columns)
    for index in range(node_count):
        assert struct.unpack_from("<qq", batch.buffer, nodes_at + 16 * index) == (count, 0)
    buffers_at, buffer_count = batch.vector(2)
    spans = [
        struct.unpack_from("<qq", batch.buffer, buffers_at + 16 * index)
        for index in range(buffer_count)
    ]
    spans.reverse()

    def take():
        offset, length = spans.pop()
        assert offset % 8 == 0
        return body[offset : offset + length]

    values = {}
    for name, kind in columns:
        assert take() == b"", "no validity bitmap"
        if kind == INT64:
            values[name] = [item[0] for item in struct.iter_unpack("<q", take())]
        else:
            ends = [item[0] for item in struct.iter_unpack("<i", take())]
            text = take()
            assert ends[0] == 0 and ends[-1] == len(text)
            values[name] = [
                text[start:end].decode("utf-8") for start, end in zip(ends, ends[1:])
            ]
        assert len(values[name]) == count
    assert not spans
    rows = [{name: values[name][row] for name, _ in columns} for row in range(count)]
    assert notes["quantforge-result-format"] == str(FORMAT)
    assert columns == TABLES[notes["quantforge-result"]]
    return notes["quantforge-result"], rows


def metrics_file(metrics):
    """`metrics.json`: the fields in order, with no whitespace."""
    assert list(metrics) == METRICS_FIELDS
    return json.dumps(metrics, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def named(raw):
    return "sha256:" + hashlib.sha256(raw).hexdigest()


# The engine's pinned run (tests/backtest_engine.rs): SMA cross, 10010 cash,
# a 10 bps fee, closed out at the end.
SMA_TRADES = [
    {
        "symbol": "BTCUSDT",
        "entry_time_ms": 180000,
        "exit_time_ms": 480000,
        "entry_price": "100",
        "exit_price": "100.1",
        "qty": "100",
        "gross_quote_pnl": "-10.01",
    },
    {
        "symbol": "BTCUSDT",
        "entry_time_ms": 720000,
        "exit_time_ms": 899999,
        "entry_price": "99.9",
        "exit_price": "110.11",
        "qty": "100",
        "gross_quote_pnl": "999.999",
    },
]

SMA_EQUITY = [
    {"open_time_ms": 60000 * bar, "equity": equity}
    for bar, equity in enumerate(
        [
            "10010",
            "10010",
            "10010",
            "10300",
            "10500",
            "11000",
            "10300",
            "9900",
            "9999.99",
            "9999.99",
            "9999.99",
            "9999.99",
            "10300",
            "10500",
            "11011",
        ]
    )
]

LARGEST = "79228162514264337593543950335"
SMALLEST = "0.0000000000000000000000000001"


def series(count):
    """`count` one-minute bars from 2026-10-20T00:00:00Z, each amount with
    two decimal places."""
    return [
        {"open_time_ms": 1792454400000 + 60000 * bar, "equity": f"{10000 + bar}.{bar % 100:02d}"}
        for bar in range(count)
    ]


def table_case(name, table, rows, doc, listed=True):
    raw = arrow_file(table, rows)
    assert read_arrow_file(raw) == (table, rows), name
    case = {"name": name, "result": table, "doc": doc}
    if listed:
        case["rows"] = rows
        case["file_hex"] = raw.hex()
    else:
        case["series"] = len(rows)
    case["hash"] = named(raw)
    case["size_bytes"] = len(raw)
    return case


def metrics_case(name, metrics, doc):
    raw = metrics_file(metrics)
    return {
        "name": name,
        "result": "metrics",
        "doc": doc,
        "metrics": metrics,
        "file_hex": raw.hex(),
        "hash": named(raw),
        "size_bytes": len(raw),
    }


def cases():
    return [
        metrics_case(
            "metrics.sma_cross",
            {
                "initial_cash": "10010",
                "final_equity": "10999.989",
                "total_return_pct": "9.89",
                "max_drawdown_pct": "10",
                "trade_count": 2,
            },
            "The pinned SMA-cross run's summary.",
        ),
        metrics_case(
            "metrics.scales",
            {
                "initial_cash": "10000.00",
                "final_equity": "0",
                "total_return_pct": "-100.0000000000000000000000000",
                "max_drawdown_pct": SMALLEST,
                "trade_count": 0,
            },
            "Every amount keeps its scale; a run that lost everything and never traded.",
        ),
        table_case("trades.none", "trades", [], "A run that closed no trade: one empty record batch."),
        table_case("trades.sma_cross", "trades", SMA_TRADES, "The pinned SMA-cross run's two trades."),
        table_case(
            "trades.extremes",
            "trades",
            [
                {
                    "symbol": "币安人生USDT",
                    "entry_time_ms": -9223372036854775808,
                    "exit_time_ms": 9223372036854775807,
                    "entry_price": SMALLEST,
                    "exit_price": LARGEST,
                    "qty": "1.10",
                    "gross_quote_pnl": "-" + LARGEST,
                },
                {
                    "symbol": "",
                    "entry_time_ms": 0,
                    "exit_time_ms": -1,
                    "entry_price": "81940.01000000",
                    "exit_price": "1.1",
                    "qty": "0.000",
                    "gross_quote_pnl": "0",
                },
            ],
            "The widest values a column holds: 64-bit times, the largest and smallest "
            "decimals, trailing zeros, a symbol that is not ASCII and an empty one.",
        ),
        table_case("equity.none", "equity", [], "No bars: one empty record batch."),
        table_case(
            "equity.two_bars",
            "equity",
            [
                {"open_time_ms": 1792454400000, "equity": "10000"},
                {"open_time_ms": 1792454460000, "equity": "10012.50"},
            ],
            "The spec's worked example.",
        ),
        table_case("equity.sma_cross", "equity", SMA_EQUITY, "The pinned SMA-cross run's curve."),
        table_case(
            "equity.extremes",
            "equity",
            [
                {"open_time_ms": -9223372036854775808, "equity": "-" + LARGEST},
                {"open_time_ms": 0, "equity": SMALLEST},
                {"open_time_ms": 9223372036854775807, "equity": "0.00"},
            ],
            "64-bit times and the widest decimals.",
        ),
        table_case(
            "equity.series",
            "equity",
            series(5000),
            "5,000 one-minute bars from 2026-10-20T00:00:00Z; bar i holds the amount "
            "10000 + i with i mod 100 as its two decimal places. Only its hash is listed.",
            listed=False,
        ),
    ]


def main():
    fixture = {
        "format": FORMAT,
        "note": (
            "Shared test cases of the QuantForge result spec (docs/result-spec.md), for "
            "every implementation. Each case lists a metrics object, or a table's rows "
            "with every decimal as its exact text, and the file they are written as, as "
            "hex, with its hash and size. The files and hashes were computed by "
            "spikes/result-format/result_cases.py, an independent Python implementation "
            "of the spec, not by the Rust code under test."
        ),
        "cases": cases(),
    }
    root = pathlib.Path(__file__).resolve().parents[2]
    path = root / "tests" / "fixtures" / "result_cases.json"
    text = json.dumps(fixture, indent=2, ensure_ascii=False) + "\n"
    path.write_bytes(text.encode("utf-8"))
    print(f"wrote {path.relative_to(root)}: {len(fixture['cases'])} cases", file=sys.stderr)


if __name__ == "__main__":
    main()
