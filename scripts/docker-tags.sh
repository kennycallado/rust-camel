#!/usr/bin/env bash
# docker-tags.sh — single source of truth for the release workflow's image
# tag plans. Pure tag-plan generator: no side effects, no registry
# invocations. Prints the exact tag list for ONE variant, one tag per
# line, to stdout.
#
# Usage:
#   docker-tags.sh <version> <stable|rc> <suffix> <semantic-tag> [--arch amd64|arm64]
#
# Rules (docker-tag-remap design §1/§5):
#   - default (no --arch): immutable {version}{suffix} ALWAYS emitted;
#     floating latest{suffix} and {semantic-tag} emitted ONLY when mode
#     is stable.
#   - --arch A: emits {version}-A{suffix} always, and latest-A{suffix}
#     ONLY when mode is stable (arch intermediates are GHCR-only push
#     targets; the semantic tag is never arch-tagged).
#   - rc versions never emit floating tags of any kind (the quarantine).
#
# --test: runs the fixture table plus the full parameter sweep
#   (3 variants x stable/rc x no-arch/amd64/arm64 = 18 combinations) and
#   expect-fail cases (argv hardening, quarantine guard), prints a unified
#   diff on mismatch, exits non-zero on any failure, prints
#   "docker-tags: N/N fixtures pass" on success.
set -euo pipefail

# --- generator -----------------------------------------------------------

generate() {
  local version="$1" mode="$2" suffix="$3" semantic="$4" arch="${5:-}"
  if [[ -n "$arch" ]]; then
    printf '%s-%s%s\n' "$version" "$arch" "$suffix"
    if [[ "$mode" == "stable" ]]; then
      printf 'latest-%s%s\n' "$arch" "$suffix"
    fi
  else
    printf '%s%s\n' "$version" "$suffix"
    if [[ "$mode" == "stable" ]]; then
      printf 'latest%s\n' "$suffix"
      printf '%s\n' "$semantic"
    fi
  fi
}

# --- test harness --------------------------------------------------------

check_case() {
  local desc="$1" expected="$2"
  shift 2
  local actual
  actual="$(generate "$@")"
  total=$((total + 1))
  if [[ "$actual" != "$expected" ]]; then
    failures=$((failures + 1))
    printf 'FAIL: %s\n' "$desc"
    diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true
  fi
}

# Expect-fail case: the CLI must reject the invocation with exit 2.
check_fail() {
  local desc="$1"
  shift
  local out rc
  out="$(main "$@" 2>&1)" && rc=0 || rc=$?
  total=$((total + 1))
  if [[ "$rc" -ne 2 ]]; then
    failures=$((failures + 1))
    printf 'FAIL: %s (expected exit 2, got %d)\n' "$desc" "$rc"
    printf '%s\n' "$out" | sed 's/^/  /'
  fi
}

# Happy-path case through the CLI entry point: exercises main's argv
# parsing (the path the workflow consumes) plus the generator.
check_main() {
  local desc="$1" expected="$2"
  shift 2
  local actual rc
  actual="$(main "$@" 2>&1)" && rc=0 || rc=$?
  total=$((total + 1))
  if [[ "$rc" -ne 0 || "$actual" != "$expected" ]]; then
    failures=$((failures + 1))
    printf 'FAIL: %s (exit %d)\n' "$desc" "$rc"
    diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual") || true
  fi
}

