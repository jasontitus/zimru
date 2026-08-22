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
set -uo pipefail

UPSTREAM_DIR="${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}"
OUR_DIR="${OUR_DIR:-./target/release}"
PAIRS="${PAIRS:-1}"
OUT="${OUT:-/tmp/ztoolset}"
mkdir -p "$OUT"
DETAIL="${DETAIL:-$OUT/run-times.log}"
: > "$DETAIL"
DUMPDIR="$OUT/dumpdir"

[[ -d "$UPSTREAM_DIR" ]] || { echo "upstream tools not found: $UPSTREAM_DIR" >&2; exit 1; }

ZIM=""      # set per archive; run_once re-warms it
run_once() {
    local start end rc
    rm -rf "$DUMPDIR"
    cat "$ZIM" > /dev/null 2>&1
    start=$(date +%s.%N)
    bash -c "$1" >/dev/null 2>&1; rc=$?
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
    [[ -f "$zim" ]] || { echo "skip (missing): $zim" >&2; continue; }
    ZIM="$zim"
    name=$(basename "$zim" .zim)

    # label | upstream command | zimru command
    CASES=(
      "zimcheck -C|$UPSTREAM_DIR/zimcheck -C '$zim'|$OUR_DIR/zimcheck -C '$zim'"
      "zimcheck -I|$UPSTREAM_DIR/zimcheck -I '$zim'|$OUR_DIR/zimcheck -I '$zim'"
      "zimcheck -R|$UPSTREAM_DIR/zimcheck -R '$zim'|$OUR_DIR/zimcheck -R '$zim'"
      "zimcheck -A|$UPSTREAM_DIR/zimcheck -A '$zim'|$OUR_DIR/zimcheck -A '$zim'"
      "zimdump info|$UPSTREAM_DIR/zimdump info '$zim'|$OUR_DIR/zimdump info '$zim'"
      "zimdump list|$UPSTREAM_DIR/zimdump list '$zim'|$OUR_DIR/zimdump list '$zim'"
      "zimdump dump|$UPSTREAM_DIR/zimdump dump --dir=$DUMPDIR --redirect '$zim'|$OUR_DIR/zimdump dump --dir=$DUMPDIR --redirect '$zim'"
      # NOT COMPARABLE — kept so the asymmetry stays visible rather than
      # being quietly dropped. Upstream's zimbench collects the URL lists and
      # then exits 0 without running either read phase, so it reports no
      # throughput and its time measures dirent lookups only. zimru runs all
      # three phases. A "speedup" here is upstream doing none of the work,
      # and exit status does not reveal it: both return 0.
      "zimbench -n 1000|$UPSTREAM_DIR/zimbench -n 1000 '$zim'|$OUR_DIR/zimbench -n 1000 '$zim'"
    )

    for entry in "${CASES[@]}"; do
        IFS='|' read -r label upcmd ourcmd <<< "$entry"
        zr_best=""; up_best=""; zr_rc="?"; up_rc="?"
        for ((i = 1; i <= PAIRS; i++)); do
            for slot in zr up up zr; do
                if [[ $slot == zr ]]; then
                    read -r t rc < <(run_once "$ourcmd")
                    echo "$name|$label|zimru|$t|rc=$rc" >> "$DETAIL"
                    zr_rc=$rc
                    if [[ -z "$zr_best" ]] || awk "BEGIN{exit !($t < $zr_best)}"; then zr_best=$t; fi
                else
                    read -r t rc < <(run_once "$upcmd")
                    echo "$name|$label|upstream|$t|rc=$rc" >> "$DETAIL"
                    up_rc=$rc
                    if [[ -z "$up_best" ]] || awk "BEGIN{exit !($t < $up_best)}"; then up_best=$t; fi
                fi
            done
        done
        speedup=$(awk "BEGIN{printf \"%.2fx\", $up_best / $zr_best}")
        printf "%-34s %-22s %10s %10s %9s %9s\n" "$name" "$label" \
            "${zr_best}s" "${up_best}s" "$speedup" "$zr_rc/$up_rc"
    done
    rm -rf "$DUMPDIR"
done
