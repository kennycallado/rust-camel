#!/usr/bin/env bash
# shellcheck shell=bash
#
# verify-release-assets-test.sh — offline stub battery for
# scripts/verify-release-assets.sh (bd rc-6ep0i).
#
# No network access is performed: a stub `gh` on PATH serves the asset listings
# and records every repair upload. Any invocation the stub does not implement
# exits 64 and is appended to $VERIFY_STUB_UNEXPECTED_LOG, which the runner
# asserts empty after every scenario.
#
# Usage: bash scripts/tests/verify-release-assets-test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
VERIFY_SCRIPT="$REPO_ROOT/scripts/verify-release-assets.sh"

TAG="vTEST"
EXPECTED_UNVERIFIABLE="ERROR: release asset set for tag '$TAG' could not be verified (asset listing failed)"

ROOT="$(mktemp -d)"
trap 'rm -rf "$ROOT"' EXIT

FIXTURES="$ROOT/fixtures"
# Per-test working directory, set by the runner before each test function.
WORK=""

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------

# Fake dist set: three local files whose sizes must match the full listing.
make_dist() {
  local dist="$1"
  mkdir -p "$dist"
  dd if=/dev/zero of="$dist/camel-a.tar.gz" bs=100 count=1 status=none
  dd if=/dev/zero of="$dist/camel-a.tar.gz.sha256" bs=64 count=1 status=none
  dd if=/dev/zero of="$dist/camel-b.tar.gz" bs=200 count=1 status=none
}

# Bridge-named dist set: exercises the prefix-agnostic expected-set scan.
make_bridge_dist() {
  local dist="$1"
  mkdir -p "$dist"
  dd if=/dev/zero of="$dist/cxf-bridge-9.9.9.tar.gz" bs=100 count=1 status=none
  dd if=/dev/zero of="$dist/cxf-bridge-9.9.9.zip" bs=200 count=1 status=none
  dd if=/dev/zero of="$dist/cxf-bridge-9.9.9-SHA256SUMS" bs=64 count=1 status=none
}

write_fixtures() {
  mkdir -p "$FIXTURES"
  cat >"$FIXTURES/assets-full.json" <<'JSON'
{"assets":[
  {"name":"camel-a.tar.gz","size":100},
  {"name":"camel-a.tar.gz.sha256","size":64},
  {"name":"camel-b.tar.gz","size":200}
]}
JSON
  cat >"$FIXTURES/assets-missing-a.json" <<'JSON'
{"assets":[
  {"name":"camel-a.tar.gz.sha256","size":64},
  {"name":"camel-b.tar.gz","size":200}
]}
JSON
  cat >"$FIXTURES/assets-wrong-size.json" <<'JSON'
{"assets":[
  {"name":"camel-a.tar.gz","size":99},
  {"name":"camel-a.tar.gz.sha256","size":64},
  {"name":"camel-b.tar.gz","size":200}
]}
JSON
  cat >"$FIXTURES/assets-bridge-partial.json" <<'JSON'
{"assets":[
  {"name":"cxf-bridge-9.9.9.tar.gz","size":100},
  {"name":"cxf-bridge-9.9.9-SHA256SUMS","size":64}
]}
JSON
}

# ---------------------------------------------------------------------------
# Stub gh
# ---------------------------------------------------------------------------

