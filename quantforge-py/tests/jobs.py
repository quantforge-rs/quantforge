"""What the job tests share: a `quantforge serve run` whose workers are this
interpreter, running bundles through the package under test, on a store of
either backend; bundles as their canonical tar; the job routes spoken to
with the standard library alone; and the two Arrow files of a result read
back as text (docs/python-worker.md).

- **The server** is the binary the reader's tests run (`stores.binary()`),
  started in the place's directory on the place's store, on a free port of
  this machine, its workers' scratch directories in a directory of the
  test's own, and `--job-python` this interpreter: the virtualenv the tests
  run in, which holds the package under test. It is stopped as the block
  that started it ends.
- **A bundle** is its files' canonical tar, as the bundle spec gives it,
  named by its SHA-256.
- **A result's files** are read by a reader of Arrow's published layout,
  written for the two tables of the result spec and no other: every value
  as the text the engine wrote, every time as its integer.
"""

import contextlib
import csv
import hashlib
import http.client
import json
import os
import pathlib
import queue
import signal
import struct
import subprocess
import sys
import tarfile
import threading
import time

import stores

GOLDEN = stores.REPOSITORY / "tests" / "fixtures" / "golden"

# How long a step may take before a test fails instead of waiting for good.
PATIENCE = 60

# The market every job here runs on, as the protocol writes one.
MARKET = {"exchange": "binance", "category": "spot", "symbol": "BTCUSDT", "interval": "1m"}

# The settings the goldens were run with.
GOLDEN_SETTINGS = {"initial_cash": "10000", "fee_bps": "10", "close_out_at_end": True}

# A result's summary, field by field, and the columns of its trades
# (docs/result-spec.md).
METRICS = ("initial_cash", "final_equity", "total_return_pct", "max_drawdown_pct", "trade_count")
TRADE_COLUMNS = (
    "symbol",
    "entry_time_ms",
    "exit_time_ms",
    "entry_price",
    "exit_price",
    "qty",
    "gross_quote_pnl",
)


def golden_klines() -> list[list]:
    """The goldens' frozen day of candles, as the exchange would have
    written it."""
    with open(GOLDEN / "candles.csv", newline="", encoding="utf-8") as file:
        rows = list(csv.DictReader(file))
    prices = ("open", "high", "low", "close", "volume")
    return [
        [
            int(row["open_time_ms"]),
            *[row[column] for column in prices],
            int(row["close_time_ms"]),
            "0.00000000",
            int(row["trades"]),
            "0.00000000",
            "0.00000000",
            "0",
        ]
        for row in rows
    ]


def golden() -> dict:
    """What the Rust SMA cross made of the frozen day: `sma_cross.json`."""
    with open(GOLDEN / "sma_cross.json", encoding="utf-8") as file:
        return json.load(file)


def canonical_tar(files: dict[str, bytes]) -> bytes:
    """The canonical tar of a bundle's files, as the bundle spec writes it."""
    out = bytearray()
    for path in sorted(files):
        info = tarfile.TarInfo(path)
        info.size = len(files[path])
        out += info.tobuf(tarfile.USTAR_FORMAT)
        out += files[path] + b"\0" * (-len(files[path]) % 512)
    return bytes(out + b"\0" * 1024)


def code_hash(tar: bytes) -> str:
    return "sha256:" + hashlib.sha256(tar).hexdigest()


def manifest(entry: str, *, params: str = "", deps: str = "", python: str | None = None) -> bytes:
    """A bundle's `quantforge.toml`: its entry, the engine this package is,
    the Python the tests run on unless another is named, and the bodies of
    its `params` and `deps` tables, as TOML."""
    import quantforge_py

    python = python or "%d.%d" % sys.version_info[:2]
    text = (
        f'format = 1\nentry = "{entry}"\nengine = "=={quantforge_py.ENGINE_VERSION}"\n'
        f'python = "{python}"\n'
    )
    if params:
        text += f"\n[params]\n{params}\n"
    if deps:
        text += f"\n[deps]\n{deps}\n"
    return text.encode()


def strategy_file(source: str) -> bytes:
    """A strategy's source, its indentation as written in a test."""
    import textwrap

    return textwrap.dedent(source).lstrip("\n").encode()


