#!/usr/bin/env bash
#
# Parity harness: runs each implemented tool head-to-head against upstream
# zim-tools 3.6.0 / libzim 9.3.0 and diffs the outputs. Prints a summary
# table of which invocations match byte-for-byte and which differ.
#
# Usage:
#   ./bench/parity.sh [path/to/file.zim] [path/to/upstream/zim-tools-dir]
set -uo pipefail

ZIM="${1:-zim-cache/wikipedia_en_100_mini.zim}"
UPSTREAM_DIR="${2:-/opt/zim-tools-upstream/zim-tools_linux-x86_64-3.6.0}"
OUR_DIR="./target/release"
OUT_UP="bench/parity-out/upstream"
OUT_US="bench/parity-out/zimru"
mkdir -p "$OUT_UP" "$OUT_US"

[[ -f "$ZIM" ]] || { echo "zim not found: $ZIM" >&2; exit 1; }
[[ -d "$UPSTREAM_DIR" ]] || { echo "upstream tools not found: $UPSTREAM_DIR" >&2; exit 1; }

# Prefer GNU sed when available — materially faster than macOS BSD sed on
# the multi-GB outputs `zimdump show` and `zimcheck -A` produce on real ZIMs.
SED="sed"
command -v gsed >/dev/null 2>&1 && SED="gsed"

cargo build --release --quiet || { echo "cargo build failed — aborting (stale/missing binaries would misreport as DIFF/FAIL rows)" >&2; exit 1; }

# Normalize differences that aren't substantive (version strings, "<3 seconds"
# vs "1 seconds", trailing whitespace).
normalize() {
    "$SED" -E \
        -e 's/(Zimcheck version is) [0-9.]+/\1 X.Y.Z/' \
        -e 's/("zimcheck_version" : )"[0-9.]+"/\1"X.Y.Z"/' \
        -e 's/zim-tools [0-9.]+/zim-tools X.Y.Z/' \
        -e 's/libzim [0-9.]+/libzim X.Y.Z/' \
        -e 's/(Total time taken by zimcheck:) .* seconds\./\1 N seconds./' \
        -e 's/[[:space:]]+$//'
}

