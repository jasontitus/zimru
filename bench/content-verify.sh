#!/usr/bin/env bash
#
# Three-way content verification: source vs zimru's zimrecreate vs upstream's
# zimrecreate. Answers "is the smaller archive still the same archive?".
#
# Every entry is read through **real libzim**, by the zim-manifest helper in
# zimru-misc (which is where GPL-linking tools live). Comparing zimru's
# output using zimru's own reader proves only that the two agree with each
# other; a shared misunderstanding of the format would pass unnoticed.
#
# The obvious neutral alternative — `zimdump dump` plus md5sum over the
# extracted tree — does not work on real archives: it routes every path
# through the filesystem, and Korean Wikipedia contains entries named "%"
# and "$" on which upstream zimdump aborts ("Error creating symlink") after
# two dozen entries, silently truncating the comparison to nothing. Reading
# through the API touches no filesystem path.
#
# libzim's iterEfficient() yields user entries only, so the Xapian indexes
# and the regenerated metadata (Counter, Scraper) stay out of the comparison
# by construction — they are rebuilt by different code and are expected to
# differ. Index equivalence is a separate question, answered by
# xapianbuilder's bench/search-fidelity.sh.
#
# Both recreates run with -j so each produces the same entry set.
#
# Usage: bench/content-verify.sh <zim> [<zim>...]
#   env: UPSTREAM_DIR ZIM_MANIFEST XAPIANBUILDER OUT KEEP
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
UP_DUMP="$UPSTREAM_DIR/zimdump"
UP_RECREATE="$UPSTREAM_DIR/zimrecreate"
ZIMRU_RECREATE="${ZIMRU_RECREATE:-./target/release/zimrecreate}"
ZIMRU_DUMP="${ZIMRU_DUMP:-./target/release/zimdump}"
ZIM_MANIFEST="${ZIM_MANIFEST:-../zimru-misc/bench/zim-manifest}"
# Without this zimru silently builds no index while upstream builds its
# title index, and the missing entry shows up as a compression win.
export XAPIANBUILDER="${XAPIANBUILDER:-/home/user/xapianbuilder/target/release/xapianbuilder}"
THREADS="${THREADS:-$(nproc)}"
LEVEL="${LEVEL:-19}"
OUT="${OUT:-/tmp/zverify}"
mkdir -p "$OUT"

human() { numfmt --to=iec --suffix=B "$1"; }

[[ -x "$ZIM_MANIFEST" ]] || {
    echo "zim-manifest not found at $ZIM_MANIFEST." >&2
    echo "Build it from zimru-misc: g++ -O2 -std=c++17 bench/zim-manifest.cpp \\" >&2
    echo "    -I<libzim>/include -L<libzim>/lib/x86_64-linux-gnu -lzim -lcrypto \\" >&2
    echo "    -o bench/zim-manifest" >&2
    exit 1
}

# Compressed vs uncompressed cluster bytes — the mechanism behind any size
# difference, rather than just its magnitude.
cluster_profile() {
    $ZIMRU_DUMP analyze "$1" 2>/dev/null | awk '
        $2 ~ /^(zstd|xz|none)$/ { n[$2]++; b[$2] += $4; d[$2] += $5 }
        END { for (k in n) printf "%s: %d clusters, %.0fMB on disk from %.0fMB raw  ", \
                              k, n[k], b[k] / 1048576, d[k] / 1048576 }'
}

for src in "$@"; do
    [[ -f "$src" ]] || { echo "skip (missing): $src" >&2; continue; }
    name=$(basename "$src" .zim)
    echo "================================================================"
    echo "SOURCE: $name ($(human "$(stat -c%s "$src")"))"

    zr="$OUT/$name.zr.zim"; up="$OUT/$name.up.zim"
    rm -f "$zr" "$up"
    $ZIMRU_RECREATE "$src" "$zr" --compression zstd --compression-level "$LEVEL" \
        -J "$THREADS" -j >/dev/null 2>&1 || { echo "  zimru recreate FAILED"; continue; }
    $UP_RECREATE "$src" "$up" -J "$THREADS" -j >/dev/null 2>&1 \
        || { echo "  upstream recreate FAILED"; continue; }

    zrs=$(stat -c%s "$zr"); ups=$(stat -c%s "$up")
    printf "  zimru   %10s   upstream %10s   delta %+.1f%%\n" \
        "$(human "$zrs")" "$(human "$ups")" \
        "$(awk "BEGIN{print 100.0*($zrs-$ups)/$ups}")"

    for tag in src zr up; do
        case $tag in
            src) f="$src" ;; zr) f="$zr" ;; up) f="$up" ;;
        esac
        $ZIM_MANIFEST "$f" 2>/dev/null | LC_ALL=C sort > "$OUT/$name.$tag.manifest"
    done

    for tag in zr up; do
        a="$OUT/$name.src.manifest"; b="$OUT/$name.$tag.manifest"
        total=$(wc -l < "$a")
        same=$(comm -12 "$a" "$b" | wc -l)
        # An entry present in both but with different content appears in both
        # "only" lists, so path-set drift and content drift stay distinct.
        paths_only_src=$(comm -23 <(cut -f1 "$a") <(cut -f1 "$b") | wc -l)
        paths_only_out=$(comm -13 <(cut -f1 "$a") <(cut -f1 "$b") | wc -l)
        changed=$(( total - same - paths_only_src ))
        verdict=IDENTICAL
        (( changed || paths_only_src || paths_only_out )) && verdict=DIFFERS
        printf "  %-8s entries=%-9s identical=%-9s content-changed=%-6s missing=%-5s extra=%-5s  %s\n" \
            "$tag" "$total" "$same" "$changed" "$paths_only_src" "$paths_only_out" "$verdict"
        if [[ $verdict == DIFFERS ]]; then
            { echo "### missing from $tag (in source, not in output):"
              comm -23 <(cut -f1 "$a") <(cut -f1 "$b") | head -20
              echo "### extra in $tag:"
              comm -13 <(cut -f1 "$a") <(cut -f1 "$b") | head -20
              echo "### same path, different content (source line then $tag line):"
              join -t"$(printf '\t')" -j1 <(LC_ALL=C sort -t"$(printf '\t')" -k1,1 "$a") \
                                           <(LC_ALL=C sort -t"$(printf '\t')" -k1,1 "$b") \
                  | awk -F'\t' '$2 != $4 || $3 != $5' | head -20
            } > "$OUT/$name.$tag.diff"
            echo "           -> $OUT/$name.$tag.diff"
        fi
    done

    echo "  clusters zimru:    $(cluster_profile "$zr")"
    echo "  clusters upstream: $(cluster_profile "$up")"
    echo "  clusters source:   $(cluster_profile "$src")"
    [[ -n "${KEEP:-}" ]] || rm -f "$zr" "$up"
done
