"""`docs/python-package.md` and the manifests, held to the package: the
page's tables and its tree are the package's own, and the workspace is wired
as the page says. Each table and the tree sit between `<!-- begin: name -->`
and `<!-- end: name -->` markers."""

import re
import subprocess

import pytest

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


def test_the_page_states_the_extras_the_distribution_offers(page, quoted, pyproject):
    project = pyproject["project"]
    # The package requires nothing, and NumPy and pandas are its extras.
    assert "dependencies" not in project
    rows = [
        [quoted(extra), [quoted(one) for one in requires.split(", ")], quoted(installed)]
        for extra, requires, installed in page(PAGE).rows("extras")
    ]
    assert rows == [
        [extra, required, f'pip install "{project["name"]}[{extra}]"']
        for extra, required in project["optional-dependencies"].items()
    ]
    assert rows == [
        ["numpy", ["numpy>=2.0,<3"], 'pip install "quantforge-py[numpy]"'],
        # A frame is made of a batch's arrays: the NumPy they need, and pandas.
        ["pandas", ["numpy>=2.0,<3", "pandas>=2.2.2,<4"], 'pip install "quantforge-py[pandas]"'],
    ]


def test_ci_names_the_oldest_python_first_where_the_oldest_extras_are_tested(repository, pyproject):
    # The check script tests the oldest NumPy and the oldest pandas on the
    # first Python it is given, if that is the oldest the package supports:
    # each has a wheel for it, and none for the newest Python. CI is where
    # that has to hold.
    workflow = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    (named,) = re.findall(r"^\s+run: \./scripts/python-package-check\.sh (.+)$", workflow, re.M)
    interpreters = named.split()
    oldest = pyproject["project"]["requires-python"].removeprefix(">=")
    assert interpreters[0] == f"python{oldest}"
    assert len(interpreters) >= 2, "the oldest Python and the newest"
    script = (repository / "scripts" / "python-package-check.sh").read_text(encoding="utf-8")
    assert 'if [[ $first_python == "$oldest_python" ]]; then' in script
    assert 'tested "$1" "$work/wheel" oldest-extras' in script
    # That run installs both pins, and tells the tests of both.
    assert "requirements+=$'\\n'$oldest_numpy$'\\n'$oldest_pandas" in script
    assert (
        "QUANTFORGE_PY_TEST_OLDEST_NUMPY=1 QUANTFORGE_PY_TEST_OLDEST_PANDAS=1 \\\n"
        '                "$work/test-$runs/bin/python" -m pytest'
    ) in script
    assert sorted(pyproject["dependency-groups"]) == ["oldest-numpy", "oldest-pandas", "test"]


def test_the_check_script_tests_the_package_without_its_extras(repository, pyproject):
    # One run installs the wheel alone, and is told that it has no NumPy and
    # no pandas. Every other run installs it with the extras pyproject.toml
    # names, as the in-place install does.
    script = (repository / "scripts" / "python-package-check.sh").read_text(encoding="utf-8")
    assert 'tested "$1" "$work/wheel" without-extras' in script
    assert (
        "QUANTFORGE_PY_TEST_WITHOUT_NUMPY=1 QUANTFORGE_PY_TEST_WITHOUT_PANDAS=1 \\\n"
        '                "$work/test-$runs/bin/python" -m pytest'
    ) in script
    assert 'virtualenv "$python" "$work/test-$runs" "$requirements" "${wheel}[${extras}]"' in script
    assert 'maturin develop --extras "$extras"' in script
    assert list(pyproject["project"]["optional-dependencies"]) == ["numpy", "pandas"]
    # Neither is ever compiled where the tests run: each comes as a wheel.
    assert "--only-binary numpy,pandas" in script


def test_the_check_script_tests_the_package_with_numpy_and_without_pandas(repository, pyproject):
    # One run installs the wheel with its `numpy` extra alone, as whoever
    # asked for the arrays and not for the frames has it, and is told that it
    # has no pandas: the tests of the frames are skipped there, and the rest
    # hold the package to working with NumPy and without pandas.
    script = (repository / "scripts" / "python-package-check.sh").read_text(encoding="utf-8")
    assert 'tested "$1" "$work/wheel" without-pandas' in script
    assert 'virtualenv "$python" "$work/test-$runs" "$requirements" "${wheel}[numpy]"' in script
    assert (
        "        without-pandas)\n"
        '            QUANTFORGE_PY_TEST_WITHOUT_PANDAS=1 "$work/test-$runs/bin/python" -m pytest\n'
    ) in script
    # The extra it names is the one that brings NumPy alone.
    assert pyproject["project"]["optional-dependencies"]["numpy"] == ["numpy>=2.0,<3"]
    # Seven runs of the tests in all, for two Pythons: each Python with both
    # extras, the oldest of both, NumPy alone, neither, the SQLite-only
    # wheel, and the in-place install.
    assert script.count("\ntested ") + script.count("\n    tested ") == 5
    assert script.count("    python -m pytest\n") == 1


