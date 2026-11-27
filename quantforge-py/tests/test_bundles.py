"""A strategy bundle as the package builds one, in the test's own process
(docs/python-bundles.md): the files a caller names made into a bundle by
the engine's own code, the manifest written for them, and every refusal,
before any server is asked anything.

- **The bundle spec's shared cases** (`tests/fixtures/bundle_cases.json`),
  which every implementation passes, hold through the package: each case of
  files gives its code hash or its problems, and is admitted or not against
  its runtime. The cases that alter an archive are the server's reader's,
  which a client has no use for.
- **A code hash is checked against a builder of the test's own,** written
  on the standard library's `tarfile` as the spec shows one: the package
  packs with the engine's code, and the two agree on every bundle.
- **A manifest is read as a server reads one,** whatever TOML this Python
  reads: what Python's own `tomllib` takes from 3.15 on, and a server
  refuses, is refused here.

What a client does with a bundle once it is built, the requests it makes in
order, is in `test_submit.py`, and against a real server in
`test_submit_jobs.py`.
"""

import json
import os
import pickle
import re
import subprocess
import sys
import textwrap
import tomllib

import pytest

import jobs
import quantforge_py as qf
from quantforge_py import _bundles, _native

SPEC = "docs/bundle-spec.md"

# A strategy that holds nothing, and a manifest of its own for it.
HOLD = b"import quantforge_py as qf\n\n\nclass Hold(qf.Strategy):\n    def on_bar(self, ctx, bar):\n        return None\n"
OWN = b'format = 1\nentry = "strategy.py:Hold"\nengine = ">=0.1.0"\npython = "3.12"\n'

# The server a refusal names, where a test names one.
SERVER = "http://127.0.0.1:9600/"


def offered(python="3.12", engine=None, **packages):
    """A runtime as a server offers one, and the answer it came in: what a
    bundle is built for and admitted against."""
    answer = {
        "runtime": {
            "engine": engine or qf.ENGINE_VERSION,
            "python": python,
            "packages": {name: list(versions) for name, versions in packages.items()},
        }
    }
    text = json.dumps(answer)
    return _native._server_runtime(text), text


def written(entry="strategy.py:Hold", params=None, deps=None, files=None):
    """A draft of files given in memory, beside a manifest written from the
    arguments: built and checked as it is made."""
    return _bundles._Draft.of_files(files or [("strategy.py", HOLD)], entry, params, deps)


def manifest(built) -> str:
    """The manifest a built bundle holds, read out of its canonical tar."""
    tar = built["tar"]
    at = 0
    while tar[at : at + 512].strip(b"\0"):
        name = tar[at : at + 100].rstrip(b"\0").decode()
        size = int(tar[at + 124 : at + 135], 8)
        if name == "quantforge.toml":
            return tar[at + 512 : at + 512 + size].decode()
        at += 512 + size + (-size % 512)
    raise AssertionError("the bundle holds no manifest")


def codes(error: qf.BundleError) -> list[str]:
    return [code for code, _ in error.value.problems]


def tree(root, files: dict[str, bytes]):
    """Writes files below a directory, each at its path."""
    for path, data in files.items():
        target = root.joinpath(*path.split("/"))
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    return root


# The shared cases.


def shared_cases(repository) -> list[dict]:
    with open(repository / "tests" / "fixtures" / "bundle_cases.json", encoding="utf-8") as file:
        fixture = json.load(file)
    assert fixture["format"] == 1
    return fixture["cases"]


def files_of(case) -> list[tuple[str, bytes]]:
    """A case's files, in the order it lists them: a path may be listed
    twice, which is one of the cases."""
    return [
        (file["path"], file["text"].encode() if "text" in file else bytes.fromhex(file["hex"]))
        for file in case["files"]
    ]


def test_every_shared_case_of_files_holds_through_the_package(repository):
    cases = shared_cases(repository)
    of_files = [case for case in cases if "files" in case]
    # The rest alter another case's canonical tar, and are held by the
    # reader a server reads an upload with: a client reads no archive.
    assert len(cases) >= 60 and len(cases) - len(of_files) == 17
    assert all("archive" in case for case in cases if "files" not in case)
    seen = set()
    for case in of_files:
        name, files = case["name"], files_of(case)
        limits = case.get("limits")
        if limits is not None:
            limits = (limits["max_file_bytes"], limits["max_total_bytes"], limits["max_files"])
        passed, problems = _native._bundle(None, files, limits)
        found = sorted(code for code, _ in problems)
        seen.update(found)
        if "problems" in case:
            assert passed is None and found == case["problems"], name
            # Each problem says something of its own, on one line.
            assert all(said and "\n" not in said for _, said in problems), name
            continue
        assert problems == [] and passed["code_hash"] == case["code_hash"], name
        # And the canonical tar is the one the standard library's tarfile
        # makes of the same files, as the spec shows a builder.
        tar = jobs.canonical_tar(dict(files))
        assert passed["tar"] == tar and jobs.code_hash(tar) == case["code_hash"], name
        assert passed["paths"] == sorted(dict(files)), name
        if "runtime" in case:
            answer = json.dumps({"runtime": case["runtime"]})
            admitted, refusals = _native._bundle(None, files, limits, answer)
            found = sorted(code for code, _ in refusals)
            seen.update(found)
            assert found == case["admission"], name
            assert (admitted is None) == bool(case["admission"]), name
            if admitted is not None:
                assert admitted["code_hash"] == case["code_hash"], name
    # Every problem a bundle of files can have, but a link, which only a
    # directory holds, and an archive that is not canonical, which only a
    # server reads.
    table = [code for code, _ in (row for row in spec_rows(repository, "problem codes"))]
    assert sorted(seen) == sorted(set(table) - {"not_a_regular_file", "archive_not_canonical"})


def spec_rows(repository, name: str) -> list[list[str]]:
    """A marked table of the bundle spec, cell by cell, without backticks."""
    lines = (repository / "docs" / "bundle-spec.md").read_text(encoding="utf-8").splitlines()
    begin, end = (lines.index(f"<!-- {kind}: {name} -->") for kind in ("begin", "end"))
    return [
        [cell.strip().strip("`") for cell in row.strip().strip("|").split("|")]
        for row in lines[begin + 3 : end]
    ]


