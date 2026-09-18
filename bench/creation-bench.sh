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
#   noft      both tools pass -j, which drops the fulltext index and keeps
#             the title index — upstream's semantics, and now zimru's too
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
source "$(dirname "$0")/verify-common.sh"
[[ $# -gt 0 ]] || { echo "usage: $0 <zim>..." >&2; exit 1; }
failed=0

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
UP_RECREATE="$UPSTREAM_DIR/zimrecreate"
UP_CHECK="$UPSTREAM_DIR/zimcheck"
UP_SEARCH="$UPSTREAM_DIR/zimsearch"
ZIMRU_RECREATE="${ZIMRU_RECREATE:-./target/release/zimrecreate}"
ZIMRU_DUMP="${ZIMRU_DUMP:-./target/release/zimdump}"
export XAPIANBUILDER="${XAPIANBUILDER:-../xapianbuilder/target/release/xapianbuilder}"
# One "pair" is a full ABBA cycle: two runs of each tool. Raising it trades
# wall-clock for a tighter best-of.
PAIRS="${PAIRS:-1}"
THREADS="${THREADS:-1}"
LEVEL="${LEVEL:-19}"
OUT="${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/zbench.XXXXXX")}"
mkdir -p "$OUT" || exit 1
DETAIL="${DETAIL:-$OUT/run-times.log}"
: > "$DETAIL"

[[ -x "$UP_RECREATE" ]] || { echo "upstream zimrecreate not found at $UP_RECREATE" >&2; exit 1; }
for tool in "$UP_CHECK" "$UP_SEARCH" "$ZIMRU_RECREATE" "$ZIMRU_DUMP"; do
    [[ -x "$tool" ]] || { echo "missing executable: $tool" >&2; exit 1; }
done
[[ $PAIRS =~ ^[1-9][0-9]*$ ]] || { echo "PAIRS must be positive" >&2; exit 1; }
[[ -x "$XAPIANBUILDER" ]] || echo "warning: xapianbuilder not executable at $XAPIANBUILDER — zimru will build no index" >&2

human() { numfmt --to=iec --suffix=B "$1"; }

# One timed run. Echoes "<seconds>", or "FAIL" on a non-zero exit.
#
# Each run starts from the same state: the previous output is removed, the
# source is re-read into page cache, and the previous run's dirty pages are
# already flushed (see the sync below). Without the re-warm the second tool
# reads a source the first tool's output writes have partly evicted, which
# on this box means whoever runs second pays for disk reads the other did
# not — an order effect that can be worth more than the difference being
# measured.
run_once() {
    local outfile="$1" src="$2"; shift 2
    local start end rc
    rm -f "$outfile"
    cat "$src" > /dev/null || return 1
    start=$(date +%s.%N)
    "$@" > "$outfile.stdout" 2> "$outfile.stderr"; rc=$?
    end=$(date +%s.%N)
    # Outside the timed region: flush this run's writes so the next run does
    # not inherit them as background writeback.
    sync
    if [[ $rc -ne 0 || ! -s "$outfile" ]]; then echo FAIL; return 1; fi
    awk "BEGIN{printf \"%.2f\", $end - $start}"
}

# Run the two tools in ABBA order, $PAIRS times, and keep each tool's best.
#
# ABBA (zimru, upstream, upstream, zimru) rather than AB: it gives both tools
# the same number of first-position and last-position runs, so any residual
# ordering advantage cancels instead of accruing to whichever tool the script
# happens to invoke first. Every individual run time is written to the
# detail log so the spread — and any surviving order effect — stays auditable
# rather than being hidden behind a single "best of".
ab_compare() {
    local zr_out="$1" up_out="$2" src="$3" label="$4"; shift 4
    local zr_best="" up_best="" t i
    for ((i = 1; i <= PAIRS; i++)); do
        for slot in zr up up zr; do
            if [[ $slot == zr ]]; then
                t=$(run_once "$zr_out" "$src" "${ZR_CMD[@]}") || { echo "FAIL FAIL"; return 1; }
                echo "$label run$i zimru    $t" >> "$DETAIL"
                if [[ -z "$zr_best" ]] || awk "BEGIN{exit !($t < $zr_best)}"; then zr_best=$t; fi
            else
                t=$(run_once "$up_out" "$src" "${UP_CMD[@]}") || { echo "$zr_best FAIL"; return 1; }
                echo "$label run$i upstream $t" >> "$DETAIL"
                if [[ -z "$up_best" ]] || awk "BEGIN{exit !($t < $up_best)}"; then up_best=$t; fi
            fi
        done
    done
    echo "$zr_best $up_best"
}

