"""The candle reader, before it reads anything: the class it is, which store
its settings select, an argument over its variable and a variable over the
default, every refusal of settings that names where a value came from and
never the value, and a reader that is closed, shared between threads or
carried into another process.

The stores here are SQLite files the `quantforge` command line wrote
(`stores.py`); a PostgreSQL URL here reaches no server. What a reader reads
of a store, on each backend, is `test_reader_stores.py`.
"""

import copy
import datetime
import inspect
import os
import pathlib
import pickle
import socket
import subprocess
import sys
import threading
import time
import traceback

import pytest

import quantforge_py as qf
import stores

# A user and a password no message may hold.
USER = "reader-sentinel-user"
WORD = "reader-sentinel-word"
# A PostgreSQL URL that reaches nothing: no server listens on the discard
# port of this machine, which refuses a connection at once.
NOWHERE = f"postgres://{USER}:{WORD}@127.0.0.1:9/market"

ARGUMENTS = [
    "storage",
    "db",
    "postgres_url",
    "postgres_pool_size",
    "postgres_connect_timeout_secs",
]


def one_candle(place: stores.Place, close: str) -> stores.Place:
    """A store of one candle, told from another by its close."""
    place.store([stores.kline(0, close)])
    return place


def close_of(reader: qf.CandleReader) -> str:
    """The close of the one candle of the store a reader opened."""
    (close,) = reader.load(qf.MarketId("binance_spot", "BTCUSDT", "1m")).close
    return str(close)


@pytest.fixture
def stored(tmp_path):
    """A SQLite store the command line wrote: three minutes of candles."""
    place = stores.Place.sqlite(tmp_path)
    place.store([stores.kline(index, f"8190{index}.50000000") for index in range(3)])
    return place


def test_the_reader_is_a_class_of_the_package_with_these_members_and_no_other():
    reader = qf.CandleReader
    assert (reader.__module__, reader.__qualname__) == ("quantforge_py", "CandleReader")
    assert repr(reader) == "<class 'quantforge_py.CandleReader'>"
    assert (
        str(inspect.signature(reader))
        == "(*, " + ", ".join(f"{argument}=None" for argument in ARGUMENTS) + ")"
    )
    assert str(inspect.signature(reader.load)) == "(self, /, market, *, start=None, end=None)"
    assert str(inspect.signature(reader.close)) == "(self, /)"
    public = {
        name: type(member).__name__ for name, member in vars(reader).items() if name[0] != "_"
    }
    assert public == {
        "load": "method_descriptor",
        "close": "method_descriptor",
        "backend": "getset_descriptor",
        "location": "getset_descriptor",
        "closed": "getset_descriptor",
    }
    for name in ["__enter__", "__exit__", "__repr__", "__new__"]:
        assert name in vars(reader), name
    for member in [
        reader,
        reader.load,
        reader.close,
        reader.backend,
        reader.location,
        reader.closed,
    ]:
        assert member.__doc__
    with pytest.raises(TypeError, match="not an acceptable base type"):
        type("Derived", (reader,), {})


def test_a_reader_is_itself_and_is_neither_copied_nor_pickled(stored):
    with (
        qf.CandleReader(**stored.arguments) as reader,
        qf.CandleReader(**stored.arguments) as other,
    ):
        # Two readers of one store are two readers.
        assert reader == reader and reader != other
        assert len({reader, other}) == 2
        for copied in (copy.copy, copy.deepcopy, pickle.dumps):
            with pytest.raises(TypeError, match="cannot pickle 'quantforge_py.CandleReader'"):
                copied(reader)
        # And nothing is set on one.
        with pytest.raises(AttributeError):
            reader.location = "elsewhere"
        with pytest.raises(AttributeError):
            reader.note = "x"


def test_a_reader_says_which_store_it_opened_and_whether_it_is_closed(stored):
    reader = qf.CandleReader(**stored.arguments)
    assert (reader.backend, reader.location, reader.closed) == ("sqlite", stored.location, False)
    assert repr(reader) == f"<CandleReader sqlite {stored.location}>"
    assert close_of_first(reader) == "81900.50000000"
    assert reader.close() is None
    assert reader.closed is True
    # Closed, it still says which store it was.
    assert (reader.backend, reader.location) == ("sqlite", stored.location)
    assert repr(reader) == f"<CandleReader sqlite {stored.location}, closed>"
    # Closing it again does nothing.
    assert reader.close() is None and reader.closed is True


def close_of_first(reader: qf.CandleReader) -> str:
    return str(reader.load(qf.MarketId("binance_spot", "BTCUSDT", "1m")).close[0])


def test_a_closed_reader_loads_nothing_and_is_not_entered(stored, refused, market):
    reader = qf.CandleReader(**stored.arguments)
    reader.close()
    refused(ValueError, "the reader is closed", reader.load, market)
    refused(ValueError, "the reader is closed", reader.__enter__)
    with pytest.raises(ValueError, match="the reader is closed"):
        with reader:
            raise AssertionError("a closed reader was entered")


def test_a_with_block_gives_the_reader_and_closes_it_whatever_the_block_raises(stored, market):
    with qf.CandleReader(**stored.arguments) as reader:
        assert type(reader) is qf.CandleReader and not reader.closed
        assert len(reader.load(market)) == 3
    assert reader.closed
    # What the block raises is raised, and the reader is closed all the same.
    with pytest.raises(KeyError, match="mine"):
        with qf.CandleReader(**stored.arguments) as reader:
            raise KeyError("mine")
    assert reader.closed
    # `__exit__` answers nothing, whatever it is handed.
    reader = qf.CandleReader(**stored.arguments)
    assert reader.__exit__(None, None, None) is None and reader.closed


