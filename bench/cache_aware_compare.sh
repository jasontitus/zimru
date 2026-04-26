#!/usr/bin/env bash
#
# Cache-aware head-to-head comparison: real libzim (system) vs
# libzim-shim → zimru, on identical reader and writer workloads.
#
# The OS page cache is pre-warmed before each timed iteration via
# hyperfine's --prepare hook (`cat <files> > /dev/null`), so all
# timed runs start from the same warm-cache state. Without this,
# the first run pays disk I/O the second run skips, which is what
# made earlier round-trip benches noisy. We aren't running with
# sudo (so `purge` to drop the page cache isn't available); the
# warm-equally-before-each-run approach is the next-best fair
# comparison.
#
# Usage:
#   bench/cache_aware_compare.sh <zim>           # reader workloads
#   bench/cache_aware_compare.sh <zim> <site>    # add writer (zimwriterfs)
#   bench/cache_aware_compare.sh <zim> <site> <icon.png>
#
# Requires:
#   - hyperfine on PATH
#   - /opt/homebrew/bin/{zimcheck,zimdump,zimwriterfs} (real libzim)
#   - $LIBZIM_SHIM_BUILD/libzim.9.0.0.dylib  (shim build)
#   - zimru binaries in this repo (target/release/{zimcheck,zimdump,zimwriterfs})
set -euo pipefail

ZIM="${1:-}"
SITE="${2:-}"
ICON="${3:-}"

if [[ -z "$ZIM" || ! -f "$ZIM" ]]; then
  echo "usage: $0 <zim> [site-dir [icon.png]]" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHIM_BUILD="${LIBZIM_SHIM_BUILD:-${ROOT}/../libzim-shim/build}"
REAL_BIN_DIR="${REAL_BIN_DIR:-/opt/homebrew/bin}"

if [[ ! -f "$SHIM_BUILD/libzim.9.0.0.dylib" ]]; then
  echo "missing $SHIM_BUILD/libzim.9.0.0.dylib — set LIBZIM_SHIM_BUILD or build the shim first" >&2
  exit 2
fi

# Build zimru release binaries if missing.
ZIMRU_BIN_DIR="$ROOT/target/release"
if [[ ! -x "$ZIMRU_BIN_DIR/zimcheck" || ! -x "$ZIMRU_BIN_DIR/zimdump" ]]; then
  echo "# building zimru binaries…" >&2
  (cd "$ROOT" && cargo build --release --quiet)
fi

# Build patched copies of the system tools that resolve their NEEDED
# libzim.9.dylib to the shim instead. install_name_tool + ad-hoc
# resign keeps the binary launchable. Patch only once per session;
# subsequent runs are no-ops.
SHIM_BINS_DIR="${SHIM_BINS_DIR:-/tmp/zimtools-shim}"
mkdir -p "$SHIM_BINS_DIR"
patch_to_shim() {
  local bin="$1"
  local out="$SHIM_BINS_DIR/$(basename "$bin")"
  if [[ ! -x "$out" || "$bin" -nt "$out" ]]; then
    cp "$bin" "$out"
    install_name_tool -change \
      /opt/homebrew/opt/libzim/lib/libzim.9.dylib \
      "$SHIM_BUILD/libzim.9.0.0.dylib" \
      "$out" 2>/dev/null || true
    codesign -f -s - "$out" 2>/dev/null || true
  fi
  echo "$out"
}

REAL_ZIMCHECK="$REAL_BIN_DIR/zimcheck"
REAL_ZIMDUMP="$REAL_BIN_DIR/zimdump"
SHIM_ZIMCHECK="$(patch_to_shim "$REAL_BIN_DIR/zimcheck")"
SHIM_ZIMDUMP="$(patch_to_shim "$REAL_BIN_DIR/zimdump")"

# Cache-warmer: read every byte of the inputs into the OS page cache.
# hyperfine runs this before every timed iteration, so each run
# starts from the same warm state.
WARM="cat \"$ZIM\" > /dev/null"

run() {
  local label="$1"; shift
  echo
  echo "==> $label"
  hyperfine --warmup 1 --runs 5 "$@"
}

