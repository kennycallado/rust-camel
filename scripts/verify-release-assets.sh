#!/usr/bin/env bash
# shellcheck shell=bash
#
# verify-release-assets.sh — verify (and if needed repair) the published asset
# set of a GitHub release against the locally built dist files.
#
# Why this exists (bd rc-6ep0i):
# v0.65.0 (release run 41810273383) published 21 of 22 assets.
# softprops/action-gh-release hit an eventual-consistency 404 while resolving
# asset metadata, logged "Matching asset is present after refresh;
# continuing..." and reported success, so
# camel-full-aarch64-unknown-linux-gnu.tar.gz never landed while its .sha256
# sidecar did. This script is the independent gate that runs after upload: it
# reads the release asset list, requires every local `camel-*` file to be
# present with a remote size exactly equal to the local file size, retries
# transient listing 404s within a bounded window, re-uploads exactly the
# unverified set once with --clobber, verifies again from a fresh listing, and
# fails loudly either with `MISSING: <basename>` lines (confirmed absent or
# size-mismatched) or with an explicit "could not be verified" diagnostic when
# the listing itself never succeeds. A listing failure is never reported as a
# missing asset.
#
# Usage: scripts/verify-release-assets.sh <tag> <dist-dir>
#
# Env knobs (defaults in parentheses):
#   VERIFY_MAX_ATTEMPTS         (6)   initial fresh-listing attempts
#   VERIFY_SLEEP_SECS           (10)  sleep after each failed initial attempt
#   VERIFY_REPAIR_MAX_ATTEMPTS  (3)   post-repair fresh-listing attempts
#   VERIFY_REPAIR_SLEEP_SECS    (10)  sleep after each failed post-repair attempt

set -euo pipefail

if [[ $# -ne 2 ]]; then
  printf 'Usage: %s <tag> <dist-dir>\n' "$(basename "$0")" >&2
  exit 2
fi

TAG="$1"
DIST_DIR="$2"

if ! command -v gh >/dev/null 2>&1 || ! command -v jq >/dev/null 2>&1; then
  echo "ERROR: gh and jq are required" >&2
  exit 2
fi

if [[ ! -d "$DIST_DIR" ]]; then
  echo "ERROR: dist directory '$DIST_DIR' does not exist" >&2
  exit 2
fi

# Normalise a trailing slash so repair paths stay single-slashed.
if [[ "$DIST_DIR" != "/" ]]; then
  DIST_DIR="${DIST_DIR%/}"
fi

VERIFY_MAX_ATTEMPTS="${VERIFY_MAX_ATTEMPTS:-6}"
VERIFY_SLEEP_SECS="${VERIFY_SLEEP_SECS:-10}"
VERIFY_REPAIR_MAX_ATTEMPTS="${VERIFY_REPAIR_MAX_ATTEMPTS:-3}"
VERIFY_REPAIR_SLEEP_SECS="${VERIFY_REPAIR_SLEEP_SECS:-10}"

# Unverified basenames from the most recent verify_pass with an observable
# listing (global on purpose: bash cannot return arrays).
UNVERIFIED=()

# Fetch a fresh asset listing for the release.
# Prints one `<name><TAB><size>` line per asset and returns 0 on success.
# Prints nothing and returns 1 when either gh or jq fails (retryable listing
# failure, never evidence that an asset is missing).
list_assets() {
  local listing status
  set +e
  listing="$(gh release view "$TAG" --json assets 2>/dev/null \
    | jq -r '.assets[] | "\(.name)\t\(.size)"')"
  status=$?
  set -e
  if [[ $status -ne 0 ]]; then
    return 1
  fi
  printf '%s\n' "$listing"
}

# One verification pass against a fresh listing.
# Returns 0 when every expected asset is present with the exact local size,
# 1 when the listing was observable and names the unverified set in the global
# UNVERIFIED array, 2 when the listing itself failed.
verify_pass() {
  local listing path name size row_name row_size found files scan_status
  # A failed directory scan must never be mistaken for an empty expected set:
  # that would verify zero files and report success vacuously. An empty but
  # readable directory is still allowed to pass (upstream owns local-set
  # completeness).
  set +e
  files="$(find "$DIST_DIR" -maxdepth 1 -type f -name 'camel-*' | sort)"
  scan_status=$?
  set -e
  if [[ $scan_status -ne 0 ]]; then
    printf "ERROR: failed to scan dist directory '%s'\n" "$DIST_DIR" >&2
    exit 1
  fi

  if ! listing="$(list_assets)"; then
    return 2
  fi

  UNVERIFIED=()
  if [[ -n "$files" ]]; then
    while IFS= read -r path; do
      name="$(basename "$path")"
      size="$(stat -c %s "$path")"
      found=0
      while IFS=$'\t' read -r row_name row_size; do
        if [[ "$row_name" == "$name" && "$row_size" == "$size" ]]; then
          found=1
          break
        fi
      done <<<"$listing"
      if [[ $found -eq 0 ]]; then
        UNVERIFIED+=("$name")
      fi
    done <<<"$files"
  fi

  if [[ ${#UNVERIFIED[@]} -gt 0 ]]; then
    return 1
  fi
  return 0
}

print_unverifiable() {
  printf "ERROR: release asset set for tag '%s' could not be verified (asset listing failed)\n" "$TAG" >&2
}

# --- Initial retry window ---------------------------------------------------
# Every decision below is based on a fresh listing; no earlier observation is
# ever reused to declare success.
OBSERVED_LISTING=0
CONFIRMED_UNVERIFIED=()
attempt=1
while [[ $attempt -le $VERIFY_MAX_ATTEMPTS ]]; do
  rc=0
  verify_pass || rc=$?
  case $rc in
    0)
      printf "Verified release assets for tag '%s'\n" "$TAG"
      exit 0
      ;;
    1)
      OBSERVED_LISTING=1
      CONFIRMED_UNVERIFIED=("${UNVERIFIED[@]}")
      ;;
  esac
  # spec: sleep after every attempt that did not verify
  sleep "$VERIFY_SLEEP_SECS"
  attempt=$((attempt + 1))
done

if [[ $OBSERVED_LISTING -eq 0 ]]; then
  print_unverifiable
  exit 1
fi

# --- Repair pass (exactly one upload) --------------------------------------
repair_paths=()
for name in "${CONFIRMED_UNVERIFIED[@]}"; do
  repair_paths+=("$DIST_DIR/$name")
done

if ! gh release upload "$TAG" --clobber "${repair_paths[@]}"; then
  echo "WARN: repair upload failed; re-verifying from a fresh listing anyway" >&2
fi

# --- Post-repair verification (fresh listings only) ------------------------
POST_OBSERVED_LISTING=0
POST_UNVERIFIED=()
attempt=1
while [[ $attempt -le $VERIFY_REPAIR_MAX_ATTEMPTS ]]; do
  rc=0
  verify_pass || rc=$?
  case $rc in
    0)
      printf "Verified release assets for tag '%s' after repair\n" "$TAG"
      exit 0
      ;;
    1)
      POST_OBSERVED_LISTING=1
      POST_UNVERIFIED=("${UNVERIFIED[@]}")
      ;;
  esac
  sleep "$VERIFY_REPAIR_SLEEP_SECS"
  attempt=$((attempt + 1))
done

# Never derive MISSING from a stale pre-repair observation.
if [[ $POST_OBSERVED_LISTING -eq 0 ]]; then
  print_unverifiable
  exit 1
fi

for name in "${POST_UNVERIFIED[@]}"; do
  printf 'MISSING: %s\n' "$name" >&2
done
exit 1