def test_the_default_store_is_the_command_lines_own_default(tmp_path, monkeypatch):
    # Neither is told where the store is: the command line writes it where
    # it runs, and the reader finds it there.
    with stores.Exchange({("BTCUSDT", "1m"): [stores.kline(0, "70000.10000000")]}) as exchange:
        stores.sync(exchange, "BTCUSDT", "1m", cwd=tmp_path)
    assert (tmp_path / "data" / "market.sqlite").is_file()
    monkeypatch.chdir(tmp_path)
    # The path is the command line's own text, on every platform.
    default = "data/market.sqlite"
    with qf.CandleReader() as reader:
        assert (reader.backend, reader.location) == ("sqlite", default)
        assert close_of(reader) == "70000.10000000"
    # Named explicitly, the backend is the same one.
    with qf.CandleReader(storage="sqlite") as reader:
        assert reader.location == default and close_of(reader) == "70000.10000000"


def test_a_reader_keeps_the_file_it_opened_wherever_the_process_goes(tmp_path, monkeypatch):
    # Two directories, each with a store at the default path, told apart by
    # the close of their one candle, and a third with none.
    for name, close in [("here", "1.00000000"), ("there", "2.00000000")]:
        (tmp_path / name).mkdir()
        with stores.Exchange({("BTCUSDT", "1m"): [stores.kline(0, close)]}) as exchange:
            stores.sync(exchange, "BTCUSDT", "1m", cwd=tmp_path / name)
    (tmp_path / "nowhere").mkdir()
    monkeypatch.chdir(tmp_path / "here")
    with qf.CandleReader() as reader, qf.CandleReader(db="data/market.sqlite") as named:
        for elsewhere in ("there", "nowhere", "here"):
            monkeypatch.chdir(tmp_path / elsewhere)
            # The path it shows is the one it was given, and the file it
            # reads the one that path named as it opened.
            assert reader.location == named.location == "data/market.sqlite"
            assert close_of(reader) == close_of(named) == "1.00000000"
        monkeypatch.chdir(tmp_path / "there")
        with qf.CandleReader() as other:
            assert close_of(other) == "2.00000000"
        assert close_of(reader) == "1.00000000"
    assert list((tmp_path / "nowhere").iterdir()) == []


def test_a_variable_is_read_as_python_has_it_in_os_environ(tmp_path, monkeypatch):
    # The store at the default path, and another, told apart by their close.
    with stores.Exchange({("BTCUSDT", "1m"): [stores.kline(0, "1.00000000")]}) as exchange:
        stores.sync(exchange, "BTCUSDT", "1m", cwd=tmp_path)
    (tmp_path / "other").mkdir()
    other = one_candle(stores.Place.sqlite(tmp_path / "other"), "2.00000000")
    monkeypatch.chdir(tmp_path)
    try:
        # The process's own environment alone: not the environment Python has.
        os.putenv("QF_DB", other.location)
        assert "QF_DB" not in os.environ
        with qf.CandleReader() as reader:
            assert close_of(reader) == "1.00000000"
        # The environment Python has alone: read.
        os.environ["QF_DB"] = other.location
        os.unsetenv("QF_DB")
        with qf.CandleReader() as reader:
            assert close_of(reader) == "2.00000000"
    finally:
        os.environ.pop("QF_DB", None)
        os.unsetenv("QF_DB")


def test_a_connect_timeout_bounds_the_wait_for_a_server_that_never_answers(
    storage_backends, monkeypatch
):
    if "postgres" not in storage_backends:
        pytest.skip("the SQLite-only build opens no PostgreSQL store")
    # A server that takes connections and never says a word. It looks at
    # its stop every tenth of a second: closing a socket from another thread
    # does not always wake a call that waits on it.
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(16)
    listener.settimeout(0.1)
    taken = []
    stop = threading.Event()

    def take():
        while not stop.is_set():
            try:
                taken.append(listener.accept()[0])
            except TimeoutError:
                continue
            except OSError:
                return

    taker = threading.Thread(target=take, daemon=True)
    taker.start()
    silent = f"postgres://{USER}:{WORD}@127.0.0.1:{listener.getsockname()[1]}/market"
    said = (
        f"cannot open the postgres store at postgres://127.0.0.1:{listener.getsockname()[1]}/"
        "market: storage unavailable: cannot get a connection: the server did not answer in time"
    )

    def waited(seconds_given, **arguments):
        started = time.monotonic()
        with pytest.raises(ConnectionError) as caught:
            qf.CandleReader(storage="postgres", postgres_url=silent, **arguments)
        waited = time.monotonic() - started
        assert str(caught.value) == said
        # Given up on after the timeout, and long before the default five
        # seconds where the timeout is shorter.
        assert seconds_given - 0.1 <= waited < seconds_given + 2.5, waited

    try:
        waited(1, postgres_connect_timeout_secs=1)
        with monkeypatch.context() as patched:
            patched.setenv("QF_POSTGRES_CONNECT_TIMEOUT_SECS", "2")
            waited(2)
            # The argument over the variable.
            waited(1, postgres_connect_timeout_secs=1)
    finally:
        stop.set()
        taker.join(5)
        listener.close()
        for connection in taken:
            connection.close()
    assert not taker.is_alive()


