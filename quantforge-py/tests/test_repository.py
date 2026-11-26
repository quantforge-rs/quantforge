"""`docs/python-package.md` and the manifests, held to the package: the
page's tables and its tree are the package's own, and the workspace is wired
as the page says. Each table and the tree sit between `<!-- begin: name -->`
and `<!-- end: name -->` markers."""

import ast
import os
import re
import subprocess

import pytest

import quantforge_py
import stores
from quantforge_py import _native

PAGE = "docs/python-package.md"
CI_PAGE = "docs/ci.md"


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


def test_the_check_script_builds_the_binary_and_ci_has_the_server_the_readers_tests_need(
    repository,
):
    # The reader's tests read stores the command line wrote: the script
    # builds it before any run of the tests, and tells each where it is.
    script = (repository / "scripts" / "python-package-check.sh").read_text(encoding="utf-8")
    built = "\ncargo build --locked\n"
    named = f"\nexport {stores.CLI}=$target/debug/quantforge\n"
    assert built in script and named in script
    assert script.index(built) < script.index(named) < script.index('\nfor python in "$@"; do\n')
    # Where cargo put it, which CARGO_TARGET_DIR can move out of ./target.
    assert "\ntarget=$(cargo metadata --format-version 1 --no-deps |\n" in script
    assert """print(json.load(sys.stdin)["target_directory"])')\n""" in script
    # No run is told to go without PostgreSQL: what the caller exported
    # reaches every one of them.
    assert f"unset {stores.POSTGRES_URL}" not in script
    assert stores.POSTGRES_URL in script and stores.POSTGRES_REQUIRED in script
    # CI's job has a server for them, and says that it has to be tested, so
    # a test that could not reach it fails where it would otherwise skip.
    workflow = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    (job,) = re.findall(r"^  python:\n(?:(?:    .*)?\n)+", workflow, re.M)
    assert "    services:\n" in job and "        image: postgres:18\n" in job
    assert f'          {stores.POSTGRES_REQUIRED}: "1"\n' in job
    served = rf"          {stores.POSTGRES_URL}: postgres://\S+@127\.0\.0\.1:5432/postgres\?sslmode=disable\n"
    assert re.search(served, job)
    assert job.index("run: ./scripts/python-package-check.sh") < job.index(stores.POSTGRES_REQUIRED)


def workflow_job(repository, name: str) -> str:
    """A job of CI's workflow as written: its line and every line under it."""
    workflow = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    (job,) = re.findall(rf"^  {name}:\n(?:(?:    .*)?\n)+", workflow, re.M)
    return job


def test_the_page_declares_the_two_builds_the_crate_has(page, quoted, crate):
    # The crate's one feature, PostgreSQL, is its default one: a build has
    # it or is the SQLite-only one, and there is no third to declare.
    assert crate["features"] == {"default": ["postgres"], "postgres": ["quantforge/postgres"]}
    builds = page(PAGE).rows("builds")
    assert [build[0] for build in builds] == ["The package", "The SQLite-only package"]
    features = [re.findall(r"`([^`]+)`", build[1]) for build in builds]
    assert features == [crate["features"]["default"], ["--no-default-features"]]
    stated = [ast.literal_eval(quoted(build[2])) for build in builds]
    assert stated == [("sqlite", "postgres"), ("sqlite",)]
    # The build under test is one of the two, and the package alone is
    # published.
    assert quantforge_py.STORAGE_BACKENDS in stated
    assert [build[3] == "Never" for build in builds] == [False, True]


