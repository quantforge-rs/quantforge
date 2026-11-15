"""`docs/python-package.md` and the manifests, held to the package: the
page's tables and its tree are the package's own, and the workspace is wired
as the page says. Each table and the tree sit between `<!-- begin: name -->`
and `<!-- end: name -->` markers."""

import re
import subprocess

import quantforge_py
from quantforge_py import _native

PAGE = "docs/python-package.md"


def documented_files(page) -> dict[str, str]:
    """The page's tree of `quantforge-py/`: each file's path to its note."""
    lines = page(PAGE).section("layout")
    assert (lines[0], lines[1], lines[-1]) == ("```text", "quantforge-py/", "```")
    files, parents = {}, []
    for line in lines[2:-1]:
        name_at = re.search(r"[^│├└─ ]", line).start()
        depth = name_at // 4
        assert depth * 4 == name_at and 1 <= depth <= len(parents) + 1, line
        del parents[depth - 1 :]
        name, _, note = line[name_at:].partition(" ")
        path = "".join(f"{parent}/" for parent in parents) + name
        if name.endswith("/"):
            parents.append(name.removesuffix("/"))
        else:
            assert note.strip(), f"{PAGE}: {path} has no note"
            assert path not in files, f"{PAGE} lists {path} twice"
            files[path] = note.strip()
    return files


def files_of_the_package(repository) -> set[str]:
    """What git holds of `quantforge-py/`, with what it would take: the
    extension `maturin develop` builds in place, a virtualenv and the caches
    are ignored, and so left out."""
    listed = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "--", "quantforge-py"],
        cwd=repository,
        check=True,
        capture_output=True,
        text=True,
    )
    return {path.removeprefix("quantforge-py/") for path in listed.stdout.splitlines()}


def test_the_page_names_the_package_as_the_manifests_do(page, package_dir, pyproject, crate):
    names = {what: name.strip("`") for what, name in page(PAGE).rows("names")}
    assert names == {
        "The distribution, as `pip` installs it": pyproject["project"]["name"],
        "The package, as Python imports it": quantforge_py.__name__,
        "The compiled extension module, which is private": _native.__name__,
        "The Cargo package that builds it": crate["package"]["name"],
    }
    built = pyproject["tool"]["maturin"]
    assert built["module-name"] == _native.__name__
    package = package_dir / built["python-source"] / quantforge_py.__name__
    assert (package / "__init__.py").is_file()


def test_the_page_lists_the_names_the_package_publishes(page):
    listed = [name.strip("`") for name, _ in page(PAGE).rows("public names")]
    assert listed == quantforge_py.__all__


def test_the_page_states_the_oldest_python_the_distribution_requires(page, pyproject):
    stated = [line.strip() for line in page(PAGE).section("oldest python") if line.strip()]
    assert [f">={oldest}" for oldest in stated] == [pyproject["project"]["requires-python"]]


def test_the_page_maps_every_file_of_the_package(page, repository):
    assert set(documented_files(page)) == files_of_the_package(repository)


def test_the_readme_shows_only_names_the_package_publishes_and_its_example_runs(page):
    readme = page("quantforge-py/README.md")
    shown = re.findall(r"\bqf\.(\w+)", readme.text)
    assert shown, "the readme shows the package at work"
    assert set(shown) <= set(quantforge_py.__all__)
    # What PyPI shows is what a reader copies: every example runs as it is
    # written, one after another.
    examples = readme.fences("python")
    assert examples
    names = {}
    for example in examples:
        exec(compile(example, readme.name, "exec"), names)


def test_the_crate_and_the_distribution_are_described_alike(crate, pyproject):
    assert crate["package"]["description"] == pyproject["project"]["description"]


def test_the_crate_never_goes_to_crates_io(crate):
    assert crate["package"]["publish"] is False


def test_the_quantforge_crate_is_packaged_without_the_python_package(core_crate):
    assert "quantforge-py/" in core_crate["package"]["exclude"]


def test_cargo_at_the_root_acts_on_the_quantforge_crate_alone(core_crate):
    workspace = core_crate["workspace"]
    assert workspace["members"] == ["quantforge-py"]
    # With no default members named, a command without `-p` takes the root
    # package only.
    assert "default-members" not in workspace


def test_a_build_follows_the_lock_file_as_it_is(pyproject):
    assert pyproject["tool"]["maturin"]["locked"] is True


def test_a_test_that_waits_for_good_ends_the_run(pyproject, pytestconfig):
    # Some tests wait on another thread, which a fault in how the
    # interpreter is held would leave waiting. pytest ends a run on a
    # timeout from 9.0 on, so that is the oldest the tests take.
    settings = pyproject["tool"]["pytest"]["ini_options"]
    assert settings["faulthandler_exit_on_timeout"] is True
    assert 0 < settings["faulthandler_timeout"] <= 120
    assert settings["minversion"] == "9"
    assert pyproject["dependency-groups"]["test"] == ["pytest>=9"]
    # And the run these tests are in has the limit.
    assert pytestconfig.getini("faulthandler_exit_on_timeout") is True
    assert float(pytestconfig.getini("faulthandler_timeout")) == settings["faulthandler_timeout"]
