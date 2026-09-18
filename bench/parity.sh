#!/usr/bin/env bash
# Compare exit status, normalized stdout AND meaningful stderr. Raw streams,
# statuses and differences are retained. No finding bodies are discarded.
# Usage: bench/parity.sh [archive] [upstream-dir]; env: UPSTREAM_DIR OUR_DIR OUT
# ZIMRU_BUILD=0 skips the release build (OUR_DIR already holds binaries).
set -uo pipefail
export LC_ALL=C
ZIM="${1:-zim-cache/wikipedia_en_100_mini.zim}"
UPSTREAM_DIR="${2:-${UPSTREAM_DIR:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.8.0}}"
OUR_DIR="${OUR_DIR:-./target/release}"
OUT="${OUT:-bench/parity-out}"
OUT_UP="$OUT/upstream"; OUT_US="$OUT/zimru"
mkdir -p "$OUT_UP" "$OUT_US" || exit 1
[[ -f "$ZIM" ]] || { echo "zim not found: $ZIM" >&2; exit 1; }
for dir in "$UPSTREAM_DIR" "$OUR_DIR"; do
    for tool in zimdump zimcheck; do
        [[ -x "$dir/$tool" ]] || { echo "missing executable: $dir/$tool" >&2; exit 1; }
    done
done
[[ ${ZIMRU_BUILD:-1} == 0 ]] || cargo build --release --quiet || exit 1
normalize() {
    sed -E \
        -e 's/(Zimcheck version is) [0-9.]+/\1 X.Y.Z/' \
        -e 's/("zimcheck_version" : )"[0-9.]+"/\1"X.Y.Z"/' \
        -e 's/zim-tools [0-9.]+/zim-tools X.Y.Z/' \
        -e 's/libzim [0-9.]+/libzim X.Y.Z/' \
        -e 's/(Total time taken by zimcheck:) .* seconds\./\1 N seconds./' \
        -e 's/[[:space:]]+$//'
}
normalize_stderr() {
    normalize | sed -E -e "/^No stemming for language '.*'\$/d" -e '/^Cannot read stopwords file /d'
}
# Normalize only ordering of complete redundant-pair records; preserve every
# dangling link and every JSON finding. Ordering-only differences remain visible
# rather than converting an unverified structural comparison into a PASS.
normalize_stdout() {
    normalize | awk '
        /^\[WARNING\] Redundant Data: / {
            body=substr($0,27); split_at=index(body," and ")
            if (split_at) {
                a=substr(body,1,split_at-1); b=substr(body,split_at+5)
                red[n++]=(a < b ? a " and " b : b " and " a); next
            }
        }
        { flush(); print }
        END { flush() }
        function flush( i,j,t) {
            for(i=1;i<n;i++) { t=red[i]; for(j=i-1;j>=0 && red[j]>t;j--) red[j+1]=red[j]; red[j+1]=t }
            for(i=0;i<n;i++) print "[WARNING] Redundant Data: " red[i]
            n=0
        }
    '
}
passed=0; failed=0
for label in zimdump-info zimdump-list zimdump-list-details zimdump-show-redirect zimcheck-checksum zimcheck-integrity zimcheck-metadata zimcheck-favicon zimcheck-mainpage zimcheck-redirect zimcheck-all zimcheck-all-json; do
    case "$label" in
        zimdump-info) args=(zimdump info "$ZIM");;
        zimdump-list) args=(zimdump list "$ZIM");;
        zimdump-list-details) args=(zimdump list --details "$ZIM");;
        zimdump-show-redirect) args=(zimdump show --idx=0 "$ZIM");;
        zimcheck-checksum) args=(zimcheck -C "$ZIM");;
        zimcheck-integrity) args=(zimcheck -I "$ZIM");;
        zimcheck-metadata) args=(zimcheck -M "$ZIM");;
        zimcheck-favicon) args=(zimcheck -F "$ZIM");;
        zimcheck-mainpage) args=(zimcheck -P "$ZIM");;
        zimcheck-redirect) args=(zimcheck -L "$ZIM");;
        zimcheck-all) args=(zimcheck -A "$ZIM");;
        zimcheck-all-json) args=(zimcheck -A -J "$ZIM");;
    esac
    up="$OUT_UP/$label"; us="$OUT_US/$label"
    up_rc=0; us_rc=0
    "$UPSTREAM_DIR/${args[0]}" "${args[@]:1}" > "$up.stdout" 2> "$up.stderr" || up_rc=$?
    "$OUR_DIR/${args[0]}" "${args[@]:1}" > "$us.stdout" 2> "$us.stderr" || us_rc=$?
    printf '%s\n' "$up_rc" > "$up.status"; printf '%s\n' "$us_rc" > "$us.status"
    valid=1
    for base in "$up" "$us"; do
        normalize_stdout < "$base.stdout" > "$base.stdout.norm" || valid=0
        normalize_stderr < "$base.stderr" > "$base.stderr.norm" || valid=0
        # Only raw show may legitimately produce zero bytes (an empty item).
        if [[ $label != zimdump-show-redirect && ! -s "$base.stdout.norm" && ! -s "$base.stderr.norm" ]]; then valid=0; fi
    done
    # Statuses must agree. Any status in the shell's signal range (129..165,
    # e.g. 134 SIGABRT, 139 SIGSEGV) is a crash and never passes, even when
    # both tools die the same way. Upstream's documented 255 ("entry is a
    # redirect") is an ordinary status and must simply match.
    for rc in "$up_rc" "$us_rc"; do
        if [[ $rc -ge 129 && $rc -le 165 ]]; then valid=0; fi
    done
    [[ $up_rc == "$us_rc" ]] || valid=0
    diff -u "$up.stdout.norm" "$us.stdout.norm" > "$OUT/$label.stdout.diff" || valid=0
    diff -u "$up.stderr.norm" "$us.stderr.norm" > "$OUT/$label.stderr.diff" || valid=0
    if [[ $valid == 1 ]]; then
        echo "$label: PASS (stdout, stderr, exit=$up_rc)"; passed=$((passed+1))
    else
        echo "$label: DIFF/ERROR (exit=$up_rc/$us_rc; see $OUT)"; failed=$((failed+1))
    fi
done
echo "Summary: $passed/$((passed+failed)) cases pass; raw diagnostics in $OUT"
[[ $failed == 0 ]]