def test_the_specs_worked_example_hashes_through_the_package_as_the_spec_says(page):
    spec = page(SPEC)

    def fenced(name):
        lines = spec.section(name)
        assert lines[0].startswith("```") and lines[-1] == "```"
        return "".join(f"{line}\n" for line in lines[1:-1]).encode()

    files = [
        ("quantforge.toml", fenced("example quantforge.toml")),
        ("strategy.py", fenced("example strategy.py")),
    ]
    passed, problems = _native._bundle(None, files)
    assert problems == []
    (stated,) = [line.strip() for line in spec.section("example code hash") if line.strip()]
    assert f"`{passed['code_hash']}`" == stated
    assert len(passed["tar"]) == 3072
    assert (passed["entry"], passed["engine"], passed["python"]) == (
        "strategy.py:SmaCross",
        ">=0.3.0, <0.4.0",
        "3.12",
    )
    assert json.loads(passed["params"]) == {"fast": 20, "slow": 50}
    assert passed["deps"] == {"numpy": "2.1.3"}


def test_the_limits_a_client_checks_against_are_the_specs_defaults(repository):
    limits = {name: int(value) for name, value, _ in spec_rows(repository, "limits")}
    most = limits["max_file_bytes"]
    assert (most, limits["max_total_bytes"], limits["max_files"]) == (1048576, 8388608, 256)
    # A file of exactly the most bytes one may hold passes, and one byte
    # more is refused.
    passed, problems = _native._bundle(
        None, [("quantforge.toml", OWN), ("strategy.py", b"#" * most)]
    )
    assert problems == [] and len(passed["tar"]) == 512 * 2 + 512 + most + 1024
    passed, problems = _native._bundle(
        None, [("quantforge.toml", OWN), ("strategy.py", b"#" * (most + 1))]
    )
    assert passed is None and [code for code, _ in problems] == ["file_too_large"]
    assert problems[0][1] == (
        f'"strategy.py" is {most + 1} bytes, over the limit of {most} bytes for one file'
    )


# A manifest is read as a server reads one.


def test_a_manifest_is_read_as_a_server_reads_one_whatever_toml_this_python_reads(tmp_path):
    # TOML 1.1 lets an inline table end with a comma. A server reads a
    # manifest as TOML 1.0, and refuses it; Python's own tomllib reads TOML
    # 1.1 from 3.15 on. The package reads with the server's reader.
    text = (
        'format = 1\nentry = "strategy.py:Hold"\nengine = ">=0.1.0"\npython = "3.12"\n'
        "params = { fast = 20, }\n"
    )
    try:
        tomllib.loads(text)
        python_reads_it = True
    except tomllib.TOMLDecodeError:
        python_reads_it = False
    assert python_reads_it == (sys.version_info >= (3, 15))
    tree(tmp_path, {"quantforge.toml": text.encode(), "strategy.py": HOLD})
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(tmp_path, None, None, None)
    assert codes(refused) == ["manifest_invalid"]
    assert str(refused.value).startswith("the bundle is refused: quantforge.toml line 5, column ")
    # The same manifest without the comma is a bundle, on every Python.
    (tmp_path / "quantforge.toml").write_text(text.replace("20, }", "20 }"), encoding="utf-8")
    built = _bundles._Draft.of_path(tmp_path, None, None, None).built()
    assert json.loads(built["params"]) == {"fast": 20}


# A directory.


def test_a_directory_is_collected_as_the_spec_collects_one(tmp_path):
    files = {
        "quantforge.toml": OWN,
        "strategy.py": HOLD,
        "pkg/__init__.py": b"",
        "pkg/levels.json": b"[1, 2]\n",
        "notes.txt": b"anything at all \xff\x00\r\n",
    }
    left_out = {
        ".git/config": b"x",
        ".DS_Store": b"x",
        "pkg/.hidden.py": b"x",
        "__pycache__/strategy.cpython-311.pyc": b"x",
        "pkg/__pycache__/x.pyc": b"x",
    }
    tree(tmp_path, {**files, **left_out})
    built = _bundles._Draft.of_path(tmp_path, None, None, None).built()
    assert built["paths"] == sorted(files)
    # The files' bytes as they are, and nothing else: the same bundle as
    # the same files given in memory, by the test's own builder.
    tar = jobs.canonical_tar(files)
    assert (built["tar"], built["code_hash"]) == (tar, jobs.code_hash(tar))
    assert built["entry"] == "strategy.py:Hold" and built["python"] == "3.12"
    # A path the caller gives as text, or as any os.PathLike.
    assert _bundles._Draft.of_path(str(tmp_path), None, None, None).built() == built


def test_the_same_files_give_the_same_bundle_whatever_their_times_modes_and_order(tmp_path):
    files = {"quantforge.toml": OWN, "strategy.py": HOLD, "a/b.py": b"x = 1\n", "z.txt": b"z"}
    one = tree(tmp_path / "one", files)
    two = tree(tmp_path / "two", dict(reversed(files.items())))
    for path in two.rglob("*"):
        if path.is_file():
            os.utime(path, (1_000_000_000, 1_000_000_000))
            path.chmod(0o755 if path.suffix == ".py" else 0o600)
    built = [_bundles._Draft.of_path(root, None, None, None).built() for root in (one, two)]
    assert built[0] == built[1]
    # Whatever the name of the directory, and however it is reached.
    here = os.getcwd()
    try:
        os.chdir(two)
        assert _bundles._Draft.of_path(".", None, None, None).built() == built[0]
    finally:
        os.chdir(here)


