#!/usr/bin/env bash
# What CI's `python` job checks of the Python package (docs/ci.md,
# docs/python-package.md): the extension crate's lints, with and without
# PostgreSQL, and its oldest Rust; then the wheel maturin builds, installed
# with its `numpy` extra and tested on every Python named, once more on the
# oldest NumPy the extra takes, and once without the extra, where there is
# no NumPy; the SQLite-only wheel; and the in-place install a contributor
# works with.
#
#   scripts/python-package-check.sh [<python>...]
#
# Each <python> is an interpreter the one wheel is tested on, `python3` when
# none is named, and the first one builds. CI names the oldest Python the
# package supports and the newest. The oldest NumPy has a wheel for the
# oldest Python and for none of the newest, so it is tested where the first
# <python> is that oldest one, as in CI, and the script says so where it is
# not. The script needs `rustup toolchain install 1.85`, takes maturin,
# pytest and NumPy from PyPI as pyproject.toml asks for them, into
# virtualenvs it removes, and leaves the extension built in place in
# quantforge-py/python/, where git ignores it. It is written for Linux and
# macOS: a virtualenv's programs are looked for in its bin/.
set -euo pipefail

cd "$(dirname "$0")/.."

if (($# == 0)); then
    set -- python3
fi

# PyO3's build script asks a Python for its configuration: the one that builds.
export PYO3_PYTHON=$1
# Each run below says itself which build it tests, and on which NumPy or
# on none.
unset QUANTFORGE_PY_TEST_SQLITE_ONLY QUANTFORGE_PY_TEST_OLDEST_NUMPY \
    QUANTFORGE_PY_TEST_WITHOUT_NUMPY

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo clippy -p quantforge-py --all-targets -- -D warnings
cargo clippy -p quantforge-py --all-targets --no-default-features -- -D warnings
cargo +1.85 check --locked -p quantforge-py

# What pyproject.toml asks for, one requirement a line: its build backend,
# what the package's extras require (`extras-require`), or the dependency
# group named. `extras` is the names of the extras, as `pip` takes them
# between brackets. `oldest-python` is the oldest Python the package
# supports, written as `3.11`, and `python` the version of the Python that
# builds, written the same way.
requirements() {
    "$PYO3_PYTHON" - "$1" <<'EOF'
import sys
import tomllib

with open("quantforge-py/pyproject.toml", "rb") as file:
    pyproject = tomllib.load(file)
if sys.argv[1] == "build":
    print(*pyproject["build-system"]["requires"], sep="\n")
elif sys.argv[1] == "extras":
    print(",".join(pyproject["project"]["optional-dependencies"]))
elif sys.argv[1] == "extras-require":
    for required in pyproject["project"]["optional-dependencies"].values():
        print(*required, sep="\n")
elif sys.argv[1] == "oldest-python":
    print(pyproject["project"]["requires-python"].removeprefix(">="))
elif sys.argv[1] == "python":
    print("%d.%d" % sys.version_info[:2])
else:
    print(*pyproject["dependency-groups"][sys.argv[1]], sep="\n")
EOF
}

# Read by plain assignments, which take the exit status of their command
# substitution: a pyproject.toml that cannot be read stops the script here.
build_requirements=$(requirements build)
extras=$(requirements extras)
extras_requirements=$(requirements extras-require)
test_requirements=$(requirements test)
oldest_numpy=$(requirements oldest-numpy)
oldest_python=$(requirements oldest-python)
first_python=$(requirements python)

# A virtualenv of <python> at <directory>, with <requirements> installed, one
# a line, and every <wheel> after them.
virtualenv() {
    local python=$1 directory=$2 requirements=$3
    shift 3
    "$python" -m venv "$directory"
    printf '%s\n' "$requirements" >"$directory/requirements.txt"
    # NumPy comes as a wheel or not at all: it is never compiled here.
    "$directory/bin/python" -m pip install --quiet --disable-pip-version-check \
        --only-binary numpy --requirement "$directory/requirements.txt" ${1+"$@"}
}

virtualenv "$1" "$work/build" "$build_requirements"
(
    cd quantforge-py
    "$work/build/bin/maturin" build --interpreter "$work/build/bin/python" --out "$work/wheel"
    "$work/build/bin/maturin" build --interpreter "$work/build/bin/python" --out "$work/wheel-sqlite" \
        --no-default-features
)

# Installs the wheel in <directory> into a fresh virtualenv of <python>, with
# its extras, and runs the tests against it, with the newest NumPy pip finds
# for that Python. What is particular to a run is named, and the tests are
# told: `sqlite-only` is the build without PostgreSQL, `oldest-numpy` installs
# the oldest NumPy the extra takes, which pyproject.toml pins, in place of
# the newest, and `without-numpy` installs the wheel without its extras, as
# `pip install quantforge-py` does, so that there is no NumPy.
runs=0
tested() {
    local python=$1 directory=$2 particular=${3:-}
    local requirements=$test_requirements wheel
    runs=$((runs + 1))
    # The one wheel of the directory.
    for wheel in "$directory"/*.whl; do :; done
    case $particular in
    oldest-numpy)
        requirements+=$'\n'$oldest_numpy
        ;;
    esac
    if [[ $particular == without-numpy ]]; then
        virtualenv "$python" "$work/test-$runs" "$requirements" "$wheel"
    else
        virtualenv "$python" "$work/test-$runs" "$requirements" "${wheel}[${extras}]"
    fi
    (
        cd quantforge-py
        case $particular in
        sqlite-only)
            QUANTFORGE_PY_TEST_SQLITE_ONLY=1 "$work/test-$runs/bin/python" -m pytest
            ;;
        oldest-numpy)
            QUANTFORGE_PY_TEST_OLDEST_NUMPY=1 "$work/test-$runs/bin/python" -m pytest
            ;;
        without-numpy)
            QUANTFORGE_PY_TEST_WITHOUT_NUMPY=1 "$work/test-$runs/bin/python" -m pytest
            ;;
        *)
            "$work/test-$runs/bin/python" -m pytest
            ;;
        esac
    )
}

# The one wheel, on every Python named: nothing is rebuilt between them.
for python in "$@"; do
    tested "$python" "$work/wheel"
done
# And on the oldest NumPy, where the Python is the one it has a wheel for.
if [[ $first_python == "$oldest_python" ]]; then
    tested "$1" "$work/wheel" oldest-numpy
    numpy="and on the oldest numpy"
else
    numpy="the oldest numpy not tested: $1 is python $first_python, not $oldest_python"
fi
# And without its extras: the package alone, where there is no NumPy.
tested "$1" "$work/wheel" without-numpy
tested "$1" "$work/wheel-sqlite" sqlite-only

# The in-place install, as docs/python-package.md gives it. maturin installs
# into the virtualenv that is active, and what the extras require with it:
# that is put there first, by the same pip as everywhere else here, so that
# NumPy is a wheel in this virtualenv too.
virtualenv "$1" "$work/develop" \
    "$build_requirements"$'\n'"$test_requirements"$'\n'"$extras_requirements"
(
    cd quantforge-py
    export VIRTUAL_ENV=$work/develop PATH=$work/develop/bin:$PATH
    maturin develop --extras "$extras"
    python -c 'import quantforge_py'
    python -m pytest
)

echo "python package: the wheel on $# interpreters ($numpy), the wheel without numpy, the sqlite-only wheel and the in-place install passed"
