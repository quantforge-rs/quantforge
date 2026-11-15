"""What the package's tests share: where the checkout is, its manifests, the
distribution under test, and how a refusal and a `repr` are checked.

The tests run against `quantforge_py` as it is installed, from a wheel or by
`maturin develop`, and hold it to the files of the checkout they are in.
"""

import decimal
import importlib.metadata
import json
import os
import pathlib
import tomllib

import pytest

import quantforge_py

# quantforge-py/, and the repository it is a member of.
PACKAGE_DIR = pathlib.Path(__file__).resolve().parents[1]
REPOSITORY = PACKAGE_DIR.parent

DISTRIBUTION = "quantforge-py"

# Says that the build under test is the SQLite-only one
# (`--no-default-features`). Unset, the tests expect the package as it is
# published, with both stores.
SQLITE_ONLY = "QUANTFORGE_PY_TEST_SQLITE_ONLY"


def read(path: pathlib.Path) -> str:
    return path.read_text(encoding="utf-8")


def toml(path: pathlib.Path) -> dict:
    return tomllib.loads(read(path))


@pytest.fixture(scope="session")
def package_dir() -> pathlib.Path:
    return PACKAGE_DIR


@pytest.fixture(scope="session")
def repository() -> pathlib.Path:
    return REPOSITORY


@pytest.fixture(scope="session")
def crate() -> dict:
    """quantforge-py/Cargo.toml."""
    return toml(PACKAGE_DIR / "Cargo.toml")


@pytest.fixture(scope="session")
def core_crate() -> dict:
    """The repository's Cargo.toml: the `quantforge` crate and the workspace."""
    return toml(REPOSITORY / "Cargo.toml")


@pytest.fixture(scope="session")
def pyproject() -> dict:
    return toml(PACKAGE_DIR / "pyproject.toml")


@pytest.fixture(scope="session")
def distribution() -> importlib.metadata.Distribution:
    """The installed distribution the imported package belongs to."""
    return importlib.metadata.distribution(DISTRIBUTION)


@pytest.fixture(scope="session")
def editable(distribution) -> bool:
    """Whether `maturin develop` installed it, in place of a wheel."""
    record = distribution.read_text("direct_url.json")
    if record is None:
        return False
    return json.loads(record).get("dir_info", {}).get("editable", False)


class Page:
    """A page of the repository's documentation, as the tests read one: its
    tables and its lists sit between `<!-- begin: name -->` and `<!-- end:
    name -->` markers, and its examples in fences."""

    def __init__(self, path: pathlib.Path, name: str):
        self.name = name
        self.text = read(path)
        self.lines = self.text.splitlines()

    def section(self, name: str) -> list[str]:
        """The lines of a marked section, as written."""

        def marker(kind):
            text = f"<!-- {kind}: {name} -->"
            found = [at for at, line in enumerate(self.lines) if line.strip() == text]
            assert len(found) == 1, f"{self.name} has one {text}"
            return found[0]

        return self.lines[marker("begin") + 1 : marker("end")]

    def rows(self, name: str) -> list[list[str]]:
        """A marked table's rows after its header and separator, cell by
        cell. No cell of these tables holds a `|`."""
        return [
            [cell.strip() for cell in row.strip().strip("|").split("|")]
            for row in self.section(name)[2:]
        ]

    def fences(self, language: str) -> list[str]:
        """The text of every fence of `language`, in the page's order."""
        found, inside = [], None
        for line in self.lines:
            if inside is None and line.strip() == f"```{language}":
                inside = []
            elif inside is not None and line.strip() == "```":
                found.append("\n".join(inside) + "\n")
                inside = None
            elif inside is not None:
                inside.append(line)
        assert inside is None, f"{self.name} leaves a {language} fence open"
        return found


def code(cell: str) -> str:
    """What a table's cell quotes: the text of its one code span, which two
    backticks open when the text holds a backtick of its own."""
    ticks = "``" if cell.startswith("``") else "`"
    assert cell.startswith(ticks) and cell.endswith(ticks) and len(cell) > 2 * len(ticks), cell
    quoted = cell[len(ticks) : -len(ticks)]
    assert ticks not in quoted, cell
    return quoted


@pytest.fixture(scope="session")
def page():
    """Opens a page of the repository by its path from the root."""
    return lambda name: Page(REPOSITORY / name, name)


@pytest.fixture(scope="session")
def quoted():
    """Reads what a table's cell quotes."""
    return code


def pytest_configure(config):
    """Stops the run on a value of the variable that says neither build."""
    said = os.environ.get(SQLITE_ONLY)
    if said not in (None, "1"):
        raise pytest.UsageError(
            f"{SQLITE_ONLY} is {said!r}: set it to 1 for the SQLite-only build, or leave it unset"
        )


@pytest.fixture(scope="session")
def storage_backends() -> tuple[str, ...]:
    """The stores the build under test is expected to open."""
    if os.environ.get(SQLITE_ONLY) == "1":
        return ("sqlite",)
    return ("sqlite", "postgres")


@pytest.fixture(scope="session")
def refused():
    """Holds a call to its refusal: this exception itself, not one derived
    from it, and this message word for word."""

    def refused(kind, message, call, *args, **kwargs):
        with pytest.raises(kind) as caught:
            call(*args, **kwargs)
        assert type(caught.value) is kind
        assert str(caught.value) == message

    return refused


@pytest.fixture(scope="session")
def evaluated():
    """What a `repr` evaluates back to, where the package's own names and
    `Decimal` are all it may name."""
    names = {name: getattr(quantforge_py, name) for name in quantforge_py.__all__}
    names["Decimal"] = decimal.Decimal

    def evaluated(text):
        return eval(text, {"__builtins__": {}}, names)

    return evaluated