def test_a_file_that_is_no_bundles_refuses_the_directory_and_is_never_left_out(tmp_path):
    tree(
        tmp_path, {"quantforge.toml": OWN, "strategy.py": HOLD, "notes.md": b"x", "data.pkl": b"x"}
    )
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(tmp_path, None, None, None)
    assert codes(refused) == ["extension_not_allowed", "extension_not_allowed"]
    assert str(refused.value) == (
        "the bundle is refused for 2 reasons:\n"
        '  - "data.pkl" cannot be in a bundle: a bundle holds only .py, .toml, .json, and .txt '
        "files, so leave it out or convert it\n"
        '  - "notes.md" cannot be in a bundle: a bundle holds only .py, .toml, .json, and .txt '
        "files, so leave it out or convert it"
    )


def test_a_link_in_a_directory_is_refused_and_never_followed(tmp_path):
    outside = tmp_path / "outside.py"
    outside.write_bytes(b"secret = 1\n")
    root = tree(tmp_path / "bundle", {"quantforge.toml": OWN, "strategy.py": HOLD})
    try:
        os.symlink(outside, root / "linked.py")
    except (OSError, NotImplementedError):
        pytest.skip("this account may not make a symlink here")
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(root, None, None, None)
    assert codes(refused) == ["not_a_regular_file"]
    assert '"linked.py" is a symlink or junction' in str(refused.value)
    # And a manifest that is one is no manifest of the directory's own.
    (root / "linked.py").unlink()
    (root / "quantforge.toml").unlink()
    os.symlink(tmp_path / "elsewhere.toml", root / "quantforge.toml")
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(root, None, None, None)
    assert codes(refused) == ["not_a_regular_file", "manifest_missing"]


def test_oversize_bundles_are_refused_by_each_limit_they_break(tmp_path):
    mebibyte = 1024 * 1024
    # One file over the most one file holds.
    root = tree(tmp_path / "file", {"quantforge.toml": OWN, "strategy.py": HOLD})
    (root / "big.txt").write_bytes(b"x" * (mebibyte + 1))
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(root, None, None, None)
    assert codes(refused) == ["file_too_large"]
    # Files that are each within it, and together over the most all hold.
    root = tree(tmp_path / "total", {"quantforge.toml": OWN, "strategy.py": HOLD})
    for index in range(9):
        (root / f"part{index}.txt").write_bytes(b"x" * mebibyte)
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(root, None, None, None)
    assert codes(refused) == ["bundle_too_large"]
    assert "over the limit of 8388608 bytes in all" in str(refused.value)
    # More files than a bundle holds. A builder may stop collecting once it
    # has found more than that, as this one does: these come after the
    # manifest and the strategy, which are read before it stops.
    root = tree(tmp_path / "count", {"quantforge.toml": OWN, "strategy.py": HOLD})
    for index in range(256):
        (root / f"z{index}.py").write_bytes(b"")
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(root, None, None, None)
    assert codes(refused) == ["too_many_files"]
    assert "more than the limit of 256 files" in str(refused.value)
    # A single file, which is read whole, is held to the same limit.
    big = tmp_path / "strategy.py"
    big.write_bytes(HOLD + b"#" * mebibyte)
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(big, "strategy.py:Hold", None, None)
    assert codes(refused) == ["file_too_large"]


# What `strategy` names.


def test_a_file_is_bundled_under_its_own_name_beside_a_manifest_written_for_it(tmp_path):
    source = tmp_path / "deep" / "sma.py"
    tree(tmp_path, {"deep/sma.py": HOLD, "deep/other.py": b"never read\n"})
    draft = _bundles._Draft.of_path(source, "sma.py:Hold", {"fast": 2}, None)
    built = draft.built()
    # The file alone, at the bundle's root, whatever lies beside it.
    assert built["paths"] == ["quantforge.toml", "sma.py"]
    text = (
        f'format = 1\nentry = "sma.py:Hold"\nengine = "=={qf.ENGINE_VERSION}"\npython = "3.11"\n'
        "\n[params]\nfast = 2\n"
    )
    assert manifest(built) == text
    tar = jobs.canonical_tar({"quantforge.toml": text.encode(), "sma.py": HOLD})
    assert (built["tar"], built["code_hash"]) == (tar, jobs.code_hash(tar))


def test_a_directory_without_a_manifest_is_bundled_beside_one_written_for_it(tmp_path):
    tree(tmp_path, {"strategy.py": HOLD, "pkg/__init__.py": b"", ".venv/x.py": b"x"})
    built = _bundles._Draft.of_path(tmp_path, "strategy.py:Hold", None, None).built()
    assert built["paths"] == ["pkg/__init__.py", "quantforge.toml", "strategy.py"]
    assert (built["entry"], built["engine"]) == ("strategy.py:Hold", f"=={qf.ENGINE_VERSION}")
    # The directory was given no file: its manifest is the bundle's alone.
    assert sorted(path.name for path in tmp_path.iterdir()) == [".venv", "pkg", "strategy.py"]


def test_a_directorys_own_manifest_is_its_manifest_and_takes_no_argument_beside_it(
    tmp_path, refused
):
    tree(tmp_path, {"quantforge.toml": OWN, "strategy.py": HOLD})
    own = os.path.join(tmp_path, "quantforge.toml")
    for given, named in [
        ({"entry": "strategy.py:Hold"}, "entry"),
        ({"params": {}}, "params"),
        ({"deps": []}, "deps"),
        (
            {"entry": "strategy.py:Hold", "params": {"x": 1}, "deps": ["numpy"]},
            "entry and params and deps",
        ),
    ]:
        arguments = {"entry": None, "params": None, "deps": None, **given}
        refused(
            ValueError,
            f"{own} names the strategy's entry, its params and its deps: leave out {named}, or "
            "remove the file to have it written from them",
            _bundles._Draft.of_path,
            tmp_path,
            **arguments,
        )
    built = _bundles._Draft.of_path(tmp_path, None, None, None).built()
    assert manifest(built).encode() == OWN


