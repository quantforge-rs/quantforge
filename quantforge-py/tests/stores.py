"""What the reader's tests share: stores the `quantforge` command line wrote,
on each backend, for the reader to read.

A store is written by the binary itself: `quantforge data sync` against an
exchange that answers on this machine with the candles a test gives it
(`Exchange`), into a SQLite file or into a database of its own on a
PostgreSQL server (`Server`). Nothing here writes a candle but that command,
unless a test says so in SQL of its own, for what no command writes yet: a
product line without an adapter, a candle without a trade count.

- **The binary** is the one `QUANTFORGE_PY_TEST_CLI` names, or the one
  `cargo build` leaves in the repository's `target/debug/`. A run that finds
  neither fails, and skips nothing.
- **The PostgreSQL server** is the one `QUANTFORGE_TEST_POSTGRES_URL` names,
  the variable the crate's own tests read: a user allowed to create
  databases, over plain TCP on this machine. Without it the PostgreSQL
  tests skip, unless `QUANTFORGE_TEST_POSTGRES_REQUIRED` is `1`, as in CI:
  then a test that would skip fails.

The server is spoken to in its wire protocol with the standard library
alone, so the tests need no driver: enough of it to log in with a password,
run a statement and read rows as text.
"""

import base64
import contextlib
import functools
import hashlib
import hmac
import http.server
import itertools
import json
import os
import pathlib
import secrets
import socket
import sqlite3
import struct
import subprocess
import sys
import threading
import urllib.parse

CLI = "QUANTFORGE_PY_TEST_CLI"
POSTGRES_URL = "QUANTFORGE_TEST_POSTGRES_URL"
POSTGRES_REQUIRED = "QUANTFORGE_TEST_POSTGRES_REQUIRED"

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]

# Every variable the command line and the reader read for a setting: a
# test's process and the commands it runs start without any of them.
SETTINGS = [
    "QF_STORAGE",
    "QF_DB",
    "QF_POSTGRES_URL",
    "QF_POSTGRES_POOL_SIZE",
    "QF_POSTGRES_CONNECT_TIMEOUT_SECS",
]

MINUTE = 60_000
# 2026-01-01T12:00:00Z, the first minute of the candles the tests store.
NOON = 1_767_268_800_000


def binary() -> pathlib.Path:
    """The `quantforge` binary the tests write stores with."""
    named = os.environ.get(CLI)
    built = (
        REPOSITORY / "target" / "debug" / ("quantforge.exe" if os.name == "nt" else "quantforge")
    )
    path = pathlib.Path(named) if named else built
    if not path.is_file():
        how = f"{CLI} names {path}" if named else f"there is none at {path}"
        raise FileNotFoundError(
            f"the reader's tests read stores the quantforge command line wrote, and {how}: build "
            f"it with `cargo build` at the repository's root, or name one with {CLI}"
        )
    return path