run_test() {
  local total=0 failures=0

  # Fixture table (docker-tag-remap design §5).
  check_case "0.51.0 stable \"\" regular" \
    $'0.51.0\nlatest\nregular' \
    0.51.0 stable "" regular
  check_case "0.51.0 stable -slim slim" \
    $'0.51.0-slim\nlatest-slim\nslim' \
    0.51.0 stable -slim slim
  check_case "0.51.0 stable -full full" \
    $'0.51.0-full\nlatest-full\nfull' \
    0.51.0 stable -full full
  check_case "0.51.0-rc.3 rc \"\" regular" \
    $'0.51.0-rc.3' \
    0.51.0-rc.3 rc "" regular
  check_case "0.51.0-rc.3 rc -slim slim" \
    $'0.51.0-rc.3-slim' \
    0.51.0-rc.3 rc -slim slim
  check_case "0.51.0-rc.3 rc -full full" \
    $'0.51.0-rc.3-full' \
    0.51.0-rc.3 rc -full full
  check_case "0.51.0 stable -slim slim --arch amd64" \
    $'0.51.0-amd64-slim\nlatest-amd64-slim' \
    0.51.0 stable -slim slim amd64
  check_case "0.51.0-rc.3 rc -full full --arch arm64" \
    $'0.51.0-rc.3-arm64-full' \
    0.51.0-rc.3 rc -full full arm64

  # Expect-fail cases: argv hardening + quarantine guard (exit 2).
  check_fail "trailing junk after --arch" \
    0.51.0 stable -slim slim --arch amd64 EXTRA_JUNK
  check_fail "rc version with stable mode" \
    0.51.0-rc.3 stable -slim slim
  check_fail "empty version" \
    "" stable "" regular

  # Happy path through the CLI entry point (argv parsing + generate).
  check_main "main 0.51.0 stable -slim slim" \
    $'0.51.0-slim\nlatest-slim\nslim' \
    0.51.0 stable -slim slim
  check_main "main 0.51.0-rc.3 rc -full full" \
    $'0.51.0-rc.3-full' \
    0.51.0-rc.3 rc -full full

  # Full parameter sweep: 3 variants x stable/rc x no-arch/amd64/arm64.
  local -a variants_suffix=("" "-slim" "-full")
  local -a variants_semantic=(regular slim full)
  local -a modes=(stable rc)
  local -a arches=("" amd64 arm64)
  local v mode arch suffix semantic expected
  for v in 0 1 2; do
    suffix="${variants_suffix[$v]}"
    semantic="${variants_semantic[$v]}"
    for mode in "${modes[@]}"; do
      for arch in "${arches[@]}"; do
        local version="0.51.0"
        [[ "$mode" == "rc" ]] && version="0.51.0-rc.3"
        expected="$(sweep_expected "$version" "$mode" "$suffix" "$semantic" "$arch")"
        check_case "sweep $version $mode '$suffix' '$semantic' arch='$arch'" \
          "$expected" \
          "$version" "$mode" "$suffix" "$semantic" "$arch"
      done
    done
  done

  if [[ "$failures" -gt 0 ]]; then
    printf 'docker-tags: %d/%d fixtures FAILED\n' "$failures" "$total" >&2
    exit 1
  fi
  printf 'docker-tags: %d/%d fixtures pass\n' "$total" "$total"
}

# Expected output for the sweep, derived from the rules (coverage check;
# the fixture table above is the independent ground truth).
sweep_expected() {
  local version="$1" mode="$2" suffix="$3" semantic="$4" arch="${5:-}"
  local -a tags=()
  if [[ -n "$arch" ]]; then
    tags+=("${version}-${arch}${suffix}")
    if [[ "$mode" == "stable" ]]; then
      tags+=("latest-${arch}${suffix}")
    fi
  else
    tags+=("${version}${suffix}")
    if [[ "$mode" == "stable" ]]; then
      tags+=("latest${suffix}" "$semantic")
    fi
  fi
  printf '%s\n' "${tags[@]}"
}

# --- CLI -----------------------------------------------------------------

main() {
  if [[ "${1:-}" == "--test" ]]; then
    run_test
    return
  fi
  if [[ $# -lt 4 ]]; then
    printf 'usage: docker-tags.sh <version> <stable|rc> <suffix> <semantic-tag> [--arch amd64|arm64]\n' >&2
    exit 2
  fi
  local version="$1" mode="$2" suffix="$3" semantic="$4" arch=""
  shift 4
  if [[ $# -gt 0 ]]; then
    if [[ "$1" == "--arch" ]]; then
      arch="${2:-}"
      if [[ -z "$arch" || ( "$arch" != "amd64" && "$arch" != "arm64" ) ]]; then
        printf 'docker-tags: --arch must be amd64 or arm64\n' >&2
        exit 2
      fi
      shift 2
      if [[ $# -gt 0 ]]; then
        printf 'docker-tags: unexpected argument: %s\n' "$1" >&2
        exit 2
      fi
    else
      printf 'docker-tags: unknown argument: %s\n' "$1" >&2
      exit 2
    fi
  fi
  if [[ "$mode" != "stable" && "$mode" != "rc" ]]; then
    printf 'docker-tags: mode must be stable or rc\n' >&2
    exit 2
  fi
  if [[ -z "$version" ]]; then
    printf 'docker-tags: version must not be empty\n' >&2
    exit 2
  fi
  if [[ -z "$semantic" ]]; then
    printf 'docker-tags: semantic-tag must not be empty\n' >&2
    exit 2
  fi
  if [[ "$mode" == "stable" && "$version" == *-rc.* ]]; then
    printf 'docker-tags: stable mode with rc version %s\n' "$version" >&2
    exit 2
  fi
  generate "$version" "$mode" "$suffix" "$semantic" "$arch"
}

main "$@"