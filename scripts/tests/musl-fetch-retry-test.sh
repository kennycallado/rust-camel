#!/usr/bin/env bash
# shellcheck shell=bash
#
# musl-fetch-retry-test.sh — offline stub battery for the musl toolchain
# fetch in bridges/jms/build-native.sh, bridges/cxf/build-native.sh and
# bridges/xml/build-native.sh (bd rc-1c47h mission 530, bd rc-8gpgd
# mission 536).
#
# No network access is performed: a python3 http.server stub on 127.0.0.1
# (ephemeral port) answers 504 for the first N requests, then 200 with a
# small payload. The battery sources the production download functions
# printed by `build-native.sh --print-musl-fetch` (jms script), so it
# exercises exactly the logic CI runs instead of a copy that can drift.
# CXF and XML parity are asserted by t_cxf_function_parity and
# t_xml_function_parity (byte-identical function printout).
#
# Usage: bash scripts/tests/musl-fetch-retry-test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_NATIVE="$REPO_ROOT/bridges/jms/build-native.sh"
BUILD_NATIVE_CXF="$REPO_ROOT/bridges/cxf/build-native.sh"
BUILD_NATIVE_XML="$REPO_ROOT/bridges/xml/build-native.sh"

ROOT="$(mktemp -d)"
SERVER_PID=""
trap 'if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
      fi
      rm -rf "$ROOT"' EXIT

FIXTURE_PAYLOAD_FILE="$ROOT/fixture-payload.bin"
FIXTURE_WRONG_FILE="$ROOT/fixture-wrong.bin"
FIXTURE_SHA256=""
FUNC_SRC="$ROOT/printed-functions.sh"

# Per-scenario state
WORK=""
FAIL_DETAIL=""
SERVER_LOG=""
URL=""

print_functions() {
  bash "$BUILD_NATIVE" --print-musl-fetch > "$FUNC_SRC"
  grep -qE '^fetch_musl_toolchain ?\(\)' "$FUNC_SRC" || {
    echo "FAIL: --print-musl-fetch did not print fetch_musl_toolchain" >&2
    return 1
  }
  grep -qE '^ensure_musl_archive ?\(\)' "$FUNC_SRC" || {
    echo "FAIL: --print-musl-fetch did not print ensure_musl_archive" >&2
    return 1
  }
}

start_stub_server() {
  local fail_first="$1" payload="$2"
  SERVER_LOG="$ROOT/server-requests.log"
  rm -f "$SERVER_LOG"
  cat > "$ROOT/stub_server.py" <<PY
import http.server
import sys

fail_first = int(sys.argv[1])
log_path = sys.argv[2]
with open(sys.argv[3], "rb") as f:
    payload = f.read()
count = 0


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        global count
        count += 1
        with open(log_path, "a") as log:
            log.write(str(count) + "\n")
        if count <= fail_first:
            self.send_error(504)
        else:
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    def log_message(self, fmt, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
PY
  python3 "$ROOT/stub_server.py" "$fail_first" "$SERVER_LOG" "$payload" \
    > "$ROOT/server-port" &
  SERVER_PID=$!
  local i
  for i in $(seq 1 50); do
    if [[ -s "$ROOT/server-port" ]]; then
      break
    fi
    sleep 0.1
  done
  if ! grep -qE '^[0-9]+$' "$ROOT/server-port" 2>/dev/null; then
    echo "FAIL: stub server did not report a port" >&2
    return 1
  fi
  URL="http://127.0.0.1:$(cat "$ROOT/server-port")/toolchain.tgz"
}

stop_stub_server() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""
  fi
}

request_count() {
  if [[ -f "$SERVER_LOG" ]]; then
    wc -l < "$SERVER_LOG" | tr -d '[:space:]'
  else
    echo 0
  fi
}

assert_eq() {
  local got="$1" want="$2" what="$3"
  if [[ "$got" != "$want" ]]; then
    FAIL_DETAIL="$what: got [$got], want [$want]"
    return 1
  fi
}

assert_contains() {
  local file="$1" needle="$2" what="$3"
  if ! grep -qF -- "$needle" "$file"; then
    FAIL_DETAIL="$what: missing [$needle]"
    return 1
  fi
}

# Point the production functions at the stub and the fixture SHA256.
reset_scenario_env() {
  ARCHIVE="$WORK/archive/toolchain.tgz"
  MUSL_TOOLCHAIN_URL="$URL"
  MUSL_TOOLCHAIN_SHA256="$FIXTURE_SHA256"
  MUSL_FETCH_BUDGET_SECONDS="600"
  MUSL_FETCH_DELAYS="1 1 1 1"
  mkdir -p "$(dirname "$ARCHIVE")"
  rm -f "$ARCHIVE"
}