def quantforge(*arguments, cwd, variables=None) -> subprocess.CompletedProcess:
    """Runs the command line in `cwd`, with no variable but the ones given,
    and holds it to succeeding."""
    environment = {name: os.environ[name] for name in ("PATH", "SYSTEMROOT") if name in os.environ}
    environment.update(variables or {})
    done = subprocess.run(
        [str(binary()), "--log-level", "error", *arguments],
        cwd=cwd,
        env=environment,
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert done.returncode == 0, f"quantforge {' '.join(arguments)}: {done.stdout}{done.stderr}"
    return done


def kline(index: int, close: str, *, trades: int = 100, volume: str = "0.17235000") -> list:
    """The minute `index` after noon as Binance writes a kline, every price
    `close`: twelve values, the prices and the volume as text."""
    opened = NOON + index * MINUTE
    return [
        opened,
        close,
        close,
        close,
        close,
        volume,
        opened + MINUTE - 1,
        "0.00000000",
        trades,
        "0.00000000",
        "0.00000000",
        "0",
    ]


class Exchange:
    """A Binance Spot that answers on this machine: `GET /api/v3/klines`
    with the klines it was given for a symbol and an interval, as the real
    one pages them, and nothing else."""

    def __init__(self, klines: dict[tuple[str, str], list[list]]):
        self.klines = klines
        # How many klines each answer held, in the order they were asked for.
        self.answers = []
        exchange = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                url = urllib.parse.urlsplit(self.path)
                asked = dict(urllib.parse.parse_qsl(url.query))
                if url.path != "/api/v3/klines":
                    self.send_error(404)
                    return
                rows = exchange.klines.get((asked["symbol"], asked["interval"]), [])
                first = int(asked.get("startTime", -(2**63)))
                last = int(asked.get("endTime", 2**63 - 1))
                page = [row for row in sorted(rows) if first <= row[0] <= last]
                page = page[: int(asked.get("limit", 500))]
                exchange.answers.append(len(page))
                body = json.dumps(page).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *arguments):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        # It looks often for the call that stops it: a test waits for that.
        self.thread = threading.Thread(
            target=self.server.serve_forever, kwargs={"poll_interval": 0.01}, daemon=True
        )
        self.thread.start()

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_address[1]}/"

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.close()