def test_a_strategy_without_a_manifest_needs_an_entry(tmp_path, refused):
    tree(tmp_path, {"strategy.py": HOLD})
    refused(
        ValueError,
        f'entry must name the strategy, such as entry="strategy.py:MyStrategy": {tmp_path} holds '
        "no quantforge.toml that names it",
        _bundles._Draft.of_path,
        tmp_path,
        None,
        {"fast": 2},
        None,
    )
    refused(
        ValueError,
        "entry must name the strategy, such as entry='strategy.py:MyStrategy': a file holds no "
        "quantforge.toml that names it",
        _bundles._Draft.of_path,
        tmp_path / "strategy.py",
        None,
        None,
        None,
    )


def test_what_names_no_file_and_no_directory_is_refused(tmp_path, refused):
    missing = tmp_path / "nowhere"
    refused(
        FileNotFoundError,
        f"there is no file or directory at {missing}: strategy names a strategy's .py file, or "
        "the directory its files are in",
        _bundles._Draft.of_path,
        missing,
        "strategy.py:Hold",
        None,
        None,
    )
    refused(
        ValueError,
        "strategy is empty: name a strategy's .py file, or the directory its files are in",
        _bundles._Draft.of_path,
        "",
        None,
        None,
        None,
    )
    for given, kind in [
        (None, "NoneType"),
        (7, "int"),
        (b"strategy.py", "bytes"),
        (qf.Strategy, "type"),
    ]:
        refused(
            TypeError,
            f"strategy must be a str or an os.PathLike, not {kind}",
            _bundles._Draft.of_path,
            given,
            "strategy.py:Hold",
            None,
            None,
        )


@pytest.mark.skipif(not hasattr(os, "mkfifo"), reason="this system has no named pipe")
def test_what_is_neither_a_file_nor_a_directory_is_refused_unread(tmp_path, refused):
    pipe = tmp_path / "strategy.py"
    os.mkfifo(pipe)
    # A pipe nobody writes to would be read for good.
    refused(
        ValueError,
        f"{pipe} is neither a file nor a directory: strategy names a strategy's .py file, or the "
        "directory its files are in",
        _bundles._Draft.of_path,
        pipe,
        "strategy.py:Hold",
        None,
        None,
    )


def test_a_file_that_is_no_py_file_is_refused_as_a_strategy_whatever_entry_is_given(
    tmp_path, refused
):
    # A file is the one Python file of its bundle, which the entry names. One
    # of another kind was first answered with an entry to write that named
    # the file itself, which no bundle takes: it is refused for what it is.
    for name in ["notes.txt", "quantforge.toml", "README.md", "Strategy.PY", "strategy"]:
        (tmp_path / name).write_bytes(HOLD)
        for entry in [None, "strategy.py:Hold", f"{name}:Hold"]:
            refused(
                ValueError,
                f"{tmp_path / name} is no .py file: strategy names a strategy's .py file, or the "
                "directory its files are in",
                _bundles._Draft.of_path,
                tmp_path / name,
                entry,
                {"fast": 2},
                None,
            )
    # A .py file beside them is still a strategy, and none of them is read.
    (tmp_path / "hold.py").write_bytes(HOLD)
    built = _bundles._Draft.of_path(tmp_path / "hold.py", "hold.py:Hold", None, None).built()
    assert built["paths"] == ["hold.py", "quantforge.toml"]


def test_a_file_whose_name_is_no_text_is_refused_by_the_path_rules(tmp_path):
    # A file system may hold a name whose bytes are no text, which Python
    # gives with a lone surrogate in the place of each such byte. It is
    # named with a mark there, as the spec's own walk of a directory names
    # one, and is no path of a bundle. The name is written out here as
    # `os.fsdecode` gives it where a file system holds bytes: on Windows,
    # which holds text, the same call refuses the bytes.
    assert _bundles._named("strategy.py") == "strategy.py"
    name = "str\udcffategy.py"
    assert _bundles._named(name) == "str\ufffdategy.py"
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_files([(_bundles._named(name), HOLD)], "strategy.py:Hold", None, None)
    assert codes(refused) == ["path_invalid", "entry_not_found"]
    assert '"str\ufffdategy.py" is not a bundle path: it must be ASCII' in str(refused.value)
    try:
        (tmp_path / name).write_bytes(HOLD)
    except (OSError, UnicodeError):
        # This file system takes no such name, as macOS's takes none.
        return
    with pytest.raises(qf.BundleError) as refused:
        _bundles._Draft.of_path(tmp_path / name, "strategy.py:Hold", None, None)
    assert codes(refused) == ["path_invalid", "entry_not_found"]


# The entry.


@pytest.mark.parametrize(
    "entry, said",
    [
        (
            "strategy:MyStrategy",
            "entry 'strategy:MyStrategy' names a module: name the strategy's file and its class, "
            "as a bundle's manifest does, such as 'strategy.py:MyStrategy'",
        ),
        (
            "pkg.sma:SmaCross",
            "entry 'pkg.sma:SmaCross' names a module: name the strategy's file and its class, as "
            "a bundle's manifest does, such as 'pkg/sma.py:SmaCross'",
        ),
        (
            "strategy.py",
            "entry 'strategy.py' names no .py file and class: write the strategy's file, a colon "
            "and its class, such as 'strategy.py:MyStrategy'",
        ),
        (
            "MyStrategy",
            "entry 'MyStrategy' names no .py file and class: write the strategy's file, a colon "
            "and its class, such as 'strategy.py:MyStrategy'",
        ),
        (
            "",
            "entry '' names no .py file and class: write the strategy's file, a colon and its "
            "class, such as 'strategy.py:MyStrategy'",
        ),
        (
            "strategy.txt:Hold",
            "entry 'strategy.txt:Hold' names no .py file and class: write the strategy's file, a "
            "colon and its class, such as 'strategy.py:MyStrategy'",
        ),
    ],
)
def test_an_entry_names_a_file_and_a_class_as_a_manifest_does(entry, said, refused):
    refused(ValueError, said, written, entry=entry)


