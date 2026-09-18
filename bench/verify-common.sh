#!/usr/bin/env bash
# Shared, fail-closed handling of zimcheck's text report. Keep raw stdout,
# stderr and status beside each archive; exit 1 means findings, not a crash.
export LC_ALL=C
file_size() {
    if stat -c%s "$1" >/dev/null 2>&1; then stat -c%s "$1"; else stat -f%z "$1"; fi
}
check_report() {
    local checker="$1" archive="$2" report="$3" rc=0
    "$checker" -A "$archive" > "$report" 2> "$report.stderr" || rc=$?
    printf '%s\n' "$rc" > "$report.status"
    if [[ $rc -gt 1 ]]; then
        echo "checker execution failed ($rc): $report" >&2; return 1
    fi
    if ! awk -v rc="$rc" '
        /^\[ERROR\]/ {
            errors++
            if ($0 !~ /^\[ERROR\] [^:]+: .+/) bad=1
            else { sub(/^\[ERROR\] /, ""); sub(/:.*/, ""); print }
        }
        /^\[INFO\] Overall Test Status: (Pass|Fail)$/ { status++; pass=($NF == "Pass") }
        /^\[INFO\] Total time taken by zimcheck: .* seconds\.$/ { complete++ }
        END { if (status != 1 || complete != 1 || bad || (pass && (errors || rc != 0)) || (!pass && (!errors || rc != 1))) exit 1 }
    ' "$report" > "$report.classes.raw"; then
        echo "incomplete or unknown checker report: $report" >&2; return 1
    fi
    sort -u "$report.classes.raw" > "$report.classes" || return 1
}
check_verdict() {
    local checker="$1" archive="$2" report="$3" baseline="$4"
    check_report "$checker" "$archive" "$report" || { echo ERROR; return 1; }
    if [[ ! -s "$report.classes" ]]; then echo PASS; return 0; fi
    comm -13 "$baseline.classes" "$report.classes" > "$report.new-classes" || return 1
    if [[ -s "$report.new-classes" ]]; then echo REGRESS; return 1; fi
    echo '=src'
}