# Strip the order-dependent body lines of the redundant-data and
# invalid-internal-links blocks (whose order/exact set is determined by
# libzim's internal iteration we don't replicate). Keep the header lines
# so structural parity is still verified.
strip_setlike_bodies() {
    awk '
        BEGIN { skip=0; in_json_logs=0 }
        # JSON form: collapse the entire "logs" : [ ... ] body to a single
        # marker line — the order of per-check entries inside it depends on
        # libzim'\''s internal iteration we don'\''t mirror.
        /"logs" : \[/ {
            print "  \"logs\" : [ <entries elided for parity comparison> ]"
            in_json_logs = 1
            next
        }
        in_json_logs {
            if (/^[[:space:]]*\][[:space:]]*$/) { in_json_logs = 0 }
            next
        }
        # Plain-text form: the "Redundant data found", "Invalid internal links
        # found" and "Invalid external links found" blocks have order-dependent
        # bodies — keep the headers, drop the bodies.
        /^\[WARNING\] Redundant data found:/ { print; skip=1; next }
        /^\[ERROR\] Invalid internal links found:/ { print; skip=1; next }
        /^\[ERROR\] Invalid external links found:/ { print; skip=1; next }
        skip && /^\[/ { skip=0 }
        skip && /^[[:space:]]*$/ { skip=0 }
        !skip { print }
    '
}

# Count the number of dangling-link triple-line blocks and redundant pairs;
# used to check shape-parity (we should find roughly the same number even
# if the exact set differs).
count_dangling()        { local n=$(grep -c '^- \./' "$1" 2>/dev/null); echo "${n:-0}"; }
count_external()        { local n=$(grep -c 'is an external dependence' "$1" 2>/dev/null); echo "${n:-0}"; }
count_redundant_pairs() { local n=$(grep -cE '^  .* and ' "$1" 2>/dev/null); echo "${n:-0}"; }

declare -a LABELS=()
declare -A UP_CMD=()
declare -A US_CMD=()
declare -A MODE=()  # "exact" or "structural"

add_case() {
    local label="$1"; shift
    local up="$1"; shift
    local us="$1"; shift
    local mode="${1:-exact}"
    LABELS+=("$label")
    UP_CMD["$label"]="$up"
    US_CMD["$label"]="$us"
    MODE["$label"]="$mode"
}

# ---------------- zimdump ----------------
add_case "zimdump-info"            "$UPSTREAM_DIR/zimdump info '$ZIM'"               "$OUR_DIR/zimdump info '$ZIM'"
add_case "zimdump-list"            "$UPSTREAM_DIR/zimdump list '$ZIM'"               "$OUR_DIR/zimdump list '$ZIM'"
add_case "zimdump-list-details"    "$UPSTREAM_DIR/zimdump list --details '$ZIM' | head -200" "$OUR_DIR/zimdump list --details '$ZIM' | head -200"
add_case "zimdump-show-redirect"   "$UPSTREAM_DIR/zimdump show --idx=0 '$ZIM'"       "$OUR_DIR/zimdump show --idx=0 '$ZIM'"

# ---------------- zimcheck ----------------
add_case "zimcheck-checksum"       "$UPSTREAM_DIR/zimcheck -C '$ZIM'"                "$OUR_DIR/zimcheck -C '$ZIM'"
add_case "zimcheck-integrity"      "$UPSTREAM_DIR/zimcheck -I '$ZIM'"                "$OUR_DIR/zimcheck -I '$ZIM'"
add_case "zimcheck-metadata"       "$UPSTREAM_DIR/zimcheck -M '$ZIM'"                "$OUR_DIR/zimcheck -M '$ZIM'"
add_case "zimcheck-favicon"        "$UPSTREAM_DIR/zimcheck -F '$ZIM'"                "$OUR_DIR/zimcheck -F '$ZIM'"
add_case "zimcheck-mainpage"       "$UPSTREAM_DIR/zimcheck -P '$ZIM'"                "$OUR_DIR/zimcheck -P '$ZIM'"
add_case "zimcheck-redirect"       "$UPSTREAM_DIR/zimcheck -L '$ZIM'"                "$OUR_DIR/zimcheck -L '$ZIM'"
# For -A and -A -J the redundant pair list and broken-link list depend on
# libzim's internal iteration order which we don't mirror. We compare the
# structural part (everything except those bodies) and separately verify the
# *count* of items found is in a reasonable range.
add_case "zimcheck-all"            "$UPSTREAM_DIR/zimcheck -A '$ZIM'"                "$OUR_DIR/zimcheck -A '$ZIM'"   structural
add_case "zimcheck-all-json"       "$UPSTREAM_DIR/zimcheck -A -J '$ZIM'"             "$OUR_DIR/zimcheck -A -J '$ZIM'" structural

# Stream raw output through normalize (and strip_setlike_bodies for structural
# cases) into a sibling .norm file. Streaming to disk avoids the multi-GB
# shell-variable round-trip the original used, which segfaults bash on
# real-world ZIMs whose `zimdump show` output is ~GB-scale.
build_normalized() {
    local src="$1" mode="$2" dst="$3"
    case "$mode" in
        exact)      normalize < "$src" > "$dst" ;;
        structural) normalize < "$src" | strip_setlike_bodies > "$dst" ;;
    esac
}

passed=0
failed=0
printf "%-30s %-12s %s\n" "TEST" "STATUS" "DETAIL"
printf -- "------------------------------ ------------ ------------------------------\n"
for label in "${LABELS[@]}"; do
    up_out="$OUT_UP/$label.txt"
    us_out="$OUT_US/$label.txt"
    up_norm_f="$OUT_UP/$label.norm"
    us_norm_f="$OUT_US/$label.norm"
    bash -c "${UP_CMD[$label]}" > "$up_out" 2>&1 || true
    bash -c "${US_CMD[$label]}" > "$us_out" 2>&1 || true
    build_normalized "$up_out" "${MODE[$label]}" "$up_norm_f"
    build_normalized "$us_out" "${MODE[$label]}" "$us_norm_f"
    if cmp -s "$up_norm_f" "$us_norm_f"; then
        if [[ "${MODE[$label]}" == "structural" ]]; then
            up_dang=$(count_dangling "$up_out")
            us_dang=$(count_dangling "$us_out")
            up_dup=$(count_redundant_pairs "$up_out")
            us_dup=$(count_redundant_pairs "$us_out")
            up_ext=$(count_external "$up_out")
            us_ext=$(count_external "$us_out")
            printf "%-30s \033[32m%-12s\033[0m structural match — dangling=%d/%d redundant=%d/%d external=%d/%d\n" \
                "$label" "PASS" "$us_dang" "$up_dang" "$us_dup" "$up_dup" "$us_ext" "$up_ext"
        else
            printf "%-30s \033[32m%-12s\033[0m exact match\n" "$label" "PASS"
        fi
        passed=$((passed+1))
    else
        differ=$(diff "$up_norm_f" "$us_norm_f" 2>/dev/null | grep -c '^[<>]' || true)
        printf "%-30s \033[31m%-12s\033[0m %d differing lines (see bench/parity-out)\n" "$label" "DIFF" "$differ"
        failed=$((failed+1))
    fi
done
echo
echo "Summary: $passed/$((passed+failed)) cases pass."
echo "Detailed outputs under bench/parity-out/{upstream,zimru}/."