t_retries_until_success() {
  start_stub_server 4 "$FIXTURE_PAYLOAD_FILE"
  reset_scenario_env
  local rc=0
  {
    ensure_musl_archive
  } >"$WORK/out.log" 2>"$WORK/err.log" || rc=$?

  assert_eq "$rc" "0" "ensure_musl_archive should succeed after 4 x 504" || return 1
  assert_eq "$(request_count)" "5" "server request count (4 x 504 + 1 x 200 = N+1)" || return 1
  if ! cmp -s "$ARCHIVE" "$FIXTURE_PAYLOAD_FILE"; then
    FAIL_DETAIL="downloaded archive does not match the fixture payload"
    return 1
  fi
  assert_contains "$WORK/out.log" "Attempt 5:" "stdout should show the 5th attempt" || return 1
  stop_stub_server
}

t_gives_up_after_budget() {
  start_stub_server 1000000 "$FIXTURE_PAYLOAD_FILE"
  reset_scenario_env
  MUSL_FETCH_BUDGET_SECONDS="3"
  MUSL_FETCH_DELAYS="1 1 1 1 1"
  local started=$SECONDS
  local rc=0
  {
    ensure_musl_archive
  } >"$WORK/out.log" 2>"$WORK/err.log" || rc=$?
  local took=$((SECONDS - started))

  if [[ "$rc" -eq 0 ]]; then
    FAIL_DETAIL="ensure_musl_archive should fail when the server always answers 504"
    return 1
  fi
  assert_contains "$WORK/err.log" "$URL" "stderr should name the URL" || return 1
  # $SECONDS is integer-truncated, so exhaustion legitimately lands after 2 or
  # 3 attempts depending on second-boundary alignment. Assert the shape of the
  # message and a sane count range instead of an exact attempt number.
  if ! grep -qE 'Gave up after [0-9]+ attempt\(s\)' "$WORK/err.log"; then
    FAIL_DETAIL="stderr should report the attempt count (no 'Gave up after N attempt(s)' line)"
    return 1
  fi
  local attempts
  attempts="$(grep -oE 'Gave up after [0-9]+' "$WORK/err.log" | grep -oE '[0-9]+' | head -1)"
  if (( attempts < 2 || attempts > 4 )); then
    FAIL_DETAIL="attempt count out of the expected 2..4 range: got [${attempts}]"
    return 1
  fi
  assert_contains "$WORK/err.log" "3s budget" "stderr should name the elapsed budget" || return 1
  if [[ -f "$ARCHIVE" ]]; then
    FAIL_DETAIL="partial archive was not removed after exhaustion"
    return 1
  fi
  if (( took > 15 )); then
    FAIL_DETAIL="battery hung beyond the 3s budget (took ${took}s)"
    return 1
  fi
  stop_stub_server
}

t_sha_mismatch_aborts() {
  start_stub_server 0 "$FIXTURE_WRONG_FILE"
  reset_scenario_env
  local rc=0
  {
    ensure_musl_archive
  } >"$WORK/out.log" 2>"$WORK/err.log" || rc=$?

  if [[ "$rc" -eq 0 ]]; then
    FAIL_DETAIL="a SHA256 mismatch must abort"
    return 1
  fi
  assert_contains "$WORK/err.log" "SHA256 mismatch after download" "stderr should report the mismatch" || return 1
  assert_contains "$WORK/err.log" "may have been replaced" "stderr should carry the replaced-asset message" || return 1
  if [[ -f "$ARCHIVE" ]]; then
    FAIL_DETAIL="mismatched archive was not removed"
    return 1
  fi
  stop_stub_server
}

t_cache_hit_skips_download() {
  start_stub_server 0 "$FIXTURE_PAYLOAD_FILE"
  reset_scenario_env
  # Pre-seed the cache: a verified tarball at $ARCHIVE.
  cp "$FIXTURE_PAYLOAD_FILE" "$ARCHIVE"
  local rc=0
  {
    ensure_musl_archive
  } >"$WORK/out.log" 2>"$WORK/err.log" || rc=$?

  assert_eq "$rc" "0" "cache hit should succeed" || return 1
  assert_eq "$(request_count)" "0" "server must receive zero requests on a cache hit" || return 1
  assert_contains "$WORK/out.log" "Using cached musl toolchain tarball" "cache-hit fast-path message" || return 1
  stop_stub_server
}

