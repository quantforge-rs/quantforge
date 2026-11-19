"""`docs/python-pandas.md`, held to the package: the four functions it says
there are, the index and the columns it says each frame has, every decimal
it takes to a frame and back, each refusal it shows, its examples, and the
pandas it says the package's extra requires.

What the page says of a run without pandas is held in
`test_pandas_optional.py`, which runs where there is none as well."""

import doctest
import inspect
from decimal import Decimal

import pytest

import quantforge_py as qf
from quantforge_py import _frames

# The frames need pandas and NumPy, the package's two extras. Where the
# package is installed without them there is nothing of the frames to test,
# and the run says so: `test_pandas_optional.py` holds what the package says
# there.
np = pytest.importorskip("numpy")
pd = pytest.importorskip("pandas")

PAGE = "docs/python-pandas.md"

# The functions the page is the contract of, in the order `__all__` gives
# them.
FUNCTIONS = [
    "candles_to_dataframe",
    "dataframe_to_candles",
    "trades_to_dataframe",
    "equity_to_dataframe",
]

# What the page says of a column's crossing each way, by its type:
# `test_pandas.py` holds the package to each.
CROSSINGS = {
    "datetime64[ms, UTC]": ("Exact", "Exact"),
    "float64": (
        "The float nearest to the decimal",
        "The decimal of the float's shortest text",
    ),
    "UInt64": (
        "Exact, and `<NA>` where the count is unknown",
        "Exact, and unknown where it is `<NA>`",
    ),
}

# How the page marks the row of a frame's index among its columns.
THE_INDEX = ", the index"

# Making a frame and reading one draw no warning, from pandas or from NumPy.
# Every call of the four functions is held to that here as well: those the
# page's examples make, those of its tables, and those of the tests below.
pytestmark = pytest.mark.usefixtures("no_warning_drawn_by_the_package")


@pytest.fixture(scope="module")
def session(page, no_warning_drawn_by_the_package):
    """The page's examples, run as one session from top to bottom: a later
    example uses an earlier one's names, as a reader does. Gives what the
    run came to, what it said, and the names it left.

    A frame is printed with spaces to the right of its index's name, which
    an editor takes off the end of a line: so the space between two words
    of an example's output is not compared, and everything else is."""
    examples = page(PAGE).fences("python")
    assert len(examples) > 8
    assert all(">>> " in example for example in examples)
    session = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    said = []
    runner = doctest.DocTestRunner(verbose=False, optionflags=doctest.NORMALIZE_WHITESPACE)
    results = runner.run(session, out=said.append, clear_globs=False)
    return results, "".join(said), len(session.examples), session.globs


def columns_of(rows) -> tuple[str | None, dict[str, str]]:
    """A table of a frame's columns, as the page lists them: the name of the
    index, if a row is marked as it, and each column's type by its name."""
    index, columns = None, {}
    for named, dtype, *_ in rows:
        if named.endswith(THE_INDEX):
            assert index is None, "one row is the index"
            index = code_of(named.removesuffix(THE_INDEX))
            columns[index] = code_of(dtype)
        else:
            columns[code_of(named)] = code_of(dtype)
    return index, columns


def code_of(cell: str) -> str:
    """What a cell quotes: the text of its one code span."""
    assert cell.startswith("`") and cell.endswith("`") and "`" not in cell[1:-1], cell
    return cell[1:-1]


def shape_of(frame) -> tuple[str | None, dict[str, str]]:
    """A frame's index and columns as the page's tables list them."""
    columns = {name: str(dtype) for name, dtype in frame.dtypes.items()}
    if type(frame.index) is pd.DatetimeIndex:
        return frame.index.name, {frame.index.name: str(frame.index.dtype), **columns}
    return None, columns


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, names = session
    assert examples > 40
    assert results.failed == 0, said
    assert results.attempted == examples
    # The four functions the examples called are the package's own, each
    # held to drawing no warning as it ran.
    for name in FUNCTIONS:
        assert getattr(names["qf"], name).__wrapped__ is getattr(_frames, name)


