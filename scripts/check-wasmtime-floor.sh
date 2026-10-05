#!/usr/bin/env bash
# Guard the workspace wasmtime family version floor.
# RUSTSEC-2026-0325/0326/0327 are patched in wasmtime 49.0.2; a caret floor of
# "49.0.1" still admits the vulnerable 49.0.1 release, so the manifest
# requirement for BOTH crates must resolve to a floor >= 49.0.2.
# Pre-release/build requirements (e.g. "49.0.2-alpha.1") are rejected: a
# pre-release is not a stable security floor and must never be normalized down
# to its release version.
#
# Usage: check-wasmtime-floor.sh [Cargo.toml]
#        check-wasmtime-floor.sh --self-test
set -euo pipefail

SELF=${BASH_SOURCE[0]}
MANIFEST=${1:-Cargo.toml}
FLOOR=49.0.2

fail() { echo "FAIL: $1" >&2; exit 1; }

# True when version $1 >= version $2 under version sort.
ge() {
  [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -n1)" = "$2" ]
}

check_manifest() {
  local manifest="$1" dep req floor
  for dep in wasmtime wasmtime-wasi; do
    req=$(grep -E "^${dep}[[:space:]]*=" "$manifest" | head -n1 \
          | grep -oE '"[0-9][^"]*"' | head -n1 | tr -d '"') || true
    [ -n "$req" ] || fail "$dep: no version requirement found in $manifest"
    case "$req" in
      *-*|*+*) fail "$dep: '$req' is a pre-release/build requirement, not a stable security floor" ;;
    esac
    floor=$(printf '%s' "$req" | sed -E 's/^[~^=]*//; s/^>=?//; s/[[:space:]]//g')
    [[ "$floor" =~ ^[0-9]+(\.[0-9]+)*$ ]] || fail "$dep: unparseable/unsupported requirement '$req'"
    ge "$floor" "$FLOOR" || fail "$dep: requirement '$req' floor < $FLOOR (admits vulnerable 49.0.1)"
  done
}

# Repeatable negative/positive controls on ephemeral manifests.
self_test() {
  local rc out
  SELFTEST_DIR=$(mktemp -d)
  trap 'rm -rf "$SELFTEST_DIR"' EXIT
  run_case() { # <name> <pass|fail> <wasmtime> <wasi>
    printf '[workspace.dependencies]\nwasmtime = { version = "%s", features = [] }\nwasmtime-wasi = "%s"\n' \
      "$3" "$4" >"$SELFTEST_DIR/Cargo.toml"
    if out=$(bash "$SELF" "$SELFTEST_DIR/Cargo.toml" 2>&1); then rc=0; else rc=$?; fi
    if { [ "$2" = pass ] && [ "$rc" -ne 0 ]; } || { [ "$2" = fail ] && [ "$rc" -eq 0 ]; }; then
      echo "SELFTEST FAIL [$2] $1: rc=$rc out=$out"
      return 1
    fi
    echo "SELFTEST OK [$2] $1: $out"
  }
  run_case "current positive"        pass 49.0.2         49.0.2
  run_case "wasmtime prerelease"     fail 49.0.2-alpha.1 49.0.2
  run_case "wasi prerelease"         fail 49.0.2         49.0.2-alpha.1
  run_case "wasmtime 49.0.1"         fail 49.0.1         49.0.2
  run_case "wasi 49.0.1"             fail 49.0.2         49.0.1
  run_case "wasmtime build metadata" fail 49.0.2+build.1 49.0.2
  run_case "wasi wildcard"           fail 49.0.2         "49.*"
  echo "SELFTEST PASS"
}

if [ "$MANIFEST" = "--self-test" ]; then
  self_test
  exit 0
fi

check_manifest "$MANIFEST"
echo "OK: wasmtime and wasmtime-wasi require >= $FLOOR"