t_cxf_function_parity() {
  # Constants live outside `declare -f` output, so diff that region separately:
  # a drift in the pinned URL/SHA256/cache dir must fail here too.
  if ! diff -q <(sed -n '/^MUSL_PREFIX=/,/^ARCHIVE=/p' "$BUILD_NATIVE") \
      <(sed -n '/^MUSL_PREFIX=/,/^ARCHIVE=/p' "$BUILD_NATIVE_CXF") >/dev/null; then
    FAIL_DETAIL="cxf constants drift: MUSL_PREFIX..ARCHIVE block differs from jms"
    return 1
  fi
  local cxf_src="$ROOT/cxf-functions.sh"
  bash "$BUILD_NATIVE_CXF" --print-musl-fetch > "$cxf_src"
  if ! cmp -s "$FUNC_SRC" "$cxf_src"; then
    FAIL_DETAIL="cxf --print-musl-fetch output differs from jms (blocks are not textually aligned)"
    return 1
  fi
}

t_xml_function_parity() {
  # Same alignment contract as t_cxf_function_parity, extended to the XML
  # bridge (rc-8gpgd): the constants block and the --print-musl-fetch
  # output must stay byte-identical to the jms reference.
  if ! diff -q <(sed -n '/^MUSL_PREFIX=/,/^ARCHIVE=/p' "$BUILD_NATIVE") \
      <(sed -n '/^MUSL_PREFIX=/,/^ARCHIVE=/p' "$BUILD_NATIVE_XML") >/dev/null; then
    FAIL_DETAIL="xml constants drift: MUSL_PREFIX..ARCHIVE block differs from jms"
    return 1
  fi
  local xml_src="$ROOT/xml-functions.sh"
  bash "$BUILD_NATIVE_XML" --print-musl-fetch > "$xml_src"
  if ! cmp -s "$FUNC_SRC" "$xml_src"; then
    FAIL_DETAIL="xml --print-musl-fetch output differs from jms (blocks are not textually aligned)"
    return 1
  fi
}

t_ci_cache_key_sync() {
  # The ci.yml cache keys embed the pinned SHA256 literal. If the pin is
  # bumped in build-native.sh but the keys go stale, actions/cache keeps
  # restoring a dead entry and every run re-downloads (save is skipped:
  # the key already exists). Make that drift loud here instead of silent
  # in CI. All three bridges (jms, cxf, xml) must carry a key.
  local sha
  sha="$(grep -oE '^MUSL_TOOLCHAIN_SHA256="[0-9a-f]+"' "$BUILD_NATIVE" | cut -d'"' -f2)"
  if [[ -z "$sha" ]]; then
    FAIL_DETAIL="could not extract MUSL_TOOLCHAIN_SHA256 from build-native.sh"
    return 1
  fi
  local bridge
  for bridge in jms cxf xml; do
    if ! grep -qF -- "musl-toolchain-${bridge}-v11.2.1-${sha}" \
        "$REPO_ROOT/.github/workflows/ci.yml"; then
      FAIL_DETAIL="ci.yml cache key for the ${bridge} bridge is missing or stale (expected musl-toolchain-${bridge}-v11.2.1-${sha})"
      return 1
    fi
  done
}

main() {
  dd if=/dev/zero of="$FIXTURE_PAYLOAD_FILE" bs=100 count=1 status=none
  dd if=/dev/zero of="$FIXTURE_WRONG_FILE" bs=64 count=1 status=none
  FIXTURE_SHA256="$(sha256sum "$FIXTURE_PAYLOAD_FILE" | cut -d' ' -f1)"
  print_functions
  source "$FUNC_SRC"

  local tests=(
    t_retries_until_success
    t_gives_up_after_budget
    t_sha_mismatch_aborts
    t_cache_hit_skips_download
    t_cxf_function_parity
    t_xml_function_parity
    t_ci_cache_key_sync
  )

  local passed=0
  local failed=0
  local name rc
  for name in "${tests[@]}"; do
    WORK="$ROOT/$name"
    mkdir -p "$WORK"
    FAIL_DETAIL=""
    rc=0
    "$name" || rc=$?
    if [[ "$rc" -eq 0 ]]; then
      printf 'PASS: %s\n' "$name"
      passed=$((passed + 1))
    else
      printf 'FAIL: %s: %s\n' "$name" "${FAIL_DETAIL:-assertion failed}"
      failed=$((failed + 1))
    fi
    stop_stub_server
  done

  printf '%d passed, %d failed\n' "$passed" "$failed"
  if [[ "$failed" -ne 0 ]]; then
    exit 1
  fi
}

main "$@"