def test_what_else_an_entry_is_the_bundles_own_check_says():
    for entry, code, said in [
        (
            "other.py:Hold",
            "entry_not_found",
            'the entry names "other.py", which the bundle does not hold; its Python files are strategy.py',
        ),
        ("strategy.py:1Hold", "entry_invalid", 'the class "1Hold" is not a Python identifier'),
        ("strategy.py:", "entry_invalid", 'the class "" is not a Python identifier'),
        ("my-strategy.py:Hold", "entry_invalid", '"my-strategy" is not a Python identifier'),
        ("class.py:Hold", "entry_invalid", '"class" is a Python keyword'),
    ]:
        files = [(entry.partition(":")[0] if code == "entry_invalid" else "strategy.py", HOLD)]
        with pytest.raises(qf.BundleError) as refused:
            written(entry=entry, files=files)
        assert codes(refused) == [code], entry
        assert said in str(refused.value), entry
    for entry in (7, None, b"strategy.py:Hold", ["strategy.py:Hold"]):
        with pytest.raises(TypeError, match="^entry must be a str, not "):
            written(entry=entry)


# The manifest written from the arguments.


def test_the_same_arguments_write_the_same_manifest_in_its_one_form():
    one = written(
        "pkg/sma.py:Sma",
        {"slow": 50, "fast": 20},
        {"pandas": "2.2.3", "numpy": "2.1.3"},
        files=[("pkg/sma.py", HOLD)],
    )
    two = written(
        "pkg/sma.py:Sma",
        {"fast": 20, "slow": 50},
        {"numpy": "2.1.3", "pandas": "2.2.3"},
        files=[("pkg/sma.py", HOLD)],
    )
    text = manifest(one.built())
    assert text == (
        f'format = 1\nentry = "pkg/sma.py:Sma"\nengine = "=={qf.ENGINE_VERSION}"\npython = "3.11"\n'
        '\n[params]\nfast = 20\nslow = 50\n\n[deps]\nnumpy = "2.1.3"\npandas = "2.2.3"\n'
    )
    assert one.built() == two.built()
    # No table where there is nothing to put in it.
    bare = manifest(written().built())
    assert bare == (
        f'format = 1\nentry = "strategy.py:Hold"\nengine = "=={qf.ENGINE_VERSION}"\npython = "3.11"\n'
    )
    assert manifest(written(params={}, deps={}).built()) == bare
    assert manifest(written(params={}, deps=[]).built()) == bare
    # It is written for the Python a server runs, once that is known.
    for_a_server = written().built(*offered("3.13"), SERVER)
    assert 'python = "3.13"\n' in manifest(for_a_server) and for_a_server["python"] == "3.13"


def test_every_value_a_parameter_is_reads_back_as_the_strategy_is_handed_it():
    params = {
        "n": -7,
        "zero": 0,
        "largest": 2**53 - 1,
        "smallest": -(2**53 - 1),
        "ratio": 0.1,
        "whole": 2.0,
        "big": 1e20,
        "tiny": 1e-7,
        "negative_zero": -0.0,
        "least": 5e-324,
        "most": 1.7976931348623157e308,
        # A float between two shortest texts that are equally near: Python
        # writes one and the manifest's writer the other, and both are it.
        "tie": 1669054756414022.2,
        "on": True,
        "off": False,
        "name": 'a "q" \\ é\ttab\nline',
        "control": "\x00\x01\x7f\x85",
        "emoji": "币安人生 \U0001f600",
        "empty": "",
        "levels": [1, 2.5, "x", [True, []], {"k": [1]}],
        "window": {"size": 5, "kind": "ema", "deeper": {"x": {}}},
        "odd key": 1,
        "": 2,
        "ключ": 3,
        'q"uote': 4,
    }
    built = written(params=params).built()
    back = json.loads(built["params"])
    assert _bundles._same(back, params)
    # Each as the value it is: neither 2.0 for 2, nor 1 for True.
    assert [type(back[name]) for name in ("n", "whole", "on", "negative_zero")] == [
        int,
        float,
        bool,
        float,
    ]
    assert str(back["negative_zero"]) == "-0.0" and back["tie"] == 1669054756414022.2
    text = manifest(built)
    for line in [
        "big = 1e20\n",
        "tiny = 1e-7\n",
        "whole = 2.0\n",
        "negative_zero = -0.0\n",
        "tie = 1669054756414022.3\n",
        'name = "a \\"q\\" \\\\ é\\ttab\\nline"\n',
        'control = "\\u0000\\u0001\\u007F\\u0085"\n',
        'levels = [1, 2.5, "x", [true, []], { k = [1] }]\n',
        'window = { deeper = { x = {} }, kind = "ema", size = 5 }\n',
        '"odd key" = 1\n',
        '"" = 2\n',
        '"ключ" = 3\n',
        '"q\\"uote" = 4\n',
    ]:
        assert line in text, line
    # The params sorted by their names, as text is ordered.
    names = re.findall(r"^(\S.*?) = ", text.split("[params]\n")[1], re.M)
    assert names == sorted(names, key=lambda name: json.loads(name) if name[0] == '"' else name)
    # What was given is copied as it is checked: changing it afterwards
    # changes nothing of what is built.
    given = {"levels": [1, 2], "window": {"size": 5}}
    draft = written(params=given)
    before = draft.built()
    given["levels"].append(3)
    given["window"]["size"] = 6
    given["new"] = 1
    assert draft.built() == before