write_stub() {
  local stub="$1"
  mkdir -p "$(dirname "$stub")"
  cat >"$stub" <<'STUB'
#!/usr/bin/env bash
# Stub `gh` for scripts/tests/verify-release-assets-test.sh. Never touches the
# network. Behaviour is driven by $VERIFY_TEST_SCENARIO.
set -euo pipefail

scenario="${VERIFY_TEST_SCENARIO:-success}"

upload_log_has_a() {
  [[ -f "$VERIFY_STUB_UPLOAD_LOG" ]] && grep -qx 'camel-a.tar.gz' "$VERIFY_STUB_UPLOAD_LOG"
}

# Record and reject an invocation the stub does not implement. Every exit-64
# path routes through here so $VERIFY_STUB_UNEXPECTED_LOG is authoritative.
unexpected() {
  printf '%s\n' "$*" >>"$VERIFY_STUB_UNEXPECTED_LOG"
  printf 'stub gh: %s\n' "$*" >&2
  exit 64
}

if [[ "${1:-}" == "release" && "${2:-}" == "view" ]]; then
  [[ "${3:-}" == "$VERIFY_TEST_TAG" ]] || {
    unexpected "unexpected view tag ${3:-<none>}"
  }
  [[ "${4:-}" == "--json" && "${5:-}" == "assets" ]] || {
    unexpected "unexpected view arguments: $*"
  }
  printf 'view\n' >>"$VERIFY_STUB_VIEW_COUNT"
  views="$(wc -l <"$VERIFY_STUB_VIEW_COUNT" | tr -d '[:space:]')"

  case "$scenario" in
    success)
      cat "$VERIFY_TEST_ASSETS_FULL"
      exit 0
      ;;
    404_then_success)
      if (( views <= ${VERIFY_TEST_404_CALLS:-1} )); then
        echo "HttpError: Not Found" >&2
        exit 1
      fi
      cat "$VERIFY_TEST_ASSETS_FULL"
      exit 0
      ;;
    persistent_404)
      echo "HttpError: Not Found" >&2
      exit 1
      ;;
    missing_a)
      cat "$VERIFY_TEST_ASSETS_MISSING"
      exit 0
      ;;
    repair_success)
      if upload_log_has_a; then
        cat "$VERIFY_TEST_ASSETS_FULL"
      else
        cat "$VERIFY_TEST_ASSETS_MISSING"
      fi
      exit 0
      ;;
    wrong_size)
      if upload_log_has_a; then
        cat "$VERIFY_TEST_ASSETS_FULL"
      else
        cat "$VERIFY_TEST_ASSETS_WRONG_SIZE"
      fi
      exit 0
      ;;
    repair_then_listing_failure)
      if upload_log_has_a; then
        echo "HttpError: Not Found" >&2
        exit 1
      fi
      cat "$VERIFY_TEST_ASSETS_MISSING"
      exit 0
      ;;
    repair_upload_failure_then_verify)
      if upload_log_has_a; then
        cat "$VERIFY_TEST_ASSETS_FULL"
      else
        cat "$VERIFY_TEST_ASSETS_MISSING"
      fi
      exit 0
      ;;
    bridge_missing)
      cat "$VERIFY_TEST_ASSETS_BRIDGE_PARTIAL"
      exit 0
      ;;
    *)
      unexpected "unknown scenario $scenario"
      ;;
  esac
fi

if [[ "${1:-}" == "release" && "${2:-}" == "upload" ]]; then
  [[ "${3:-}" == "$VERIFY_TEST_TAG" ]] || {
    unexpected "unexpected upload tag ${3:-<none>}"
  }
  saw_clobber=0
  for arg in "${@:4}"; do
    if [[ "$arg" == "--clobber" ]]; then
      saw_clobber=1
      continue
    fi
    printf '%s\n' "$(basename "$arg")" >>"$VERIFY_STUB_UPLOAD_LOG"
  done
  if [[ $saw_clobber -ne 1 ]]; then
    unexpected "upload without --clobber: $*"
  fi
  if [[ "$scenario" == "repair_upload_failure_then_verify" ]]; then
    exit 1
  fi
  exit 0
fi

unexpected "unhandled invocation: $*"
STUB
  chmod +x "$stub"
}

# ---------------------------------------------------------------------------
# Harness
# ---------------------------------------------------------------------------

STATUS=0
FAIL_DETAIL=""

# Run the script under test for the current test directory.
# usage: run_verify <max-attempts> <repair-max-attempts>
run_verify() {
  local max_attempts="$1"
  local repair_max_attempts="$2"

  : >"$WORK/uploads.log"
  : >"$WORK/views.count"
  : >"$WORK/unexpected.log"

  set +e
  PATH="$WORK/bin:$PATH" \
    VERIFY_STUB_UPLOAD_LOG="$WORK/uploads.log" \
    VERIFY_STUB_VIEW_COUNT="$WORK/views.count" \
    VERIFY_STUB_UNEXPECTED_LOG="$WORK/unexpected.log" \
    VERIFY_TEST_TAG="$TAG" \
    VERIFY_TEST_ASSETS_FULL="$FIXTURES/assets-full.json" \
    VERIFY_TEST_ASSETS_MISSING="$FIXTURES/assets-missing-a.json" \
    VERIFY_TEST_ASSETS_WRONG_SIZE="$FIXTURES/assets-wrong-size.json" \
    VERIFY_SLEEP_SECS=0 \
    VERIFY_REPAIR_SLEEP_SECS=0 \
    VERIFY_MAX_ATTEMPTS="$max_attempts" \
    VERIFY_REPAIR_MAX_ATTEMPTS="$repair_max_attempts" \
    bash "$VERIFY_SCRIPT" "$TAG" "$WORK/dist" \
    >"$WORK/stdout.log" 2>"$WORK/stderr.log"
  STATUS=$?
  set -e
}

