"""`docs/python-numpy.md`, held to the package: the array it says each
column is, every decimal it takes to NumPy and back, each refusal it shows,
its examples, the NumPy it says the package's extra requires, and the script
it says times the conversions.

What the page says of a run without NumPy is held in
`test_numpy_optional.py`, which runs where there is none as well."""

import doctest
import subprocess
import sys
from decimal import Decimal

import pytest

import quantforge_py as qf

# The arrays need NumPy, the package's one extra. Where the package is
# installed without it there is nothing of them to test, and the run says so:
# `test_numpy_optional.py` holds what the package says there.
np = pytest.importorskip("numpy")

PAGE = "docs/python-numpy.md"

COLUMNS = ["open_time_ms", "close_time_ms", "open", "high", "low", "close", "volume", "trades"]

# How the page names the two kinds of array there are.
ARRAYS = {np.ndarray: "numpy.ndarray", np.ma.MaskedArray: "numpy.ma.MaskedArray"}

# What the page says of a column's crossing each way, by its cells' type:
# `test_numpy.py` holds the package to each.
CROSSINGS = {
    "int64": ("Exact", "Exact"),
    "float64": (
        "The float nearest to the decimal",
        "The decimal of the float's shortest text",
    ),
    "uint64": (
        "Exact, and masked where the count is unknown",
        "Exact, and unknown where it is masked",
    ),
}


@pytest.fixture(scope="module")
def session(page):
    """The page's examples, run as one session from top to bottom: a later
    example uses an earlier one's names, as a reader does. Gives what the
    run came to, what it said, and the names it left."""
    examples = page(PAGE).fences("python")
    assert len(examples) > 5
    assert all(">>> " in example for example in examples)
    session = doctest.DocTestParser().get_doctest("\n".join(examples), {}, PAGE, PAGE, 0)
    said = []
    runner = doctest.DocTestRunner(verbose=False)
    results = runner.run(session, out=said.append, clear_globs=False)
    return results, "".join(said), len(session.examples), session.globs


def test_every_example_on_the_page_runs_as_written(session):
    results, said, examples, _ = session
    assert examples > 20
    assert results.failed == 0, said
    assert results.attempted == examples


def test_the_page_lists_every_column_as_the_array_a_batch_gives(page, quoted, session):
    rows = page(PAGE).rows("columns")
    assert [quoted(column) for column, *_ in rows] == COLUMNS
    *_, names = session
    arrays = names["batch"].to_numpy()
    assert list(arrays) == COLUMNS
    for column, dtype, array, to_numpy, from_numpy in rows:
        given = arrays[quoted(column)]
        assert quoted(dtype) == str(given.dtype), column
        assert quoted(array) == ARRAYS[type(given)], column
        assert (to_numpy, from_numpy) == CROSSINGS[str(given.dtype)], column
        # One dimension, and a cell for each candle, as the page says of all.
        assert given.shape == (len(names["batch"]),), column
    assert "its shape is `(len(batch),)`" in " ".join(page(PAGE).text.split())


def test_every_decimal_the_page_takes_there_and_back_comes_back_as_it_says(page, quoted, session):
    rows = page(PAGE).rows("there and back")
    assert len(rows) >= 10
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
    assert came_back >= 8 and refused >= 1
    same = [quoted(went) == quoted(comes) for went, comes, _ in rows]
    assert any(same) and not all(same)


def test_every_refusal_the_page_shows_is_refused_so(page, quoted, session):
    rows = page(PAGE).rows("refusals")
    assert len(rows) >= 12
    # The names are those the page's examples left, as it says. Python's
    # own are left among them: NumPy imports parts of itself as it is used.
    *_, names = session
    kinds = {"ValueError": ValueError, "TypeError": TypeError}
    for written, refusal in rows:
        kind, _, message = quoted(refusal).partition(": ")
        with pytest.raises(kinds[kind]) as caught:
            eval(quoted(written), names)
        assert type(caught.value) is kinds[kind], written
        assert str(caught.value) == message, written
    assert {quoted(refusal).partition(": ")[0] for _, refusal in rows} == set(kinds)
    # Each column's type, a dimension, a length and a value are all shown.
    shown = " ".join(quoted(refusal) for _, refusal in rows)
    for words in [
        "must be a numpy.ndarray of float64, not",
        "must be a numpy.ndarray of int64, not",
        "must be a numpy.ndarray or a numpy.ma.MaskedArray of uint64, not",
        "must have one dimension, not",
        "rows, expected",
        "not finite",
        "beyond the decimal range",
        "more than 28 decimal places",
        "market must be a MarketId or None, not",
    ]:
        assert words in shown, words


def test_the_page_states_the_numpy_the_packages_extra_requires(page, quoted, pyproject):
    stated = {what: quoted(which) for what, which in page(PAGE).rows("numpy")}
    (oldest,) = pyproject["dependency-groups"]["oldest-numpy"]
    ((extra, (required,)),) = pyproject["project"]["optional-dependencies"].items()
    assert stated == {
        "The package with its extra, as `pip` is asked for it": f"{pyproject['project']['name']}[{extra}]",
        "What the extra requires": required,
        "The oldest NumPy the tests run on": oldest.removeprefix("numpy=="),
    }
    assert (extra, required) == ("numpy", "numpy>=2.0,<3")
    # NumPy is asked of nobody who did not ask for it.
    assert "dependencies" not in pyproject["project"]


def test_the_script_the_page_names_times_both_conversions(page, package_dir):
    (commands,) = page(PAGE).fences("bash")
    assert commands.splitlines() == [
        "cd quantforge-py",
        "maturin develop --release --extras numpy",
        "python benches/candle_arrays.py",
    ]
    script = package_dir / "benches" / "candle_arrays.py"
    assert script.is_file()
    # It runs as the page gives it, here on a few rows: a million are for
    # whoever wants the figures.
    ran = subprocess.run(
        [sys.executable, str(script), "2000"], capture_output=True, text=True, timeout=120
    )
    assert ran.returncode == 0, ran.stderr
    said = ran.stdout.splitlines()
    assert said[0] == "rows: 2000"
    assert said[1].split(", ")[1:] == [f"numpy {np.__version__}", f"engine {qf.ENGINE_VERSION}"]
    assert [line.split(":")[0] for line in said[2:]] == [
        "from_numpy",
        "to_numpy",
        "the five decimal columns through tuples of Decimal, in Python",
    ]
    assert all(line.endswith(" million rows a second") for line in said[2:])
