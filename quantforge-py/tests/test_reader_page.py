"""`docs/python-reader.md`, held to the package: the reader's signatures and
members, the settings it shares with the command line, the ways a bound is
written, the sentence for each range that holds nothing, every refusal it
shows, its examples, which run where the command line wrote a store, and
its PostgreSQL commands, which run where there is a server.

What the reader does beyond what the page shows is in `test_reader.py` and
`test_reader_stores.py`."""

import doctest
import inspect
import logging
import re
import shlex

import pytest

import quantforge_py as qf
import stores

PAGE = "docs/python-reader.md"
CONFIGURATION = "docs/configuration.md"

# What the page's examples read: five minutes the command line stored where
# they run, at its default path.
CLOSES = ["81940.01000000", "81955.25000000", "81900.00000000", "81900.10000000", "81912.50000000"]
COUNTS = [412, 3, 0, 18, 7]
KLINES = [
    stores.kline(index, close, trades=count)
    for index, (close, count) in enumerate(zip(CLOSES, COUNTS))
]


def prompted(examples: list[str]) -> list[str]:
    """The fences that are a session at Python's prompt."""
    return [example for example in examples if ">>> " in example]


@pytest.fixture(scope="module")
def session(page, tmp_path_factory):
    """The page's examples, run as one session from top to bottom, in a
    directory where the command line stored the candles they read, under no
    setting from outside. Gives what the run came to, what it said, how
    many examples it had, and the names it left. The tests of this module
    run in that directory."""
    directory = tmp_path_factory.mktemp("page")
    with stores.Exchange({("BTCUSDT", "1m"): KLINES}) as exchange:
        stores.sync(exchange, "BTCUSDT", "1m", cwd=directory)
    examples = prompted(page(PAGE).fences("python"))
    assert len(examples) >= 6
    parsed = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    said = []
    with pytest.MonkeyPatch.context() as patched:
        patched.chdir(directory)
        for variable in stores.SETTINGS:
            patched.delenv(variable, raising=False)
        results = doctest.DocTestRunner(verbose=False).run(
            parsed, out=said.append, clear_globs=False
        )
        yield results, "".join(said), len(parsed.examples), parsed.globs
        # The page leaves its reader open, as its tables use it.
        parsed.globs["reader"].close()


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, names = session
    assert examples > 20
    assert results.failed == 0, said
    assert results.attempted == examples
    # What the examples read is what the command line stored.
    assert [format(close, "f") for close in names["candles"].close] == CLOSES
    assert names["candles"].trades == tuple(COUNTS)
    # The page takes its own handler off the logger again.
    assert logging.getLogger("quantforge_py").handlers == []


def test_the_page_gives_the_reader_as_python_does(page, quoted):
    reader = qf.CandleReader
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")

    def called(method) -> str:
        signature = inspect.signature(method)
        return str(signature.replace(parameters=list(signature.parameters.values())[1:]))

    assert lines[1:-1] == [
        f"CandleReader{inspect.signature(reader)}",
        f"CandleReader.load{called(reader.load)}",
        f"CandleReader.close{called(reader.close)}",
    ]
    # Its members, each once: the class as it is called, its methods as
    # they are called, and its properties.
    members = [quoted(member) for member, _ in page(PAGE).rows("members")]
    public = {
        name: type(member).__name__ for name, member in vars(reader).items() if name[0] != "_"
    }
    methods = [name for name, kind in public.items() if kind == "method_descriptor"]
    properties = [name for name, kind in public.items() if kind == "getset_descriptor"]
    assert sorted(methods + properties) == sorted(public)
    assert members == [
        "CandleReader(...)",
        *[f"{name}{called(getattr(reader, name))}" for name in ("load", "close")],
        "backend",
        "location",
        "closed",
    ]
    assert sorted(methods) == ["close", "load"]
    assert sorted(properties) == ["backend", "closed", "location"]