def test_the_readme_shows_only_names_the_package_publishes_and_its_example_runs(
    page, pyproject, without_numpy, without_pandas
):
    readme = page("quantforge-py/README.md")
    shown = re.findall(r"\bqf\.(\w+)", readme.text)
    assert shown, "the readme shows the package at work"
    assert set(shown) <= set(quantforge_py.__all__)
    # What PyPI shows is what a reader copies: every example runs as it is
    # written, one after another. The last two are of the arrays and of the
    # frames, the only ones that need NumPy, and pandas with it.
    *examples, of_the_arrays, of_the_frames = readme.fences("python")
    assert examples and "to_numpy" in of_the_arrays and "to_dataframe" in of_the_frames
    assert not any("numpy" in example or "dataframe" in example for example in examples)
    assert "dataframe" not in of_the_arrays and "numpy" not in of_the_frames
    names = {}
    for example in examples:
        exec(compile(example, readme.name, "exec"), names)
    # The readme says how the package is installed with each extra, as
    # pyproject.toml names them.
    install = f'pip install "{pyproject["project"]["name"]}[numpy]"'
    with_pandas = f'pip install "{pyproject["project"]["name"]}[pandas]"'
    assert f"`{install}`" in readme.text and f"`{with_pandas}`" in readme.text
    if without_numpy:
        # Installed without the extra, the example of the arrays says what
        # to install, at its first conversion.
        with pytest.raises(ModuleNotFoundError) as caught:
            exec(compile(of_the_arrays, readme.name, "exec"), names)
        assert str(caught.value).endswith(f": {install}")
        assert "arrays" not in names and "batch" in names
    else:
        exec(compile(of_the_arrays, readme.name, "exec"), names)
    if without_pandas:
        # And the example of the frames, at its first frame.
        with pytest.raises(ModuleNotFoundError) as caught:
            exec(compile(of_the_frames, readme.name, "exec"), names)
        assert str(caught.value).endswith(f": {with_pandas}")
        assert "frame" not in names
    else:
        exec(compile(of_the_frames, readme.name, "exec"), names)
    # The run it shows brings what it says beside it.
    result = names["result"]
    assert type(result) is quantforge_py.BacktestResult
    shown = dict(re.findall(r"^result\.([\w.\[\]]+) +# (Decimal\('[\d.]+'\))", readme.text, re.M))
    assert set(shown) == {
        "final_equity",
        "total_return_pct",
        "trades[0].entry_price",
        "trades[0].exit_price",
    }
    for field, value in shown.items():
        assert repr(eval(f"result.{field}", {"result": result})) == value, field
    if without_numpy:
        return
    # And the arrays it shows are what it says beside them.
    arrays, batch = names["arrays"], names["batch"]
    assert f'arrays["close"]             # float64: {arrays["close"]!r}' in readme.text
    assert [str(arrays[column].dtype) for column in ("close", "open_time_ms", "trades")] == [
        "float64",
        "int64",
        "uint64",
    ]
    assert arrays["trades"].mask.all(), "no candle of the example has a trade count"
    assert names["again"] == batch and names["again"].market == batch.market
    assert (
        quantforge_py.run_backtest(names["again"], names["EnterThenExit"](), names["config"])
        == result
    )
    if without_pandas:
        return
    # And so are the frames: each line that shows a value shows the value.
    frame, trades, equity = names["frame"], names["trades"], names["equity"]
    shown = dict(re.findall(r"^(\w+\[\"\w+\"\]\.tolist\(\)) +# (\[[\d., ]+\])", readme.text, re.M))
    assert set(shown) == {
        'frame["close"].tolist()',
        'trades["gross_quote_pnl"].tolist()',
        'equity["equity"].tolist()',
    }
    for written, value in shown.items():
        assert repr(eval(written, {"frame": frame, "trades": trades, "equity": equity})) == value
    assert frame.index.name == "open_time" and str(frame.index.dtype) == "datetime64[ms, UTC]"
    assert quantforge_py.dataframe_to_candles(frame, market=batch.market) == batch
    assert len(trades) == len(result.trades) and len(equity) == len(result.equity_curve)
    assert equity.index.equals(frame.index)


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