# Verdict on upstream's full sweep of our output.
#
#   PASS     clean
#   =src     fails only in ways the source already failed
#   REGRESS  introduces an error class the source does not have
#
# A plain Pass/Fail is not usable any more: zim-tools 3.8.0 added the
# M/Counter regex check, and virtually every published archive fails it (its
# mimetype histogram contains parameterised types like
# `image/svg+xml; charset=utf-8; …`). Comparing error *classes* against the
# source is what actually answers "did the writer break anything".
# `upstream zimcheck -A` is the single most expensive step in this script on
# a multi-GB archive — slower than either recreate it is verifying. Run it
# exactly once per archive and derive both the pass/fail and the error-class
# set from the same captured report; the source's classes are computed once
# per file and reused across modes.
SRC_REPORT=""
zimcheck_verdict() {
    check_verdict "$UP_CHECK" "$1" "$1.check" "$SRC_REPORT"
}

# "OK" when both index entries exist and zimsearch's top hit matches the
# source archive's; "NOIDX" / "MISMATCH" / "NOQUERY" otherwise.
index_verdict() {
    local out="$1" src="$2" query="$3"
    local have
    "$ZIMRU_DUMP" list --ns=X "$out" > "$out.index-list" 2> "$out.index-list.stderr" || { echo ERROR; return 1; }
    have=$(grep -c 'xapian$' "$out.index-list")
    [[ "$have" == "2" ]] || { echo "NOIDX($have/2)"; return 1; }
    [[ -n "$query" ]] || { echo "NOQUERY"; return 1; }
    local a b
    "$UP_SEARCH" "$src" "$query" > "$out.source-search" 2> "$out.source-search.stderr" || { echo ERROR; return 1; }
    "$UP_SEARCH" "$out" "$query" > "$out.search" 2> "$out.search.stderr" || { echo ERROR; return 1; }
    a=$(awk -F '\\t' '/^score/ { sub(/^[^\\t]*\\t[^\\t]*\\t/, ""); print; exit }' "$out.source-search")
    b=$(awk -F '\\t' '/^score/ { sub(/^[^\\t]*\\t[^\\t]*\\t/, ""); print; exit }' "$out.search")
    if [[ -z "$a" || -z "$b" ]]; then echo NOHITS; return 1
    elif [[ "$a" == "$b" ]]; then echo OK
    else echo MISMATCH; return 1; fi
}

# Total bytes of the two X/*/xapian blobs, so the two indexers' output sizes
# can be compared directly rather than inferred from whole-file sizes.
index_bytes() {
    "$ZIMRU_DUMP" analyze --by-item "$1" 2> "$1.analyze.stderr" \
        | awk '/X\/(fulltext|title)\/xapian$/ { s += $5 } END { print s + 0 }'
}