@pytest.mark.parametrize(
    "params, kind, said",
    [
        (["fast", 20], TypeError, "params must be a dict or None, not list"),
        ("fast=20", TypeError, "params must be a dict or None, not str"),
        (
            {"at": __import__("datetime").date(2024, 1, 1)},
            TypeError,
            "params.at must be a str, a bool, an int, a float, a list or a dict, not date: a "
            "strategy's parameters reach it as JSON, and an amount that must stay exact goes in "
            "a str",
        ),
        (
            {"cash": __import__("decimal").Decimal("10.5")},
            TypeError,
            "params.cash must be a str, a bool, an int, a float, a list or a dict, not Decimal: "
            "a strategy's parameters reach it as JSON, and an amount that must stay exact goes "
            "in a str",
        ),
        (
            {"window": {"levels": [1, (2, 3)]}},
            TypeError,
            "params.window.levels[1] must be a str, a bool, an int, a float, a list or a dict, "
            "not tuple: a strategy's parameters reach it as JSON, and an amount that must stay "
            "exact goes in a str",
        ),
        (
            {"nothing": None},
            TypeError,
            "params.nothing must be a str, a bool, an int, a float, a list or a dict, not "
            "NoneType: a strategy's parameters reach it as JSON, and an amount that must stay "
            "exact goes in a str",
        ),
        (
            {"odd key": b"x"},
            TypeError,
            'params."odd key" must be a str, a bool, an int, a float, a list or a dict, not bytes: a strategy\'s parameters reach it as JSON, and an amount that must stay exact goes in a str',
        ),
        ({7: 1}, TypeError, "a key of params must be a str, not int"),
        ({"window": {None: 1}}, TypeError, "a key of params.window must be a str, not NoneType"),
        (
            {"big": 2**53},
            ValueError,
            "params.big is an integer beyond 2^53 - 1, which not every JSON reader holds exactly; "
            "write it as a str",
        ),
        (
            {"levels": [0, -(2**53)]},
            ValueError,
            "params.levels[1] is an integer beyond 2^53 - 1, which not every JSON reader holds "
            "exactly; write it as a str",
        ),
        ({"x": float("nan")}, ValueError, "params.x is not a finite number"),
        ({"x": [float("inf")]}, ValueError, "params.x[0] is not a finite number"),
        ({"x": {"y": float("-inf")}}, ValueError, "params.x.y is not a finite number"),
        ({"$ref": 1}, ValueError, 'params."$ref" starts with $, which is reserved'),
        ({"a": {"$b": 1}}, ValueError, 'params.a."$b" starts with $, which is reserved'),
        (
            {"text": "lone \ud800"},
            ValueError,
            "params.text holds a lone surrogate, which no file holds as text",
        ),
        (
            {"\udfff": 1},
            ValueError,
            "a key of params holds a lone surrogate, which no file holds as text",
        ),
    ],
)
def test_a_parameter_is_a_value_json_holds_of_exactly_that_type(params, kind, said, refused):
    refused(kind, said, written, params=params)


def test_a_type_derived_from_one_json_holds_is_none_of_them():
    class Text(str):
        pass

    class Whole(int):
        pass

    class Table(dict):
        pass

    for value, name in [(Text("x"), "Text"), (Whole(1), "Whole"), (Table(), "Table")]:
        with pytest.raises(TypeError, match=f"^params.x must be .*, not {name}: "):
            written(params={"x": value})
    with pytest.raises(TypeError, match="^params must be a dict or None, not Table$"):
        written(params=Table())


def test_params_nest_as_deep_as_a_manifest_does_and_no_deeper(repository, refused):
    # The manifest's own table is the first level and params the second.
    source = (repository / "src" / "protocol" / "bundle" / "manifest.rs").read_text("utf-8")
    (most,) = re.findall(r"^pub const MAX_MANIFEST_DEPTH: usize = (\d+);$", source, re.M)
    assert _bundles._MOST_DEPTH == int(most) == 32

    def nested(levels, innermost):
        value = innermost
        for _ in range(levels):
            value = {"x": value}
        return value

    # Thirty tables below params, the last at the thirty-second level.
    deepest = nested(29, [1])
    assert json.loads(written(params={"x": deepest}).built()["params"]) == {"x": deepest}
    path = "params." + ".".join("x" * 31)
    refused(
        ValueError,
        f"{path} lies more than 32 levels deep in the manifest, whose own table is the first "
        "level and params the second",
        written,
        params={"x": nested(30, [1])},
    )
    # A value that holds itself is as deep as any.
    loop = []
    loop.append(loop)
    with pytest.raises(ValueError, match="lies more than 32 levels deep"):
        written(params={"x": loop})
    # The whole numbers a parameter holds are the spec's.
    source = (repository / "src" / "protocol" / "bundle" / "params.rs").read_text("utf-8")
    assert "const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;" in source
    assert _bundles._MOST_SAFE_INTEGER == (1 << 53) - 1


def test_what_the_writer_and_the_reader_make_of_params_is_compared_before_anything_is_sent(
    monkeypatch,
):
    # Two pieces of code write a manifest and read it back. Were they ever
    # to differ, the strategy would be handed another value than the caller
    # gave: nothing is sent then.
    real = _native._manifest
    draft = written(params={"fast": 20})

    class Changing:
        def __getattr__(self, name):
            return getattr(_native, name)

        @staticmethod
        def _manifest(entry, python, params, deps):
            return real(entry, python, {"fast": 20.0}, deps)

    monkeypatch.setattr(_bundles, "_native", Changing())
    with pytest.raises(RuntimeError) as raised:
        draft.built()
    assert str(raised.value) == (
        "the manifest written for the bundle does not read back as the params it was given, "
        "which is a fault of quantforge_py's own: nothing was sent"
    )
    same = _bundles._same
    assert same({"a": [1, {"b": 2.5}], "c": True}, {"c": True, "a": [1, {"b": 2.5}]})
    for one, other in [
        (1, 1.0),
        (1, True),
        (0.0, -0.0),
        ([1], [1, 2]),
        ({"a": 1}, {"a": 1, "b": 2}),
        ({"a": 1}, {"b": 1}),
        ("1", 1),
        ([1, 2], [2, 1]),
    ]:
        assert not same(one, other) and not same({"x": [one]}, {"x": [other]}), (one, other)


# The packages a strategy imports.