def test_the_page_gives_the_four_functions_as_python_does(page, quoted):
    assert qf.__all__[-len(FUNCTIONS) :] == FUNCTIONS
    lines = page(PAGE).section("signatures")
    assert (lines[0], lines[-1]) == ("```text", "```")
    assert lines[1:-1] == [f"{name}{inspect.signature(getattr(qf, name))}" for name in FUNCTIONS]
    rows = page(PAGE).rows("functions")
    assert [quoted(name) for name, _, _ in rows] == FUNCTIONS
    for name, takes, gives in rows:
        # What each takes is named as the type it is, in the page and in
        # the function's own first argument.
        taken = list(inspect.signature(getattr(qf, quoted(name))).parameters)
        expected = {
            "candles": "A `CandleBatch`",
            "result": "A `BacktestResult`",
            "frame": "Such a frame",
        }
        assert takes.startswith(expected[taken[0]]), name
        assert ("`MarketId`" in takes) == ("market" in taken), name
        assert gives.startswith(
            "A `CandleBatch`" if taken[0] == "frame" else "A frame with one row"
        ), name


def test_the_page_lists_the_index_and_every_column_of_a_frame_of_candles(page, session):
    rows = page(PAGE).rows("candle columns")
    *_, names = session
    frame = names["frame"]
    assert type(frame) is pd.DataFrame
    assert columns_of(rows) == shape_of(frame)
    assert columns_of(rows)[0] == "open_time"
    # The index first, and then the columns in the frame's own order.
    assert [row[0] for row in rows][0].endswith(THE_INDEX)
    assert list(columns_of(rows)[1])[1:] == list(frame.columns)
    for named, dtype, what, to_pandas, from_pandas in rows:
        assert what, named
        assert (to_pandas, from_pandas) == CROSSINGS[code_of(dtype)], named
    # The names of its columns are text of Python's own, as the page says of
    # every frame, whichever pandas this is.
    said = " ".join(page(PAGE).text.split())
    assert "the names of the columns are text of Python's own, of the type `object`" in said
    for made in (frame, names["trades"], names["equity"]):
        assert made.columns.dtype == np.dtype(object)
        assert {type(name) for name in made.columns} == {str}
    # And what it takes back is what it gives.
    assert qf.dataframe_to_candles(frame, market=names["market"]) == names["batch"]


def test_the_page_lists_every_column_of_a_frame_of_trades_and_of_equity(page, session):
    *_, names = session
    trades, equity, result = names["trades"], names["equity"], names["result"]
    assert type(result) is qf.BacktestResult
    assert trades.equals(qf.trades_to_dataframe(result))
    assert equity.equals(qf.equity_to_dataframe(result))
    rows = page(PAGE).rows("trade columns")
    assert columns_of(rows) == shape_of(trades)
    assert columns_of(rows)[0] is None and type(trades.index) is pd.RangeIndex
    assert list(columns_of(rows)[1]) == list(trades.columns)
    # Each column is a field of a trade, and the page names the field where
    # the column is named otherwise.
    fields = list(inspect.signature(qf.ClosedTrade).parameters)
    for (named, _, what), field in zip(rows, fields, strict=True):
        column = code_of(named)
        assert what, named
        assert (f"the field `{field}`" in what) == (column != field), named
        assert column == field.removesuffix("_ms"), named
    rows = page(PAGE).rows("equity columns")
    assert columns_of(rows) == shape_of(equity)
    assert columns_of(rows)[0] == "open_time"
    assert all(what for *_, what in rows)
    # The fields of a point, as a frame has them.
    assert list(inspect.signature(qf.EquityPoint).parameters) == ["open_time_ms", "equity"]


