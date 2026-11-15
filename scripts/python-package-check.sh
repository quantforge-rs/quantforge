#!/usr/bin/env bash
# What CI's `python` job checks of the Python package (docs/ci.md,
# docs/python-package.md): the extension crate's lints, with and without
# PostgreSQL, and its oldest Rust; then the wheel maturin builds, installed
# and tested on every Python named; the SQLite-only wheel; and the in-place
# install a contributor works with.
#
#   scripts/python-package-check.sh [<python>...]
#
# Each <python> is an interpreter the one wheel is tested on, `python3` when
# none is named, and the first one builds. CI names the oldest Python the
# package supports and the newest. The script needs `rustup toolchain install
# 1.85`, takes maturin and pytest from PyPI as pyproject.toml asks for them,
# into virtualenvs it removes, and leaves the extension built in place in
# quantforge-py/python/, where git ignores it. It is written for Linux and
# macOS: a virtualenv's programs are looked for in its bin/.
set -euo pipefail

cd "$(dirname "$0")/.."

if (($# == 0)); then
    set -- python3
fi

# PyO3's build script asks a Python for its configuration: the one that builds.
export PYO3_PYTHON=$1
# Each run below says itself which build it tests.
unset QUANTFORGE_PY_TEST_SQLITE_ONLY

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cargo clippy -p quantforge-py --all-targets -- -D warnings
cargo clippy -p quantforge-py --all-targets --no-default-features -- -D warnings
cargo +1.85 check --locked -p quantforge-py

# What pyproject.toml asks for, one requirement a line: its build backend, or
# the dependency group named.
requirements() {
    "$PYO3_PYTHON" - "$1" <<'EOF'
import sys
import tomllib

with open("quantforge-py/pyproject.toml", "rb") as file:
    pyproject = tomllib.load(file)
if sys.argv[1] == "build":
    print(*pyproject["build-system"]["requires"], sep="\n")
else:
    print(*pyproject["dependency-groups"][sys.argv[1]], sep="\n")
EOF
}

# Read by plain assignments, which take the exit status of their command
# substitution: a pyproject.toml that cannot be read stops the script here.
build_requirements=$(requirements build)
test_requirements=$(requirements test)

# A virtualenv of <python> at <directory>, with <requirements> installed, one
# a line, and every <wheel> after them.
virtualenv() {
    local python=$1 directory=$2 requirements=$3
    shift 3
    "$python" -m venv "$directory"
    printf '%s\n' "$requirements" >"$directory/requirements.txt"
    "$directory/bin/python" -m pip install --quiet --disable-pip-version-check \
        --requirement "$directory/requirements.txt" ${1+"$@"}
}

virtualenv "$1" "$work/build" "$build_requirements"
(
    cd quantforge-py
    "$work/build/bin/maturin" build --interpreter "$work/build/bin/python" --out "$work/wheel"
    "$work/build/bin/maturin" build --interpreter "$work/build/bin/python" --out "$work/wheel-sqlite" \
        --no-default-features
)

# Installs the wheel in <directory> into a fresh virtualenv of <python> and
# runs the tests against it. `sqlite-only` tells them which build it is.
runs=0
tested() {
    local python=$1 directory=$2 build=${3:-}
    runs=$((runs + 1))
    virtualenv "$python" "$work/test-$runs" "$test_requirements" "$directory"/*.whl
    (
        cd quantforge-py
        if [[ $build == sqlite-only ]]; then
            QUANTFORGE_PY_TEST_SQLITE_ONLY=1 "$work/test-$runs/bin/python" -m pytest
        else
            "$work/test-$runs/bin/python" -m pytest
        fi
    )
}

# The one wheel, on every Python named: nothing is rebuilt between them.
for python in "$@"; do
    tested "$python" "$work/wheel"
done
tested "$1" "$work/wheel-sqlite" sqlite-only

# The in-place install, as docs/python-package.md gives it. maturin installs
# into the virtualenv that is active.
virtualenv "$1" "$work/develop" "$build_requirements"$'\n'"$test_requirements"
(
    cd quantforge-py
    export VIRTUAL_ENV=$work/develop PATH=$work/develop/bin:$PATH
    maturin develop
    python -c 'import quantforge_py'
    python -m pytest
)

echo "python package: the wheel on $# interpreters, the sqlite-only wheel and the in-place install passed"
