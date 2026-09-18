#!/usr/bin/env bash
# Rewrite archives and compare framed user-entry digests, NOT concatenated
# blob bytes: M metadata, X indexes/listings and modern W/mainPage regenerate.
# Modern C entries and legacy user entries (qualified as namespace/path) include
# path, title, MIME, body, and redirect target. Empty content is explicit.
# Usage: bench/recreate-suite.sh [glob]; env: OUR_DIR UPSTREAM_DIR OUT KEEP
# ZIMRU_BUILD=0 skips the release build (OUR_DIR already holds binaries).
set -uo pipefail
source "$(dirname "$0")/verify-common.sh"
UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
OUR_DIR="${OUR_DIR:-./target/release}"
OUT="${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/recreate-suite.XXXXXX")}" 
mkdir -p "$OUT" || exit 1
[[ ${ZIMRU_BUILD:-1} == 0 ]] || cargo build --release --quiet || exit 1
for tool in "$OUR_DIR/zimru" "$OUR_DIR/zimrecreate" "$UPSTREAM_DIR/zimcheck"; do
    [[ -x "$tool" ]] || { echo "missing executable: $tool" >&2; exit 1; }
done
GLOB="${1:-zim-cache/*.zim}"
FILES=()
while IFS= read -r f; do FILES+=("$f"); done < <(compgen -G "$GLOB")
[[ ${#FILES[@]} -gt 0 ]] || { echo "no ZIMs matching $GLOB" >&2; exit 1; }
# Exactly one count and digest are required: empty stdout is never a digest.
digest() {
    local archive="$1" report="$2"
    "$OUR_DIR/zimru" readall "$archive" --content-md5 --quiet > "$report" 2> "$report.stderr" || return 1
    awk '
        /^content_entries: [0-9]+$/ { count++ }
        /^content_md5: [0-9a-f]+$/ && length($2) == 32 { digest++ }
        END { exit !(count == 1 && digest == 1 && NR == 2) }
    ' "$report"
}
failed=0
for src in "${FILES[@]}"; do
    name=$(basename "$src")
    out="$OUT/$name.recreated.zim"
    if ! digest "$src" "$OUT/$name.src.digest" ||
       ! check_report "$UPSTREAM_DIR/zimcheck" "$src" "$OUT/$name.src.check"; then
        echo "$name: ERROR verifying source (see $OUT)"; failed=1; continue
    fi
    rm -f "$out"
    if ! "$OUR_DIR/zimrecreate" "$src" "$out" --compression zstd > "$OUT/$name.recreate.stdout" 2> "$OUT/$name.recreate.stderr" || [[ ! -s "$out" ]]; then
        echo "$name: RECREATE FAIL (see $OUT)"; failed=1; continue
    fi
    if ! digest "$out" "$OUT/$name.out.digest"; then
        echo "$name: ERROR reading recreated content (see $OUT)"; failed=1; continue
    fi
    if ! verdict=$(check_verdict "$UPSTREAM_DIR/zimcheck" "$out" "$OUT/$name.out.check" "$OUT/$name.src.check"); then
        echo "$name: $verdict (see $OUT)"; failed=1; continue
    fi
    if cmp -s "$OUT/$name.src.digest" "$OUT/$name.out.digest"; then
        echo "$name: CONTENT MATCH; checker $verdict"
    else
        echo "$name: CONTENT DIFF (see $OUT)"; failed=1; continue
    fi
    [[ -n "${KEEP:-}" ]] || rm -f "$out"
done
exit "$failed"
