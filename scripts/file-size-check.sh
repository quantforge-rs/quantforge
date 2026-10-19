#!/usr/bin/env bash
# Enforces the file-size budget on src/**/*.rs: a file with more than
# WARN_LINES lines warns, and one with more than MAX_LINES fails unless
# scripts/file-size-allowlist.txt grants it a higher, justified ceiling. Lines
# are counted raw, in-file tests included, and a local run also scans untracked
# files. CI and scripts/release-check.sh run it; see CONTRIBUTING.md.
set -euo pipefail

cd "$(dirname "$0")/.."

readonly WARN_LINES=400
readonly MAX_LINES=600
readonly ALLOWLIST=scripts/file-size-allowlist.txt

errors=0
warnings=0

# Prints one finding; on GitHub Actions it becomes an annotation on the file.
# The counters must stay in this shell: findings reported from a pipeline
# subshell, as in `find | while read`, would be lost and the check would pass.
report() {
    local level=$1 file=$2 message=$3
    if [[ ${GITHUB_ACTIONS:-} == true ]]; then
        echo "::$level file=$file,title=file size budget::$file: $message"
    else
        echo "$level: $file: $message" >&2
    fi
    if [[ $level == error ]]; then
        errors=$((errors + 1))
    else
        warnings=$((warnings + 1))
    fi
}

# Both inputs are read by plain assignments, which take the exit status of
# their command substitution, so a failing find or a missing allowlist stops
# the script here. A loop reading `< <(find ...)` never sees find fail, and
# bash before 5.2 carries on past a failed `done < file`.
files=$(find src -type f -name '*.rs' | LC_ALL=C sort)
entries=$(< "$ALLOWLIST")

# Parallel arrays and index loops: macOS ships bash 3.2, which has no
# associative arrays and treats "${array[@]}" of an empty array as unbound
# under set -u.
allowed_paths=()
allowed_ceilings=()
allowed_matched=()

allowed_index() {
    local i
    for ((i = 0; i < ${#allowed_paths[@]}; i++)); do
        if [[ ${allowed_paths[i]} == "$1" ]]; then
            echo "$i"
            return
        fi
    done
    echo -1
}

# Both loops read through printf without a trailing newline, so an empty input
# gives no iterations and `|| [[ -n ... ]]` still takes the last line. The
# ceiling must be digits only before any arithmetic touches it: bash reads 0650
# as octal and evaluates any other text as an expression.
whole='^[1-9][0-9]*$'
number=0
while read -r path ceiling reason || [[ -n $path ]]; do
    number=$((number + 1))
    if [[ -z $path || $path == '#'* ]]; then
        continue
    fi
    if [[ $path != src/*.rs ]]; then
        report error "$ALLOWLIST" "line $number: $path is not a src/**/*.rs path; fix or remove the entry"
    elif ! [[ $ceiling =~ $whole ]] || ((ceiling <= MAX_LINES)); then
        report error "$ALLOWLIST" "line $number: the ceiling for $path must be a whole number above $MAX_LINES"
    elif [[ -z $reason ]]; then
        report error "$ALLOWLIST" "line $number: $path needs a justification after its ceiling"
    elif (($(allowed_index "$path") >= 0)); then
        report error "$ALLOWLIST" "line $number: $path is listed twice; keep one entry"
    else
        allowed_paths+=("$path")
        allowed_ceilings+=("$ceiling")
        allowed_matched+=(0)
    fi
done < <(printf '%s' "$entries")

checked=0
while IFS= read -r file || [[ -n $file ]]; do
    checked=$((checked + 1))
    lines=$(awk 'END { print NR }' "$file")
    index=$(allowed_index "$file")
    if ((index < 0)); then
        if ((lines > MAX_LINES)); then
            report error "$file" "$lines lines, over the $MAX_LINES-line limit; split the module, or add a justified entry to $ALLOWLIST"
        elif ((lines > WARN_LINES)); then
            report warning "$file" "$lines lines, over the $WARN_LINES-line warning threshold; consider splitting it before it exceeds $MAX_LINES"
        fi
    else
        allowed_matched[index]=1
        ceiling=${allowed_ceilings[index]}
        if ((lines > ceiling)); then
            report error "$file" "$lines lines, over its allowlisted ceiling of $ceiling; split the module, or raise the ceiling in $ALLOWLIST with a justification"
        elif ((lines <= MAX_LINES)); then
            report error "$file" "$lines lines, back within the $MAX_LINES-line limit; remove its entry from $ALLOWLIST"
        else
            report warning "$file" "$lines lines, allowed up to $ceiling by $ALLOWLIST"
        fi
    fi
done < <(printf '%s' "$files")

# Guards against passing on a scan that checked nothing.
if ((checked == 0)); then
    echo "error: no src/**/*.rs files found under $PWD" >&2
    exit 1
fi

for ((i = 0; i < ${#allowed_paths[@]}; i++)); do
    if ((allowed_matched[i] == 0)); then
        report error "$ALLOWLIST" "${allowed_paths[i]} matches no file under src/; remove its entry"
    fi
done

echo "file size budget: $checked files checked, $warnings warnings, $errors errors"
if ((errors > 0)); then
    exit 1
fi
