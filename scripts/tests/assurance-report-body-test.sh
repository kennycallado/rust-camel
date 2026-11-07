#!/usr/bin/env bash
# shellcheck shell=bash
#
# assurance-report-body-test.sh — offline scenario battery for
# scripts/assurance-report-body.sh and the "Publish issue" step of
# .github/workflows/assurance-monthly.yml (bd rc-g3nls).
#
# No network access is performed. A stub `gh` on PATH serves
# `gh issue list`, `gh issue view` and `gh issue create` and records every
# call. Any invocation the stub does not implement exits 64 and is appended to
# $STUB_UNEXPECTED_LOG, which is asserted empty after every publish scenario.
#
# Usage: bash scripts/tests/assurance-report-body-test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BODY_SCRIPT="$REPO_ROOT/scripts/assurance-report-body.sh"
WORKFLOW="$REPO_ROOT/.github/workflows/assurance-monthly.yml"

# Survivor .name values baked into the shared outcomes fixture. Sorted order is
# SURVIVOR_A then SURVIVOR_B.
SURVIVOR_A="crates/camel-core/src/a.rs:10: replace + with -"
SURVIVOR_B="crates/camel-core/src/b.rs:20: replace true with false"

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

# Per-test working directory, set by the runner before each test function.
WORK=""

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

# Valid cargo-mutants outcomes: 2 missed, 1 caught, 1 unviable.
make_outcomes() {
  local dir="$1"
  mkdir -p "$dir"
  cat >"$dir/outcomes.json" <<JSON
{"outcomes":[
  {"summary":"MissedMutant","scenario":{"Mutant":{"name":"$SURVIVOR_A"}}},
  {"summary":"MissedMutant","scenario":{"Mutant":{"name":"$SURVIVOR_B"}}},
  {"summary":"CaughtMutant","scenario":{"Mutant":{"name":"crates/camel-core/src/c.rs:30: replace 1 with 0"}}},
  {"summary":"UnviableMutant","scenario":{"Mutant":{"name":"crates/camel-core/src/d.rs:40: replace 2 with 3"}}}
]}
JSON
}

# Valid outcomes with ZERO MissedMutant entries (empty survivor marker set).
make_outcomes_zero_missed() {
  local dir="$1"
  mkdir -p "$dir"
  cat >"$dir/outcomes.json" <<'JSON'
{"outcomes":[
  {"summary":"CaughtMutant","scenario":{"Mutant":{"name":"crates/camel-core/src/c.rs:30: replace 1 with 0"}}},
  {"summary":"UnviableMutant","scenario":{"Mutant":{"name":"crates/camel-core/src/d.rs:40: replace 2 with 3"}}}
]}
JSON
}

make_fuzz() {
  local dir="$1"
  mkdir -p "$dir"
  cat >"$dir/fuzz-findings.txt" <<'TXT'
CLEAN camel-core-parse
CRASH-OR-ERROR camel-core-route exit=134 (see deep-camel-core-route.log excerpt below)
TXT
}

make_rationale() {
  local dir="$1"
  mkdir -p "$dir"
  printf 'Rotated camel-core/src/a.rs: this window targets parser boundaries.\n' \
    >"$dir/rotation-rationale.txt"
}

# next_candidates lives at $dir/.github/assurance-monthly.json, matching the
# path the report script reads under ARTIFACTS_ROOT.
make_config() {
  local dir="$1"
  mkdir -p "$dir/.github"
  cat >"$dir/.github/assurance-monthly.json" <<'JSON'
{"next_candidates":["a","b"]}
JSON
}

# LCA-shaped artifact tree as download-artifact materialises it:
#   art-mutants-baseline/mutants.out/<run>/outcomes.json
#   art-mutants-rotating/mut-wt/target-mutants/<run>/{outcomes,rotation-rationale}
#   art-fuzz-deep/<state>/fuzz-findings.txt
make_baseline_artifact() {
  local root="$1"
  make_outcomes "$root/art-mutants-baseline/mutants.out/run-2026-11"
}

make_rotating_artifact() {
  local root="$1"
  local dir="$root/art-mutants-rotating/mut-wt/target-mutants/run-2026-11"
  make_outcomes "$dir"
  make_rationale "$dir"
}

