# Candle segment spec

**Segment format 1.** A segment is one market's closed candles from one
window, packed once into a payload that never changes and named by its
hash. This document is the contract between QuantForge servers, which pack
segments from the candles they store, and every client that fetches and
reads them.

Because a segment never changes, its name can be cached for good, by a
client or by anything between it and the server, and a backtest can record
exactly which candles it ran on by naming its segments.

The reference implementation is
[`quantforge::storage::segments`](../src/storage/segments/mod.rs). The
shared cases in
[`tests/fixtures/segment_cases.json`](../tests/fixtures/segment_cases.json)
hold every implementation to this document: an implementation conforms when
it passes all of them.

## Contents

- [Identity](#identity)
- [Windows](#windows)
- [The payload](#the-payload)
- [Decimals](#decimals)
- [Compression](#compression)
- [The hash](#the-hash)
- [Reading a segment](#reading-a-segment)
- [A worked example](#a-worked-example)
- [In Python](#in-python)
- [The shared cases](#the-shared-cases)
- [Why not Arrow IPC](#why-not-arrow-ipc)
- [Changes](#changes)

## Identity

A segment's name is the hash of its bytes, and its bytes hold no market: the
same candles under two markets pack to one name. What a name stands for in
one store is the index entry that lists it, and that entry's identity is
the market's product line, its symbol, its interval and the window's start:

- on the wire, the product line is `exchange`, the venue's registry name,
  with `category`, its product family (`spot`, `linear` or `inverse`);
- in a store, the two are one column, `exchange`, holding the line's storage
  name, such as `binance_spot`.

The storage name has carried the venue and the family since QuantForge
0.1.0, so an index entry written before `category` reached the wire names
the same segment it always did, and a name once published stays readable and
verifiable against its bytes. A segment identity is never reinterpreted
under another key: a change to what an entry means, or to what its name
covers, comes with a new segment format.

## Windows

A market's candles are cut into windows, each the length of this many bars
of the market's interval:

<!-- begin: window bars -->
1440
<!-- end: window bars -->

Windows are counted from the Unix epoch. With `span` the window's length in
milliseconds, 1,440 times the interval's, the window holding a candle that
opens at `t` starts at `floor(t / span) × span` and ends where the next one
starts. So every window of `1m` candles or longer starts at a UTC midnight:

| Interval | Window |
| --- | --- |
| `1s` | 24 minutes |
| `1m` | 1 day |
| `3m` | 3 days |
| `5m` | 5 days |
| `15m` | 15 days |
| `30m` | 30 days |
| `1h` | 60 days |
| `2h` | 120 days |
| `4h` | 240 days |
| `6h` | 360 days |
| `8h` | 480 days |
| `12h` | 720 days |
| `1d` | 1,440 days |
| `3d` | 4,320 days |
| `1w` | 10,080 days |

A segment holds every candle the server stores whose open time lies in its
window, oldest first. Where the exchange published no bar, or the server
has not synced one, there is none, so a segment can hold fewer than 1,440
candles. A full window holds exactly 1,440, wherever the interval's bars
start: weekly bars open on Mondays, not on the epoch's Thursday.

A window is packed once it is closed, which is once the server stores a
candle of a later window. The window holding a market's newest candle is
the tip: its candles are served one by one, never as a segment, and so are
those of any window the server has not packed yet. A server packs only
final candles; QuantForge stores a candle only once a second has passed
since its close.

A window whose candles change after it was packed, as when a sync fills a
gap in it, is packed again under a new name. The segment it had keeps its
name and its candles: a name never comes to stand for other candles.

## The payload

A segment's payload is its candles as columns. Every integer is
little-endian, and `n` is the number of candles.

<!-- begin: payload layout -->
| Bytes | Field |
| --- | --- |
| 8 | the magic: the ASCII text `QFSEGMNT` |
| 4 | the format: `1`, unsigned |
| 4 | `n`: 1 to 1,440, unsigned |
| 8 × n | open times: signed milliseconds since the epoch, strictly rising |
| 8 × n | close times: signed milliseconds |
| 8 × n | trade counts: unsigned, `0` where unknown |
| n | `1` where the trade count is known, `0` where it is not |
| n + their sum | opens: each text's length in bytes, then the texts, one after another |
| n + their sum | highs, the same way |
| n + their sum | lows |
| n + their sum | closes |
| n + their sum | volumes |
<!-- end: payload layout -->

Nothing follows the volumes. A payload names no market: the index that
lists a segment gives its exchange, symbol, and interval.

Each set of candles has exactly one payload. A trade count that is unknown
is written as `0` with its flag `0`, so it differs from a known count of
zero, and each decimal is written in its one exact form.

## Decimals

A price or a volume is its exact decimal text in ASCII: an optional minus
sign, the integer digits without leading zeros, and an optional point
followed by fraction digits, `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, with no minus
sign on zero. It has at most 28 fraction digits, and its digits without the
point read as a number below 2^96, so every value fits a 96-bit decimal and
no text is longer than 31 bytes.

Trailing zeros are kept, because they are the scale the exchange reported.
`1.10` and `1.1` are equal numbers but different texts, so candles that
differ only in them pack into different payloads, with different names.

## Compression

A segment is stored and sent as exactly one zstd frame
([RFC 8878](https://www.rfc-editor.org/rfc/rfc8878)) holding the payload,
with the payload's size in the frame header. The compression level is not
part of the format: it changes the stored bytes, never the payload or the
name.

A payload is at most

<!-- begin: max payload bytes -->
266416
<!-- end: max payload bytes -->

bytes: 16 bytes of header, then per candle 25 bytes of times, trade count,
and flag, and five decimals of at most 31 bytes with their lengths.

## The hash

A segment is named by the SHA-256 of its payload, the bytes before
compression, written `sha256:` and 64 lowercase hex digits, the protocol's
content hash. The same candles always get the same name, however they were
compressed and by whichever build.

## Reading a segment

A reader that fetched a segment by its name:

1. Refuses anything that is not exactly one zstd frame, and a frame whose
   recorded size is more than 266,416 bytes.
2. Decompresses it, allowing at most 266,416 bytes.
3. Hashes the payload, and refuses it unless it matches the name.
4. Refuses a payload that does not start with the magic, and a format it
   does not know.
5. Reads the columns, refusing:
   - a count outside 1 to 1,440;
   - open times that do not rise;
   - a flag other than `0` or `1`, or a count set where its flag is `0`;
   - a decimal not in its exact form;
   - a payload that ends early, or bytes after the volumes.

A reader that refuses everything an encoder would not write reads each
payload one way only. Decoding a payload and encoding the candles again must
give back the same bytes.

## A worked example

Two `1m` candles from 2026-10-20T00:00:00Z; the second has no trade count:

<!-- begin: worked example candles -->
```json
[
  {
    "open_time_ms": 1792454400000,
    "close_time_ms": 1792454459999,
    "open": "67012.34",
    "high": "67020",
    "low": "67001.1",
    "close": "67015.5",
    "volume": "12.0451",
    "trades": 1234
  },
  {
    "open_time_ms": 1792454460000,
    "close_time_ms": 1792454519999,
    "open": "67015.5",
    "high": "67015.5",
    "low": "67010",
    "close": "67010",
    "volume": "0.5",
    "trades": null
  }
]
```
<!-- end: worked example candles -->

Their payload, 137 bytes, as hex, field by field:

<!-- begin: worked example payload -->
```text
51465345474d4e54                  the magic, QFSEGMNT
01000000                          format 1
02000000                          2 candles
00989b56a1010000                  open times: 1792454400000
60829c56a1010000                              1792454460000
5f829c56a1010000                  close times: 1792454459999
bf6c9d56a1010000                               1792454519999
d204000000000000                  trade counts: 1234
0000000000000000                                unknown
0100                              known, not known
0807                              open lengths
36373031322e333436373031352e35    67012.34, 67015.5
0507                              high lengths
363730323036373031352e35          67020, 67015.5
0705                              low lengths
36373030312e313637303130          67001.1, 67010
0705                              close lengths
36373031352e353637303130          67015.5, 67010
0703                              volume lengths
31322e30343531302e35              12.0451, 0.5
```
<!-- end: worked example payload -->

and its name:

<!-- begin: worked example hash -->
sha256:40f256656d99aa4df38ff7225d2846a06a5d164becf8f72d32015661914b211f
<!-- end: worked example hash -->

## In Python

An encoder builds the payload from decimal texts it has kept since it read
them. It never formats a `float`, nor a `Decimal` through `str`, which can
write `1E-7`; `format(value, "f")` writes `0.0000001`.

```python
import hashlib
import re
import struct

EXACT = re.compile(r"-?(0|[1-9][0-9]*)(\.[0-9]+)?")
COLUMNS = ("open", "high", "low", "close", "volume")


def exact(text: str) -> bool:
    """Whether text is a decimal in its exact form."""
    if len(text) > 31 or not EXACT.fullmatch(text):
        return False
    unsigned = text.removeprefix("-")
    if text.startswith("-") and set(unsigned) <= {"0", "."}:
        return False  # no sign on zero
    integer, _, fraction = unsigned.partition(".")
    return len(fraction) <= 28 and int(integer + fraction) < 2**96


def payload(candles: list[dict]) -> bytes:
    """The payload of 1 to 1,440 candles with rising open times."""
    n = len(candles)
    out = bytearray(b"QFSEGMNT") + struct.pack("<II", 1, n)
    out += struct.pack(f"<{n}q", *(c["open_time_ms"] for c in candles))
    out += struct.pack(f"<{n}q", *(c["close_time_ms"] for c in candles))
    out += struct.pack(f"<{n}Q", *(c["trades"] or 0 for c in candles))
    out += bytes(c["trades"] is not None for c in candles)
    for column in COLUMNS:
        texts = [c[column] for c in candles]
        if not all(exact(text) for text in texts):
            raise ValueError(f"a {column} is not an exact decimal")
        out += bytes(len(text) for text in texts) + "".join(texts).encode("ascii")
    return bytes(out)


def name(payload: bytes) -> str:
    return "sha256:" + hashlib.sha256(payload).hexdigest()
```

## The shared cases

[`tests/fixtures/segment_cases.json`](../tests/fixtures/segment_cases.json)
holds the cases every implementation passes. Each case lists a window's
candles, with every decimal as its exact text and `trades` as an integer or
`null`, then the payload they encode to, as `payload_hex`, and its name, as
`hash`. An implementation conforms when, for every case, encoding the
candles gives exactly that payload and name, and decoding the payload gives
the candles back with the same texts.

The payloads and names were computed by an independent Python
implementation of this document, not by the reference implementation, so
the cases test the reference implementation too.

## Why not Arrow IPC

The story that introduced segments preferred Apache Arrow's IPC format,
which pyarrow and polars read directly. Format 1 is its own instead:

- **Its bytes are fixed by this document alone.** An Arrow IPC stream's
  bytes come from its writer: its flatbuffer metadata, alignment, and
  padding. pyarrow and arrow-rs write different bytes for the same table,
  and an arrow-rs upgrade could change them, renaming every segment it
  wrote again.
- **Exact decimals do not fit Arrow's decimal types.** `Decimal128` has one
  scale per column, while each value keeps its own, so the decimals would
  be text columns anyway.
- **It costs no dependencies.** arrow-ipc brings about 15 to 20 crates, and
  Rust 1.85, QuantForge's minimum, stops at arrow-rs 59, since arrow-rs 60
  needs Rust 1.88.
- **Python does not read the files.** QuantForge's Python strategies
  receive their candles in process, as the columns of a `CandleBatch`, not
  as segments.

## Changes

- Format 1: the first format.
- QUA-37 wrote down the identity of an index entry, and added `category`
  to it on the wire. The payload, the hash and the stored names are
  unchanged.
