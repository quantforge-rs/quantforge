#!/usr/bin/env bash
# What CI's `wheel` legs check of the wheel each builds (docs/ci.md,
# docs/python-package.md): the one wheel in the directory given, as
# `maturin build --release` leaves it, installed with its extras, NumPy and
# pandas, into a fresh virtualenv of every Python named, and the package's
# whole suite run against it in each, the golden parity test among it.
#
# The wheel is held to the package as it is published, with both stores: no
# run is told that it is the SQLite-only build, so a wheel built without
# PostgreSQL fails the tests instead of passing for that build. The candle
# reader's tests read stores the `quantforge` command line wrote, so the
# script builds the binary first and tells the tests where it is. They write
# a SQLite file each, and a database of its own on the PostgreSQL server
# QUANTFORGE_TEST_POSTGRES_URL names, the variable the crate's tests read;
# where it names none those skip, unless QUANTFORGE_TEST_POSTGRES_REQUIRED
# is 1, as on CI's Linux leg, where they fail instead. CI's macOS and
# Windows legs have no server: there the package is imported and tested
# with no database at all.
#
#   scripts/python-wheel-check.sh <directory> [<python>...]
#
# <directory> holds the one wheel to test: a directory with no wheel, or
# with more than one, is refused, so that no wheel another build left there
# is taken for the one built. Each <python> is an interpreter the wheel is
# tested on, by name or by path, `python3` when none is named; CI names the
# oldest Python the package supports and the newest, each by its path. The
# script takes pytest, NumPy and pandas from PyPI as pyproject.toml and the
# wheel ask for them, into virtualenvs it removes, and ends by naming the
# wheel it tested and its SHA-256. It runs on Linux and macOS, and on
# Windows in Git Bash, where a virtualenv's programs are in its Scripts/ and
# the binary is quantforge.exe.
set -euo pipefail

if (($# == 0)); then
    echo "usage: scripts/python-wheel-check.sh <directory> [<python>...]" >&2
    exit 2
fi

# A path as the programs the script runs take it. Git Bash on Windows
# writes its own, such as /d/a/..., which no Windows program reads, and
# `cygpath` there gives the one Windows writes; elsewhere a path is itself.
native() {
    if command -v cygpath >/dev/null; then
        cygpath --mixed "$1"
    else
        printf '%s\n' "$1"
    fi
}

# Read before the script moves to the repository's root: the directory is
# named from where the script was run. Without CDPATH, which would have `cd`
# print where it went beside what `pwd` prints.
directory=$(CDPATH='' cd -- "$1" && pwd)
shift
if (($# == 0)); then
    set -- python3
fi

cd "$(dirname "$0")/.."

count=0
for found in "$directory"/*.whl; do
    if [[ -f $found ]]; then
        count=$((count + 1))
        wheel=$found
    fi
done
if ((count != 1)); then
    echo "error: $directory holds $count wheels, and the check tests one: build into a directory of its own" >&2
    exit 1
fi
wheel=$(native "$wheel")
first=$(native "$1")

# The tests are told nothing of the build or of the extras: the wheel is the
# package as it is published, installed with both extras, on the newest
# NumPy and the newest pandas pip finds for each Python.
unset QUANTFORGE_PY_TEST_SQLITE_ONLY QUANTFORGE_PY_TEST_OLDEST_NUMPY \
    QUANTFORGE_PY_TEST_WITHOUT_NUMPY QUANTFORGE_PY_TEST_OLDEST_PANDAS \
    QUANTFORGE_PY_TEST_WITHOUT_PANDAS

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
work=$(native "$scratch")

# The command line itself, which writes the stores the reader's tests read,
# found where cargo put it: CARGO_TARGET_DIR, or cargo's own configuration,
# can move what it builds out of ./target. Written with no line's end, which
# Python on Windows would end with a carriage return.
cargo build --locked
target=$(cargo metadata --format-version 1 --no-deps |
    "$first" -c 'import json, sys; sys.stdout.write(json.load(sys.stdin)["target_directory"])')
binary=$(native "$target")/debug/quantforge
if [[ -f $binary.exe ]]; then
    binary=$binary.exe
fi
export QUANTFORGE_PY_TEST_CLI=$binary
if [[ -n ${QUANTFORGE_TEST_POSTGRES_URL:-} ]]; then
    stores="the reader on sqlite and on postgresql"
else
    stores="the reader on sqlite, and not on postgresql: QUANTFORGE_TEST_POSTGRES_URL names no server"
fi

# What pyproject.toml asks for: `extras`, the names of the extras, as `pip`
# takes them between brackets, or the requirements of the dependency group
# named, one a line. Python on Windows ends each line with a carriage
# return, which is taken out.
asked() {
    local python=$1 what=$2
    "$python" - "$what" <<'EOF' | tr -d '\r'
import sys
import tomllib

with open("quantforge-py/pyproject.toml", "rb") as file:
    pyproject = tomllib.load(file)
if sys.argv[1] == "extras":
    print(",".join(pyproject["project"]["optional-dependencies"]))
else:
    print(*pyproject["dependency-groups"][sys.argv[1]], sep="\n")
EOF
}

# Read by plain assignments, which take the exit status of their command
# substitution: a pyproject.toml that cannot be read stops the script here.
extras=$(asked "$first" extras)
test_requirements=$(asked "$first" test)

# Installs the wheel into a fresh virtualenv of <python> with its extras, as
# `pip install "quantforge-py[numpy,pandas]"` installs it, and pytest beside
# it, then runs the tests against it. NumPy and pandas come as wheels or not
# at all: neither is ever compiled here.
runs=0
tested() {
    local python=$1 environment interpreter
    runs=$((runs + 1))
    environment=$work/test-$runs
    "$python" -m venv "$environment"
    interpreter=$environment/bin/python
    if [[ -f $environment/Scripts/python.exe ]]; then
        interpreter=$environment/Scripts/python.exe
    fi
    printf '%s\n' "$test_requirements" >"$environment/requirements.txt"
    "$interpreter" -m pip install --quiet --disable-pip-version-check \
        --only-binary numpy,pandas --requirement "$environment/requirements.txt" \
        "${wheel}[${extras}]"
    (
        cd quantforge-py
        "$interpreter" -m pytest
    )
}

# The one wheel, on every Python named: nothing is rebuilt between them.
for python in "$@"; do
    tested "$(native "$python")"
done

# The SHA-256 a record of the run names the file by (docs/parity/README.md).
digest=$("$first" -c 'import hashlib, pathlib, sys; sys.stdout.write(
    hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())' "$wheel")
echo "python wheel: ${wheel##*/} (sha256:$digest) passed on $# interpreters, $stores"