def test_ci_builds_the_wheel_of_each_platform_as_a_release_would_publish_it(
    repository, page, quoted, crate
):
    job = workflow_job(repository, "wheel")
    # A leg for each platform, each a check of its own, as the page lists
    # them.
    (platforms,) = re.findall(r"^        os: \[(.+)\]$", job, re.M)
    legs = [f"wheel ({platform})" for platform in platforms.split(", ")]
    assert legs == ["wheel (ubuntu-latest)", "wheel (macos-latest)", "wheel (windows-latest)"]
    assert "    name: wheel (${{ matrix.os }})\n" in job
    listed = page(CI_PAGE).rows("wheels")
    assert [quoted(leg) for leg, _, _ in listed] == legs
    # Built for release, with the crate's default features, both stores,
    # into the directory the tests take it from, and only with a tag PyPI
    # takes.
    (args,) = re.findall(r"^          args: (.+)$", job, re.M)
    built = args.split()
    assert "--release" in built
    assert built[built.index("--manifest-path") + 1] == "quantforge-py/Cargo.toml"
    assert built[built.index("--out") + 1] == "dist"
    assert built[built.index("--compatibility") + 1] == "pypi"
    assert not {"--features", "-F", "--no-default-features", "--all-features"} & set(built)
    assert crate["features"]["default"] == ["postgres"]
    # The Linux wheel in the manylinux container the page names.
    assert '          manylinux: "2_28"\n' in job
    assert listed[0][1] == "In a `manylinux_2_28` container"


