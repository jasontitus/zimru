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
SIZE_BYTES=$(stat -c%s "$ZIM")
SIZE_HUMAN=$(numfmt --to=iec --suffix=B "$SIZE_BYTES")
echo "ZIM: $ZIM ($SIZE_HUMAN)"
echo

cargo build --release --quiet
ZIMRU=./target/release/zimru
ZIMRU_CHECK=./target/release/zimcheck
ZIMRU_DUMP=./target/release/zimdump
UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0}"
UP_CHECK="$UPSTREAM_DIR/zimcheck"
UP_DUMP="$UPSTREAM_DIR/zimdump"

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
hyperfine --warmup 2 --runs 10 --export-markdown bench/results-info.md \
    -n "zimru zimdump info" "$ZIMRU_DUMP info $ZIM" \
    -n "upstream zimdump info" "$UP_DUMP info $ZIM"
echo

echo "All benchmarks complete. Markdown summaries in bench/results-*.md"