def golden_bundle() -> dict[str, bytes]:
    """The goldens' SMA cross as a bundle: its file as it is, and its two
    windows as the bundle's parameters."""
    return {
        "quantforge.toml": manifest("sma_cross.py:SmaCross", params="fast = 20\nslow = 50"),
        "sma_cross.py": (GOLDEN / "sma_cross.py").read_bytes(),
    }


class Refused(Exception):
    """A route's refusal: its status and its error envelope."""

    def __init__(self, status: int, body: dict):
        super().__init__(f"{status}: {body}")
        self.status = status
        self.code = body.get("error", {}).get("code")
        self.message = body.get("error", {}).get("message", "")


class Served:
    """A running `serve run` on the place's store, its workers this
    interpreter: what it said as it started, by key, and its routes.
    `variables` are more of its environment, beside the place's own."""

    def __init__(self, place, scratch: pathlib.Path, *arguments, python=None, variables=None):
        self.place = place
        environment = {
            name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ
        }
        environment.update(place.variables)
        environment.update(variables or {})
        for variable in ("TMPDIR", "TMP", "TEMP"):
            environment[variable] = str(scratch)
        command = [
            str(stores.binary()),
            "--log-level",
            "error",
            "serve",
            "run",
            "--listen",
            "127.0.0.1:0",
            "--job-concurrency",
            "1",
        ]
        if python is not False:
            command += ["--job-python", str(python or sys.executable)]
        self.process = subprocess.Popen(
            [*command, *arguments],
            cwd=place.directory,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self._lines = queue.Queue()
        threading.Thread(target=self._read, daemon=True).start()
        self.said = {}
        while "listening" not in self.said:
            try:
                line = self._lines.get(timeout=PATIENCE)
            except queue.Empty:
                self.stop()
                raise AssertionError(f"serve run did not start: {self.said}") from None
            if line is None:
                self.process.wait(PATIENCE)
                raise AssertionError(
                    f"serve run stopped as it started: {self.process.stderr.read()}"
                )
            key, _, value = line.partition(": ")
            self.said[key] = value
        self.port = int(self.said["listening"].rpartition(":")[2])

    def _read(self):
        for line in self.process.stdout:
            self._lines.put(line.rstrip("\n"))
        self._lines.put(None)

    def stop(self):
        if self.process.poll() is None:
            if os.name == "posix":
                self.process.send_signal(signal.SIGTERM)
            else:
                self.process.terminate()
            try:
                self.process.wait(PATIENCE)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(PATIENCE)
        self.process.stderr.close()

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.stop()

    def request(self, method: str, path: str, body: bytes | None = None, headers=None):
        """The status, the headers and the body of one request."""
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=PATIENCE)
        try:
            connection.request(method, path, body=body, headers=headers or {})
            answer = connection.getresponse()
            return answer.status, dict(answer.getheaders()), answer.read()
        finally:
            connection.close()

    def json(self, method: str, path: str, body=None, *, ok=(200,)):
        """A route's JSON answer, or its refusal raised."""
        headers = {}
        if body is not None:
            body = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        status, _, answer = self.request(method, path, body, headers)
        parsed = json.loads(answer) if answer else {}
        if status not in ok:
            raise Refused(status, parsed)
        return parsed

    def upload(self, files: dict[str, bytes]) -> str:
        """Stores a bundle, and answers its code hash."""
        tar = canonical_tar(files)
        named = code_hash(tar)
        status, _, answer = self.request(
            "PUT", f"/v1/bundles/{named}", tar, {"Content-Type": "application/x-tar"}
        )
        if status not in (200, 201):
            raise Refused(status, json.loads(answer))
        return named

    def submit(self, bundle: str, start: int, end: int, settings=None) -> str:
        """Submits a backtest of the bundle over `MARKET` from `start` to
        `end`, and answers the job's ID."""
        submission = {
            "bundle": bundle,
            "data": {**MARKET, "start_open_ms": start, "end_open_ms": end},
            "backtest": settings or GOLDEN_SETTINGS,
        }
        return self.json("POST", "/v1/backtests", submission, ok=(201,))["job_id"]

    def status(self, job: str) -> dict:
        return self.json("GET", f"/v1/backtests/{job}")

    def ended(self, job: str) -> dict:
        """The job's state once it has ended."""
        deadline = time.monotonic() + PATIENCE
        while True:
            state = self.status(job)["state"]
            if state["kind"] in ("succeeded", "failed", "cancelled"):
                return state
            assert time.monotonic() < deadline, f"job {job} is still {state['kind']}"
            time.sleep(0.02)

    def log(self, job: str) -> dict:
        """The job's whole log, as its last page says it: its lines, read a
        page at a time."""
        lines, after = [], 0
        while True:
            page = self.json("GET", f"/v1/backtests/{job}/logs?after={after}")
            lines += page["lines"]
            if not page["lines"]:
                return {**page, "lines": lines}
            after = page["next_after"]

    def result(self, job: str) -> dict:
        """A succeeded job's result: the metrics its route lists, and each
        file read, `metrics.json` as JSON and the tables as rows of text."""
        listed = self.json("GET", f"/v1/backtests/{job}/result")
        files = {"listed": listed["metrics"]}
        for each in listed["results"]:
            status, _, raw = self.request("GET", each["path"])
            assert status == 200, (each, status)
            assert code_hash(raw) == each["hash"], each
            files[each["name"]] = json.loads(raw) if each["name"] == "metrics" else arrow(raw)
        return files