def test_deps_pin_each_package_or_name_it_for_the_servers_version(refused):
    numpy_and_pandas = {"numpy": ["2.4.6"], "pandas": ["3.1.1"]}
    # A dict pins, as the manifest's own table does.
    pinned = written(deps={"pandas": "3.1.1", "numpy": "2.4.6"})
    assert pinned.built()["deps"] == {"numpy": "2.4.6", "pandas": "3.1.1"}
    for_a_server = pinned.built(*offered(**numpy_and_pandas), SERVER)
    assert for_a_server["deps"] == {"numpy": "2.4.6", "pandas": "3.1.1"}
    # Names alone are pinned at the version the server offers, and make the
    # bundle the same pins make.
    for names in (["numpy", "pandas"], ("pandas", "numpy")):
        named = written(deps=names)
        assert named.built(*offered(**numpy_and_pandas), SERVER) == for_a_server
        # Before a server is known they are checked as names, at a version
        # that is none: such a bundle is never sent.
        assert named.built()["deps"] == {"numpy": "0", "pandas": "0"}
    # Where the server offers more than one version of a package, the
    # caller says which.
    several = offered(numpy=["2.1.3", "2.4.6"])
    refused(
        ValueError,
        f"deps names numpy without a version, and the server at {SERVER} offers 2.1.3 and 2.4.6 "
        "of it: pin one, as in deps={'numpy': '2.4.6'}",
        written(deps=["numpy"]).built,
        *several,
        SERVER,
    )
    assert written(deps={"numpy": "2.1.3"}).built(*several, SERVER)["deps"] == {"numpy": "2.1.3"}


def test_a_package_the_server_does_not_offer_is_refused_in_the_admission_checks_words():
    numpy = offered(numpy=["2.4.6"])
    cases = [
        (
            ["scipy"],
            ["dependency_not_allowed"],
            f"the server at {SERVER} cannot run the bundle: the bundle imports scipy, which this "
            "server does not offer; it offers numpy",
        ),
        (
            {"scipy": "1.14.0"},
            ["dependency_not_allowed"],
            f"the server at {SERVER} cannot run the bundle: the bundle imports scipy, which this "
            "server does not offer; it offers numpy",
        ),
        (
            {"numpy": "2.1.3"},
            ["dependency_version_unavailable"],
            f"the server at {SERVER} cannot run the bundle: the bundle pins numpy 2.1.3, but this "
            "server offers numpy 2.4.6 only",
        ),
        (
            ["scipy", "numpy", "torch"],
            ["dependency_not_allowed", "dependency_not_allowed"],
            f"the server at {SERVER} cannot run the bundle for 2 reasons:\n"
            "  - the bundle imports scipy, which this server does not offer; it offers numpy\n"
            "  - the bundle imports torch, which this server does not offer; it offers numpy",
        ),
    ]
    for deps, expected, said in cases:
        draft = written(deps=deps)
        with pytest.raises(qf.BundleError) as refused:
            draft.built(*numpy, SERVER)
        assert codes(refused) == expected and str(refused.value) == said, deps
    # A server that offers no package at all says so.
    with pytest.raises(qf.BundleError) as refused:
        written(deps=["numpy"]).built(*offered(), SERVER)
    assert str(refused.value).endswith("which this server does not offer; it offers no packages")


def test_a_bundle_is_admitted_against_a_servers_engine_and_python_too(tmp_path):
    # A manifest that is written is written for the server's Python, and
    # for this package's engine, exactly.
    draft = written()
    assert draft.built(*offered("3.14"), SERVER)["python"] == "3.14"
    with pytest.raises(qf.BundleError) as refused:
        draft.built(*offered(engine="9.9.9"), SERVER)
    assert codes(refused) == ["engine_mismatch"]
    assert str(refused.value) == (
        f"the server at {SERVER} cannot run the bundle: the bundle needs engine "
        f"=={qf.ENGINE_VERSION}, but this server runs 9.9.9; change engine in the manifest or use "
        "a matching server"
    )
    # A directory's own manifest says which Python and which engine itself.
    tree(tmp_path, {"quantforge.toml": OWN, "strategy.py": HOLD})
    own = _bundles._Draft.of_path(tmp_path, None, None, None)
    assert own.built(*offered("3.12", engine="0.1.0"), SERVER)["python"] == "3.12"
    with pytest.raises(qf.BundleError) as refused:
        own.built(*offered("3.13", engine="0.0.9"), SERVER)
    assert codes(refused) == ["engine_mismatch", "python_mismatch"]
    assert str(refused.value).startswith(f"the server at {SERVER} cannot run the bundle for 2 ")
    # What breaks the spec is refused as that, whatever the server.
    (tmp_path / "notes.md").write_bytes(b"x")
    with pytest.raises(qf.BundleError) as refused:
        own.built(*offered("3.13", engine="0.0.9"), SERVER)
    assert codes(refused) == ["extension_not_allowed"]
    assert str(refused.value).startswith("the bundle is refused: ")


@pytest.mark.parametrize(
    "deps, kind, said",
    [
        (
            "numpy",
            TypeError,
            "deps must be a dict of each package's version, a list or a tuple of package names, or None, not str",
        ),
        (
            {"numpy"},
            TypeError,
            "deps must be a dict of each package's version, a list or a tuple of package names, or None, not set",
        ),
        ([7], TypeError, "deps[0] must be a str, a package's name, not int"),
        (["numpy", None], TypeError, "deps[1] must be a str, a package's name, not NoneType"),
        ({"numpy": 2.1}, TypeError, "deps['numpy'] must be a str, the version to pin, not float"),
        (
            {"numpy": None},
            TypeError,
            "deps['numpy'] must be a str, the version to pin, not NoneType",
        ),
        ({7: "1.0"}, TypeError, "a key of deps must be a str, not int"),
        (["numpy", "pandas", "numpy"], ValueError, "deps names numpy twice"),
    ],
)
def test_deps_are_a_dict_of_pins_or_a_list_of_names(deps, kind, said, refused):
    refused(kind, said, written, deps=deps)


