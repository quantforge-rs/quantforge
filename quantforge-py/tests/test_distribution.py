"""The distribution as it is installed: the name pip knows it by, the Pythons
it says it runs on, its license, and the files it brings."""

import pathlib
import re
import sys

import quantforge_py


def tags(distribution) -> list[str]:
    """The compatibility tags the wheel was built with."""
    wheel = distribution.read_text("WHEEL")
    return [line.removeprefix("Tag: ") for line in wheel.splitlines() if line.startswith("Tag: ")]


def oldest_python(pyproject) -> tuple[str, str]:
    """`requires-python` as the oldest version's major and minor."""
    oldest = re.fullmatch(r">=(3)\.([1-9][0-9]*)", pyproject["project"]["requires-python"])
    assert oldest, "requires-python names one oldest version, as >=3.X"
    return oldest.group(1), oldest.group(2)


def test_the_package_imported_is_the_one_installed(distribution, editable, package_dir):
    # Every other test reads the distribution's record and the imported
    # package as one thing: a stray copy on the path would make them two.
    imported = pathlib.Path(quantforge_py.__file__).resolve()
    if editable:
        # `maturin develop` installs a path file that names the checkout's
        # `python/`, where the package stays.
        path_file = pathlib.Path(distribution.locate_file("quantforge_py.pth"))
        named = path_file.read_text(encoding="utf-8").strip()
        assert pathlib.Path(named).resolve() == (package_dir / "python").resolve()
        assert imported == (package_dir / "python" / "quantforge_py" / "__init__.py").resolve()
    else:
        installed = pathlib.Path(distribution.locate_file("quantforge_py/__init__.py"))
        assert imported == installed.resolve()


def test_the_distribution_is_named_as_pyproject_names_it(distribution, pyproject):
    assert distribution.metadata["Name"] == pyproject["project"]["name"]


def test_the_package_imports_under_the_distributions_name(distribution):
    # The name with its separators as Python spells them: what a user
    # installs is what they import.
    spelled = re.sub(r"[-_.]+", "_", distribution.metadata["Name"]).lower()
    assert spelled == quantforge_py.__name__


def test_the_installed_version_is_the_packages(distribution):
    assert distribution.version == quantforge_py.__version__


def test_one_oldest_python_is_named_by_the_crate_the_metadata_and_the_tags(
    distribution, pyproject, crate
):
    major, minor = oldest_python(pyproject)
    stable_abi = [
        feature for feature in crate["dependencies"]["pyo3"]["features"] if "abi3" in feature
    ]
    assert stable_abi == [f"abi3-py{major}{minor}"]
    assert distribution.metadata["Requires-Python"] == f">={major}.{minor}"
    # A wheel may state several platforms, each with a tag of its own.
    built_for = {tuple(tag.split("-")[:2]) for tag in tags(distribution)}
    assert built_for == {(f"cp{major}{minor}", "abi3")}


def test_the_distribution_carries_the_repositorys_license(distribution, repository):
    assert distribution.metadata["License-Expression"] == "MIT"
    assert distribution.metadata.get_all("License-File") == ["LICENSE"]
    text = (repository / "LICENSE").read_text(encoding="utf-8")
    assert distribution.read_text("licenses/LICENSE") == text


def test_the_distribution_brings_the_package_and_nothing_beside_it(distribution, editable):
    brought = sorted(
        str(file)
        for file in distribution.files
        if not file.parts[0].endswith(".dist-info") and "__pycache__" not in file.parts
    )
    if editable:
        # `maturin develop` leaves the package in the checkout, and installs
        # a path file that points there.
        assert brought == ["quantforge_py.pth"]
    else:
        extension = "_native.pyd" if sys.platform == "win32" else "_native.abi3.so"
        assert brought == ["quantforge_py/__init__.py", f"quantforge_py/{extension}"]