def rfc3339(ms: int) -> str:
    """A whole second since the epoch as the command line's `--start` and
    `--end` take it."""
    import datetime

    assert ms % 1000 == 0, ms
    moment = datetime.datetime.fromtimestamp(ms // 1000, datetime.timezone.utc)
    return moment.strftime("%Y-%m-%dT%H:%M:%SZ")


def sync(exchange: Exchange, symbol: str, interval: str, *, cwd, variables=None, settings=()):
    """`quantforge data sync` of every kline the exchange has for a market,
    into the store the settings and the variables select."""
    rows = exchange.klines[(symbol, interval)]
    opened = [row[0] for row in rows]
    return quantforge(
        *settings,
        "--base-url",
        exchange.base_url,
        "data",
        "sync",
        "--symbol",
        symbol,
        "--interval",
        interval,
        "--start",
        rfc3339(min(opened)),
        "--end",
        rfc3339(max(opened)),
        cwd=cwd,
        variables=variables,
    )


class ServerError(Exception):
    """What a PostgreSQL server refused a statement with."""


class Server:
    """A PostgreSQL server, by the URL of one of its databases: statements
    run on a connection of their own, over plain TCP, and rows come back as
    text, a NULL as `None`."""

    def __init__(self, url: str):
        parts = urllib.parse.urlsplit(url)
        assert parts.scheme in ("postgres", "postgresql"), "a PostgreSQL URL"
        self.url = url
        self.host = parts.hostname
        self.port = parts.port or 5432
        self.user = urllib.parse.unquote(parts.username)
        self.secret = urllib.parse.unquote(parts.password or "")
        self.database = urllib.parse.unquote(parts.path.removeprefix("/"))
        self.parts = parts

    def of(self, database: str) -> "Server":
        """The same server, by the URL of another of its databases."""
        return Server(self.parts._replace(path=f"/{database}").geturl())

    def query(self, sql: str) -> list[list[str | None]]:
        with socket.create_connection((self.host, self.port), timeout=30) as connection:
            wire = _Wire(connection)
            wire.start(self.user, self.database, self.secret)
            return wire.run(sql)

    def value(self, sql: str) -> str | None:
        ((value,),) = self.query(sql)
        return value

    @contextlib.contextmanager
    def session(self):
        """One connection kept open, for what has to outlast a statement,
        such as a lock: `session.run(sql)` runs on it."""
        with socket.create_connection((self.host, self.port), timeout=30) as connection:
            wire = _Wire(connection)
            wire.start(self.user, self.database, self.secret)
            yield wire


class _Wire:
    """One connection's messages, in PostgreSQL's protocol 3."""

    def __init__(self, connection: socket.socket):
        self.connection = connection

    def send(self, kind: bytes, body: bytes):
        self.connection.sendall(kind + struct.pack("!I", len(body) + 4) + body)

    def read(self, count: int) -> bytes:
        data = b""
        while len(data) < count:
            more = self.connection.recv(count - len(data))
            assert more, "the server closed the connection"
            data += more
        return data

    def message(self) -> tuple[bytes, bytes]:
        kind = self.read(1)
        (length,) = struct.unpack("!I", self.read(4))
        body = self.read(length - 4)
        if kind == b"E":
            fields = dict((field[:1], field[1:].decode()) for field in body.split(b"\0") if field)
            raise ServerError(f"{fields.get(b'C')}: {fields.get(b'M')}")
        return kind, body

    def start(self, user: str, database: str, secret: str):
        named = b"user\0" + user.encode() + b"\0database\0" + database.encode() + b"\0\0"
        self.send(b"", struct.pack("!I", 196608) + named)
        while True:
            kind, body = self.message()
            if kind == b"R":
                (asked,) = struct.unpack("!I", body[:4])
                if asked == 10:
                    self.scram(secret, body[4:])
                else:
                    assert asked == 0, f"an authentication these tests speak, not {asked}"
            elif kind == b"Z":
                return

    def scram(self, secret: str, mechanisms: bytes):
        """SCRAM-SHA-256, as RFC 5802 and PostgreSQL's SASL messages have it."""
        assert b"SCRAM-SHA-256" in mechanisms.split(b"\0")
        nonce = base64.b64encode(secrets.token_bytes(18)).decode()
        first = f"n=,r={nonce}"
        opening = b"n,," + first.encode()
        self.send(b"p", b"SCRAM-SHA-256\0" + struct.pack("!I", len(opening)) + opening)
        kind, body = self.message()
        assert kind == b"R" and struct.unpack("!I", body[:4]) == (11,)
        challenge = body[4:].decode()
        given = dict(part.split("=", 1) for part in challenge.split(","))
        assert given["r"].startswith(nonce)
        salted = _salted(secret.encode(), base64.b64decode(given["s"]), int(given["i"]))
        client_key = hmac.digest(salted, b"Client Key", "sha256")
        final = f"c=biws,r={given['r']}"
        signed = f"{first},{challenge},{final}".encode()
        signature = hmac.digest(hashlib.sha256(client_key).digest(), signed, "sha256")
        proof = bytes(a ^ b for a, b in zip(client_key, signature))
        self.send(b"p", f"{final},p={base64.b64encode(proof).decode()}".encode())
        kind, body = self.message()
        assert kind == b"R" and struct.unpack("!I", body[:4]) == (12,)
        server_key = hmac.digest(salted, b"Server Key", "sha256")
        expected = base64.b64encode(hmac.digest(server_key, signed, "sha256")).decode()
        assert body[4:].decode() == f"v={expected}", "the server proved it knows the password"

    def run(self, sql: str) -> list[list[str | None]]:
        self.send(b"Q", sql.encode() + b"\0")
        rows, refused = [], None
        while True:
            try:
                kind, body = self.message()
            except ServerError as error:
                # The server still says when it is ready for the next.
                refused = error
                continue
            if kind == b"D":
                (count,) = struct.unpack("!H", body[:2])
                row, at = [], 2
                for _ in range(count):
                    (length,) = struct.unpack("!i", body[at : at + 4])
                    at += 4
                    if length < 0:
                        row.append(None)
                    else:
                        row.append(body[at : at + length].decode())
                        at += length
                rows.append(row)
            elif kind == b"Z":
                if refused is not None:
                    raise refused
                return rows


@functools.cache
def _salted(secret: bytes, salt: bytes, rounds: int) -> bytes:
    """PBKDF2 with HMAC-SHA-256, one block: a password as SCRAM salts it.
    Written out, since a Python built without OpenSSL has no fast one, and
    kept, since a server salts a password the same way every time."""
    block = last = hmac.digest(secret, salt + b"\x00\x00\x00\x01", "sha256")
    for _ in range(rounds - 1):
        last = hmac.digest(secret, last, "sha256")
        block = (int.from_bytes(block, "big") ^ int.from_bytes(last, "big")).to_bytes(32, "big")
    return block


_databases = itertools.count()


class Database:
    """A database of its own on the test server, dropped as the block that
    made it ends: `server` is the server by its URL."""

    def __init__(self, server: Server):
        name = f"qf_python_{os.getpid()}_{next(_databases)}"
        server.query(f"CREATE DATABASE {name}")
        self.admin = server
        self.name = name
        self.server = server.of(name)

    @property
    def url(self) -> str:
        return self.server.url

    def drop(self):
        self.admin.query(f"DROP DATABASE IF EXISTS {self.name} WITH (FORCE)")

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.drop()


def server() -> Server | None:
    """The PostgreSQL server the tests run on, or `None` to skip: never
    where the run was told that PostgreSQL has to be tested."""
    url = os.environ.get(POSTGRES_URL)
    if url:
        return Server(url)
    if os.environ.get(POSTGRES_REQUIRED) == "1":
        raise AssertionError(
            f"{POSTGRES_URL} is unset, and {POSTGRES_REQUIRED} says PostgreSQL must be tested"
        )
    return None


class Place:
    """Where a store lives, on one backend: what selects it, for the command
    line and for a reader, and its tables in SQL.

    - `arguments` open a reader on it: `CandleReader(**place.arguments)`.
    - `variables` select it for a command, and for a reader left to its
      variables.
    - `directory` is where its commands run, and holds nothing else.
    """

    def __init__(
        self, backend: str, directory: pathlib.Path, arguments: dict, variables: dict, sql
    ):
        self.backend = backend
        self.directory = directory
        self.arguments = arguments
        self.variables = variables
        self._sql = sql

    @classmethod
    def sqlite(cls, directory: pathlib.Path) -> "Place":
        """A file in `directory`, which is not there until a command writes
        it."""
        path = directory / "market.sqlite"

        def sql(statement):
            connection = sqlite3.connect(path, isolation_level=None)
            try:
                return connection.execute(statement).fetchall()
            finally:
                connection.close()

        place = cls("sqlite", directory, {"db": str(path)}, {"QF_DB": str(path)}, sql)
        place.path = path
        place.location = str(path)
        return place

    @classmethod
    def postgres(cls, database: "Database", directory: pathlib.Path) -> "Place":
        """A database of its own, with no table until a command makes
        them."""
        arguments = {"storage": "postgres", "postgres_url": database.url}
        variables = {"QF_STORAGE": "postgres", "QF_POSTGRES_URL": database.url}
        place = cls("postgres", directory, arguments, variables, database.server.query)
        server = database.server
        place.location = f"postgres://{server.host}:{server.port}/{database.name}"
        return place

    def sql(self, statement: str) -> list[list[str | None]]:
        """Every row of a statement run behind the store's back, each value
        as its text.

        A SQLite file is opened here with Python's own sqlite3, a second
        copy of SQLite in the process beside the one the package carries. Two
        copies in one process each drop the other's locks on a file as they
        close it (`docs/python-reader.md`), which is safe here only because
        the connection lives within this call, a reader holds none between
        its calls, and no other process writes the file meanwhile. A test
        that needs another process at the file uses `Holder` or
        `die_writing`."""
        return [
            [None if value is None else str(value) for value in row] for row in self._sql(statement)
        ]

    def value(self, statement: str) -> str | None:
        ((value,),) = self.sql(statement)
        return value

    def sync(self, exchange: Exchange, symbol: str = "BTCUSDT", interval: str = "1m"):
        """`quantforge data sync` of the exchange's klines for a market,
        into this place: the command creates the store where there is
        none."""
        return sync(exchange, symbol, interval, cwd=self.directory, variables=self.variables)

    def store(self, klines: list[list], symbol: str = "BTCUSDT", interval: str = "1m"):
        """The same, from an exchange that has `klines` for the market and
        answers for as long as the command runs."""
        with Exchange({(symbol, interval): klines}) as exchange:
            return self.sync(exchange, symbol, interval)

    def tables(self) -> int:
        """How many tables the database holds, by its own catalog."""
        if self.backend == "sqlite":
            counted = "SELECT count(*) FROM sqlite_master WHERE type = 'table'"
        else:
            counted = "SELECT count(*) FROM pg_tables WHERE schemaname = current_schema()"
        return int(self.value(counted))


# Another process's part, each run by `python -c` with the path of a
# store's file: Python's own sqlite3 in a process of its own, never in the
# one under test, which carries another copy of SQLite (see `Place.sql`).
HOLD = """
import sqlite3, sys
connection = sqlite3.connect(sys.argv[1], isolation_level=None, timeout=30)
# A rollback journal, under which a writer's lock keeps readers out.
connection.execute("PRAGMA journal_mode=DELETE")
connection.execute("BEGIN EXCLUSIVE")
print("held", flush=True)
sys.stdin.readline()
connection.execute("COMMIT")
connection.close()
"""

DIE_WRITING = """
import os, sqlite3, sys
connection = sqlite3.connect(sys.argv[1], isolation_level=None)
mode, statements = sys.argv[2], sys.argv[3:]
if mode == "wal":
    # Frames the writer commits stay in the log, and nothing moves them
    # into the database: it is killed before it closes.
    connection.execute("PRAGMA wal_autocheckpoint=0")
    for statement in statements:
        connection.execute(statement)
else:
    # A rollback journal, and a write that spills pages into the database
    # before it commits: the journal it leaves holds what they were.
    connection.execute("PRAGMA journal_mode=DELETE")
    connection.execute("PRAGMA cache_size=1")
    connection.execute("BEGIN")
    for statement in statements:
        connection.execute(statement)
print("written", flush=True)
os._exit(0)
"""


class Holder:
    """Another process holding a SQLite store's file under an exclusive lock,
    after switching it to a rollback journal, until it is let go."""

    def __init__(self, path: pathlib.Path):
        self.process = subprocess.Popen(
            [sys.executable, "-I", "-c", HOLD, str(path)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        assert self.process.stdout.readline() == "held\n"

    def release(self):
        if self.process.poll() is None:
            self.process.stdin.write("\n")
            self.process.stdin.flush()
            assert self.process.wait(30) == 0

    def __enter__(self):
        return self

    def __exit__(self, *raised):
        self.release()


def die_writing(path: pathlib.Path, mode: str, statements: list[str]):
    """Another process writes the statements to a store's file and is
    killed: in `wal` mode after it committed them, so that its frames stay in
    the log; in `journal` mode before it committed, so that its journal
    stays beside the database."""
    done = subprocess.run(
        [sys.executable, "-I", "-c", DIE_WRITING, str(path), mode, *statements],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert (done.returncode, done.stdout) == (0, "written\n"), done.stderr


def candle_row(
    exchange: str, symbol: str, index: int, close: str, trades="NULL", late: int = 0
) -> str:
    """A candle written in SQL behind a store's back, for what no command
    writes yet: the minute `index` after noon of `symbol` on the product
    line `exchange`, every price `close`, opened `late` milliseconds into
    its minute."""
    opened = NOON + index * MINUTE + late
    return (
        "INSERT INTO candles (exchange, symbol, interval, open_time_ms, close_time_ms, open, "
        "high, low, close, volume, trades) "
        f"VALUES ('{exchange}', '{symbol}', '1m', {opened}, {opened + MINUTE - 1}, '{close}', "
        f"'{close}', '{close}', '{close}', '1', {trades})"
    )


if __name__ == "__main__":
    # `python stores.py <sql>` runs a statement on the test server: a quick
    # look at what a test left.
    for row in Server(os.environ[POSTGRES_URL]).query(sys.argv[1]):
        print(row)
