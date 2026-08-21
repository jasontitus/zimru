#!/usr/bin/env bash
#
# Head-to-head ZIM *creation* benchmark: zimru's `zimrecreate` (spawning the
# `xapianbuilder` helper for the search indexes) against upstream
# `zimrecreate` from kiwix zim-tools 3.8.0 / libzim 9.8.2 (which links Xapian
# in-process).
#
# Two configurations per file, so the indexer cost can be isolated by
# subtraction:
#
#   index     both tools build X/fulltext/xapian + X/title/xapian
#   noindex   both tools pass -j / --withoutFTIndex
#
# Fairness notes:
#   * Upstream's zimrecreate exposes no compression knobs — it uses libzim's
#     default (zstd, level 19). zimru is therefore driven at
#     `--compression zstd --compression-level 19` so both sides do the same
#     amount of entropy coding. Comparing against zimru's own default
#     (zstd 3) measures preset choice, not implementation.
#   * Both sides get the same worker count via -J.
#   * Timing is wall-clock of the best of $RUNS runs, page cache warm (the
#     source is read once before timing starts).
#
# Every output is validated, not just timed: `upstream zimcheck -A` must not
# regress relative to the source, and when indexes were requested both
# X/ entries must be present and answer `upstream zimsearch` with the same
# top hit as the source archive.
#
# Usage: ./bench/creation-bench.sh [zim ...]
#   env: UPSTREAM_DIR RUNS THREADS OUT XAPIANBUILDER LEVEL
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
UP_RECREATE="$UPSTREAM_DIR/zimrecreate"
UP_CHECK="$UPSTREAM_DIR/zimcheck"
UP_SEARCH="$UPSTREAM_DIR/zimsearch"
ZIMRU_RECREATE="${ZIMRU_RECREATE:-./target/release/zimrecreate}"
ZIMRU_DUMP="${ZIMRU_DUMP:-./target/release/zimdump}"
export XAPIANBUILDER="${XAPIANBUILDER:-/home/user/xapianbuilder/target/release/xapianbuilder}"
RUNS="${RUNS:-1}"
THREADS="${THREADS:-$(nproc)}"
LEVEL="${LEVEL:-19}"
OUT="${OUT:-/tmp/zbench}"
mkdir -p "$OUT"

[[ -x "$UP_RECREATE" ]] || { echo "upstream zimrecreate not found at $UP_RECREATE" >&2; exit 1; }
[[ -x "$XAPIANBUILDER" ]] || echo "warning: xapianbuilder not executable at $XAPIANBUILDER — zimru will build no index" >&2

human() { numfmt --to=iec --suffix=B "$1"; }

# Best-of-RUNS wall clock. Echoes "<seconds> <output-bytes>", or "FAIL -" on
# a non-zero exit.
timeit() {
    local outfile="$1"; shift
    local best="" t start end rc
    for _ in $(seq 1 "$RUNS"); do
        rm -f "$outfile"
        start=$(date +%s.%N)
        "$@" >/dev/null 2>&1; rc=$?
        end=$(date +%s.%N)
        if [[ $rc -ne 0 ]]; then echo "FAIL -"; return 1; fi
        t=$(awk "BEGIN{printf \"%.2f\", $end - $start}")
        if [[ -z "$best" ]] || awk "BEGIN{exit !($t < $best)}"; then best=$t; fi
    done
    echo "$best $(stat -c%s "$outfile" 2>/dev/null || echo 0)"
}

# "PASS" / "=src" / "REGRESS": does the output survive upstream's full sweep,
# and if not, did the source already fail it?
zimcheck_verdict() {
    local out="$1" src="$2"
    if $UP_CHECK -A "$out" 2>/dev/null | grep -q "Overall Test Status: Pass"; then
        echo PASS
    elif $UP_CHECK -A "$src" 2>/dev/null | grep -q "Overall Test Status: Pass"; then
        echo REGRESS
    else
        echo "=src"
    fi
}

# "OK" when both index entries exist and zimsearch's top hit matches the
# source archive's; "NOIDX" / "MISMATCH" / "NOQUERY" otherwise.
index_verdict() {
    local out="$1" src="$2" query="$3"
    local have
    have=$($ZIMRU_DUMP list --ns=X "$out" 2>/dev/null | grep -c 'xapian$')
    [[ "$have" == "2" ]] || { echo "NOIDX($have/2)"; return; }
    [[ -n "$query" ]] || { echo "NOQUERY"; return; }
    local a b
    a=$($UP_SEARCH "$src" "$query" 2>/dev/null | grep -m1 '^score' | cut -f3-)
    b=$($UP_SEARCH "$out" "$query" 2>/dev/null | grep -m1 '^score' | cut -f3-)
    if [[ -z "$a$b" ]]; then echo "NOHITS"
    elif [[ "$a" == "$b" ]]; then echo OK
    else echo "MISMATCH"; fi
}

# A query term guaranteed to be in the archive: the title of the first
# content article long enough to be indexed.
pick_query() {
    $ZIMRU_DUMP list "$1" 2>/dev/null | awk 'length($0) >= 4 { print; exit }'
}

printf "%-40s %9s %6s %10s %10s %8s %12s %12s %8s %8s\n" \
    FILE SIZE MODE ZIMRU UPSTREAM SPEEDUP ZIMRU-OUT UP-OUT CHECK INDEX
printf -- "%s\n" "$(printf '%.0s-' {1..135})"

for src in "$@"; do
    [[ -f "$src" ]] || { echo "skip (missing): $src" >&2; continue; }
    name=$(basename "$src" .zim)
    size=$(stat -c%s "$src")
    query=$(pick_query "$src")
    cat "$src" > /dev/null 2>&1   # warm the page cache for both sides alike

    for mode in index noindex; do
        zr_out="$OUT/$name.zr.$mode.zim"
        up_out="$OUT/$name.up.$mode.zim"
        if [[ $mode == index ]]; then
            zr_extra=(); up_extra=()
        else
            zr_extra=(-j); up_extra=(-j)
        fi

        read -r zr_t zr_sz < <(timeit "$zr_out" \
            "$ZIMRU_RECREATE" "$src" "$zr_out" --compression zstd \
            --compression-level "$LEVEL" -J "$THREADS" "${zr_extra[@]}")
        read -r up_t up_sz < <(timeit "$up_out" \
            "$UP_RECREATE" "$src" "$up_out" -J "$THREADS" "${up_extra[@]}")

        if [[ "$zr_t" == FAIL || "$up_t" == FAIL ]]; then
            speedup="-"
        else
            speedup=$(awk "BEGIN{printf \"%.2fx\", $up_t / $zr_t}")
        fi
        if [[ "$zr_t" == FAIL ]]; then
            chk="-"; idx="-"
        else
            chk=$(zimcheck_verdict "$zr_out" "$src")
            if [[ $mode == index ]]; then idx=$(index_verdict "$zr_out" "$src" "$query"); else idx="n/a"; fi
        fi
        printf "%-40s %9s %6s %10s %10s %8s %12s %12s %8s %8s\n" \
            "$name" "$(human "$size")" "$mode" \
            "${zr_t}s" "${up_t}s" "$speedup" \
            "$([[ "$zr_sz" == - ]] && echo - || human "$zr_sz")" \
            "$([[ "$up_sz" == - ]] && echo - || human "$up_sz")" \
            "$chk" "$idx"
    done
done