assert_status() {
  local want="$1"
  if [[ "$STATUS" -ne "$want" ]]; then
    FAIL_DETAIL="expected exit $want, got $STATUS; stdout=[$(tr '\n' '|' <"$WORK/stdout.log")] stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

assert_stdout_line() {
  local want="$1"
  if ! grep -Fxq -- "$want" "$WORK/stdout.log"; then
    FAIL_DETAIL="stdout missing exact line '$want'; stdout=[$(tr '\n' '|' <"$WORK/stdout.log")]"
    return 1
  fi
}

assert_stderr_line() {
  local want="$1"
  if ! grep -Fxq -- "$want" "$WORK/stderr.log"; then
    FAIL_DETAIL="stderr missing exact line '$want'; stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

assert_no_missing_lines() {
  if grep -q '^MISSING:' "$WORK/stderr.log"; then
    FAIL_DETAIL="stderr must not contain MISSING lines; stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

# Assert the stderr MISSING set is exactly one line, `MISSING: <basename>`.
assert_single_missing_line() {
  local want="MISSING: $1"
  local count
  count="$(grep -c '^MISSING:' "$WORK/stderr.log" || true)"
  if [[ "$count" -ne 1 ]]; then
    FAIL_DETAIL="expected exactly 1 MISSING line, got $count; stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
  if ! grep -Fxq -- "$want" "$WORK/stderr.log"; then
    FAIL_DETAIL="expected MISSING line '$want'; stderr=[$(tr '\n' '|' <"$WORK/stderr.log")]"
    return 1
  fi
}

# Assert the recorded upload set (basenames, sorted) equals the given set.
assert_uploads_exactly() {
  local actual="$WORK/uploads.actual"
  local expected="$WORK/uploads.expected"
  sort "$WORK/uploads.log" >"$actual"
  if [[ $# -gt 0 ]]; then
    printf '%s\n' "$@" | sort >"$expected"
  else
    : >"$expected"
  fi
  if ! diff -u "$expected" "$actual" >"$WORK/uploads.diff" 2>&1; then
    FAIL_DETAIL="upload set mismatch: $(tr '\n' '|' <"$WORK/uploads.diff")"
    return 1
  fi
}

assert_view_count_gt() {
  local bound="$1"
  local views
  views="$(wc -l <"$WORK/views.count" | tr -d '[:space:]')"
  if [[ "$views" -le "$bound" ]]; then
    FAIL_DETAIL="expected more than $bound listing views (proves post-upload re-verification), got $views"
    return 1
  fi
}

# ---------------------------------------------------------------------------
# Scenarios
# ---------------------------------------------------------------------------

t_success() {
  export VERIFY_TEST_SCENARIO=success
  run_verify 3 3
  assert_status 0 || return 1
  assert_stdout_line "Verified release assets for tag '$TAG'" || return 1
  assert_uploads_exactly || return 1
}

t_transient_404_then_success() {
  export VERIFY_TEST_SCENARIO=404_then_success
  export VERIFY_TEST_404_CALLS=1
  run_verify 3 3
  assert_status 0 || return 1
  assert_stdout_line "Verified release assets for tag '$TAG'" || return 1
  assert_uploads_exactly || return 1
}

t_persistent_404_unverifiable() {
  export VERIFY_TEST_SCENARIO=persistent_404
  run_verify 2 2
  assert_status 1 || return 1
  assert_stderr_line "$EXPECTED_UNVERIFIABLE" || return 1
  assert_no_missing_lines || return 1
  assert_uploads_exactly || return 1
}

t_missing_asset_reports_missing_files() {
  export VERIFY_TEST_SCENARIO=missing_a
  run_verify 2 1
  assert_status 1 || return 1
  assert_single_missing_line "camel-a.tar.gz" || return 1
}

t_failed_repair_uploads_unverified_set_and_reverifies() {
  export VERIFY_TEST_SCENARIO=missing_a
  run_verify 2 1
  assert_status 1 || return 1
  assert_single_missing_line "camel-a.tar.gz" || return 1
  assert_uploads_exactly "camel-a.tar.gz" || return 1
  assert_view_count_gt 2 || return 1
}

t_successful_repair() {
  export VERIFY_TEST_SCENARIO=repair_success
  run_verify 2 2
  assert_status 0 || return 1
  assert_stdout_line "Verified release assets for tag '$TAG' after repair" || return 1
  assert_uploads_exactly "camel-a.tar.gz" || return 1
}

t_size_mismatch_triggers_repair() {
  export VERIFY_TEST_SCENARIO=wrong_size
  run_verify 2 2
  assert_status 0 || return 1
  assert_stdout_line "Verified release assets for tag '$TAG' after repair" || return 1
  assert_uploads_exactly "camel-a.tar.gz" || return 1
}

t_repair_then_listing_failure_no_stale_missing() {
  export VERIFY_TEST_SCENARIO=repair_then_listing_failure
  run_verify 2 2
  assert_status 1 || return 1
  assert_stderr_line "$EXPECTED_UNVERIFIABLE" || return 1
  assert_no_missing_lines || return 1
  assert_uploads_exactly "camel-a.tar.gz" || return 1
}

t_repair_upload_failure_warns_and_reverifies() {
  export VERIFY_TEST_SCENARIO=repair_upload_failure_then_verify
  run_verify 2 2
  assert_status 0 || return 1
  assert_stdout_line "Verified release assets for tag '$TAG' after repair" || return 1
  assert_stderr_line "WARN: repair upload failed; re-verifying from a fresh listing anyway" || return 1
  assert_no_missing_lines || return 1
  assert_uploads_exactly "camel-a.tar.gz" || return 1
  assert_view_count_gt 2 || return 1
}

t_bridge_named_missing_asset_fails() {
  rm -f "$WORK/dist"/camel-*
  make_bridge_dist "$WORK/dist"
  export VERIFY_TEST_SCENARIO=bridge_missing
  export VERIFY_TEST_ASSETS_BRIDGE_PARTIAL="$FIXTURES/assets-bridge-partial.json"
  run_verify 2 1
  assert_status 1 || return 1
  assert_single_missing_line "cxf-bridge-9.9.9.zip" || return 1
  assert_uploads_exactly "cxf-bridge-9.9.9.zip" || return 1
}

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

main() {
  if [[ ! -f "$VERIFY_SCRIPT" ]]; then
    printf 'FATAL: script under test not found: %s\n' "$VERIFY_SCRIPT" >&2
    exit 2
  fi

  write_fixtures

  local tests=(
    t_success
    t_transient_404_then_success
    t_persistent_404_unverifiable
    t_missing_asset_reports_missing_files
    t_failed_repair_uploads_unverified_set_and_reverifies
    t_successful_repair
    t_size_mismatch_triggers_repair
    t_repair_then_listing_failure_no_stale_missing
    t_repair_upload_failure_warns_and_reverifies
    t_bridge_named_missing_asset_fails
  )

  local passed=0
  local failed=0
  local name rc
  for name in "${tests[@]}"; do
    WORK="$ROOT/$name"
    mkdir -p "$WORK"
    make_dist "$WORK/dist"
    write_stub "$WORK/bin/gh"

    FAIL_DETAIL=""
    unset VERIFY_TEST_SCENARIO VERIFY_TEST_404_CALLS
    rc=0
    "$name" || rc=$?

    if [[ $rc -eq 0 && -s "$WORK/unexpected.log" ]]; then
      rc=1
      FAIL_DETAIL="unexpected gh invocation recorded: $(tr '\n' '|' <"$WORK/unexpected.log")"
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

  printf '%d passed, %d failed\n' "$passed" "$failed"
  if [[ $failed -ne 0 ]]; then
    exit 1
  fi
}

main "$@"