def test_the_page_lists_the_settings_the_command_line_has(page, quoted):
    rows = page(PAGE).rows("settings")
    arguments = [quoted(argument) for argument, *_ in rows]
    assert arguments == list(inspect.signature(qf.CandleReader).parameters)
    # Each is the command line's flag of the same name, with that flag's
    # variable and that flag's default, as the configuration matrix has it.
    matrix = {}
    for line in page(CONFIGURATION).lines:
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) == 5 and cells[1].startswith("`--"):
            flag = cells[1].strip("`").split(" ")[0]
            matrix[flag] = (cells[2], cells[3])
    assert len(matrix) > 10, "the configuration matrix was read"
    for argument, variable, default, described in rows:
        flag = "--" + quoted(argument).replace("_", "-")
        stated_variable, stated_default = matrix[flag]
        assert variable == stated_variable, flag
        assert quoted(variable) in stores.SETTINGS
        if quoted(argument) == "postgres_pool_size":
            # A command's two, where the matrix says only that the default
            # depends on the command.
            assert (default, stated_default) == ("`2`", "none")
        else:
            assert default == stated_default, flag
        assert described
    assert [quoted(variable) for _, variable, *_ in rows] == stores.SETTINGS


def test_each_way_the_page_writes_a_bound_names_the_same_range(page, session):
    *_, names = session
    reader, market, noon = names["reader"], names["market"], names["noon"]
    rows = page(PAGE).rows("bounds")
    assert [bound for bound, _ in rows] == [
        "An `int`",
        "A `datetime.datetime` with a time zone",
        "A `str`",
        "`None`, or left out",
    ]
    first, last = noon + names["minute"], noon + 3 * names["minute"]
    ways = [
        (stores.NOON + 60_000, stores.NOON + 180_000),
        (first, last),
        ("2026-01-01T12:01:00Z", "2026-01-01T12:03:00Z"),
    ]
    expected = reader.load(market).open_time_ms[1:4]
    for start, end in ways:
        assert reader.load(market, start=start, end=end).open_time_ms == expected
    assert reader.load(market, start=None, end=None) == reader.load(market) == names["candles"]


def test_the_page_gives_the_sentence_for_each_range_that_holds_nothing(
    page, session, quoted, caplog
):
    *_, names = session
    reader, market = names["reader"], names["market"]
    other = qf.MarketId("binance_spot", "ETHUSDT", "1m")
    asked = [
        ("A market the store holds nothing of, whatever the range", other, {}),
        (
            "A range from one time to another",
            market,
            dict(start="2026-01-02T00:00:00Z", end="2026-01-03T00:00:00Z"),
        ),
        ("A range with a start alone", market, dict(start="2026-01-02T00:00:00Z")),
        ("A range with an end alone", market, dict(end="2025-12-31T00:00:00Z")),
        (
            "A range that ends before it starts",
            market,
            dict(start="2026-01-03T00:00:00Z", end="2026-01-02T00:00:00Z"),
        ),
    ]
    rows = page(PAGE).rows("nothing found")
    assert [what for what, _ in rows] == [what for what, *_ in asked]
    for (_, sentence), (_, asked_market, bounds) in zip(rows, asked):
        caplog.clear()
        with caplog.at_level(logging.WARNING, logger="quantforge_py"):
            assert len(reader.load(asked_market, **bounds)) == 0
        (record,) = caplog.records
        assert record.getMessage() == f"{reader.location} {quoted(sentence)}"
    # The unknown market says the same whatever the range.
    caplog.clear()
    with caplog.at_level(logging.WARNING, logger="quantforge_py"):
        reader.load(other, start=0, end=1)
    assert caplog.records[0].getMessage() == f"{reader.location} {quoted(rows[0][1])}"