make_fuzz_artifact() {
  local root="$1"
  make_fuzz "$root/art-fuzz-deep/deep-state"
}

make_config_artifact() {
  local root="$1"
  make_config "$root"
}

# Complete evidence tree: baseline outcomes + rotating outcomes/rationale +
# fuzz findings + rotation config.
make_all_present() {
  local root="$1"
  make_baseline_artifact "$root"
  make_rotating_artifact "$root"
  make_fuzz_artifact "$root"
  make_config_artifact "$root"
}

# ---------------------------------------------------------------------------
# Stub gh
# ---------------------------------------------------------------------------

write_gh_stub() {
  local stub="$1"
  mkdir -p "$(dirname "$stub")"
  cat >"$stub" <<'STUB'
#!/usr/bin/env bash
# Stub `gh` for scripts/tests/assurance-report-body-test.sh. Never touches the
# network. Behaviour is driven by $STUB_PREV_EXISTS / $STUB_PREV_NUMBER.
set -euo pipefail

unexpected() {
  printf '%s\n' "$*" >>"$STUB_UNEXPECTED_LOG"
  printf 'stub gh: %s\n' "$*" >&2
  exit 64
}

has_arg() {
  local needle="$1" arg
  shift
  for arg in "$@"; do
    [[ "$arg" == "$needle" ]] && return 0
  done
  return 1
}

if [[ "${1:-}" == "issue" && "${2:-}" == "list" ]]; then
  has_arg --state "$@" || unexpected "issue list without --state: $*"
  has_arg --search "$@" || unexpected "issue list without --search: $*"
  has_arg --json "$@" || unexpected "issue list without --json: $*"
  if [[ "${STUB_PREV_EXISTS:-0}" == "1" ]]; then
    printf '%s\n' "${STUB_PREV_NUMBER:-123}"
  else
    printf '\n'
  fi
  exit 0
fi

if [[ "${1:-}" == "issue" && "${2:-}" == "view" ]]; then
  [[ "${STUB_PREV_EXISTS:-0}" == "1" ]] || unexpected "issue view with no previous issue"
  [[ "${3:-}" == "${STUB_PREV_NUMBER:-123}" ]] || unexpected "unexpected view number ${3:-<none>}"
  has_arg --json "$@" || unexpected "issue view without --json: $*"
  has_arg body "$@" || unexpected "issue view without body field: $*"
  cat <<'BODY'
# assurance-monthly: 2026-10

<!--baseline-data-start-->
old/crate/src/old.rs:1: replace old with new
<!--baseline-data-end-->
BODY
  exit 0
fi

if [[ "${1:-}" == "issue" && "${2:-}" == "create" ]]; then
  title=""
  body_file=""
  args=("${@:3}")
  i=0
  while (( i < ${#args[@]} )); do
    case "${args[$i]}" in
      --title)
        title="${args[$((i + 1))]:-}"
        i=$((i + 2))
        ;;
      --body-file)
        body_file="${args[$((i + 1))]:-}"
        i=$((i + 2))
        ;;
      *)
        unexpected "unexpected create argument ${args[$i]}"
        ;;
    esac
  done
  [[ -n "$title" ]] || unexpected "issue create without --title"
  [[ -n "$body_file" ]] || unexpected "issue create without --body-file"
  printf 'title %s\n' "$title" >>"$STUB_CREATE_LOG"
  printf 'body-file %s\n' "$body_file" >>"$STUB_CREATE_LOG"
  cp "$body_file" "$STUB_CREATED_BODY"
  exit 0
fi

unexpected "unhandled invocation: $*"
STUB
  chmod +x "$stub"
}

# ---------------------------------------------------------------------------
# Publish-step extraction
# ---------------------------------------------------------------------------

# Extract the shell of the "Publish issue" step from the workflow.
# usage: extract_publish_step <workflow> <out.sh>
extract_publish_step() {
  local workflow="$1" out="$2"
  local raw="$WORK/publish-step.raw"
  local start base

  awk '
    seen && /^[[:space:]]*- name:/ { exit }
    /name: Publish issue/ { seen = 1 }
    seen { print }
  ' "$workflow" >"$raw"

  if ! grep -q 'name: Publish issue' "$raw"; then
    FAIL_DETAIL="workflow has no 'name: Publish issue' step"
    return 1
  fi

  start="$(awk '/^[[:space:]]*run: [|]$/ { print NR; exit }' "$raw")"
  if [[ -z "$start" ]]; then
    FAIL_DETAIL="publish step has no 'run: |' block"
    return 1
  fi

  base="$(awk -v s="$start" 'NR > s && NF > 0 { match($0, /^ */); print RLENGTH; exit }' "$raw")"
  if [[ -z "$base" ]]; then
    FAIL_DETAIL="could not detect publish run-block indentation"
    return 1
  fi

  awk -v s="$start" -v b="$base" 'NR > s { print substr($0, b + 1) }' "$raw" >"$out"
}