def test_each_docstring_names_the_columns_the_page_does(page):
    text = " ".join(page(PAGE).text.split())
    for name, table in [
        ("candles_to_dataframe", "candle columns"),
        ("dataframe_to_candles", "candle columns"),
        ("trades_to_dataframe", "trade columns"),
        ("equity_to_dataframe", "equity columns"),
    ]:
        doc = " ".join(inspect.getdoc(getattr(qf, name)).split())
        index, columns = columns_of(page(PAGE).rows(table))
        for column, dtype in columns.items():
            assert f"`{column}`" in doc, (name, column)
            # A type that is taken back is named by what it has to be: a
            # time in any zone and any unit is no `datetime64[ms, UTC]`.
            stated = dtype if name != "dataframe_to_candles" else dtype.split("[")[0]
            assert f"`{stated}`" in doc, (name, dtype)
        if index is not None:
            assert f"Index: `{index}`, a `DatetimeIndex`" in doc, name
    # The page says that the docstrings say it.
    assert "is stated in each function's docstring" in text


def test_every_decimal_the_page_takes_there_and_back_comes_back_as_it_says(page, quoted, session):
    rows = page(PAGE).rows("there and back")
    assert len(rows) >= 6
    *_, names = session
    there_and_back = names["there_and_back"]
    came_back = refused = 0
    for went, comes, because in rows:
        assert because, went
        decimal = eval(quoted(went), {"__builtins__": {}, "Decimal": Decimal})
        assert type(decimal) is Decimal, went
        said = quoted(comes)
        if said.startswith("Decimal("):
            expected = eval(said, {"__builtins__": {}, "Decimal": Decimal})
            back = there_and_back(decimal)
            # Digit for digit and place for place, not by value alone.
            assert type(back) is Decimal
            assert back.as_tuple() == expected.as_tuple(), went
            came_back += 1
        else:
            kind, _, message = said.partition(": ")
            assert kind == "ValueError", went
            with pytest.raises(ValueError) as caught:
                there_and_back(decimal)
            assert type(caught.value) is ValueError
            assert str(caught.value) == message, went
            refused += 1
    # The page shows a decimal that comes back as it went, one that comes
    # back another, and one that does not come back.
    assert came_back >= 5 and refused >= 1
    same = [quoted(went) == quoted(comes) for went, comes, _ in rows]
    assert any(same) and not all(same)
    # Each is what the same decimal comes to through a batch's arrays: a
    # frame is made of them.
    for went, comes, _ in rows:
        decimal = eval(quoted(went), {"__builtins__": {}, "Decimal": Decimal})
        one = qf.CandleBatch.from_columns(
            open_time_ms=[0],
            close_time_ms=[59_999],
            open=[1],
            high=[1],
            low=[1],
            close=[decimal],
            volume=[1],
            trades=[None],
        )
        try:
            through_arrays = repr(qf.CandleBatch.from_numpy(**one.to_numpy()).close[0])
        except ValueError as error:
            through_arrays = f"ValueError: {error}"
        assert through_arrays.replace("'", '"') == quoted(comes), went


def test_every_refusal_the_page_shows_is_refused_so(page, quoted, session):
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 20
    # The names are those the page's examples left, as it says. Python's
    # own are left among them: pandas imports parts of itself as it is used.
    *_, names = session
    kinds = {"ValueError": ValueError, "TypeError": TypeError}
    for written, refusal in rows:
        kind, _, message = quoted(refusal).partition(": ")
        with pytest.raises(kinds[kind]) as caught:
            eval(quoted(written), names)
        assert type(caught.value) is kinds[kind], written
        assert str(caught.value) == message, written
    assert {quoted(refusal).partition(": ")[0] for _, refusal in rows} == set(kinds)
    # Each function, the frame, its index, each kind of column, a time and
    # a float are all shown.
    shown = " ".join(quoted(refusal) for _, refusal in rows)
    for words in [
        "candles must be a CandleBatch, not",
        "pandas reads it as no time at all (NaT)",
        "frame must be a pandas.DataFrame, not",
        "the index must be a pandas.DatetimeIndex named open_time, not",
        "the index must be named open_time, not",
        "must be an index of datetime64 with a time zone, not of",
        "frame has no column named",
        "columns named",
        "must be a column of datetime64 with a time zone, not of",
        "must be a column of float64, not of",
        "must be a column of UInt64 or of uint64, not of",
        "market must be a MarketId or None, not",
        "must be a whole number of milliseconds",
        "a candle's time is never missing",
        "its milliseconds must fit 64 bits",
        "not finite",
        "beyond the decimal range",
        "result must be a BacktestResult, not",
    ]:
        assert words in shown, words
    written = " ".join(quoted(written) for written, _ in rows)
    for name in FUNCTIONS:
        assert f"qf.{name}(" in written, name