# A query term drawn from the middle of the archive: a content path with no
# directory or extension punctuation, i.e. an article title rather than an
# asset. Taken from the middle so it isn't an alphabetical-prefix outlier.
pick_query() {
    local src="$1" c
    while read -r c; do
        [[ -n "$c" ]] || continue
        if [[ -n "$($UP_SEARCH "$src" "$c" 2>/dev/null | grep -m1 '^score')" ]]; then
            echo "$c"; return
        fi
    done < <($ZIMRU_DUMP list "$src" 2>/dev/null \
        | awk '$0 !~ /[.\/]/ && length($0) >= 3 {a[++n]=$0}
               END { for (i = 1; i <= 12 && n; i++) print a[int(n * i / 13) + 1] }')
}

printf "%-40s %9s %6s %10s %10s %8s %10s %10s %8s %7s %9s %13s\n" \
    FILE SIZE MODE ZIMRU UPSTREAM SPEEDUP ZR-OUT UP-OUT SIZE-D CHECK SEARCH IDX-ZR/UP
printf -- "%s\n" "$(printf '%.0s-' {1..160})"

for src in "$@"; do
    [[ -f "$src" ]] || { echo "missing: $src" >&2; failed=1; continue; }
    name=$(basename "$src" .zim)
    size=$(file_size "$src") || { failed=1; continue; }
    SRC_REPORT="$OUT/$name.src.check"
    if ! check_report "$UP_CHECK" "$src" "$SRC_REPORT"; then
        echo "$name: ERROR reading source checker report" >&2; failed=1; continue
    fi
    query=$(pick_query "$src")

    for mode in index noft; do
        zr_out="$OUT/$name.zr.$mode.zim"
        up_out="$OUT/$name.up.$mode.zim"
        if [[ $mode == index ]]; then
            zr_extra=(); up_extra=()
        else
            zr_extra=(-j); up_extra=(-j)
        fi
        # Both -j runs still produce X/title/xapian, so the size column
        # compares two archives with the same entry set. Running zimru with
        # --without-indexes here instead would drop an entry upstream keeps
        # and report the missing index as a compression win.

        ZR_CMD=("$ZIMRU_RECREATE" "$src" "$zr_out" --compression zstd
                --compression-level "$LEVEL" -J "$THREADS" "${zr_extra[@]}")
        UP_CMD=("$UP_RECREATE" "$src" "$up_out" -J "$THREADS" "${up_extra[@]}")
        if ! times=$(ab_compare "$zr_out" "$up_out" "$src" "$name/$mode"); then
            echo "$name/$mode: execution FAILED (see $OUT)" >&2; failed=1; continue
        fi
        read -r zr_t up_t <<< "$times"
        zr_sz=$(file_size "$zr_out") || { failed=1; continue; }
        up_sz=$(file_size "$up_out") || { failed=1; continue; }

        if [[ "$zr_t" == FAIL || "$up_t" == FAIL ]]; then
            speedup="-"
        else
            speedup=$(awk "BEGIN{printf \"%.2fx\", $up_t / $zr_t}")
        fi
        # Output size relative to upstream's: negative means zimru's archive
        # is smaller. Reported as a percentage because the absolute figures
        # span three orders of magnitude across the corpus.
        if [[ "$zr_sz" -gt 0 && "$up_sz" -gt 0 ]]; then
            sizedelta=$(awk "BEGIN{printf \"%+.1f%%\", 100.0 * ($zr_sz - $up_sz) / $up_sz}")
        else
            sizedelta="-"
        fi
        if [[ "$zr_t" == FAIL ]]; then
            chk="-"; idx="-"; idxsz="-"
        else
            chk=$(zimcheck_verdict "$zr_out") || failed=1
            up_chk=$(zimcheck_verdict "$up_out") || failed=1
            chk="$chk/$up_chk"
            if [[ $mode == index ]]; then
                idx=$(index_verdict "$zr_out" "$src" "$query") || failed=1
                up_idx=$(index_verdict "$up_out" "$src" "$query") || failed=1
                idx="$idx/$up_idx"
                if [[ "$up_t" == FAIL ]]; then
                    idxsz="$(human "$(index_bytes "$zr_out")")/-"
                else
                    idxsz="$(human "$(index_bytes "$zr_out")")/$(human "$(index_bytes "$up_out")")"
                fi
            else
                idx="n/a"; idxsz="n/a"
            fi
        fi
        printf "%-40s %9s %6s %10s %10s %8s %10s %10s %8s %7s %9s %13s\n" \
            "$name" "$(human "$size")" "$mode" \
            "${zr_t}s" "${up_t}s" "$speedup" \
            "$(human "$zr_sz")" "$(human "$up_sz")" "$sizedelta" \
            "$chk" "$idx" "$idxsz"
        # Multi-GB sources produce multi-GB outputs on both sides; keeping
        # four of them per file fills the disk before the suite finishes.
        [[ $failed == 0 && -z "${KEEP:-}" ]] && rm -f "$zr_out" "$up_out"
    done
done
exit "$failed"
