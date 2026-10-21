# Strategy bundle spec

**Bundle format 1.** A bundle is how a client packs a Python strategy for a
remote backtest. This document is the contract between QuantForge servers
and every client that builds bundles, the Python SDK's builder included.

A bundle is explicit files and a manifest. It never holds a pickled object:
pickles break when versions drift, hide what code they depend on, and run
arbitrary code when loaded. A strategy's state lives in its source files,
and its parameters live in the manifest as plain values.

The reference implementation is
[`quantforge::protocol::bundle`](../src/protocol/bundle/mod.rs). The shared
cases in [`tests/fixtures/bundle_cases.json`](../tests/fixtures/bundle_cases.json)
hold every implementation to this document: an implementation conforms when
it passes all of them.

## Contents

- [Files](#files)
- [Collecting a directory](#collecting-a-directory)
- [The manifest](#the-manifest)
- [The canonical tar](#the-canonical-tar)
- [Checking a bundle](#checking-a-bundle)
- [Admission](#admission)
- [Uploading](#uploading)
- [A worked example](#a-worked-example)
- [The shared cases](#the-shared-cases)
- [Notes for implementers](#notes-for-implementers)
- [Changes](#changes)

## Files

A bundle's files are named by paths. The rules make a bundle extract the
same way on every operating system, and fit the canonical tar.

- A path is ASCII, relative, at most 100 bytes long, and separates
  directories with `/`.
- Each component starts with a letter, a digit, or `_`, and holds only
  letters, digits, `.`, `_`, and `-`. So no component is empty, `.`, `..`,
  or a dotfile.
- No component ends in `.`, which Windows drops from names.
- No component is `__pycache__`.
- No component is named for a Windows device: matched without regard to
  case on the text before the component's first `.`, the reserved names are

  <!-- begin: reserved names -->
  CON, PRN, AUX, NUL, COM1, COM2, COM3, COM4, COM5, COM6, COM7, COM8, COM9, LPT1, LPT2, LPT3, LPT4, LPT5, LPT6, LPT7, LPT8, LPT9
  <!-- end: reserved names -->

  so `aux.py` and `pkg/Con.json` are refused, while `auxiliary.py` is not.
- A file's extension, the text after the last `.` of its last component,
  is one of these, in lowercase:

  <!-- begin: extensions -->
  `.py`, `.toml`, `.json`, `.txt`
  <!-- end: extensions -->

- No two paths are equal when compared without regard to case, and no file
  has the path a directory of another path needs, such as `util.py` beside
  `util.py/x.py`.
- The manifest, `quantforge.toml`, sits at the root.

A file's bytes are anything at all, and are hashed exactly as they are:
nothing rewrites line endings or encodings.

A bundle stays within limits a server sets; a client checks against the
defaults or against the limits a server advertises. The manifest has a
limit of its own, whatever the others.

<!-- begin: limits -->
| Limit | Default | Meaning |
| --- | --- | --- |
| `max_file_bytes` | 1048576 | the most bytes one file may hold (1 MiB) |
| `max_total_bytes` | 8388608 | the most bytes all the files may hold together (8 MiB) |
| `max_files` | 256 | the most files a bundle may hold |
| `MAX_MANIFEST_BYTES` | 65536 | the most bytes the manifest may hold (64 KiB), fixed |
<!-- end: limits -->

## Collecting a directory

A builder that collects a bundle from a directory takes every regular file
below it, except:

- any entry, file or directory, whose name starts with `.`, such as `.git`,
  `.venv`, or `.DS_Store`;
- any directory named `__pycache__`.

A symlink, a Windows junction, a socket, or any other entry that is not a
regular file is refused (`not_a_regular_file`), never followed. A builder
reports the same problems as for the same files given in memory, except
that it may stop collecting once it has found more files than `max_files`.

The same directory gives the same bundle in every builder, but a checkout
can make two directories differ:

- git with `core.autocrlf` on Windows checks out text files with CRLF line
  endings, which hash differently;
- git with `core.symlinks=false` checks a symlink out as a regular file
  holding the link's target;
- files that do not start with `.` are collected, so a stray `Thumbs.db`,
  `desktop.ini`, or `notes.md` makes the bundle refused, not silently
  different.

## The manifest

`quantforge.toml` is written in bundle TOML: TOML 1.0 that both Rust's
`toml` 0.8 and Python's `tomllib` read the same way. A manifest is bundle
TOML when it

- is at most 65536 bytes of UTF-8, without a byte-order mark;
- parses as TOML 1.0, so TOML 1.1's trailing commas and newlines in inline
  tables, `\e` and `\x` escapes, and times without seconds are refused;
- holds no value nested more than 32 levels deep, counting the root table
  as one level and each table or array inside a value as one more;
- holds no `nan` or infinity, and no integer outside the 64-bit range.

Its keys:

<!-- begin: manifest keys -->
| Key | Value | Required |
| --- | --- | --- |
| `format` | the integer 1, this document's format | yes |
| `entry` | the strategy: `path/to/module.py:ClassName` | yes |
| `engine` | the engine versions it runs on, such as `">=0.3.0, <0.4.0"` | yes |
| `python` | the Python version it was written for, `MAJOR.MINOR`, such as `"3.12"` | yes |
| `params` | a table of the strategy's parameters | no |
| `deps` | a table of the packages it imports, each to its version | no |
<!-- end: manifest keys -->

A manifest with any other key is refused. That differs on purpose from the
wire's messages, which ignore fields they do not know: the manifest is part
of what the code hash names and decides what runs, so a new key comes only
with a new format.

### `entry`

A `.py` file in the bundle, a `:`, and a class name. The file must be
importable: each of its directories, and its name without `.py`, is an
ASCII Python identifier (`[A-Za-z_][A-Za-z0-9_]*`) and not one of Python's
35 hard keywords (`keyword.kwlist`), so `my-strategy.py` and `class.py`
are refused. The class name follows the same rule. A package's
`__init__.py` names the package itself, so `pkg/__init__.py:Cls` runs
`Cls` from module `pkg`; a root `__init__.py` is refused.

### `engine`

Comparators separated by commas, all of which a version must meet. Each is
an operator, `>=`, `>`, `<=`, `<`, or `==`, followed by a version
`MAJOR.MINOR.PATCH` of numbers without leading zeros. Spaces may surround
an operator and a comparator. `^`, `~`, `=`, wildcards, and two-part
versions are refused.

### `python`

`MAJOR.MINOR`, numbers without leading zeros. A bundle runs only where the
server runs exactly that Python version.

### `params`

The strategy's parameters, which reach it as JSON. A value is a string, a
boolean, an integer within ±(2^53 − 1), a finite float, an array, or a
table of those. TOML dates and times are refused, since JSON has none, and
so is any key that starts with `$`. A float is an IEEE 754 binary64, as
Python reads it; an amount that must stay exact belongs in a string.

### `deps`

Each key is a package name in its normalized form (PEP 503): lowercase
letters and digits with single `-` between them, matching
`[a-z0-9]([a-z0-9-]*[a-z0-9])?` with no `--`. Any other spelling of a
valid name is refused, with the normalized form to write instead, so
`PyYAML` must be written `pyyaml`. Each value is a version string in its
canonical form (PEP 440), matching

```text
(0|[1-9][0-9]*)(\.(0|[1-9][0-9]*))*((a|b|rc)(0|[1-9][0-9]*))?(\.post(0|[1-9][0-9]*))?(\.dev(0|[1-9][0-9]*))?
```

such as `2.1.3`, `1.0rc1`, or `2.0.post1`. A pin is compared as text, so
`1.0` and `1.0.0` are different pins. Ranges, epochs, and local versions
are refused.

## The canonical tar

A bundle's files make exactly one byte sequence, its canonical tar: a POSIX
ustar archive with one entry per file, sorted by the bytes of its path, and
no entries for directories. Each entry is a 512-byte header, then the
file's bytes, then NUL bytes up to the next multiple of 512. The archive
ends with two 512-byte blocks of NUL bytes and nothing after them: no
record padding.

Every header is these bytes, where only the name, the size, and the
checksum differ between files:

| Offset | Length | Field | Bytes |
| ---: | ---: | --- | --- |
| 0 | 100 | name | the path, then NUL bytes; a 100-byte path has none |
| 100 | 8 | mode | `0000644` and NUL |
| 108 | 8 | uid | `0000000` and NUL |
| 116 | 8 | gid | `0000000` and NUL |
| 124 | 12 | size | the file's length as 11 octal digits, then NUL |
| 136 | 12 | mtime | `00000000000` and NUL |
| 148 | 8 | checksum | 6 octal digits, NUL, and a space |
| 156 | 1 | typeflag | `0` |
| 157 | 100 | linkname | NUL bytes |
| 257 | 6 | magic | `ustar` and NUL |
| 263 | 2 | version | `00` |
| 265 | 32 | uname | NUL bytes |
| 297 | 32 | gname | NUL bytes |
| 329 | 8 | devmajor | NUL bytes |
| 337 | 8 | devminor | NUL bytes |
| 345 | 155 | prefix | NUL bytes |
| 500 | 12 | — | NUL bytes |

The checksum is the sum of the header's 512 bytes as unsigned numbers,
taken while the checksum field holds eight spaces.

The **code hash** is the SHA-256 of the canonical tar, written
`sha256:` and 64 lowercase hex digits. It names the bundle wherever the
protocol refers to one, as a job submission's `bundle` does, and it is the
same on every machine for the same files.

In Python, `tarfile.TarInfo.tobuf(tarfile.USTAR_FORMAT)` writes this header
for a `TarInfo` with only its name and size set, as it does from Python 3.9
on; the shared cases confirm it on a given version. `tarfile.TarFile` does not
write the canonical tar: it defaults to the PAX format and pads the archive
to 10240 bytes. A builder can assemble the bytes itself:

```python
import hashlib
import tarfile


def canonical_tar(files: dict[str, bytes]) -> bytes:
    """The canonical tar of files whose paths keep the path rules."""
    out = bytearray()
    for path in sorted(files):  # ASCII paths: code point order is byte order
        info = tarfile.TarInfo(path)
        info.size = len(files[path])
        out += info.tobuf(tarfile.USTAR_FORMAT)  # never the PAX default
        out += files[path] + b"\0" * (-len(files[path]) % 512)
    return bytes(out + b"\0" * 1024)  # no record padding


def code_hash(files: dict[str, bytes]) -> str:
    return "sha256:" + hashlib.sha256(canonical_tar(files)).hexdigest()
```

## Checking a bundle

A bundle is checked in this order, and every implementation reports the
same problems for the same bundle. Each problem has a code, below, and a
message for the person fixing the bundle; the codes are the contract, and
the messages may differ between implementations.

1. **The archive**, when the bundle arrives as a tar: it must be at most
   the size the limits allow, a whole number of 512-byte blocks, and
   exactly the canonical tar of its files, header for header, with its
   entries in order and nothing after its two closing blocks. The first
   fault stops the check.
2. **Each file**: the path rules, then the extension, whichever fails
   first, then the file's size. A file whose path fails is checked no
   further.
3. **All the files**: paths that collide, more files than `max_files`, and
   more bytes than `max_total_bytes`. The count and the total include every
   file, whatever its path.
4. **The manifest**: missing (`manifest_missing`), or not bundle TOML
   (`manifest_invalid`), which stops the manifest's checks. A manifest that
   failed a check of step 2 is not parsed.
5. **`format`**: missing or not an integer is `manifest_invalid`; any other
   integer than 1 is `format_unsupported`. Either stops the manifest's
   checks.
6. **Each key, on its own**: `entry`, `engine`, `python`, `params`,
   `deps`, and then each key the format does not define. A key that is
   missing or of the wrong type is a problem of that key's code; an unknown
   key is `manifest_invalid`.
7. **Admission**, separately, against a server's runtime.

<!-- begin: problem codes -->
| Code | Meaning |
| --- | --- |
| `path_invalid` | a path that breaks the path rules |
| `path_collision` | two paths that differ only in case, a path listed twice, or a file a directory needs the name of |
| `extension_not_allowed` | a file whose extension is not allowed |
| `file_too_large` | a file over `max_file_bytes` |
| `bundle_too_large` | files over `max_total_bytes` together, or an archive larger than the limits allow |
| `too_many_files` | more files than `max_files` |
| `not_a_regular_file` | a symlink, junction, socket, or other entry that is not a regular file |
| `manifest_missing` | no `quantforge.toml` at the root |
| `manifest_invalid` | a manifest that is not bundle TOML, has no format or a mistyped one, or has a key the format does not define |
| `format_unsupported` | a format other than 1 |
| `entry_invalid` | an entry that is missing, mistyped, or names no importable module and class |
| `entry_not_found` | an entry naming a file the bundle does not hold |
| `engine_invalid` | an engine that is missing, mistyped, or not comparators |
| `python_invalid` | a python that is missing, mistyped, or not `MAJOR.MINOR` |
| `param_not_json` | params that are not a table, or a value JSON cannot hold |
| `dependency_invalid` | deps that are not a table, or a name or version that is not canonical |
| `dependency_not_allowed` | a dependency the server does not offer |
| `dependency_version_unavailable` | a dependency version the server does not offer |
| `engine_mismatch` | a server engine the requirement refuses |
| `python_mismatch` | a server Python other than the manifest's |
| `archive_not_canonical` | an archive that is not exactly the canonical tar of its files |
<!-- end: problem codes -->

## Admission

A server runs a bundle only where its workers can: the engine version meets
the manifest's `engine`, the Python version equals `python`, and every
dependency is installed at its pinned version. The installed packages are
the server's allowlist: a bundle may import those and no others. A server
can advertise its runtime so that a client checks first:

```json
{"engine":"0.3.0","python":"3.12","packages":{"numpy":["2.1.3"],"pandas":["2.2.2","2.2.3"]}}
```

## Uploading

A client uploads a bundle as its canonical tar, with the media type
`application/x-tar`, and names it by its code hash. The server takes the
SHA-256 of the bytes it received, reads them back strictly, so that only a
canonical tar is accepted, checks the bundle, and admits it against its
runtime.

## A worked example

A bundle of two files. Its manifest:

<!-- begin: example quantforge.toml -->
```toml
# What to run, and on what.
format = 1
entry = "strategy.py:SmaCross"
engine = ">=0.3.0, <0.4.0"
python = "3.12"

# The strategy's parameters, handed to it as JSON.
[params]
fast = 20
slow = 50

# The packages it imports, each at the version it was written against.
[deps]
numpy = "2.1.3"
```
<!-- end: example quantforge.toml -->

and `strategy.py`:

<!-- begin: example strategy.py -->
```python
class SmaCross:
    """A moving-average crossover. The Python SDK defines the interface."""

    def __init__(self, fast: int, slow: int) -> None:
        self.fast = fast
        self.slow = slow
```
<!-- end: example strategy.py -->

Its canonical tar is 2048 bytes of headers and data, then the two closing
blocks, 3072 bytes in all, and its code hash is

<!-- begin: example code hash -->
`sha256:2bc0e7f2cd5aa061e07ceebc97999a2d01cdb25998026965255aee87eeb2754d`
<!-- end: example code hash -->

Each file's bytes are the text of its block above, with LF line endings and
a final newline.

## The shared cases

[`tests/fixtures/bundle_cases.json`](../tests/fixtures/bundle_cases.json)
holds the cases every implementation passes. Each case is either a list of
files, each a `path` with its bytes as `text` or `hex`, or an `archive`
derived from another case's canonical tar by `ops` applied in order:

- `patch`: write the `hex` bytes at `offset`;
- `fix_checksum`: rewrite the checksum of the header in that 512-byte block;
- `append_zeros` and `append_hex`: add bytes at the end;
- `truncate`: drop that many bytes from the end;
- `swap`: exchange the `length` bytes at `a` with those at `b`.

A case may set `limits`, else the defaults apply, and a `runtime`. It
expects a `code_hash`, and then an `admission` list if it has a runtime,
or the sorted list of its `problems` codes. The expected code hashes were
computed by an independent Python implementation, not by the reference
implementation.

## Notes for implementers

- Compare and sort paths as bytes. For ASCII paths, Python's `sorted` on
  `str` gives the same order.
- Match with `re.fullmatch`: `$` matches before a trailing newline.
- Use `[0-9]` and `[A-Za-z]`, never `\d`, `str.isdigit`, or
  `str.isidentifier`, which accept non-ASCII characters.
- Check that a text is ASCII before lowercasing it.
- `bool` is an `int` in Python: check a value with `type(value) is int`.
- Reject a byte-order mark before parsing the manifest; Rust's TOML parser
  strips one, Python's refuses it, and the spec refuses it.
- In Python, `os.DirEntry.is_symlink()` is false for a junction: check
  `is_junction()` too, from Python 3.12.
- Use the reserved names above, not `os.path.isreserved`, which is newer
  and has a different list.

## Changes

A change that an implementation of format 1 could not accept, such as a new
manifest key, a new path rule, or a different canonical tar, makes a new
format, and a manifest names its format so that a server can refuse one it
does not read. Format 1 may still change until QuantForge 0.3.0 is
released.