def test_a_name_or_a_version_that_is_none_is_refused_by_the_bundles_own_check():
    for deps, said in [
        (
            ["PyYAML"],
            'deps.PyYAML: invalid package name "PyYAML": names are compared in their normalized '
            'form, so write it as "pyyaml"',
        ),
        (
            {"typing_extensions": "4.12.2"},
            'deps.typing_extensions: invalid package name "typing_extensions": names are compared '
            'in their normalized form, so write it as "typing-extensions"',
        ),
        ({"numpy": ">=2.0"}, 'deps.numpy: invalid package version ">=2.0"'),
        ({"numpy": "2.1.3+local"}, 'deps.numpy: invalid package version "2.1.3+local"'),
        (["num py"], 'deps."num py": invalid package name "num py"'),
    ]:
        with pytest.raises(qf.BundleError) as refused:
            written(deps=deps)
        assert codes(refused) == ["dependency_invalid"], deps
        assert said in str(refused.value), deps


def test_the_extensions_no_entry_names_are_the_specs_other_than_python(page):
    (listed,) = [line.strip() for line in page(SPEC).section("extensions") if line.strip()]
    extensions = {each.strip("`.") for each in listed.split(", ")}
    assert extensions == set(_bundles._OTHER_EXTENSIONS) | {"py"}


def test_the_codes_of_a_bundle_a_server_cannot_run_are_the_admission_checks(repository):
    source = (repository / "src" / "protocol" / "bundle" / "runtime.rs").read_text("utf-8")
    named = set(re.findall(r"ProblemCode::(\w+)", source))
    snake = {re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower() for name in named}
    assert snake == set(_bundles._ADMISSION) and len(snake) == 4


# The exception.


def test_a_bundle_error_is_a_value_error_that_holds_every_problem_by_the_specs_code():
    assert issubclass(qf.BundleError, ValueError) and not issubclass(qf.BundleError, qf.RemoteError)
    assert (qf.BundleError.__module__, qf.BundleError.__qualname__) == (
        "quantforge_py",
        "BundleError",
    )
    with pytest.raises(ValueError) as refused:
        written(entry="other.py:Hold", files=[("strategy.py", HOLD), ("x.md", b"")])
    error = refused.value
    assert type(error) is qf.BundleError
    assert error.problems == (
        (
            "extension_not_allowed",
            '"x.md" cannot be in a bundle: a bundle holds only .py, .toml, .json, and .txt files, '
            "so leave it out or convert it",
        ),
        (
            "entry_not_found",
            'the entry names "other.py", which the bundle does not hold; its Python files are '
            "strategy.py",
        ),
    )
    assert str(error) == "the bundle is refused for 2 reasons:" + "".join(
        f"\n  - {said}" for _, said in error.problems
    )
    # One problem is said on one line.
    with pytest.raises(qf.BundleError) as refused:
        written(entry="other.py:Hold")
    assert str(refused.value) == f"the bundle is refused: {error.problems[1][1]}"
    assert refused.value.problems == (error.problems[1],)
    # It is an exception like any other: built, derived from, and pickled
    # with what it holds.
    made = qf.BundleError("refused", [("path_invalid", "a path")])
    assert (str(made), made.problems) == ("refused", (("path_invalid", "a path"),))
    assert qf.BundleError("refused").problems == ()
    copied = pickle.loads(pickle.dumps(error))
    assert type(copied) is qf.BundleError
    assert (str(copied), copied.problems) == (str(error), error.problems)
    assert issubclass(type("Derived", (qf.BundleError,), {}), qf.BundleError)
    assert qf.BundleError.__doc__


# Nothing is pickled, and nothing is run.


def test_a_strategys_files_are_bundled_and_never_imported_or_run(tmp_path):
    marker = tmp_path / "ran"
    source = textwrap.dedent(
        f"""
        import pathlib

        pathlib.Path({str(marker)!r}).write_text("imported")
        raise SystemExit("a bundle's file was run")
        """
    ).encode()
    root = tree(tmp_path / "bundle", {"strategy.py": source, "quantforge.toml": OWN})
    built = _bundles._Draft.of_path(root, None, None, None).built()
    assert built["paths"] == ["quantforge.toml", "strategy.py"] and not marker.exists()
    # Nor is a file that is no Python at all refused for it: a bundle's
    # files are bytes here.
    (root / "strategy.py").write_bytes(b"\xff\xfe not python \x00")
    assert _bundles._Draft.of_path(root, None, None, None).built()["paths"] == built["paths"]
    assert "strategy" not in sys.modules and not marker.exists()


def test_no_module_of_the_package_pickles_and_building_a_bundle_loads_no_pickler(tmp_path):
    # By design: a bundle is files and a manifest, and no object of the
    # caller's is serialized. No module of the package names a pickler.
    package = os.path.dirname(qf.__file__)
    picklers = ("pickle", "marshal", "shelve", "dill", "cloudpickle", "copyreg")
    for name in sorted(os.listdir(package)):
        if not name.endswith(".py"):
            continue
        with open(os.path.join(package, name), encoding="utf-8") as file:
            source = file.read()
        imported = re.findall(r"^\s*(?:import|from)\s+([\w.]+)", source, re.M)
        assert not set(imported) & set(picklers), name
        assert not re.search(r"\b__reduce(?:_ex)?__\b.*=", source) or name == "_jobs.py", name
    # And building one, from files and from a class, loads none.
    tree(tmp_path, {"strategy.py": HOLD})
    code = """
        import sys

        import quantforge_py as qf
        from quantforge_py import _bundles, _source

        class Mine(qf.Strategy):
            def on_bar(self, ctx, bar):
                return None

        built = _bundles._Draft.of_path(sys.argv[1], "strategy.py:Hold", {"fast": 2}, None).built()
        file, entry = _source._class_file(Mine, "import quantforge_py as qf", True)
        _bundles._Draft.of_files([file], entry, {"fast": 2}, ["numpy"]).built()
        print(sorted(name for name in ("pickle", "_pickle", "shelve", "dill", "cloudpickle") if name in sys.modules))
        print(built["code_hash"])
    """
    script = tmp_path / "builds.py"
    script.write_text(textwrap.dedent(code), encoding="utf-8")
    run = subprocess.run(
        [sys.executable, str(script), str(tmp_path / "strategy.py")],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert run.returncode == 0, run.stderr
    loaded, code_hash = run.stdout.splitlines()
    assert loaded == "[]" and code_hash.startswith("sha256:")