# ---------------------------------------------------------------------------
# Harness
# ---------------------------------------------------------------------------

STATUS=0
PUBLISH_STATUS=0
FAIL_DETAIL=""
SKIP_DETAIL=""
# Body file the marker assertions read. Defaults to the plain run_body output;
# publish scenarios point it at their RUNNER_TEMP/issue-body.md.
BODY_SOURCE=""

# Run the body script for the current test directory.
# usage: run_body_to <artifacts-root> <body-out>
run_body_to() {
  local artifacts="$1" body="$2"
  : >"$WORK/env.out"

  set +e
  ARTIFACTS_ROOT="$artifacts" \
    OUT_BODY="$body" \
    OUT_ENV="$WORK/env.out" \
    FUZZ_RESULT="${SCEN_FUZZ_RESULT:-unknown}" \
    BASE_RESULT="${SCEN_BASE_RESULT:-unknown}" \
    ROT_RESULT="${SCEN_ROT_RESULT:-unknown}" \
    bash "$BODY_SCRIPT" >"$WORK/stdout.log" 2>"$WORK/stderr.log"
  STATUS=$?
  set -e
}

run_body() {
  BODY_SOURCE="$WORK/body.md"
  run_body_to "$1" "$BODY_SOURCE"
}

# Run the extracted publish shell with a scenario-driven stub on PATH.
# usage: run_publish <runner-temp> <prev-exists 0|1> [prev-number]
run_publish() {
  local runner_temp="$1" prev_exists="$2" prev_number="${3:-123}"

  # Never let PATH fall through to a real, authenticated gh.
  if [[ ! -x "$WORK/bin/gh" ]]; then
    FAIL_DETAIL="gh stub missing: $WORK/bin/gh"
    return 1
  fi

  : >"$WORK/gh-create.log"
  : >"$WORK/gh-unexpected.log"
  rm -f "$WORK/created-body.md"

  set +e
  PATH="$WORK/bin:$PATH" \
    STAMP="$STAMP" \
    OUT_BASE="$OUT_BASE" \
    OUT_ROT="$OUT_ROT" \
    FUZZF="$FUZZF" \
    INFRA_MISSING="$INFRA_MISSING" \
    RUNNER_TEMP="$runner_temp" \
    STUB_PREV_EXISTS="$prev_exists" \
    STUB_PREV_NUMBER="$prev_number" \
    STUB_CREATE_LOG="$WORK/gh-create.log" \
    STUB_CREATED_BODY="$WORK/created-body.md" \
    STUB_UNEXPECTED_LOG="$WORK/gh-unexpected.log" \
    bash "$WORK/publish-step.sh" >"$WORK/publish.stdout" 2>"$WORK/publish.stderr"
  PUBLISH_STATUS=$?
  set -e
}

