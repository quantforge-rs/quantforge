"""`docs/python-package.md` and the manifests, held to the package: the
page's tables and its tree are the package's own, and the workspace is wired
as the page says. Each table and the tree sit between `<!-- begin: name -->`
and `<!-- end: name -->` markers."""

import re
import subprocess

import quantforge_py
from quantforge_py import _native

PAGE = "docs/python-package.md"


def section(repository, name) -> list[str]:
    """The lines of a marked section of the page, as written."""
    lines = (repository / PAGE).read_text(encoding="utf-8").splitlines()

    def marker(kind):
        text = f"<!-- {kind}: {name} -->"
        found = [at for at, line in enumerate(lines) if line.strip() == text]
        assert len(found) == 1, f"{PAGE} has one {text}"
        return found[0]

    return lines[marker("begin") + 1 : marker("end")]


def rows(repository, name) -> list[list[str]]:
    """A marked table's rows after its header and separator, cell by cell."""
    return [
        [cell.strip() for cell in row.strip().strip("|").split("|")]
        for row in section(repository, name)[2:]
    ]


def documented_files(repository) -> dict[str, str]:
    """The page's tree of `quantforge-py/`: each file's path to its note."""
    lines = section(repository, "layout")
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


def test_the_page_names_the_package_as_the_manifests_do(repository, package_dir, pyproject, crate):
    names = {what: name.strip("`") for what, name in rows(repository, "names")}
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


def test_the_page_lists_the_names_the_package_publishes(repository):
    listed = [name.strip("`") for name, _ in rows(repository, "public names")]
    assert listed == quantforge_py.__all__


def test_the_page_states_the_oldest_python_the_distribution_requires(repository, pyproject):
    stated = [line.strip() for line in section(repository, "oldest python") if line.strip()]
    assert [f">={oldest}" for oldest in stated] == [pyproject["project"]["requires-python"]]


def test_the_page_maps_every_file_of_the_package(repository):
    assert set(documented_files(repository)) == files_of_the_package(repository)


def test_the_readme_shows_the_names_the_package_publishes(package_dir):
    readme = (package_dir / "README.md").read_text(encoding="utf-8")
    shown = re.findall(r"\bqf\.(\w+)", readme)
    assert shown == quantforge_py.__all__


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
