"""What the package's tests share: where the checkout is, its manifests, and
the distribution under test.

The tests run against `quantforge_py` as it is installed, from a wheel or by
`maturin develop`, and hold it to the files of the checkout they are in.
"""

import importlib.metadata
import json
import os
import pathlib
import tomllib

import pytest

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
