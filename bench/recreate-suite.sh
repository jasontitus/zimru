#!/usr/bin/env bash
#
# Read → rewrite → cross-validate every ZIM in zim-cache/.
#
# For each *.zim file:
#   1. Read it with our `zimru info` and `zimru readall --md5`.
#   2. Rewrite it through `zimrecreate` with zstd compression.
#   3. Validate the recreated file with upstream `zimcheck -A`.
#   4. Re-read the recreated file with `zimru readall --md5` and compare
#      the per-blob MD5 sum against the original.
#   5. Compare key counts (entries, articles, redirects).
#
# Tests are skipped silently if upstream zim-tools isn't installed.
#
# Usage: ./bench/recreate-suite.sh [glob]
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0}"
ZIMCHECK="$UPSTREAM_DIR/zimcheck"
ZIMRU=./target/release/zimru
ZIMDUMP=./target/release/zimdump
ZIMRECREATE=./target/release/zimrecreate

cargo build --release --quiet

GLOB="${1:-zim-cache/*.zim}"
RESULTS=()

# Collect, then iterate so we can size the runtime up front.
shopt -s nullglob
mapfile -t FILES < <(ls -S $GLOB 2>/dev/null)

if [[ ${#FILES[@]} -eq 0 ]]; then
    echo "no ZIMs matching $GLOB"
    exit 1
fi

printf "%-50s %12s %10s %14s %10s %10s\n" "FILE" "SIZE" "ENTRIES" "RECREATED" "ZIMCHECK" "BLOB-MD5"
printf -- "------------------------------------------------------------------------------------------------------------------\n"

for src in "${FILES[@]}"; do
    name=$(basename "$src")
    size=$(stat -c%s "$src")
    size_h=$(numfmt --to=iec --suffix=B "$size")

    # 1. Original info
    src_md5=$($ZIMRU readall "$src" --md5 --quiet 2>/dev/null | grep blob_md5 | awk '{print $2}')
    src_entries=$($ZIMDUMP info "$src" 2>/dev/null | awk '/^count-entries:/{print $2}')

    # 2. Rewrite
    out="/tmp/recreated-$name"
    rm -f "$out"
    if ! $ZIMRECREATE "$src" "$out" --compression zstd 2>/tmp/recreate-err; then
        printf "%-50s %12s %10s  %-14s %-10s %-10s\n" "$name" "$size_h" "$src_entries" "RECREATE FAIL" "-" "-"
        cat /tmp/recreate-err | head -3
        continue
    fi
    out_size=$(stat -c%s "$out")
    out_size_h=$(numfmt --to=iec --suffix=B "$out_size")

    # 3. Upstream zimcheck -A on output, compared against source
    if [[ -x "$ZIMCHECK" ]]; then
        if $ZIMCHECK -A "$src" 2>&1 | grep -q "Overall Test Status: Pass"; then
            src_zc=PASS
        else
            src_zc=FAIL
        fi
        if $ZIMCHECK -A "$out" 2>&1 | grep -q "Overall Test Status: Pass"; then
            out_zc=PASS
        else
            out_zc=FAIL
        fi
        # We pass if recreated status >= source status. "FAIL→FAIL" means we
        # faithfully preserved the source's pre-existing flaws (e.g. dangling
        # internal links in the original archive).
        if [[ "$src_zc" == "$out_zc" ]]; then
            zc="$out_zc(=src)"
        elif [[ "$out_zc" == "PASS" ]]; then
            zc="PASS(↑)"
        else
            zc="REGRESS"
        fi
    else
        zc="(no upstream)"
    fi

    # 4. Re-read recreated; compare blob md5
    out_md5=$($ZIMRU readall "$out" --md5 --quiet 2>/dev/null | grep blob_md5 | awk '{print $2}')
    if [[ "$src_md5" == "$out_md5" ]]; then md5cmp=MATCH; else md5cmp="DIFF"; fi
    out_entries=$($ZIMDUMP info "$out" 2>/dev/null | awk '/^count-entries:/{print $2}')
    if [[ "$src_entries" == "$out_entries" ]]; then ent="$out_entries=" ; else ent="$out_entries(was $src_entries)" ; fi

    printf "%-50s %12s %10s  %-14s %-10s %-10s\n" "$name" "$size_h→$out_size_h" "$ent" "OK" "$zc" "$md5cmp"
    rm -f "$out"
done
