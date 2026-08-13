#!/usr/bin/env bash
#
# Creation benchmark: time zimru's `zimrecreate` against the upstream
# `zimrecreate` from kiwix zim-tools 3.6.0. Both tools take a source ZIM
# and produce a new ZIM with re-encoded clusters; this is the cleanest
# apples-to-apples writer benchmark we can run without upstream's full
# zimwriterfs+filesystem pipeline.
#
# To keep the comparison fair we pass `--withoutFTIndex` (-j) to upstream
# (we don't have a Xapian fulltext indexer yet either) and `--threads 1`
# to match our single-threaded writer. We use zstd compression in both
# cases.
#
# Usage: ./bench/recreate-bench.sh [glob]
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0}"
UP_RECREATE="$UPSTREAM_DIR/zimrecreate"
UP_CHECK="$UPSTREAM_DIR/zimcheck"
ZIMRU_RECREATE=./target/release/zimrecreate
GLOB="${1:-zim-cache/*.zim}"

cargo build --release --quiet || { echo "cargo build failed — aborting (stale/missing binaries would misreport as DIFF/FAIL rows)" >&2; exit 1; }

shopt -s nullglob
mapfile -t FILES < <(ls -S $GLOB 2>/dev/null)
if [[ ${#FILES[@]} -eq 0 ]]; then echo "no ZIMs match $GLOB" >&2; exit 1; fi

# Header
printf "%-44s %10s %12s %12s %12s %12s %8s %8s\n" \
    "FILE" "SIZE" "ZIMRU" "UPSTREAM" "ZIMRU/OUT" "UP/OUT" "ZIMRU" "UP"
printf "%-44s %10s %12s %12s %12s %12s %8s %8s\n" \
    " " " " "TIME" "TIME" "SIZE" "SIZE" "ZIMCHK" "ZIMCHK"
printf -- "-------------------------------------------------------------------------------------------------------------------------\n"

for src in "${FILES[@]}"; do
    name=$(basename "$src")
    size=$(stat -c%s "$src")
    size_h=$(numfmt --to=iec --suffix=B "$size")

    out_zr=/tmp/recreate-zr-$name
    out_up=/tmp/recreate-up-$name
    rm -f "$out_zr" "$out_up"

    # zimru
    t0=$(date +%s.%N)
    if $ZIMRU_RECREATE "$src" "$out_zr" --compression zstd > /dev/null 2>&1; then
        t1=$(date +%s.%N)
        zr_time=$(awk "BEGIN { printf \"%.2f\", $t1 - $t0 }")
        zr_size=$(numfmt --to=iec --suffix=B $(stat -c%s "$out_zr"))
        if $UP_CHECK -A "$out_zr" 2>&1 | grep -q "Overall Test Status: Pass"; then
            zr_zc="PASS"
        else
            # Compare against source — if source also fails, that's not our fault
            if $UP_CHECK -A "$src" 2>&1 | grep -q "Overall Test Status: Pass"; then
                zr_zc="REGRESS"
            else
                zr_zc="=src"
            fi
        fi
    else
        zr_time="FAIL"; zr_size="-"; zr_zc="-"
    fi

    # upstream
    t0=$(date +%s.%N)
    if $UP_RECREATE "$src" "$out_up" --withoutFTIndex --threads 1 > /dev/null 2>&1; then
        t1=$(date +%s.%N)
        up_time=$(awk "BEGIN { printf \"%.2f\", $t1 - $t0 }")
        up_size=$(numfmt --to=iec --suffix=B $(stat -c%s "$out_up"))
        if $UP_CHECK -A "$out_up" 2>&1 | grep -q "Overall Test Status: Pass"; then
            up_zc="PASS"
        else
            if $UP_CHECK -A "$src" 2>&1 | grep -q "Overall Test Status: Pass"; then
                up_zc="REGRESS"
            else
                up_zc="=src"
            fi
        fi
    else
        up_time="FAIL"; up_size="-"; up_zc="-"
    fi

    printf "%-44s %10s %12ss %12ss %12s %12s %8s %8s\n" \
        "$name" "$size_h" "$zr_time" "$up_time" "$zr_size" "$up_size" "$zr_zc" "$up_zc"

    rm -f "$out_zr" "$out_up"
done