def test_the_page_states_the_pandas_the_packages_extra_requires(page, pyproject):
    stated = dict(page(PAGE).rows("pandas"))
    (oldest,) = pyproject["dependency-groups"]["oldest-pandas"]
    extra = "pandas"
    required = pyproject["project"]["optional-dependencies"][extra]
    assert stated == {
        "The package with its extra, as `pip` is asked for it": f"`{pyproject['project']['name']}[{extra}]`",
        "What the extra requires": ", ".join(f"`{one}`" for one in required),
        "The oldest pandas the tests run on": f"`{oldest.removeprefix('pandas==')}`",
    }
    assert required == ["numpy>=2.0,<3", "pandas>=2.2.2,<4"]
    # pandas is asked of nobody who did not ask for it.
    assert "dependencies" not in pyproject["project"]
    # And the page gives the command itself.
    assert f'`pip install "{pyproject["project"]["name"]}[{extra}]"`' in page(PAGE).text


def test_what_the_page_says_of_a_frame_beside_the_one_pandas_makes_is_so(page, session):
    *_, names = session
    frame, own = names["frame"], names["own"]
    said = " ".join(page(PAGE).text.split())
    assert "**A frame equals the one pandas makes of the same cells,**" in said
    assert "`pandas.testing.assert_frame_equal` compares their type too" in said
    assert "`check_column_type=False`" in said
    assert frame.equals(own) and own.equals(frame)
    assert list(frame.columns) == list(own.columns)
    assert all(frame[name].equals(own[name]) for name in frame.columns)
    # The type of the names in the frame pandas made is pandas' own choice,
    # and the helper compares it unless it is told not to: so it takes the
    # two for equal as they are only where that choice is `object`.
    theirs = own.columns.dtype
    assert frame.columns.dtype == np.dtype(object)
    pd.testing.assert_frame_equal(frame, own, check_column_type=False)
    if theirs == np.dtype(object):
        pd.testing.assert_frame_equal(frame, own)
    else:
        with pytest.raises(AssertionError, match="DataFrame.columns are different"):
            pd.testing.assert_frame_equal(frame, own)
    # And `rename` is a step that gives the names back as pandas' own.
    assert frame.rename(columns=str.upper).columns.dtype == theirs


def test_what_the_page_says_pandas_does_with_attrs_is_so(session):
    # The page's reason for keeping no market in a frame: pandas copies a
    # frame's attrs to the frames worked out from it. If a pandas stops,
    # the page's words are to be looked at again.
    *_, names = session
    frame = names["frame"].copy()
    frame.attrs["market"] = "binance_spot BTCUSDT 1m"
    five = frame.resample("5min").agg({"close": "last"})
    assert five.attrs == {"market": "binance_spot BTCUSDT 1m"}
    assert frame[frame["close"] > 81900].attrs == frame.attrs
    # And the frames the package gives keep nothing there.
    assert names["frame"].attrs == names["trades"].attrs == names["equity"].attrs == {}
    # A market kept there as the value it is would break the frame: pandas
    # copies what attrs hold, and a market cannot be copied so yet.
    frame.attrs["market"] = names["market"]
    with pytest.raises(TypeError, match="cannot pickle"):
        frame.copy()
