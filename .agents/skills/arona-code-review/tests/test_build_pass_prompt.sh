#!/usr/bin/env bash

# SPDX-License-Identifier: MPL-2.0

# Test cases for scripts/build_pass_prompt.sh.
# Run via `make -C tests test_build_pass_prompt`.
set -uo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/lib.sh"
BP="$HERE/../scripts/build_pass_prompt.sh"

setup() {
    IN="$TMP/in1.txt";  printf 'REVIEW_SENTINEL_ONE\nsome diff\n' > "$IN"
    IN2="$TMP/in2.txt"; printf 'a completely different second input\n' > "$IN2"
    FIX="$TMP/repo"
    git init -q "$FIX"
    mkdir -p "$FIX/book/src/contributing" "$FIX/book/src/kernel" "$FIX/book/src/tools"
    printf 'CONTRIBUTION_SENTINEL\n' > "$FIX/CONTRIBUTING.md"
    printf 'CHECKS_SENTINEL\n' > "$FIX/book/src/contributing/checks.md"
    printf 'ORGANIZATION_SENTINEL\n' > "$FIX/book/src/contributing/code-organization.md"
    printf 'SAFETY_SENTINEL\n' > "$FIX/book/src/kernel/safety.md"
    printf 'TESTING_SENTINEL\n' > "$FIX/book/src/tools/testing.md"
}
bp()      { (cd "$FIX" && "$BP" "$@"); }
bp_code() { bp "$@" >/dev/null 2>&1; echo $?; }
# Everything through the REVIEW INPUT marker is independent of review input.
prefix()  { sed -n '1,/^===== REVIEW INPUT =====$/p'; }

test_requires_input()          { assert_eq "no args rejected"     "$(bp_code)"                      2; }
test_requires_persona()        { assert_eq "no persona rejected"  "$(bp_code "$IN")"                2; }
test_rejects_unknown_persona() { assert_eq "bad persona rejected" "$(bp_code "$IN" bogus)"          2; }
test_rejects_missing_input()   { assert_eq "missing input file"   "$(bp_code /no/such development)" 2; }
test_rejects_multiple_personas() {
    assert_eq "each pass has one persona" "$(bp_code "$IN" development security)" 2
}

test_orders_contract_persona_guidelines_input() {
    local o; o="$(bp "$IN" development)"
    assert_before "contract before persona" "$o" "Pass contract" "PERSONA: development"
    assert_before "persona before guidelines" "$o" "PERSONA: development" "CONTRIBUTION_SENTINEL"
    assert_before "guidelines before input" "$o" "TESTING_SENTINEL" "===== REVIEW INPUT ====="
    assert_before "input marker before body" "$o" "===== REVIEW INPUT =====" "REVIEW_SENTINEL_ONE"
}

test_inlines_actual_guideline_contents() {
    local o; o="$(bp "$IN" development)"
    assert_contains "contribution guide loaded" "$o" "CONTRIBUTION_SENTINEL"
    assert_contains "checks loaded" "$o" "CHECKS_SENTINEL"
    assert_contains "safety loaded" "$o" "SAFETY_SENTINEL"
    assert_contains "testing loaded" "$o" "TESTING_SENTINEL"
    assert_absent "unrelated document excluded" "$o" "ORGANIZATION_SENTINEL"
}

test_personas_receive_their_own_guidelines() {
    local o; o="$(bp "$IN" maintainability)"
    assert_contains "organization loaded" "$o" "ORGANIZATION_SENTINEL"
    assert_absent "security document excluded" "$o" "SAFETY_SENTINEL"
    o="$(bp "$IN" security)"
    assert_contains "security document loaded" "$o" "SAFETY_SENTINEL"
    assert_absent "testing document excluded" "$o" "TESTING_SENTINEL"
    o="$(bp "$IN" documentation)"
    assert_contains "testing commands loaded" "$o" "TESTING_SENTINEL"
    assert_absent "security document excluded from docs" "$o" "SAFETY_SENTINEL"
}

test_missing_guideline_fails_without_partial_prompt() {
    rm "$FIX/book/src/kernel/safety.md"
    bp "$IN" development > "$TMP/prompt" 2> "$TMP/error"
    assert_eq "missing guideline fails" "$?" 2
    assert_eq "no partial prompt emitted" "$(cat "$TMP/prompt")" ""
    assert_contains "missing document identified" "$(cat "$TMP/error")" "book/src/kernel/safety.md"
}

test_stable_prefix_is_input_independent() {
    local a b
    a="$(bp "$IN" development | prefix)"
    b="$(bp "$IN2" development | prefix)"
    assert_eq "stable prefix identical across inputs" "$a" "$b"
}

test_input_body_absent_from_prefix() {
    assert_absent "input body not in prefix" "$(bp "$IN" development | prefix)" "REVIEW_SENTINEL_ONE"
}

run_suite