echo "## Reader workloads (cache pre-warmed before each run)"
echo "ZIM: $ZIM ($(stat -f %z "$ZIM" | numfmt --to=iec --suffix=B))"

run "zimcheck -I (integrity)" \
  --prepare "$WARM" \
  -n "real libzim"  "$REAL_ZIMCHECK -I $ZIM" \
  -n "shim+zimru"   "$SHIM_ZIMCHECK -I $ZIM" \
  -n "zimru native" "$ZIMRU_BIN_DIR/zimcheck -I $ZIM"

run "zimcheck -C (MD5 trailer)" \
  --prepare "$WARM" \
  -n "real libzim"  "$REAL_ZIMCHECK -C $ZIM" \
  -n "shim+zimru"   "$SHIM_ZIMCHECK -C $ZIM" \
  -n "zimru native" "$ZIMRU_BIN_DIR/zimcheck -C $ZIM"

run "zimdump info" \
  --prepare "$WARM" \
  -n "real libzim"  "$REAL_ZIMDUMP info $ZIM" \
  -n "shim+zimru"   "$SHIM_ZIMDUMP info $ZIM" \
  -n "zimru native" "$ZIMRU_BIN_DIR/zimdump info $ZIM"

if [[ -n "$SITE" && -d "$SITE" ]]; then
  if [[ -z "$ICON" || ! -f "$ICON" ]]; then
    ICON="${SITE}/icon.png"
  fi
  if [[ ! -f "$ICON" ]]; then
    echo "(skipping zimwriterfs: no icon.png — pass it as 3rd arg or place at <site>/icon.png)" >&2
  else
    REAL_ZIMWRITERFS="$REAL_BIN_DIR/zimwriterfs"
    SHIM_ZIMWRITERFS="$(patch_to_shim "$REAL_BIN_DIR/zimwriterfs")"
    ZIMRU_ZIMWRITERFS="$ZIMRU_BIN_DIR/zimwriterfs"
    OUT="${SHIM_BINS_DIR}"
    ICON_NAME="$(basename "$ICON")"

    echo
    echo "## Writer workloads (input dir pre-read before each run)"
    # Three separate hyperfine calls so each variant's output ZIM
    # survives for the validation step below. zimwriterfs refuses to
    # overwrite, so each prepare hook only `rm`s its own output.
    REAL_PREPARE="rm -f \"$OUT/wfs_real.zim\" && find \"$SITE\" -type f -exec cat {} \\; > /dev/null"
    SHIM_PREPARE="rm -f \"$OUT/wfs_shim.zim\" && find \"$SITE\" -type f -exec cat {} \\; > /dev/null"
    NATIVE_PREPARE="rm -f \"$OUT/wfs_native.zim\" && find \"$SITE\" -type f -exec cat {} \\; > /dev/null"

    echo
    echo "==> zimwriterfs (real libzim)"
    hyperfine --warmup 1 --runs 5 --prepare "$REAL_PREPARE" \
      "$REAL_ZIMWRITERFS -w index.html -I $ICON_NAME -l eng -n t -t T -d d -c c -p p $SITE $OUT/wfs_real.zim"
    echo
    echo "==> zimwriterfs (shim+zimru)"
    hyperfine --warmup 1 --runs 5 --prepare "$SHIM_PREPARE" \
      "$SHIM_ZIMWRITERFS -w index.html -I $ICON_NAME -l eng -n t -t T -d d -c c -p p $SITE $OUT/wfs_shim.zim"
    echo
    echo "==> zimwriterfs (zimru native)"
    hyperfine --warmup 1 --runs 5 --prepare "$NATIVE_PREPARE" \
      "$ZIMRU_ZIMWRITERFS --welcome=index.html --illustration=$ICON_NAME --language=eng --name=t --title=T --description=d --creator=c --publisher=p $SITE $OUT/wfs_native.zim"

    echo
    echo "## Output validation (real zimcheck on each writer's output)"
    for f in wfs_real.zim wfs_shim.zim wfs_native.zim; do
      if [[ -f "$OUT/$f" ]]; then
        printf "  %-18s " "$f:"
        $REAL_ZIMCHECK -I "$OUT/$f" 2>&1 | grep -m1 'Status:' || echo "no Status line"
      fi
    done
  fi
fi