def test_ci_tests_each_wheel_where_it_was_built_before_it_keeps_it(repository, pyproject):
    job = workflow_job(repository, "wheel")
    # The oldest Python the package supports and the newest the python job
    # names, each named to the check by the path its setup gives.
    workflow = (repository / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
    (named,) = re.findall(r"^\s+run: \./scripts/python-package-check\.sh (.+)$", workflow, re.M)
    oldest = pyproject["project"]["requires-python"].removeprefix(">=")
    newest = named.split()[-1].removeprefix("python")
    assert re.findall(r'^          python-version: "(.+)"$', job, re.M) == [oldest, newest]
    assert re.findall(r"^        id: (\w+)$", job, re.M) == ["oldest", "newest"]
    tested = '        run: ./scripts/python-wheel-check.sh dist "$OLDEST" "$NEWEST"\n'
    assert tested in job
    assert "          OLDEST: ${{ steps.oldest.outputs.python-path }}\n" in job
    assert "          NEWEST: ${{ steps.newest.outputs.python-path }}\n" in job
    # One script on every leg, Windows' among them.
    assert "    defaults:\n      run:\n        shell: bash\n" in job
    # Built, then tested, and kept only once it passed.
    built = job.index("        uses: PyO3/maturin-action@")
    kept = job.index("      - uses: actions/upload-artifact@")
    assert built < job.index(tested) < kept
    assert "          path: dist/*.whl\n" in job
    assert "          if-no-files-found: error\n" in job


def test_ci_tests_the_linux_wheel_on_postgresql_and_one_check_stands_for_the_legs(repository):
    job = workflow_job(repository, "wheel")
    # GitHub runs service containers on Linux alone: the Linux leg has a
    # server and has to test it, and the others have an empty image, which
    # starts none, and empty variables, which the tests take for unset.
    assert "          - os: ubuntu-latest\n            postgres: postgres:18\n" in job
    assert "        image: ${{ matrix.postgres }}\n" in job
    required = f"          {stores.POSTGRES_REQUIRED}: " + "${{ matrix.postgres && '1' || '' }}\n"
    assert required in job
    (served,) = re.findall(
        rf"^          {stores.POSTGRES_URL}: \$\{{\{{ matrix\.postgres && '(\S+)' \|\| '' \}}\}}$",
        job,
        re.M,
    )
    assert re.fullmatch(r"postgres://\S+@127\.0\.0\.1:5432/postgres\?sslmode=disable", served)
    # One check stands for the legs, and fails where any did not pass.
    wheels = workflow_job(repository, "wheels")
    assert "    needs: wheel\n" in wheels and "    if: always()\n" in wheels
    assert '        run: test "$RESULT" = success\n' in wheels
    assert "          RESULT: ${{ needs.wheel.result }}\n" in wheels


def test_the_wheel_check_tests_the_wheel_as_the_package_is_published(repository, package_dir):
    script = (repository / "scripts" / "python-wheel-check.sh").read_text(encoding="utf-8")
    # The tests are told nothing of the build or of the extras, whatever the
    # caller exported: a wheel without PostgreSQL fails, as the package.
    told = re.findall(
        r'^\w+ = "(QUANTFORGE_PY_TEST_\w+)"$',
        (package_dir / "tests" / "conftest.py").read_text(encoding="utf-8"),
        re.M,
    )
    (unset,) = re.findall(r"^unset ((?:.*\\\n)*.*)$", script, re.M)
    assert sorted(unset.replace("\\\n", " ").split()) == sorted(told) and len(told) == 5
    # With both extras, which come as wheels or not at all.
    assert '        "${wheel}[${extras}]"\n' in script
    assert "--only-binary numpy,pandas" in script
    # The binary is built before any run of the tests, and each is told where
    # it is; and what the caller exported of PostgreSQL reaches every one.
    built = "\ncargo build --locked\n"
    named = f"\nexport {stores.CLI}=$binary\n"
    assert built in script and named in script
    assert script.index(built) < script.index(named) < script.index('\nfor python in "$@"; do\n')
    assert stores.POSTGRES_URL in script and f"unset {stores.POSTGRES_URL}" not in script


def test_the_binary_the_tests_write_stores_with_is_of_the_engine_the_package_has():
    said = subprocess.run(
        [str(stores.binary()), "--version"], check=True, capture_output=True, text=True
    )
    assert said.stdout == f"quantforge {quantforge_py.ENGINE_VERSION}\n"


def test_a_binary_named_by_a_relative_path_is_found_from_where_the_tests_started(
    monkeypatch, tmp_path
):
    found = stores.binary()
    monkeypatch.setenv(stores.CLI, os.path.relpath(found, stores.STARTED_IN))
    # A test that moved elsewhere still writes with it.
    monkeypatch.chdir(tmp_path)
    assert stores.binary().resolve() == found.resolve()
    said = subprocess.run(
        [str(stores.binary()), "--version"], check=True, capture_output=True, text=True
    )
    assert said.stdout == f"quantforge {quantforge_py.ENGINE_VERSION}\n"


def test_the_readme_shows_only_names_the_package_publishes_and_its_example_runs(
    page, pyproject, without_numpy, without_pandas, tmp_path
):
    readme = page("quantforge-py/README.md")
    shown = re.findall(r"\bqf\.(\w+)", readme.text)
    assert shown, "the readme shows the package at work"
    assert set(shown) <= set(quantforge_py.__all__)
    # What PyPI shows is what a reader copies: every example runs as it is
    # written, one after another. The last is of the client, the only one
    # that needs a server, which the client's own tests give it
    # (`test_client_jobs.py`). The two before it are of the arrays and of the
    # frames, the only ones that need NumPy, and pandas with it, and the one
    # before them is of the candle reader, the only one that needs a store.
    *examples, of_the_reader, of_the_arrays, of_the_frames, of_the_client = readme.fences(
        "python"
    )
    assert "qf.Client(" in of_the_client
    assert not any(
        "Client" in example for example in [*examples, of_the_reader, of_the_arrays, of_the_frames]
    )
    assert examples and "to_numpy" in of_the_arrays and "to_dataframe" in of_the_frames
    assert not any("numpy" in example or "dataframe" in example for example in examples)
    assert "dataframe" not in of_the_arrays and "numpy" not in of_the_frames
    assert "qf.CandleReader()" in of_the_reader
    assert not any(
        "CandleReader" in example for example in [*examples, of_the_arrays, of_the_frames]
    )
    names = {}
    for example in examples:
        exec(compile(example, readme.name, "exec"), names)
    # The reader's example reads the store the command line wrote where it
    # runs, at its default path, of the market the examples before it named.
    closes = ["81940.01000000", "81955.25000000", "81900.00000000", "81912.50000000"]
    klines = [stores.kline(index, close) for index, close in enumerate(closes)]
    with stores.Exchange({("BTCUSDT", "1m"): klines}) as exchange:
        stores.sync(exchange, "BTCUSDT", "1m", cwd=tmp_path)
    with pytest.MonkeyPatch.context() as patched:
        patched.chdir(tmp_path)
        exec(compile(of_the_reader, readme.name, "exec"), names)
    stored = names["stored"]
    assert type(stored) is quantforge_py.CandleBatch and stored.market == names["market"]
    assert [format(close, "f") for close in stored.close] == closes
    assert f"such as {stored.close[0]!r}" in readme.text
    assert names["reader"].closed and type(names["rerun"]) is quantforge_py.BacktestResult
    assert names["rerun"] == quantforge_py.run_backtest(
        stored, names["EnterThenExit"](), names["config"], market=names["market"]
    )
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
