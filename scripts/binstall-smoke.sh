#!/usr/bin/env bash
# binstall-smoke.sh — every supported target must resolve to a real release asset.
#
# Answers ONE question: do the pkg-url templates in
# crates/camel-cli/Cargo.toml — the same single source of truth
# cargo-binstall reads — map each supported target triple to an asset name
# that exists in a given GitHub release?
#
# Templates are read from the manifest via python3/tomllib, never hardcoded
# here, so a manifest edit is picked up without touching this script. Paths
# resolve against the script's repo root, so it runs from anywhere. The
# asset list comes from the GitHub API (tag endpoint for the release id,
# then the paginated assets sub-endpoint — the tag endpoint's embedded
# assets array is capped) or, for tests, from --assets-file.
#
# cargo-binstall dry-runs (--manifest-path is its documented hook for
# pre-publish metadata testing) are informational only: the manifest +
# asset-list assertion is the contract, a binstall mismatch never fails
# this script.
set -euo pipefail

usage() {
  printf 'usage: %s --release TAG [--repo OWNER/NAME] [--assets-file PATH]\n' "$0"
}

repo="kennycallado/rust-camel"
release=""
assets_file=""

# A missing flag VALUE is a usage error (exit 2), not a bash set -u abort.
need_value() {
  printf 'error: %s requires a value\n' "$1" >&2
  usage >&2
  exit 2
}

while (($#)); do
  case "$1" in
    --repo) (($# >= 2)) || need_value "$1"; repo="$2"; shift 2 ;;
    --release) (($# >= 2)) || need_value "$1"; release="$2"; shift 2 ;;
    --assets-file) (($# >= 2)) || need_value "$1"; assets_file="$2"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *)
      printf 'unknown flag: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$release" ]]; then
  printf 'error: --release is required (no default)\n' >&2
  usage >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(git -C "$script_dir" rev-parse --show-toplevel)"
manifest="$repo_root/crates/camel-cli/Cargo.toml"

if [[ ! -f "$manifest" ]]; then
  printf 'error: manifest not found: %s\n' "$manifest" >&2
  exit 1
fi

# Single source of truth: both pkg-url templates straight from the manifest
# (line 1 = default flavor, line 2 = musl override).
templates="$(python3 - "$manifest" <<'PY'
import sys
import tomllib

try:
    with open(sys.argv[1], "rb") as fh:
        binstall = tomllib.load(fh)["package"]["metadata"]["binstall"]
    print(binstall["pkg-url"])
    print(binstall["overrides"]['cfg(all(target_os = "linux", target_env = "musl"))']["pkg-url"])
except KeyError:
    sys.exit("error: manifest lacks [package.metadata.binstall] (or its overrides table)")
PY
)"
default_template="${templates%%$'\n'*}"
musl_template="${templates##*$'\n'}"

if [[ -n "$assets_file" ]]; then
  if [[ ! -r "$assets_file" ]]; then
    printf 'error: cannot read assets file: %s\n' "$assets_file" >&2
    exit 1
  fi
  assets="$(<"$assets_file")"
else
  release_id="$(gh api "repos/$repo/releases/tags/$release" --jq '.id')"
  assets="$(gh api "repos/$repo/releases/$release_id/assets?per_page=100" --jq '.[].name')"
fi

# Fixtures may be Windows-authored (CRLF): strip CR so exact-line membership
# greps don't miss on a trailing carriage return.
assets="${assets//$'\r'/}"

targets=(
  x86_64-unknown-linux-gnu
  aarch64-unknown-linux-gnu
  x86_64-unknown-linux-musl
  aarch64-unknown-linux-musl
  x86_64-apple-darwin
  aarch64-apple-darwin
  x86_64-pc-windows-msvc
)

# Collect ALL misses before deciding: no early exit inside the loop (a bare
# grep -q under set -euo pipefail would abort at the first miss and hide
# the rest).
misses=0
for target in "${targets[@]}"; do
  template="$default_template"
  if [[ "$target" == *-musl ]]; then
    template="$musl_template"
  fi
  url="${template//\{repo\}/https://github.com/$repo}"
  url="${url//\{version\}/${release#v}}"
  url="${url//\{target\}/$target}"
  expected="${url##*/}"
  if ! grep -qxF -- "$expected" <<<"$assets"; then
    printf 'FAIL: %s -> %s (missing from release %s)\n' "$target" "$expected" "$release" >&2
    misses=$((misses + 1))
  fi
done

if ((misses > 0)); then
  exit 1
fi

# Informational cross-check: does cargo-binstall itself resolve the same
# metadata? Never fatal, and skipped for --assets-file runs (the test hook
# has no real release to check against).
if [[ -z "$assets_file" ]] && command -v cargo-binstall >/dev/null 2>&1; then
  for target in "${targets[@]}"; do
    if out="$(cargo binstall --dry-run --no-confirm --manifest-path "$manifest" --target "$target" camel-cli 2>&1)"; then
      while IFS= read -r line; do
        printf 'binstall-dryrun: %s\n' "$line" >&2
      done <<<"$out"
    else
      printf 'binstall-dryrun: cargo-binstall failed for %s (informational, contract unaffected)\n' "$target" >&2
    fi
  done
fi

# The target set is a closed list of seven; misses is provably 0 past the
# exit guard, so the success line is fixed.
printf 'smoke: 7/7 targets resolve\n'
exit 0