def test_an_argument_wins_over_its_variable_and_a_variable_over_the_default(
    tmp_path, storage_backends, monkeypatch
):
    # Three stores, told apart by the close of their one candle.
    for name in ("default", "variable", "argument"):
        (tmp_path / name).mkdir()
    working = tmp_path / "default"
    with stores.Exchange({("BTCUSDT", "1m"): [stores.kline(0, "1.00000000")]}) as exchange:
        stores.sync(exchange, "BTCUSDT", "1m", cwd=working)
    by_variable = one_candle(stores.Place.sqlite(tmp_path / "variable"), "2.00000000")
    by_argument = one_candle(stores.Place.sqlite(tmp_path / "argument"), "3.00000000")
    monkeypatch.chdir(working)

    with qf.CandleReader() as reader:
        assert close_of(reader) == "1.00000000"
    monkeypatch.setenv("QF_DB", by_variable.location)
    with qf.CandleReader() as reader:
        assert (reader.location, close_of(reader)) == (by_variable.location, "2.00000000")
    with qf.CandleReader(db=by_argument.location) as reader:
        assert (reader.location, close_of(reader)) == (by_argument.location, "3.00000000")
    # `None` leaves an argument out, as not writing it does.
    with qf.CandleReader(db=None, storage=None) as reader:
        assert close_of(reader) == "2.00000000"
    # A path may be an `os.PathLike`.
    with qf.CandleReader(db=pathlib.Path(by_argument.location)) as reader:
        assert (reader.location, close_of(reader)) == (by_argument.location, "3.00000000")

    # One whose `__fspath__` is a descriptor that gives its class nothing
    # and an instance its method, which `os.fspath` takes, as the reader does.
    class Lazily:
        def __get__(self, instance, owner=None):
            return None if instance is None else (lambda: by_argument.location)

    class Located:
        __fspath__ = Lazily()

    assert Located.__fspath__ is None and os.fspath(Located()) == by_argument.location
    with qf.CandleReader(db=Located()) as reader:
        assert (reader.location, close_of(reader)) == (by_argument.location, "3.00000000")

    # And one whose type says it has no `__fspath__` is no path.
    class Unpathed:
        __fspath__ = None

    with pytest.raises(
        TypeError, match="^db must be a str or an os.PathLike of one, not Unpathed$"
    ):
        qf.CandleReader(db=Unpathed())

    # The backend is settled the same way: the variable selects PostgreSQL,
    # and the argument SQLite, which is opened without PostgreSQL's URL
    # being so much as read.
    monkeypatch.setenv("QF_STORAGE", "postgres")
    monkeypatch.setenv("QF_POSTGRES_URL", "not a url at all")
    monkeypatch.delenv("QF_DB")
    with qf.CandleReader(storage="sqlite") as reader:
        assert (reader.backend, close_of(reader)) == ("sqlite", "1.00000000")
    with qf.CandleReader(storage="sqlite", db=by_argument.location) as reader:
        assert close_of(reader) == "3.00000000"
    if "postgres" not in storage_backends:
        return
    # And the other way round: the argument selects PostgreSQL, whose URL
    # the variable holds, and it is that server which is asked.
    monkeypatch.setenv("QF_STORAGE", "sqlite")
    monkeypatch.setenv("QF_POSTGRES_URL", NOWHERE)
    with pytest.raises(ConnectionError, match="^cannot open the postgres store at "):
        qf.CandleReader(storage="postgres")
    # An argument's URL wins over the variable's.
    monkeypatch.setenv("QF_POSTGRES_URL", "not a url at all")
    with pytest.raises(ConnectionError, match="^cannot open the postgres store at "):
        qf.CandleReader(storage="postgres", postgres_url=NOWHERE)


POSTGRES = dict(storage="postgres", postgres_url=NOWHERE)


class BytesPath:
    """A path that gives itself as bytes: a file named in an encoding
    nothing here knows."""

    def __fspath__(self):
        return b"market.sqlite"