assert_status() {
  local want="$1"
  if [[ "$STATUS" -ne "$want" ]]; then
    FAIL_DETAIL="expected exit $want, got $STATUS; stdout=[$(tr '\n' '|' <"$WORK/stdout.log")] stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

assert_publish_status() {
  local want="$1"
  if [[ "$PUBLISH_STATUS" -ne "$want" ]]; then
    FAIL_DETAIL="publish expected exit $want, got $PUBLISH_STATUS; stdout=[$(tr '\n' '|' <"$WORK/publish.stdout")] stderr=[$(tr '\n' '|' <"$WORK/publish.stderr")]"
    return 1
  fi
}

assert_body_line() {
  local want="$1"
  if ! grep -Fxq -- "$want" "$WORK/body.md"; then
    FAIL_DETAIL="body missing exact line '$want'; body=[$(tr '\n' '|' <"$WORK/body.md")]"
    return 1
  fi
}

assert_body_contains() {
  local want="$1"
  if ! grep -Fq -- "$want" "$WORK/body.md"; then
    FAIL_DETAIL="body missing '$want'; body=[$(tr '\n' '|' <"$WORK/body.md")]"
    return 1
  fi
}

assert_body_not_contains() {
  local want="$1"
  if grep -Fq -- "$want" "$WORK/body.md"; then
    FAIL_DETAIL="body must not contain '$want'"
    return 1
  fi
}

assert_stderr_contains() {
  local want="$1"
  if ! grep -Fq -- "$want" "$WORK/stderr.log"; then
    FAIL_DETAIL="stderr missing '$want'; stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

# Assert zero lines containing MISSING (all evidence present).
assert_no_missing_lines() {
  if grep -Fq 'MISSING' "$WORK/body.md"; then
    FAIL_DETAIL="body must not contain MISSING lines; body=[$(tr '\n' '|' <"$WORK/body.md")]"
    return 1
  fi
}

# Read a value from the script's env file (GITHUB_ENV semantics).
env_value() {
  local key="$1"
  sed -n "s/^${key}=//p" "$WORK/env.out" | tail -n 1
}

assert_env_value() {
  local key="$1" want="$2" actual
  actual="$(env_value "$key")"
  if [[ "$actual" != "$want" ]]; then
    FAIL_DETAIL="env $key expected '$want', got '$actual'; env=[$(tr '\n' '|' <"$WORK/env.out")]"
    return 1
  fi
}

assert_body_starts_with_stamp_heading() {
  local first
  first="$(head -n 1 "$WORK/body.md")"
  if [[ "$first" != "# assurance-monthly: "* ]]; then
    FAIL_DETAIL="body must start with '# assurance-monthly: <stamp>', got '$first'"
    return 1
  fi
}

assert_markers_present() {
  if [[ ! -f "${BODY_SOURCE:-}" ]]; then
    FAIL_DETAIL="marker source body not found: ${BODY_SOURCE:-<unset>}"
    return 1
  fi
  if ! grep -Fxq -- '<!--baseline-data-start-->' "$BODY_SOURCE" ||
    ! grep -Fxq -- '<!--baseline-data-end-->' "$BODY_SOURCE"; then
    FAIL_DETAIL="body missing baseline-data markers"
    return 1
  fi
}

# Assert the lines between the baseline-data markers equal the given list
# (already in sorted .name order).
assert_markers_exactly() {
  local actual="$WORK/markers.actual"
  local expected="$WORK/markers.expected"
  if [[ ! -f "${BODY_SOURCE:-}" ]]; then
    FAIL_DETAIL="marker source body not found: ${BODY_SOURCE:-<unset>}"
    return 1
  fi
  awk '/<!--baseline-data-start-->/{inside=1; next} /<!--baseline-data-end-->/{inside=0} inside' \
    "$BODY_SOURCE" >"$actual"
  if [[ $# -gt 0 ]]; then
    printf '%s\n' "$@" >"$expected"
  else
    : >"$expected"
  fi
  if ! diff -u "$expected" "$actual" >"$WORK/markers.diff" 2>&1; then
    FAIL_DETAIL="baseline marker content mismatch: $(tr '\n' '|' <"$WORK/markers.diff")"
    return 1
  fi
}

assert_create_title() {
  local want="title $1"
  if ! grep -Fxq -- "$want" "$WORK/gh-create.log"; then
    FAIL_DETAIL="gh issue create title mismatch; expected '$want'; log=[$(tr '\n' '|' <"$WORK/gh-create.log")]"
    return 1
  fi
}

assert_create_reached() {
  if ! grep -q '^title ' "$WORK/gh-create.log"; then
    FAIL_DETAIL="gh issue create was never reached; publish stderr=[$(tr '\n' '|' <"$WORK/publish.stderr")]"
    return 1
  fi
}

assert_unexpected_empty() {
  if [[ -s "$WORK/gh-unexpected.log" ]]; then
    FAIL_DETAIL="unexpected gh invocation recorded: $(tr '\n' '|' <"$WORK/gh-unexpected.log")"
    return 1
  fi
}

assert_created_body_contains() {
  local want="$1"
  if ! grep -Fq -- "$want" "$WORK/created-body.md"; then
    FAIL_DETAIL="created body missing '$want'; body=[$(tr '\n' '|' <"$WORK/created-body.md")]"
    return 1
  fi
}

assert_created_body_not_contains() {
  local want="$1"
  if grep -Fq -- "$want" "$WORK/created-body.md"; then
    FAIL_DETAIL="created body must not contain '$want'"
    return 1
  fi
}

# Created body must begin with the pre-publish script body (the publish step
# may append the survivor diff to the same file).
assert_created_body_prefix() {
  local expected="$1"
  local bytes
  bytes="$(wc -c <"$expected")"
  if ! head -c "$bytes" "$WORK/created-body.md" | cmp -s - "$expected"; then
    FAIL_DETAIL="created body does not start with the script body"
    return 1
  fi
}

assert_created_body_equals() {
  local expected="$1"
  if ! cmp -s "$expected" "$WORK/created-body.md"; then
    FAIL_DETAIL="created body differs from the script body"
    return 1
  fi
}

# ---------------------------------------------------------------------------
# Scenarios: body construction
# ---------------------------------------------------------------------------

t_all_present() {
  make_all_present "$WORK/artifacts"
  SCEN_FUZZ_RESULT=failure
  SCEN_BASE_RESULT=success
  SCEN_ROT_RESULT=success
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  local counts
  counts="$(grep -Fc 'tested=4 caught=1 missed=2 unviable=1' "$BODY_SOURCE" || true)"
  if [[ "$counts" -ne 2 ]]; then
    FAIL_DETAIL="expected counts line exactly twice (baseline + rotating), got $counts"
    return 1
  fi
  assert_no_missing_lines || return 1
  assert_body_line "## Triage checklist (dedup is mandatory)" || return 1
  assert_body_contains '`discovered-from:rc-eba8`' || return 1
  assert_body_line "- Kill rates are informational: no threshold, no gate." || return 1
  assert_markers_present || return 1
  assert_markers_exactly "$SURVIVOR_A" "$SURVIVOR_B" || return 1
  assert_env_value INFRA_MISSING "" || return 1
  local out_base
  out_base="$(env_value OUT_BASE)"
  if [[ -z "$out_base" || "$out_base" != */outcomes.json ]]; then
    FAIL_DETAIL="env OUT_BASE expected a populated path ending /outcomes.json, got '$out_base'"
    return 1
  fi
}

t_baseline_missing() {
  make_rotating_artifact "$WORK/artifacts"
  make_fuzz_artifact "$WORK/artifacts"
  make_config_artifact "$WORK/artifacts"
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  assert_body_line "## Mutants baseline: MISSING OUTCOMES (infra failure)" || return 1
  assert_body_line "## Mutants rotating" || return 1
  assert_body_line "tested=4 caught=1 missed=2 unviable=1" || return 1
  assert_env_value INFRA_MISSING baseline || return 1
}

t_fuzz_missing() {
  make_baseline_artifact "$WORK/artifacts"
  make_rotating_artifact "$WORK/artifacts"
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  assert_body_line "MISSING FUZZ FINDINGS (infra failure)" || return 1
  assert_env_value INFRA_MISSING fuzz || return 1
}

t_rotation_missing() {
  make_baseline_artifact "$WORK/artifacts"
  make_outcomes "$WORK/artifacts/art-mutants-rotating/mut-wt/target-mutants/run-2026-11"
  make_fuzz_artifact "$WORK/artifacts"
  make_config_artifact "$WORK/artifacts"
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  assert_body_line "MISSING ROTATION RATIONALE (infra failure)" || return 1
  assert_env_value INFRA_MISSING rotating || return 1
}

t_zero_artifacts() {
  mkdir -p "$WORK/artifacts"
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  assert_body_starts_with_stamp_heading || return 1
  assert_body_line "| fuzz-deep | unknown |" || return 1
  assert_body_line "| mutants-baseline | unknown |" || return 1
  assert_body_line "| mutants-rotating | unknown |" || return 1
  assert_body_line "MISSING FUZZ FINDINGS (infra failure)" || return 1
  assert_body_line "## Mutants baseline: MISSING OUTCOMES (infra failure)" || return 1
  assert_body_line "## Mutants rotating: MISSING OUTCOMES (infra failure)" || return 1
  assert_body_line "MISSING ROTATION RATIONALE (infra failure)" || return 1
  assert_env_value INFRA_MISSING "baseline rotating fuzz" || return 1
  assert_env_value OUT_BASE "" || return 1
}

t_markers_preserved() {
  make_all_present "$WORK/artifacts"
  run_body "$WORK/artifacts"
  assert_status 0 || return 1
  assert_markers_present || return 1
  assert_markers_exactly "$SURVIVOR_A" "$SURVIVOR_B" || return 1
}

t_malformed_present_file() {
  local dir="$WORK/artifacts/art-mutants-baseline/mutants.out/run-2026-11"
  mkdir -p "$dir"
  printf 'not-json{{\n' >"$dir/outcomes.json"
  run_body "$WORK/artifacts"
  assert_status 4 || return 1
  assert_stderr_contains "is not valid JSON" || return 1
}

t_unwritable_output() {
  if [[ "$(id -u)" -eq 0 ]]; then
    SKIP_DETAIL="running as root: chmod 0555 is still writable"
    return 77
  fi
  mkdir -p "$WORK/ro"
  chmod 0555 "$WORK/ro"

  set +e
  ARTIFACTS_ROOT="$WORK/artifacts" \
    OUT_BODY="$WORK/ro/body.md" \
    OUT_ENV="$WORK/env.out" \
    bash "$BODY_SCRIPT" >"$WORK/stdout.log" 2>"$WORK/stderr.log"
  STATUS=$?
  set -e

  chmod 0755 "$WORK/ro"
  assert_status 3 || return 1
  assert_stderr_contains "OUT_BODY" || return 1
  assert_stderr_contains "$WORK/ro" || return 1
}

t_usage_errors() {
  set +e
  env -u OUT_ENV \
    OUT_BODY="$WORK/body.md" \
    ARTIFACTS_ROOT="$WORK/artifacts" \
    bash "$BODY_SCRIPT" >"$WORK/stdout.log" 2>"$WORK/stderr.log"
  STATUS=$?
  set -e
  assert_status 2 || return 1
  assert_stderr_contains "OUT_ENV" || return 1
  assert_stderr_contains "required" || return 1
}

# ---------------------------------------------------------------------------
# Scenarios: publish step
# ---------------------------------------------------------------------------

# Shared prologue: run the real body script against the current fixture and
# capture its STAMP/evidence env as a subsequent workflow step would see it.
prepare_publish_env() {
  local runner_temp="$1"

  STAMP="$(env_value STAMP)"
  # Wiring check: the issue title ends with this value. If the script ever
  # stops emitting STAMP, publication must fail here rather than ship an
  # empty title suffix.
  if [[ ! "$STAMP" =~ ^[0-9]{4}-[0-9]{2}$ ]]; then
    FAIL_DETAIL="env STAMP not populated: '$STAMP'"
    return 1
  fi
  OUT_BASE="$(env_value OUT_BASE)"
  OUT_ROT="$(env_value OUT_ROT)"
  FUZZF="$(env_value FUZZF)"
  INFRA_MISSING="$(env_value INFRA_MISSING)"

  cp "$runner_temp/issue-body.md" "$WORK/expected-body.md"
  extract_publish_step "$WORKFLOW" "$WORK/publish-step.sh"
}

t_publish_zero_artifacts_prev_exists() {
  local runner_temp="$WORK/runner-temp"
  mkdir -p "$runner_temp" "$WORK/artifacts"
  write_gh_stub "$WORK/bin/gh"

  run_body_to "$WORK/artifacts" "$runner_temp/issue-body.md"
  assert_status 0 || return 1
  prepare_publish_env "$runner_temp" || return 1
  if [[ "$INFRA_MISSING" != "baseline rotating fuzz" ]]; then
    FAIL_DETAIL="expected INFRA_MISSING='baseline rotating fuzz', got '$INFRA_MISSING'"
    return 1
  fi

  run_publish "$runner_temp" 1 123
  assert_publish_status 0 || return 1
  assert_create_reached || return 1
  assert_create_title "assurance-monthly: $STAMP" || return 1
  assert_created_body_prefix "$WORK/expected-body.md" || return 1
  assert_created_body_contains "## Baseline survivor diff" || return 1
  assert_unexpected_empty || return 1
}

t_publish_zero_survivors_prev_exists() {
  local runner_temp="$WORK/runner-temp"
  mkdir -p "$runner_temp"
  write_gh_stub "$WORK/bin/gh"

  make_outcomes_zero_missed \
    "$WORK/artifacts/art-mutants-baseline/mutants.out/run-2026-11"
  make_rotating_artifact "$WORK/artifacts"
  make_fuzz_artifact "$WORK/artifacts"
  make_config_artifact "$WORK/artifacts"

  run_body_to "$WORK/artifacts" "$runner_temp/issue-body.md"
  assert_status 0 || return 1
  BODY_SOURCE="$runner_temp/issue-body.md"
  assert_markers_present || return 1
  assert_markers_exactly || return 1
  prepare_publish_env "$runner_temp" || return 1
  assert_env_value INFRA_MISSING "" || return 1

  run_publish "$runner_temp" 1 123
  assert_publish_status 0 || return 1
  assert_create_reached || return 1
  assert_create_title "assurance-monthly: $STAMP" || return 1
  assert_created_body_prefix "$WORK/expected-body.md" || return 1
  assert_unexpected_empty || return 1
}

t_publish_no_prev() {
  local runner_temp="$WORK/runner-temp"
  mkdir -p "$runner_temp"
  write_gh_stub "$WORK/bin/gh"

  make_all_present "$WORK/artifacts"

  run_body_to "$WORK/artifacts" "$runner_temp/issue-body.md"
  assert_status 0 || return 1
  prepare_publish_env "$runner_temp" || return 1

  run_publish "$runner_temp" 0
  assert_publish_status 0 || return 1
  assert_create_reached || return 1
  assert_create_title "assurance-monthly: $STAMP" || return 1
  assert_created_body_equals "$WORK/expected-body.md" || return 1
  assert_created_body_not_contains "## Baseline survivor diff" || return 1
  assert_unexpected_empty || return 1
}

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

main() {
  if [[ ! -f "$BODY_SCRIPT" ]]; then
    printf 'FATAL: script under test not found: %s\n' "$BODY_SCRIPT" >&2
    exit 2
  fi
  if [[ ! -f "$WORKFLOW" ]]; then
    printf 'FATAL: workflow not found: %s\n' "$WORKFLOW" >&2
    exit 2
  fi

  local tests=(
    t_all_present
    t_baseline_missing
    t_fuzz_missing
    t_rotation_missing
    t_zero_artifacts
    t_markers_preserved
    t_malformed_present_file
    t_unwritable_output
    t_usage_errors
    t_publish_zero_artifacts_prev_exists
    t_publish_zero_survivors_prev_exists
    t_publish_no_prev
  )

  local passed=0
  local failed=0
  local skipped=0
  local name rc

  for name in "${tests[@]}"; do
    WORK="$ROOT/$name"
    mkdir -p "$WORK"

    FAIL_DETAIL=""
    SKIP_DETAIL=""
    BODY_SOURCE=""
    unset SCEN_FUZZ_RESULT SCEN_BASE_RESULT SCEN_ROT_RESULT
    unset STAMP OUT_BASE OUT_ROT FUZZF INFRA_MISSING
    rc=0
    "$name" || rc=$?

    if [[ $rc -eq 77 ]]; then
      printf 'SKIP: %s: %s\n' "$name" "${SKIP_DETAIL:-skipped}"
      skipped=$((skipped + 1))
      continue
    fi

    if [[ $rc -eq 0 && -s "$WORK/gh-unexpected.log" ]]; then
      rc=1
      FAIL_DETAIL="unexpected gh invocation recorded: $(tr '\n' '|' <"$WORK/gh-unexpected.log")"
    fi

    if [[ $rc -eq 0 ]]; then
      printf 'PASS: %s\n' "$name"
      passed=$((passed + 1))
    else
      printf 'FAIL: %s: %s\n' "$name" "${FAIL_DETAIL:-assertion failed}"
      failed=$((failed + 1))
    fi
  done

  rm -rf "$ROOT"
  trap - EXIT

  if [[ $skipped -ne 0 ]]; then
    printf '%d skipped\n' "$skipped"
  fi
  printf '%d passed, %d failed\n' "$passed" "$failed"
  if [[ $failed -ne 0 ]]; then
    exit 1
  fi
}

main "$@"
