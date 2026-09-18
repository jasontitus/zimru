#!/usr/bin/env bash
#
# ABBA head-to-head across the whole zim-tools surface: zimru's binaries vs
# kiwix zim-tools 3.8.0 / libzim 9.8.2, on the same archive.
#
# Why not hyperfine: it runs every repetition of A, then every repetition of
# B. For workloads that write (zimdump dump extracts 175 k files) or that
# stream a gigabyte-scale archive through page cache, that hands the second
# command a machine the first one warmed or dirtied. This harness interleaves
# ABBA — zimru, upstream, upstream, zimru — so both tools get the same number
# of first- and last-position runs, and each keeps its best. Every individual
# run time goes to the detail log so the spread stays visible rather than
# disappearing behind a single "best of".
#
# Before each run the previous output is removed and the archive is re-read
# into page cache; after it, the run's writes are synced outside the timed
# region so one run's writeback is not charged to the next.
#
# Usage: bench/toolset-bench.sh <zim> [<zim>...]
#   env: UPSTREAM_DIR OUR_DIR PAIRS OUT DETAIL
# Exit status is non-zero when any archive is missing or any run exits >1
# (a crash or hard error, not findings); such rows print ABORT, never a time.
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
OUR_DIR="${OUR_DIR:-./target/release}"
PAIRS="${PAIRS:-1}"
OUT="${OUT:-$(mktemp -d "${TMPDIR:-/tmp}/ztoolset.XXXXXX")}"
mkdir -p "$OUT" || exit 1
DETAIL="${DETAIL:-$OUT/run-times.log}"
: > "$DETAIL"
DUMPDIR="$OUT/dumpdir"

[[ $# -gt 0 ]] || { echo "usage: $0 <zim>..." >&2; exit 1; }
for dir in "$UPSTREAM_DIR" "$OUR_DIR"; do
    for tool in zimcheck zimdump zimbench; do
        [[ -x "$dir/$tool" ]] || { echo "missing executable: $dir/$tool" >&2; exit 1; }
    done
done
failed=0

ZIM=""      # set per archive; run_once re-warms it
# run_once <argv...>: the command is an argv vector, never re-parsed by a
# shell, so archive names with quotes, spaces or `|` are passed verbatim.
run_once() {
    local start end rc
    rm -rf "$DUMPDIR"
    cat "$ZIM" > /dev/null 2>&1
    start=$(date +%s.%N)
    "$@" > "$OUT/last.stdout" 2> "$OUT/last.stderr"; rc=$?
    end=$(date +%s.%N)
    sync
    # Echo the exit status alongside the time rather than treating non-zero
    # as failure. A non-zero exit is a normal outcome for zimcheck (it means
    # the archive has findings — under 3.8.0 nearly every published archive
    # fails the new M/Counter check), and for zimdump dump (warnings). But it
    # is *not* normal for zimbench, and upstream's crashes partway through
    # some archives, which would otherwise be recorded as a fast run. The
    # table prints both tools' exit codes so a suspiciously quick time can be
    # read for what it is.
    printf '%s %s' "$(awk "BEGIN{printf \"%.3f\", $end - $start}")" "$rc"
}

printf "%-34s %-22s %10s %10s %9s %9s\n" ARCHIVE CASE ZIMRU UPSTREAM SPEEDUP "EXIT zr/up"
printf -- "%s\n" "$(printf '%.0s-' {1..104})"

for zim in "$@"; do
    [[ -f "$zim" ]] || { echo "missing archive: $zim" >&2; failed=1; continue; }
    ZIM="$zim"
    name=$(basename "$zim" .zim)

    # Each case is a label plus the argument list shared by both tools; the
    # tool binary is prefixed per slot below.
    CASES=(
      "zimcheck -C"
      "zimcheck -I"
      "zimcheck -R"
      "zimcheck -A"
      "zimdump info"
      "zimdump list"
      "zimdump dump"
      # NOT COMPARABLE — kept so the asymmetry stays visible rather than
      # being quietly dropped. Upstream's zimbench collects the URL lists and
      # then exits 0 without running either read phase, so it reports no
      # throughput and its time measures dirent lookups only. zimru runs all
      # three phases. A "speedup" here is upstream doing none of the work,
      # and exit status does not reveal it: both return 0.
      "zimbench -n 1000"
    )

    for label in "${CASES[@]}"; do
        read -r tool _ <<< "$label"
        case "$label" in
            "zimdump dump") args=(dump "--dir=$DUMPDIR" --redirect "$zim");;
            "zimbench -n 1000") args=(-n 1000 "$zim");;
            *) args=(${label#* } "$zim");;
        esac
        upcmd=("$UPSTREAM_DIR/$tool" "${args[@]}")
        ourcmd=("$OUR_DIR/$tool" "${args[@]}")
        zr_best=""; up_best=""; zr_rc="?"; up_rc="?"; crashed=0
        for ((i = 1; i <= PAIRS; i++)); do
            for slot in zr up up zr; do
                if [[ $slot == zr ]]; then
                    read -r t rc < <(run_once "${ourcmd[@]}")
                    echo "$name|$label|zimru|$t|rc=$rc" >> "$DETAIL"
                    zr_rc=$rc
                    [[ $rc -le 1 ]] || crashed=1
                    if [[ -z "$zr_best" ]] || awk "BEGIN{exit !($t < $zr_best)}"; then zr_best=$t; fi
                else
                    read -r t rc < <(run_once "${upcmd[@]}")
                    echo "$name|$label|upstream|$t|rc=$rc" >> "$DETAIL"
                    up_rc=$rc
                    [[ $rc -le 1 ]] || crashed=1
                    if [[ -z "$up_best" ]] || awk "BEGIN{exit !($t < $up_best)}"; then up_best=$t; fi
                fi
            done
        done
        if [[ $crashed == 1 ]]; then
            # An aborted run (exit > 1: crash or hard error) is not a fast run:
            # no time, no speedup, non-zero exit.
            failed=1
            printf "%-34s %-22s %10s %10s %9s %9s\n" "$name" "$label" ABORT ABORT - "$zr_rc/$up_rc"
            continue
        fi
        speedup=$(awk "BEGIN{printf \"%.2fx\", $up_best / $zr_best}")
        printf "%-34s %-22s %10s %10s %9s %9s\n" "$name" "$label" \
            "${zr_best}s" "${up_best}s" "$speedup" "$zr_rc/$up_rc"
    done
    rm -rf "$DUMPDIR"
done
exit "$failed"
