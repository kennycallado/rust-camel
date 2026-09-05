#!/usr/bin/env bash
# fuzz-legs.sh — select fuzz smoke legs from changed paths.
#
# Implements the ordered selection rules of the "path-filtered PR smoke
# trigger" requirement (openspec change: canonical-path-fuzzing). Consumed
# by .github/workflows/fuzz-smoke.yml.
#
# Usage:
#   printf '%s\n' <changed paths...> | scripts/fuzz-legs.sh
#       Print the selected legs, space-separated, in canonical order
#       (dsl_yaml dsl_json dsl_template dsl_parity).
#   scripts/fuzz-legs.sh --dispatch
#       Manual workflow run: ignore stdin, select all legs.
#   scripts/fuzz-legs.sh --self-test
#       Run the embedded test cases; exit 0 iff all pass.
set -euo pipefail

readonly -a ALL_TARGETS=(dsl_yaml dsl_json dsl_template dsl_parity)
SELF=${BASH_SOURCE[0]}

# classify <path> — print the legs selected by one changed path
# (space-separated; empty when the path is outside the trigger sets).
# Rules are ordered; the first match wins for that path, and the caller
# unions selections across paths.
classify() {
  local path=$1 target
  # Rule 1: fuzz/seeds/<target>/** selects that target.
  for target in "${ALL_TARGETS[@]}"; do
    if [[ $path == "fuzz/seeds/$target/"* ]]; then
      printf '%s ' "$target"
      return
    fi
  done
  case $path in
    # Rules 2-4: front-end changes select their leg plus the parity leg.
    crates/camel-dsl/src/yaml.rs)
      printf 'dsl_yaml dsl_parity ' ;;
    crates/camel-dsl/src/json.rs)
      printf 'dsl_json dsl_parity ' ;;
    crates/camel-dsl/src/template/*)
      printf 'dsl_template dsl_parity ' ;;
    # Rule 5: any other path matching the workflow trigger selects all legs.
    crates/camel-dsl/* | fuzz/* | scripts/xtask/* | .github/workflows/fuzz-smoke.yml)
      printf '%s ' "${ALL_TARGETS[@]}" ;;
  esac
  # Rule 6: paths outside the trigger sets select nothing.
}

# Read changed paths from stdin, union their selections, and print the
# selected legs in canonical order on one line.
select_targets() {
  local -A want=()
  local path target
  # `|| [[ -n $path ]]` keeps the final line when stdin has no trailing
  # newline (read fills the variable but reports EOF).
  while IFS= read -r path || [[ -n $path ]]; do
    [[ -n $path ]] || continue
    for target in $(classify "$path"); do
      want[$target]=1
    done
  done
  local out=()
  for target in "${ALL_TARGETS[@]}"; do
    [[ -n ${want[$target]:-} ]] && out+=("$target")
  done
  printf '%s\n' "${out[*]}"
}

self_test() {
  local pass=0 fail=0
  expect() {
    local name=$1 expected=$2 input=$3
    shift 3
    local got
    got=$(printf '%s' "$input" | "$SELF" "$@")
    if [[ $got == "$expected" ]]; then
      printf 'PASS %s\n' "$name"
      pass=$((pass + 1))
    else
      printf 'FAIL %s\n  expected: %q\n  got:      %q\n' "$name" "$expected" "$got"
      fail=$((fail + 1))
    fi
  }

  local ALL='dsl_yaml dsl_json dsl_template dsl_parity'
  expect wrapper-change-selects-all "$ALL" 'scripts/xtask/src/fuzz.rs'
  expect json-frontend-selects-json-and-parity 'dsl_json dsl_parity' \
    'crates/camel-dsl/src/json.rs'
  expect yaml-frontend-selects-yaml-and-parity 'dsl_yaml dsl_parity' \
    'crates/camel-dsl/src/yaml.rs'
  expect template-selects-template-and-parity 'dsl_template dsl_parity' \
    'crates/camel-dsl/src/template/materializer.rs'
  expect seed-dir-selects-own-target 'dsl_json' \
    'fuzz/seeds/dsl_json/valid_minimal.json'
  expect shared-downstream-selects-all "$ALL" \
    'crates/camel-dsl/src/compile.rs'
  expect dispatch-selects-all "$ALL" '' --dispatch
  expect mixed-changes-union 'dsl_yaml dsl_json dsl_parity' \
    $'crates/camel-dsl/src/yaml.rs\nfuzz/seeds/dsl_json/valid_minimal.json\n'
  expect non-trigger-path-ignored 'dsl_json dsl_parity' \
    $'crates/camel-dsl/src/json.rs\nREADME.md'

  printf '%d passed, %d failed\n' "$pass" "$fail"
  ((fail == 0))
}

case ${1:-} in
  --dispatch)
    printf '%s\n' "${ALL_TARGETS[*]}" ;;
  --self-test)
    self_test ;;
  '')
    select_targets ;;
  *)
    printf 'usage: %s [--dispatch | --self-test] (stdin: changed paths)\n' \
      "$SELF" >&2
    exit 2 ;;
esac