@contextlib.contextmanager
def served(place, scratch: pathlib.Path, *arguments, python=None, variables=None):
    with Served(place, scratch, *arguments, python=python, variables=variables) as server:
        yield server


# A reader of the two tables of a result, after Arrow's published layout
# (docs/result-spec.md): one record batch, no nulls, columns of Int64 or
# Utf8. The spec's shared cases come from spikes/result-format/, whose
# reader this follows.


class _Table:
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
        return None if at is None else _Table(self.buffer, at)

    def string(self, slot):
        at = self.follow(slot)
        length = struct.unpack_from("<I", self.buffer, at)[0]
        return self.buffer[at + 4 : at + 4 + length].decode("utf-8")

    def vector(self, slot):
        at = self.follow(slot)
        return at + 4, struct.unpack_from("<I", self.buffer, at)[0]

    def tables(self, slot):
        start, count = self.vector(slot)
        return [
            _Table(self.buffer, at + struct.unpack_from("<I", self.buffer, at)[0])
            for at in (start + 4 * index for index in range(count))
        ]


def arrow(raw: bytes) -> list[dict]:
    """The rows of a result's Arrow file, each value as its text or its
    integer."""
    assert raw[:8] == b"ARROW1\0\0" and raw[-6:] == b"ARROW1"
    footer_len = struct.unpack_from("<i", raw, len(raw) - 10)[0]
    footer = _Table.root(raw[len(raw) - 10 - footer_len : len(raw) - 10])
    columns = []
    for field in footer.table(1).tables(1):
        columns.append((field.string(0), field.scalar(2, "<B", 0)))
    blocks_at, count = footer.vector(3)
    assert count == 1, "one record batch"
    batch_at, metadata_len, body_len = struct.unpack_from("<qi4xq", footer.buffer, blocks_at)
    message = _Table.root(raw[batch_at + 8 : batch_at + metadata_len])
    batch = message.table(2)
    rows = batch.scalar(0, "<q", 0)
    body = raw[batch_at + metadata_len : batch_at + metadata_len + body_len]
    buffers_at, buffer_count = batch.vector(2)
    spans = [
        struct.unpack_from("<qq", batch.buffer, buffers_at + 16 * index)
        for index in range(buffer_count)
    ]
    spans.reverse()

    def take():
        offset, length = spans.pop()
        return body[offset : offset + length]

    values = {}
    for name, kind in columns:
        assert take() == b"", "no validity bitmap"
        if kind == 2:
            values[name] = [item[0] for item in struct.iter_unpack("<q", take())]
        else:
            assert kind == 5, kind
            ends = [item[0] for item in struct.iter_unpack("<i", take())]
            text = take()
            values[name] = [text[start:end].decode("utf-8") for start, end in zip(ends, ends[1:])]
    return [{name: values[name][row] for name, _ in columns} for row in range(rows)]
