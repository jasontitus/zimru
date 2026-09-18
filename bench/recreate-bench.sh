#!/usr/bin/env bash
# Time recreation with identical worker counts, validate complete checker reports.
# Usage: bench/recreate-bench.sh [glob]; env: UPSTREAM_DIR OUR_DIR OUT KEEP
# ZIMRU_BUILD=0 skips the release build (OUR_DIR already holds binaries).
# Timing requires GNU date (gdate on macOS) and awk.
set -uo pipefail
source "$(dirname "$0")/verify-common.sh"
UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
OUR_DIR="${OUR_DIR:-./target/release}"
OUT="${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/recreate-bench.XXXXXX")}" 
mkdir -p "$OUT" || exit 1
[[ ${ZIMRU_BUILD:-1} == 0 ]] || cargo build --release --quiet || exit 1
DATE=date; command -v gdate >/dev/null 2>&1 && DATE=gdate
for tool in "$UPSTREAM_DIR/zimcheck" "$UPSTREAM_DIR/zimrecreate" "$OUR_DIR/zimrecreate"; do
    [[ -x "$tool" ]] || { echo "missing executable: $tool" >&2; exit 1; }
done
FILES=()
while IFS= read -r f; do FILES+=("$f"); done < <(compgen -G "${1:-zim-cache/*.zim}")
[[ ${#FILES[@]} -gt 0 ]] || { echo "no matching ZIMs" >&2; exit 1; }
failed=0
for src in "${FILES[@]}"; do
    name=$(basename "$src")
    if ! check_report "$UPSTREAM_DIR/zimcheck" "$src" "$OUT/$name.src.check"; then failed=1; continue; fi
    for tag in zr up; do
        out="$OUT/$name.$tag.zim"
        rm -f "$out"
        if [[ $tag == zr ]]; then
            cmd=("$OUR_DIR/zimrecreate" "$src" "$out" --compression zstd --compression-level 19 -j -J 1)
        else
            cmd=("$UPSTREAM_DIR/zimrecreate" "$src" "$out" -j -J 1)
        fi
        start=$("$DATE" +%s.%N)
        if ! "${cmd[@]}" > "$out.stdout" 2> "$out.stderr" || [[ ! -s "$out" ]]; then
            echo "$name/$tag: RECREATE FAIL (see $OUT)"; failed=1; continue
        fi
        end=$("$DATE" +%s.%N)
        elapsed=$(awk "BEGIN { printf \"%.2f\", $end - $start }") || { failed=1; continue; }
        if ! verdict=$(check_verdict "$UPSTREAM_DIR/zimcheck" "$out" "$out.check" "$OUT/$name.src.check"); then failed=1; fi
        echo "$name/$tag: ${elapsed}s; checker $verdict"
        [[ $failed == 0 && -z "${KEEP:-}" ]] && rm -f "$out"
    done
done
exit "$failed"
