#!/usr/bin/env bash
# Three-way verification through the external libzim-backed zim-manifest.
# Its user-entry rows exclude regenerated metadata/indexes. Counts are checked
# independently with zimdump info, including an explicit zero-entry case.
# Usage: bench/content-verify.sh <zim>...; env: UPSTREAM_DIR ZIM_MANIFEST
# ZIMRU_RECREATE ZIMRU_DUMP XAPIANBUILDER THREADS LEVEL OUT KEEP
set -uo pipefail
source "$(dirname "$0")/verify-common.sh"
UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
UP_RECREATE="$UPSTREAM_DIR/zimrecreate"
ZIMRU_RECREATE="${ZIMRU_RECREATE:-./target/release/zimrecreate}"
ZIMRU_DUMP="${ZIMRU_DUMP:-./target/release/zimdump}"
ZIM_MANIFEST="${ZIM_MANIFEST:-../zimru-misc/bench/zim-manifest}"
export XAPIANBUILDER="${XAPIANBUILDER:-../xapianbuilder/target/release/xapianbuilder}"
THREADS="${THREADS:-1}"
LEVEL="${LEVEL:-19}"
OUT="${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/zverify.XXXXXX")}"
mkdir -p "$OUT" || exit 1
[[ $# -gt 0 ]] || { echo "usage: $0 <zim>..." >&2; exit 1; }
for tool in "$ZIM_MANIFEST" "$ZIMRU_RECREATE" "$ZIMRU_DUMP" "$UP_RECREATE"; do
    [[ -x "$tool" ]] || { echo "missing executable: $tool" >&2; exit 1; }
done
manifest() {
    local archive="$1" base="$2" count
    "$ZIMRU_DUMP" info "$archive" > "$base.info" 2> "$base.info.stderr" || return 1
    count=$(awk '/^count-entries: [0-9]+$/ { n++; count=$2 } END { if (n != 1) exit 1; print count }' "$base.info") || return 1
    "$ZIM_MANIFEST" "$archive" > "$base.raw" 2> "$base.stderr" || return 1
    # Each entry must yield one complete TSV row with a unique nonempty path.
    # A successful helper that silently truncates is not a valid manifest.
    awk -F '\t' -v expected="$count" '
        NF != 3 || $1 == "" || $2 == "" || $3 == "" || seen[$1]++ { bad=1 }
        END { exit (bad || NR != expected) }
    ' "$base.raw" || return 1
    sort "$base.raw" > "$base.manifest" || return 1
    echo "  verified manifest: $count entries ($base.manifest)"
}
failed=0
for src in "$@"; do
    if [[ ! -f "$src" ]]; then echo "missing: $src" >&2; failed=1; continue; fi
    name=$(basename "$src" .zim)
    echo "SOURCE: $src"
    zr="$OUT/$name.zr.zim"; up="$OUT/$name.up.zim"
    if ! manifest "$src" "$OUT/$name.src"; then
        echo "ERROR: failed/incomplete source manifest (see $OUT)" >&2; failed=1; continue
    fi
    rm -f "$zr" "$up"
    if ! "$ZIMRU_RECREATE" "$src" "$zr" --compression zstd --compression-level "$LEVEL" -J "$THREADS" -j > "$OUT/$name.zr.recreate.stdout" 2> "$OUT/$name.zr.recreate.stderr" || [[ ! -s "$zr" ]]; then
        echo "ERROR: zimru recreate failed (see $OUT)" >&2; failed=1; continue
    fi
    if ! "$UP_RECREATE" "$src" "$up" -J "$THREADS" -j > "$OUT/$name.up.recreate.stdout" 2> "$OUT/$name.up.recreate.stderr" || [[ ! -s "$up" ]]; then
        echo "ERROR: upstream recreate failed (see $OUT)" >&2; failed=1; continue
    fi
    complete=1
    for tag in zr up; do
        if [[ $tag == zr ]]; then archive="$zr"; else archive="$up"; fi
        if ! manifest "$archive" "$OUT/$name.$tag"; then
            echo "ERROR: failed/incomplete $tag manifest (see $OUT)" >&2
            failed=1; complete=0
        fi
    done
    [[ $complete == 1 ]] || continue
    identical=1
    for tag in zr up; do
        if diff -u "$OUT/$name.src.manifest" "$OUT/$name.$tag.manifest" > "$OUT/$name.$tag.diff"; then
            echo "  $tag: IDENTICAL user-entry manifest (including verified empty content)"
        else
            echo "  $tag: DIFFERS or comparison failed (see $OUT/$name.$tag.diff)"
            failed=1; identical=0
        fi
    done
    if [[ $identical == 1 && -z "${KEEP:-}" ]]; then rm -f "$zr" "$up"; fi
done
exit "$failed"