def test_every_refusal_on_the_page_is_raised_as_written(page, session, quoted, storage_backends):
    *_, names = session
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 15
    shown = set()
    for call, raises in rows:
        call, raises = quoted(call), quoted(raises)
        kind, _, message = raises.partition(": ")
        if "postgres" not in storage_backends and 'storage="postgres"' in call:
            # The SQLite-only build refuses the backend before its settings.
            continue
        with pytest.raises(Exception) as caught:
            eval(call, names)
        assert type(caught.value).__name__ == kind, call
        # `...` in a message is the directory the process runs in.
        said = "(.*)".join(re.escape(part) for part in message.split("..."))
        assert re.fullmatch(said, str(caught.value)), (call, str(caught.value))
        shown.add(kind)
    assert shown == {"TypeError", "ValueError", "FileNotFoundError"}
    # The page's sentence about a variable in an argument's place.
    assert "`QF_STORAGE must be sqlite or postgres`" in page(PAGE).text
    with pytest.MonkeyPatch.context() as patched:
        patched.setenv("QF_STORAGE", "mysql")
        with pytest.raises(ValueError, match="^QF_STORAGE must be sqlite or postgres$"):
            qf.CandleReader()


def test_the_page_names_an_exception_for_each_way_a_call_fails(page, session, quoted, refused):
    *_, names = session
    rows = page(PAGE).rows("raises")
    raised = [quoted(raises.split(",")[0]) for _, raises in rows]
    assert raised == [
        "TypeError",
        "ValueError",
        "FileNotFoundError",
        "ValueError",
        "ValueError",
        "ValueError",
        "ConnectionError",
        "ValueError",
        "RuntimeError",
        "RuntimeError",
    ]
    assert rows[7][0] == "The reader is closed"
    # A reader of the page's store, closed.
    closed = qf.CandleReader()
    closed.close()
    refused(ValueError, "the reader is closed", closed.load, names["market"])
    said = " ".join(page(PAGE).text.split())
    assert "`load` raises `ValueError: the reader is closed`" in said


def commands(fence: str) -> tuple[dict[str, str], list[str]]:
    """A `bash` fence as the variables it exports and the one command it
    runs."""
    exported, command = {}, None
    for line in fence.splitlines():
        if line.startswith("export "):
            name, _, value = line.removeprefix("export ").partition("=")
            exported[name] = shlex.split(value)[0]
        elif line.strip():
            assert command is None, "one command"
            command = shlex.split(line)
    return exported, command


def test_the_pages_postgresql_commands_store_what_its_example_reads(
    page, session, tmp_path, monkeypatch, postgres_server
):
    *_, names = session
    (fence,) = page(PAGE).fences("bash")
    exported, command = commands(fence)
    (example,) = [text for text in page(PAGE).fences("python") if ">>> " not in text]
    with (
        stores.Database(postgres_server) as database,
        stores.Exchange({("BTCUSDT", "1m"): KLINES}) as exchange,
    ):
        place = stores.Place.postgres(database, tmp_path)
        # The page's two variables, with this server's URL in its sample's
        # place, and the page's command, told where the exchange is.
        assert exported == {
            "QF_STORAGE": "postgres",
            "QF_POSTGRES_URL": "postgres://USER:PASSWORD@HOST:5432/DATABASE",
        }
        assert list(exported) == list(place.variables)
        assert command[0] == "quantforge"
        stores.quantforge(
            "--base-url", exchange.base_url, *command[1:], cwd=tmp_path, variables=place.variables
        )
        monkeypatch.chdir(tmp_path)
        for variable, value in place.variables.items():
            monkeypatch.setenv(variable, value)
        read = {"qf": qf}
        exec(compile(example, PAGE, "exec"), read)
        # The same candles as the examples read from SQLite, and nothing of
        # SQLite's made where PostgreSQL was selected.
        assert read["candles"] == names["candles"]
        assert read["reader"].closed and read["reader"].backend == "postgres"
        assert read["reader"].location == place.location
        assert list(tmp_path.iterdir()) == []
