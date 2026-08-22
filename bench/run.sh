#!/usr/bin/env bash
#
# Head-to-head benchmark: zimru vs upstream kiwix zim-tools (zimcheck).
#
# Run from repo root:
#   ./bench/run.sh [path/to/file.zim]
#
# Defaults to zim-cache/wikipedia_ba_all_maxi.zim (1.1 GB Bashkir Wikipedia).
# The file is NOT in this repo; download it with:
#   curl -fLo zim-cache/wikipedia_ba_all_maxi.zim \
#     https://download.kiwix.org/zim/wikipedia/wikipedia_ba_all_maxi_2026-04.zim
#
# Requires: cargo, hyperfine, zim-tools (apt install zim-tools).
set -euo pipefail

ZIM="${1:-zim-cache/wikipedia_ba_all_maxi.zim}"
if [[ ! -f "$ZIM" ]]; then
    echo "ZIM file not found: $ZIM" >&2
    exit 1
fi
# Portable file size: GNU stat (-c%s) on Linux / coreutils-on-Mac, BSD stat
# (-f%z) on stock macOS.
file_size() {
    if command -v gstat >/dev/null 2>&1; then gstat -c%s "$1"
    elif stat -c%s "$1" >/dev/null 2>&1; then stat -c%s "$1"
    else stat -f%z "$1"
    fi
}
SIZE_BYTES=$(file_size "$ZIM")
SIZE_HUMAN=$(numfmt --to=iec --suffix=B "$SIZE_BYTES")
echo "ZIM: $ZIM ($SIZE_HUMAN)"
echo

cargo build --release --quiet
ZIMRU=./target/release/zimru
ZIMRU_CHECK=./target/release/zimcheck
ZIMRU_DUMP=./target/release/zimdump
UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
UP_CHECK="$UPSTREAM_DIR/zimcheck"
UP_DUMP="$UPSTREAM_DIR/zimdump"
OUR_BENCH=./target/release/zimbench

echo "Versions:"
echo "  zimru   : 0.1.0 (this repo)"
echo "  upstream: $($UP_CHECK --version 2>&1 | head -3 | tr '\n' ' ')"
echo

# -------------------------------------------------------------------
# 1. MD5 trailer verification
# -------------------------------------------------------------------
echo "## 1. Checksum verification (MD5 of entire file before trailer)"
hyperfine --warmup 1 --runs 5 --export-markdown bench/results-checksum.md \
    -n "zimru zimcheck -C" "$ZIMRU_CHECK -C $ZIM" \
    -n "upstream zimcheck -C" "$UP_CHECK -C $ZIM"
echo

# -------------------------------------------------------------------
# 2. Read every blob (decompress every cluster) — zimrecompress read-side
# -------------------------------------------------------------------
echo "## 2. Read every blob + MD5 each (decompress every cluster)"
hyperfine --warmup 1 --runs 3 --export-markdown bench/results-readall.md \
    -n "zimru readall --md5" "$ZIMRU readall $ZIM --md5 --quiet" \
    -n "upstream zimcheck -R" "$UP_CHECK -R $ZIM"
echo

# -------------------------------------------------------------------
# 3. Read every blob (no md5 — pure decompress throughput)
# -------------------------------------------------------------------
echo "## 3. Read every blob (no hash — pure decompress throughput)"
hyperfine --warmup 1 --runs 3 --export-markdown bench/results-decompress.md \
    -n "zimru readall" "$ZIMRU readall $ZIM --quiet"
echo

# -------------------------------------------------------------------
# 4. Full integrity sweep (-A: all checks)
# -------------------------------------------------------------------
echo "## 4. Full check sweep (-A: every check)"
hyperfine -i --warmup 1 --runs 3 --export-markdown bench/results-all.md \
    -n "zimru zimcheck -A" "$ZIMRU_CHECK -A $ZIM" \
    -n "upstream zimcheck -A" "$UP_CHECK -A $ZIM"
echo

# -------------------------------------------------------------------
# 5. Header inspection (cold call)
# -------------------------------------------------------------------
echo "## 5. zimdump info"
# --shell=none avoids shell-startup variance dominating the sub-5ms zimru run.
hyperfine --shell=none --warmup 2 --runs 10 --export-markdown bench/results-info.md \
    -n "zimru zimdump info" "$ZIMRU_DUMP info $ZIM" \
    -n "upstream zimdump info" "$UP_DUMP info $ZIM"
echo

# -------------------------------------------------------------------
# 6. Export every entry to the filesystem
# -------------------------------------------------------------------
echo "## 6. zimdump dump (export every entry)"
DUMPDIR="${TMPDIR:-/tmp}/zimru-bench-dump"
hyperfine -i --warmup 0 --runs 2 --export-markdown bench/results-dump.md \
    --prepare "rm -rf $DUMPDIR" \
    -n "zimru zimdump dump" "$ZIMRU_DUMP dump --dir=$DUMPDIR --redirect $ZIM" \
    -n "upstream zimdump dump" "$UP_DUMP dump --dir=$DUMPDIR --redirect $ZIM"
rm -rf "$DUMPDIR"
echo

# -------------------------------------------------------------------
# 7. Random-access benchmark
# -------------------------------------------------------------------
echo "## 7. zimbench -n 1000 (random + sequential entry access)"
hyperfine -i --warmup 1 --runs 3 --export-markdown bench/results-bench.md \
    -n "zimru zimbench" "$OUR_BENCH -n 1000 $ZIM" \
    -n "upstream zimbench" "$UPSTREAM_DIR/zimbench -n 1000 $ZIM"
echo

echo "All benchmarks complete. Markdown summaries in bench/results-*.md"