# What is refused before anything is opened: the arguments, the variables,
# the exception and its message. A refusal names the argument or the
# variable a value came from.
REFUSED_SETTINGS = [
    (dict(storage=1), {}, TypeError, "storage must be a str, not int"),
    (dict(storage=b"sqlite"), {}, TypeError, "storage must be a str, not bytes"),
    (dict(storage="mysql"), {}, ValueError, "storage must be sqlite or postgres"),
    (dict(storage="SQLite"), {}, ValueError, "storage must be sqlite or postgres"),
    (dict(storage=""), {}, ValueError, "storage must be sqlite or postgres"),
    ({}, {"QF_STORAGE": "mysql"}, ValueError, "QF_STORAGE must be sqlite or postgres"),
    ({}, {"QF_STORAGE": ""}, ValueError, "QF_STORAGE must be sqlite or postgres"),
    (dict(db=3), {}, TypeError, "db must be a str or an os.PathLike of one, not int"),
    (
        dict(db=b"market.sqlite"),
        {},
        TypeError,
        "db must be a str or an os.PathLike of one, not bytes",
    ),
    (
        dict(db=BytesPath()),
        {},
        TypeError,
        "db must be a str or an os.PathLike of one, not BytesPath",
    ),
    (
        dict(db=""),
        {},
        ValueError,
        "db is empty; name the SQLite database file, or leave it unset",
    ),
    (
        {},
        {"QF_DB": ""},
        ValueError,
        "QF_DB is empty; name the SQLite database file, or leave it unset",
    ),
    (
        dict(db=NOWHERE),
        {},
        ValueError,
        "db takes a SQLite file's path, and holds a PostgreSQL URL: give the URL as postgres_url "
        'or in QF_POSTGRES_URL, with storage="postgres" or QF_STORAGE=postgres',
    ),
    # Written as it is written elsewhere: by `pathlib`, which takes a `/`
    # out of `//`, for SQLAlchemy, with a driver after a `+`, quoted as in
    # an `.env` file, or after some space.
    *[
        (
            dict(db=written),
            {},
            ValueError,
            "db takes a SQLite file's path, and holds a PostgreSQL URL: give the URL as "
            'postgres_url or in QF_POSTGRES_URL, with storage="postgres" or QF_STORAGE=postgres',
        )
        for written in [
            pathlib.Path(NOWHERE),
            NOWHERE.replace("postgres://", "postgresql+psycopg2://"),
            f"'{NOWHERE}'",
            f'"{NOWHERE}"',
            f" \t{NOWHERE}",
        ]
    ],
    (
        dict(db="market\0.sqlite"),
        {},
        ValueError,
        "db holds a NUL character, which no file's path holds",
    ),
    # Text Python cannot write as UTF-8: Python's own error would hold all
    # of it, the password with the rest.
    (
        dict(storage="\udcff" + NOWHERE),
        {},
        ValueError,
        "storage is no text that can be written as UTF-8: it holds a lone surrogate",
    ),
    (
        dict(db=NOWHERE + "\udcff"),
        {},
        ValueError,
        "db is no text that can be written as UTF-8: it holds a lone surrogate",
    ),
    (
        dict(storage="postgres", postgres_url=NOWHERE + "\udcff"),
        {},
        ValueError,
        "postgres_url is no text that can be written as UTF-8: it holds a lone surrogate",
    ),
    (
        {},
        {"QF_DB": NOWHERE.replace("postgres://", "PostgreSQL://")},
        ValueError,
        "QF_DB takes a SQLite file's path, and holds a PostgreSQL URL: give the URL as "
        'postgres_url or in QF_POSTGRES_URL, with storage="postgres" or QF_STORAGE=postgres',
    ),
    (
        dict(storage="postgres"),
        {},
        ValueError,
        "postgres storage needs a connection URL: postgres://USER:PASSWORD@HOST:PORT/DATABASE; "
        "give it as postgres_url, or in QF_POSTGRES_URL, which keeps a password out of the code",
    ),
    (
        {},
        {"QF_STORAGE": "postgres"},
        ValueError,
        "postgres storage needs a connection URL: postgres://USER:PASSWORD@HOST:PORT/DATABASE; "
        "give it as postgres_url, or in QF_POSTGRES_URL, which keeps a password out of the code",
    ),
    (
        dict(postgres_url=NOWHERE),
        {},
        ValueError,
        "postgres_url is a PostgreSQL setting, but no storage backend is selected, so this would "
        "use SQLite; select the postgres backend to use it, or the sqlite backend explicitly to "
        'keep SQLite: pass storage="postgres" or storage="sqlite", or set QF_STORAGE',
    ),
    (
        {},
        {"QF_POSTGRES_URL": NOWHERE},
        ValueError,
        "QF_POSTGRES_URL is a PostgreSQL setting, but no storage backend is selected, so this "
        "would use SQLite; select the postgres backend to use it, or the sqlite backend "
        'explicitly to keep SQLite: pass storage="postgres" or storage="sqlite", or set QF_STORAGE',
    ),
    (
        dict(postgres_pool_size=4),
        {},
        ValueError,
        "postgres_pool_size is a PostgreSQL setting, but no storage backend is selected, so this "
        "would use SQLite; select the postgres backend to use it, or the sqlite backend "
        'explicitly to keep SQLite: pass storage="postgres" or storage="sqlite", or set QF_STORAGE',
    ),
    (
        dict(POSTGRES, db="market.sqlite"),
        {},
        ValueError,
        "db names a SQLite file, but the postgres backend is selected; unset it, or select the "
        "sqlite backend",
    ),
    (
        POSTGRES,
        {"QF_DB": "market.sqlite"},
        ValueError,
        "QF_DB names a SQLite file, but the postgres backend is selected; unset it, or select "
        "the sqlite backend",
    ),
    (dict(POSTGRES, postgres_url=7), {}, TypeError, "postgres_url must be a str, not int"),
    (
        dict(POSTGRES, postgres_url="market"),
        {},
        ValueError,
        "postgres_url is not a usable PostgreSQL URL: it does not parse as a URL (relative URL "
        "without a base); write postgres://USER:PASSWORD@HOST:PORT/DATABASE",
    ),
    (
        dict(storage="postgres"),
        {"QF_POSTGRES_URL": f"https://{USER}:{WORD}@db.example.com/market"},
        ValueError,
        "QF_POSTGRES_URL is not a usable PostgreSQL URL: its scheme must be postgres:// or "
        "postgresql://",
    ),
    (
        dict(
            storage="postgres",
            postgres_url=f"postgres://{USER}:{WORD}@db.example.com/market?sslmode=disable",
        ),
        {},
        ValueError,
        "postgres_url is not a usable PostgreSQL URL: sslmode=disable would allow plain text to "
        "a host other than this machine; only loopback may skip TLS, so use sslmode=verify-full "
        "(or require)",
    ),
    (
        dict(
            storage="postgres", postgres_url=f"postgres://{USER}:{WORD}@127.0.0.1/market?{WORD}=1"
        ),
        {},
        ValueError,
        "postgres_url is not a usable PostgreSQL URL: it has a parameter QuantForge does not "
        "read; only sslmode and sslrootcert are",
    ),
    (
        dict(POSTGRES, postgres_pool_size=0),
        {},
        ValueError,
        "postgres_pool_size must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    (
        dict(POSTGRES, postgres_pool_size=-2),
        {},
        ValueError,
        "postgres_pool_size must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    (
        dict(POSTGRES, postgres_pool_size=2**32),
        {},
        ValueError,
        "postgres_pool_size must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    # A pool larger than a server serves: room for all of it is made as
    # it is built, and four billion asked for memory there was none of.
    (
        dict(POSTGRES, postgres_pool_size=262_144),
        {},
        ValueError,
        "postgres_pool_size must be a whole number from 1 to 262143, the most connections a "
        "PostgreSQL server serves",
    ),
    (
        dict(POSTGRES, postgres_pool_size=True),
        {},
        TypeError,
        "postgres_pool_size must be an int, not bool",
    ),
    (
        dict(POSTGRES, postgres_pool_size="2"),
        {},
        TypeError,
        "postgres_pool_size must be an int, not str",
    ),
    (
        dict(POSTGRES, postgres_pool_size=2.0),
        {},
        TypeError,
        "postgres_pool_size must be an int, not float",
    ),
    (
        POSTGRES,
        {"QF_POSTGRES_POOL_SIZE": "two"},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    (
        POSTGRES,
        {"QF_POSTGRES_POOL_SIZE": ""},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    # Digits and nothing around them, as the command line reads a number.
    (
        POSTGRES,
        {"QF_POSTGRES_POOL_SIZE": " 4"},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a "
        "PostgreSQL server serves",
    ),
    (
        POSTGRES,
        {"QF_POSTGRES_POOL_SIZE": "262144"},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a "
        "PostgreSQL server serves",
    ),
    (
        POSTGRES,
        {"QF_POSTGRES_POOL_SIZE": "0"},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
    (
        dict(POSTGRES, postgres_connect_timeout_secs=0),
        {},
        ValueError,
        "postgres_connect_timeout_secs must be a whole number from 1 to 18446744073709551615",
    ),
    (
        dict(POSTGRES, postgres_connect_timeout_secs=2**64),
        {},
        ValueError,
        "postgres_connect_timeout_secs must be a whole number from 1 to 18446744073709551615",
    ),
    (
        dict(POSTGRES, postgres_connect_timeout_secs=1.5),
        {},
        TypeError,
        "postgres_connect_timeout_secs must be an int, not float",
    ),
    (
        POSTGRES,
        {"QF_POSTGRES_CONNECT_TIMEOUT_SECS": "5s"},
        ValueError,
        "QF_POSTGRES_CONNECT_TIMEOUT_SECS must be a whole number from 1 to 18446744073709551615",
    ),
    # A variable that is set is held to what it is, whichever backend is
    # selected: as on the command line, a wrong one is never passed over.
    (
        dict(storage="sqlite"),
        {"QF_POSTGRES_POOL_SIZE": "two"},
        ValueError,
        "QF_POSTGRES_POOL_SIZE must be a whole number from 1 to 262143, the most connections a PostgreSQL "
        "server serves",
    ),
]


@pytest.mark.parametrize(
    "arguments, variables, kind, message",
    REFUSED_SETTINGS,
    ids=[f"{index}: {message[:44]}" for index, (_, _, _, message) in enumerate(REFUSED_SETTINGS)],
)
def test_settings_are_refused_before_anything_is_opened_by_where_they_came_from(
    arguments, variables, kind, message, tmp_path, refused, storage_backends, monkeypatch
):
    if "postgres" not in storage_backends and "postgres" in (
        arguments.get("storage"),
        variables.get("QF_STORAGE"),
    ):
        pytest.skip("the SQLite-only build refuses the backend before its settings")
    monkeypatch.chdir(tmp_path)
    for variable, value in variables.items():
        monkeypatch.setenv(variable, value)
    refused(kind, message, qf.CandleReader, **arguments)
    # Nothing was made on the way.
    assert list(tmp_path.iterdir()) == []


def test_what_a_values_own_method_raises_is_raised_as_it_is(stored, market):
    class Interrupted(Exception):
        """No exception the package raises of its own accord."""

    class FailingPath:
        def __fspath__(self):
            raise Interrupted("the path's own")

    class FailingTime(datetime.datetime):
        def utcoffset(self):
            raise Interrupted("the time's own")

    with pytest.raises(Interrupted, match="^the path's own$"):
        qf.CandleReader(db=FailingPath())
    with qf.CandleReader(**stored.arguments) as reader:
        for name in ("start", "end"):
            moment = FailingTime(2026, 1, 1, tzinfo=datetime.timezone.utc)
            with pytest.raises(Interrupted, match="^the time's own$"):
                reader.load(market, **{name: moment})

        # And an interrupt stays one.
        class Stopped(datetime.datetime):
            def utcoffset(self):
                raise KeyboardInterrupt

        with pytest.raises(KeyboardInterrupt):
            reader.load(market, start=Stopped(2026, 1, 1, tzinfo=datetime.timezone.utc))


def test_a_time_beyond_the_engines_milliseconds_is_refused(stored, market, refused):
    # No `datetime` of Python's own lies that far from the epoch; a type
    # derived from one may say it does.
    class Far:
        def __floordiv__(self, other):
            return 2**70

        def __neg__(self):
            return self

    class Beyond(datetime.datetime):
        def __sub__(self, other):
            return Far()

    beyond = Beyond(2026, 1, 1, tzinfo=datetime.timezone.utc)
    with qf.CandleReader(**stored.arguments) as reader:
        for name in ("start", "end"):
            refused(
                ValueError,
                f"invalid {name}: must be between -9223372036854775808 and 9223372036854775807 "
                "milliseconds from the Unix epoch",
                reader.load,
                market,
                **{name: beyond},
            )


def test_text_that_is_no_time_is_quoted_to_eighty_characters(stored, market):
    with qf.CandleReader(**stored.arguments) as reader:
        for text, shown in [
            ("x" * 80, "x" * 80),
            ("x" * 81, "x" * 80 + "..."),
            ("é" * 200, "é" * 80 + "..."),
        ]:
            with pytest.raises(ValueError) as caught:
                reader.load(market, start=text)
            said = str(caught.value)
            assert said.startswith(f"invalid start '{shown}': time parse error: "), said
        # No time is written with an @ or an =, and a URL or a connection
        # string with a password is: such text is not quoted at all.
        for text in [NOWHERE, f"host=db.example.com user={USER} password={WORD}"]:
            for edge in ("start", "end"):
                with pytest.raises(ValueError) as caught:
                    reader.load(market, **{edge: text})
                said = everything_said(caught.value)
                assert USER not in said and WORD not in said, said
                assert str(caught.value).startswith(
                    f"invalid {edge} (not shown: it holds an @ or an =, as a URL or a connection "
                    "string with a password does): time parse error: "
                ), str(caught.value)
        # Nor is text with a lone surrogate, which Python's own error would
        # carry whole, as it would a setting: a time, a URL and a connection
        # string, each with one.
        for text in [
            "2026-01-01T00:00:00Z\udcff",
            NOWHERE + "\udcff",
            f"host=db.example.com user={USER} password={WORD}\udcff",
        ]:
            for edge in ("start", "end"):
                with pytest.raises(ValueError) as caught:
                    reader.load(market, **{edge: text})
                said = everything_said(caught.value)
                assert USER not in said and WORD not in said and "2026-01-01" not in said, said
                assert str(caught.value) == (
                    f"{edge} is no text that can be written as UTF-8: it holds a lone surrogate"
                )


def everything_said(error: BaseException) -> str:
    """All of an exception a person or a log may see: its text, its `repr`,
    its arguments and every other attribute it has, its notes, what it came
    from, and its traceback as Python prints it."""
    said = [str(error), repr(error), repr(error.args), *getattr(error, "__notes__", [])]
    said += [f"{name}={getattr(error, name, None)!r}" for name in dir(error) if name[:2] != "__"]
    said += traceback.format_exception(error)
    for linked in (error.__cause__, error.__context__):
        if linked is not None:
            said.append(everything_said(linked))
    return "\n".join(said)


# What a user may write where a SQLite file's path belongs, each holding the
# user and the password: a URL in every form it is written in elsewhere,
# and a connection string.
NOT_A_PATH = [
    NOWHERE,
    NOWHERE.upper(),
    NOWHERE.replace("postgres://", "postgresql+psycopg2://"),
    NOWHERE.replace("postgres://", "postgres+asyncpg://"),
    NOWHERE.replace("postgres://", "postgres:/"),
    NOWHERE.replace("postgres://", "postgres:"),
    NOWHERE.replace("postgres://", "psql://"),
    NOWHERE.replace("postgres://", "jdbc:postgresql://"),
    NOWHERE.replace("postgres://", "file:postgres://"),
    NOWHERE.replace("postgres://", "url="),
    NOWHERE.replace("postgres://", "//"),
    NOWHERE.removeprefix("postgres://"),
    f"jdbc:postgresql://127.0.0.1/market?user={USER}&password={WORD}",
    f"host=127.0.0.1 port=9 dbname=market user={USER} password={WORD}",
    f"'{NOWHERE}'",
    f" \t\n{NOWHERE}",
]


def test_no_refusal_and_no_failure_says_a_password_or_a_user(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    # The URL in every place a value may be written, its own and the others'.
    attempts = [
        (dict(db=NOWHERE), {}),
        (dict(storage=NOWHERE), {}),
        ({}, {"QF_DB": NOWHERE}),
        ({}, {"QF_STORAGE": NOWHERE}),
        (dict(postgres_url=NOWHERE), {}),
        ({}, {"QF_POSTGRES_URL": NOWHERE}),
        (dict(storage="postgres", postgres_url=NOWHERE, db="market.sqlite"), {}),
        (dict(storage="postgres", postgres_url=NOWHERE, postgres_pool_size=0), {}),
        (
            dict(
                storage="postgres",
                postgres_url=NOWHERE.replace("127.0.0.1", "db.example.com") + "?sslmode=disable",
            ),
            {},
        ),
        (
            dict(
                storage="postgres",
                postgres_url=NOWHERE + "?sslrootcert=/no/such/ca.pem&sslmode=require",
            ),
            {},
        ),
        (dict(storage="postgres", postgres_url=NOWHERE + "#fragment"), {}),
        (dict(storage="postgres", postgres_url=NOWHERE.replace("postgres://", "mysql://")), {}),
        (dict(storage="postgres", postgres_url=NOWHERE.replace("127.0.0.1:9", "a,b")), {}),
        (dict(storage="postgres", postgres_url=f"{USER}:{WORD}"), {}),
        (dict(storage="postgres"), {"QF_POSTGRES_URL": NOWHERE + "&&"}),
        (dict(storage="postgres"), {"QF_POSTGRES_POOL_SIZE": NOWHERE, "QF_POSTGRES_URL": NOWHERE}),
        (
            dict(storage="postgres"),
            {"QF_POSTGRES_CONNECT_TIMEOUT_SECS": WORD, "QF_POSTGRES_URL": NOWHERE},
        ),
        # A password whose own `@` and `/` were not percent-encoded: what
        # follows the `/` would be taken for the database's name.
        (
            dict(storage="postgres", postgres_url=f"postgres://{USER}:pa@ss/{WORD}@db.example.com"),
            {},
        ),
        (dict(storage="postgres", postgres_url=f"postgres://{USER}:p@127.0.0.1/{WORD}@x"), {}),
        # Text that is no UTF-8, in each place.
        (dict(storage="\udcff" + NOWHERE), {}),
        (dict(db=NOWHERE + "\udcff"), {}),
        (dict(db=pathlib.Path(NOWHERE + "\udcff")), {}),
        (dict(storage="postgres", postgres_url=NOWHERE + "\udcff"), {}),
        # Everything a user may write in a path's place.
        *[(dict(db=written), {}) for written in NOT_A_PATH],
        *[(dict(db=pathlib.Path(written)), {}) for written in NOT_A_PATH],
        *[(dict(storage="sqlite", db=written), {}) for written in NOT_A_PATH],
        *[({}, {"QF_DB": written}) for written in NOT_A_PATH],
        # And the connection itself, which is refused.
        (dict(storage="postgres", postgres_url=NOWHERE, postgres_connect_timeout_secs=5), {}),
        (dict(storage="postgres"), {"QF_POSTGRES_URL": NOWHERE}),
    ]
    if os.name != "nt":
        # A variable whose bytes are no UTF-8, as `os.environ` holds one.
        attempts += [
            ({}, {"QF_DB": NOWHERE + "\udcff"}),
            ({}, {"QF_STORAGE": "\udcff" + NOWHERE}),
            (dict(storage="postgres"), {"QF_POSTGRES_URL": NOWHERE + "\udcff"}),
        ]
    for arguments, variables in attempts:
        with monkeypatch.context() as patched:
            for variable, value in variables.items():
                patched.setenv(variable, value)
            with pytest.raises((ValueError, ConnectionError, FileNotFoundError)) as caught:
                qf.CandleReader(**arguments)
        said = everything_said(caught.value)
        assert WORD not in said and USER not in said, (arguments, variables)
    assert list(tmp_path.iterdir()) == []


def test_the_sqlite_only_build_refuses_postgresql_and_opens_no_sqlite_file_in_its_place(
    tmp_path, storage_backends, refused, monkeypatch
):
    monkeypatch.chdir(tmp_path)
    if "postgres" in storage_backends:
        # The published build has it: selecting it reaches for the server.
        with pytest.raises(ConnectionError):
            qf.CandleReader(**POSTGRES)
    else:
        said = (
            "postgres storage is not in this build, which was compiled without the `postgres` "
            "feature; use a build with default features, or select the sqlite backend; "
            "quantforge_py.STORAGE_BACKENDS names the stores this build opens"
        )
        refused(ValueError, said, qf.CandleReader, **POSTGRES)
        monkeypatch.setenv("QF_STORAGE", "postgres")
        refused(ValueError, said, qf.CandleReader)
        assert qf.STORAGE_BACKENDS == ("sqlite",)
    # Either way no SQLite file was opened or made instead.
    assert list(tmp_path.iterdir()) == []


def test_a_path_that_may_be_a_secret_is_not_shown_where_no_file_is_there(
    tmp_path, monkeypatch, refused
):
    monkeypatch.chdir(tmp_path)

    # What holds an @ or an = is not shown: a URL with a password, or a
    # connection string, given in a path's place, is no file at all. Nor is
    # the full path it was looked for at, which holds it.
    def hidden(written):
        return (
            "there is no SQLite database at the path given, which is not shown: it holds an @ or "
            "an =, as a URL or a connection string with a password does: a reader creates none, "
            "and `quantforge data sync` does; db, or QF_DB, names another file"
        )

    for written in NOT_A_PATH + ["data/a=b.sqlite", "data/someone@example.sqlite"]:
        if written.lstrip(" \t\n'").lower().startswith("postgres"):
            # Refused before it is looked for, as a URL.
            continue
        if os.name == "nt" and any(character in written for character in ':*?"<>|'):
            # Windows may refuse such a name before it looks for a file
            # there: a path that cannot be reached, not shown either.
            with pytest.raises((FileNotFoundError, ValueError)) as caught:
                qf.CandleReader(db=written)
            said = str(caught.value)
            assert (type(caught.value) is FileNotFoundError and said == hidden(written)) or (
                type(caught.value) is ValueError
                and said.startswith("cannot reach the path given: ")
            ), said
            continue
        refused(FileNotFoundError, hidden(written), qf.CandleReader, db=written)
    # Nor where what is there is no file.
    (tmp_path / "user=SOMEONE password=WORD").mkdir()
    refused(
        ValueError,
        "the path given is a directory, and a SQLite database is a file",
        qf.CandleReader,
        db="user=SOMEONE password=WORD",
    )
    (tmp_path / "user=SOMEONE password=WORD").rmdir()
    # A path that holds neither is shown, and where it was looked for.
    refused(
        FileNotFoundError,
        f"there is no SQLite database at data/x.sqlite (looked for at "
        f"{tmp_path / 'data' / 'x.sqlite'}): a reader creates none, and `quantforge data sync` "
        "does; db, or QF_DB, names another file",
        qf.CandleReader,
        db="data/x.sqlite",
    )
    assert list(tmp_path.iterdir()) == []


def test_where_a_file_was_looked_for_is_not_shown_where_the_directory_may_be_a_secret(
    tmp_path, monkeypatch, refused
):
    # The path is shown, but not the full path it was looked for at, where
    # the directory the process runs in holds an @ or an =.
    odd = tmp_path / f"user={USER} password={WORD}@db"
    odd.mkdir()
    monkeypatch.chdir(odd)
    refused(
        FileNotFoundError,
        "there is no SQLite database at missing.sqlite: a reader creates none, and `quantforge "
        "data sync` does; db, or QF_DB, names another file",
        qf.CandleReader,
        db="missing.sqlite",
    )
    with pytest.raises(FileNotFoundError) as caught:
        qf.CandleReader(db="missing.sqlite")
    said = everything_said(caught.value)
    assert USER not in said and WORD not in said, said
    assert list(odd.iterdir()) == []


@pytest.mark.skipif(os.name == "nt", reason="Windows removes no directory a process runs in")
def test_a_path_that_may_be_a_secret_is_not_shown_where_the_process_has_no_directory(
    tmp_path, monkeypatch
):
    # Where the process runs is gone, so a path that does not start at the
    # root cannot be settled: the failure names the store without the path.
    gone = tmp_path / "gone"
    gone.mkdir()
    monkeypatch.chdir(gone)
    gone.rmdir()
    # A URL without its scheme, which is not refused as one, and a
    # connection string: each is taken for a path.
    for written in [
        f"{USER}:{WORD}@127.0.0.1:9/market",
        f"host=db.example.com user={USER} password={WORD}",
    ]:
        with pytest.raises(RuntimeError) as caught:
            qf.CandleReader(db=written)
        said = everything_said(caught.value)
        assert USER not in said and WORD not in said, said
        assert str(caught.value).startswith("cannot open the sqlite store at the path given: "), (
            str(caught.value)
        )


def test_a_sqlite_file_that_is_not_there_is_refused_and_not_created(tmp_path, monkeypatch, refused):
    monkeypatch.chdir(tmp_path)
    # The default path, and any other that does not start at the root, is
    # looked for where the process runs: the refusal says where that is.
    refused(
        FileNotFoundError,
        f"there is no SQLite database at data/market.sqlite (looked for at "
        f"{tmp_path / 'data' / 'market.sqlite'}): a reader creates none, and `quantforge data "
        "sync` does; db, or QF_DB, names another file",
        qf.CandleReader,
    )
    absolute = tmp_path / "elsewhere" / "market.sqlite"
    refused(
        FileNotFoundError,
        f"there is no SQLite database at {absolute}: a reader creates none, and `quantforge data "
        "sync` does; db, or QF_DB, names another file",
        qf.CandleReader,
        db=absolute,
    )
    monkeypatch.setenv("QF_DB", str(absolute))
    with pytest.raises(FileNotFoundError):
        qf.CandleReader(storage="sqlite")
    assert list(tmp_path.iterdir()) == []


def test_load_takes_a_market_and_nothing_that_stands_for_one(stored, refused):
    with qf.CandleReader(**stored.arguments) as reader:
        for value, name in [
            ("binance_spot BTCUSDT 1m", "str"),
            (("binance_spot", "BTCUSDT", "1m"), "tuple"),
            (None, "NoneType"),
            (qf.Symbol("BTCUSDT"), "Symbol"),
        ]:
            refused(TypeError, f"market must be a MarketId, not {name}", reader.load, value)
        with pytest.raises(TypeError, match="missing 1 required positional argument: 'market'"):
            reader.load()
        with pytest.raises(TypeError, match="takes 1 positional argument"):
            reader.load(qf.MarketId("binance_spot", "BTCUSDT", "1m"), 0)
        # By its name too.
        assert len(reader.load(market=qf.MarketId("binance_spot", "BTCUSDT", "1m"))) == 3


def test_a_reader_is_used_by_many_threads_at_once(stored, market):
    with qf.CandleReader(**stored.arguments) as reader:
        expected = reader.load(market)
        loaded, failures = [], []

        def load():
            try:
                for _ in range(25):
                    loaded.append(reader.load(market))
            except BaseException as failure:  # noqa: BLE001: reported below
                failures.append(failure)

        threads = [threading.Thread(target=load, daemon=True) for _ in range(8)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join(60)
        assert not failures, failures
        assert len(loaded) == 200 and all(batch == expected for batch in loaded)
        # Closed by one thread, it is closed for all.
        closer = threading.Thread(target=reader.close)
        closer.start()
        closer.join(60)
        assert reader.closed


# What a process does with a reader it carries into a forked child: it says
# what the child found, on its standard output. It runs in an interpreter of
# its own, which has no thread but the reader's.
FORKED = """
import os
import quantforge_py as qf

market = qf.MarketId("binance_spot", "BTCUSDT", "1m")
reader = qf.CandleReader()
assert len(reader.load(market)) == 3
read, write = os.pipe()
child = os.fork()
if child == 0:
    said = "nothing"
    try:
        try:
            reader.load(market)
            said = "loaded"
        except RuntimeError as refusal:
            said = f"RuntimeError: {refusal}"
        # A reader of its own reads, and the inherited one closes.
        with qf.CandleReader() as own:
            said += f" | own: {len(own.load(market))}"
        reader.close()
        said += f" | closed: {reader.closed}"
    finally:
        os.write(write, said.encode())
        os._exit(0)
os.close(write)
with os.fdopen(read, "rb") as pipe:
    said = pipe.read().decode()
assert os.waitpid(child, 0)[1] == 0
# The parent's reader is as it was.
assert not reader.closed and len(reader.load(market)) == 3
print(os.getpid(), child, said)
"""


@pytest.mark.skipif(not hasattr(os, "fork"), reason="no fork on this platform")
def test_a_reader_belongs_to_the_process_that_opened_it(place):
    place.store([stores.kline(index, f"8190{index}.50000000") for index in range(3)])
    # A process that forks with threads of its own is warned by Python from
    # 3.12 on, and PostgreSQL's reader has them: the warning is not the
    # test's matter.
    done = subprocess.run(
        [sys.executable, "-W", "ignore::DeprecationWarning", "-c", FORKED],
        cwd=place.directory,
        env=dict(os.environ, **place.variables),
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert (done.returncode, done.stderr) == (0, ""), done.stderr
    parent, child, said = done.stdout.strip().split(" ", 2)
    assert said == (
        f"RuntimeError: this reader was opened by process {parent}, and this is process "
        f"{child}: a reader belongs to the process that opened it; open one here"
        " | own: 3 | closed: True"
    )


def test_importing_the_package_opens_no_store_and_reads_no_setting(tmp_path):
    # In a directory with no store, under settings that would be refused:
    # importing the package looks at none of it.
    environment = dict(os.environ, QF_STORAGE="nonsense", QF_DB="", QF_POSTGRES_URL="nonsense")
    done = subprocess.run(
        [sys.executable, "-c", "import quantforge_py as qf; print(qf.CandleReader.__name__)"],
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert (done.returncode, done.stdout, done.stderr) == (0, "CandleReader\n", "")
    assert list(tmp_path.iterdir()) == []
