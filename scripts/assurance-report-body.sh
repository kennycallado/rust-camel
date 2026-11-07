#!/usr/bin/env bash
# shellcheck shell=bash
#
# assurance-report-body.sh — build the monthly assurance issue body offline.
#
# Why this exists (bd rc-g3nls, monthly run 41999707610):
# The "Build issue body" step in .github/workflows/assurance-monthly.yml ran a
# bare `find` under `set -euo pipefail`. When download-artifact produced no
# `art-*` directory, `find` exited non-zero on the missing path and killed the
# step before the infra guard or `gh issue create` ran — so a total
# infrastructure failure produced no issue at all and the durable alarm never
# fired. This script makes body construction offline, testable, and impossible
# to kill with missing evidence: every lookup is non-fatal, missing artifacts
# render explicit `MISSING ... (infra failure)` sections and are reported
# through INFRA_MISSING in the env file. Only real errors (unusable output
# paths, malformed present JSON) exit non-zero.
#
# Usage: OUT_BODY=<path> OUT_ENV=<path> [ARTIFACTS_ROOT=<dir>] \
#          bash scripts/assurance-report-body.sh
#
# Env knobs (defaults in parentheses):
#   ARTIFACTS_ROOT  (.)    root where download-artifact placed art-* dirs
#   FUZZ_RESULT     (unknown)
#   BASE_RESULT     (unknown)
#   ROT_RESULT      (unknown)
#   FUZZ_SECS       (900)
#   OUT_BODY        (required) issue body output path (overwritten)
#   OUT_ENV         (required) env file, appended (GITHUB_ENV semantics)

set -euo pipefail

ARTIFACTS_ROOT="${ARTIFACTS_ROOT:-.}"
FUZZ_RESULT="${FUZZ_RESULT:-unknown}"
BASE_RESULT="${BASE_RESULT:-unknown}"
ROT_RESULT="${ROT_RESULT:-unknown}"
FUZZ_SECS="${FUZZ_SECS:-900}"

if [[ -z "${OUT_BODY:-}" || -z "${OUT_ENV:-}" ]]; then
  printf 'ERROR: OUT_BODY and OUT_ENV are required\n' >&2
  printf 'Usage: OUT_BODY=<path> OUT_ENV=<path> bash %s\n' "$(basename "$0")" >&2
  exit 2
fi

# Refuse to run when the requested output cannot be written: a body that is
# silently dropped is the failure mode this script exists to prevent.
require_writable_target() {
  local var_name="$1" path="$2" parent
  parent="$(dirname -- "$path")"
  if [[ ! -d "$parent" ]]; then
    printf "ERROR: %s parent directory '%s' does not exist (target '%s')\n" \
      "$var_name" "$parent" "$path" >&2
    exit 3
  fi
  if [[ ! -w "$parent" ]]; then
    printf "ERROR: %s parent directory '%s' is not writable (target '%s')\n" \
      "$var_name" "$parent" "$path" >&2
    exit 3
  fi
  if [[ -e "$path" ]] && { [[ -d "$path" ]] || [[ ! -w "$path" ]]; }; then
    printf "ERROR: %s target '%s' is not writable\n" "$var_name" "$path" >&2
    exit 3
  fi
}

require_writable_target OUT_BODY "$OUT_BODY"
require_writable_target OUT_ENV "$OUT_ENV"

# Evidence lookup — NEVER fatal. upload-artifact preserves the least common
# ancestor of its path list (baseline: mutants.out/...; rotating:
# mut-wt/target-mutants/...), so resolve by filename, never by flat path
# (post-mortem of run 33984285881 report). A missing directory yields an empty
# value through `|| true`, never an abort (bd rc-g3nls).
find_artifact() {
  local dir="$1" name="$2"
  find "$ARTIFACTS_ROOT/$dir" -name "$name" -print -quit 2>/dev/null || true
}

OUT_BASE="$(find_artifact art-mutants-baseline outcomes.json)"
OUT_ROT="$(find_artifact art-mutants-rotating outcomes.json)"
FUZZF="$(find_artifact art-fuzz-deep fuzz-findings.txt)"
RATIONALE="$(find_artifact art-mutants-rotating rotation-rationale.txt)"

# jq wrapper for present artifacts. Missing evidence is handled by callers;
# a file that exists but cannot be parsed is a real error (exit 4).
jq_q() {
  local filter="$1" file="$2" out
  if ! out="$(jq -r "$filter" "$file" 2>/dev/null)"; then
    printf 'ERROR: %s is not valid JSON — regenerate the artifact (real error, not missing evidence)\n' \
      "$file" >&2
    exit 4
  fi
  if [[ -n "$out" ]]; then
    printf '%s\n' "$out"
  fi
  return 0
}

is_present() {
  [[ -n "$1" && -f "$1" ]]
}

STAMP="$(date -u +%Y-%m)"

# --- Body -------------------------------------------------------------------
{
  printf '# assurance-monthly: %s\n' "$STAMP"
  printf '\n'
  printf 'Generated work order. bd stays the tracker — file follow-ups there.\n'
  printf '\n'
  printf '## Tool jobs\n'
  printf '\n'
  printf '| job | result |\n'
  printf '|---|---|\n'
  printf '| fuzz-deep | %s |\n' "$FUZZ_RESULT"
  printf '| mutants-baseline | %s |\n' "$BASE_RESULT"
  printf '| mutants-rotating | %s |\n' "$ROT_RESULT"
  printf '\n'
  printf 'failure here means infrastructure, not findings.\n'
  printf '## Fuzz deep (target matrix, %ss each)\n' "$FUZZ_SECS"
  printf '\n'
  printf '```\n'
  if is_present "$FUZZF"; then
    cat "$FUZZF"
  else
    printf 'MISSING FUZZ FINDINGS (infra failure)\n'
  fi
  printf '```\n'
  printf '\n'
  for pair in "baseline:${OUT_BASE}" "rotating:${OUT_ROT}"; do
    name="${pair%%:*}"
    f="${pair#*:}"
    if ! is_present "$f"; then
      printf '## Mutants %s: MISSING OUTCOMES (infra failure)\n' "$name"
      printf '\n'
      continue
    fi
    printf '## Mutants %s\n' "$name"
    printf '\n'
    tested="$(jq_q '.outcomes | length' "$f")"
    caught="$(jq_q '[.outcomes[] | select(.summary=="CaughtMutant")] | length' "$f")"
    missed="$(jq_q '[.outcomes[] | select(.summary=="MissedMutant")] | length' "$f")"
    unviable="$(jq_q '[.outcomes[] | select(.summary=="UnviableMutant")] | length' "$f")"
    printf 'tested=%s caught=%s missed=%s unviable=%s\n' \
      "$tested" "$caught" "$missed" "$unviable"
    printf '\n'
    printf '```\n'
    # .name is self-contained ("file:line: mutation") — avoids null-line
    # duplication from separately joined fields.
    jq_q '.outcomes[] | select(.summary=="MissedMutant")
      | .scenario.Mutant.name' "$f" | sort -u
    printf '```\n'
    printf '\n'
  done
  # Literal backticks in this block are intentional: single-quoted format
  # strings keep bash from command-substituting them.
  printf '%s\n' '## Triage checklist (dedup is mandatory)'
  printf '\n'
  printf '%s\n' '- bd create ONLY for survivors NOT already tracked — check the'
  printf '%s\n' '  existing `discovered-from:rc-eba8` bds first (rc-tb93, rc-kkwh,'
  printf '%s\n' '  rc-j5zp, rc-jtsw, rc-6f62, rc-ldvm, rc-e83t, rc-blcy, rc-xv4e).'
  printf '%s\n' '- Fuzz crashes: promote minimized inputs as committed #[test]s,'
  printf '%s\n' '  never raw corpus blobs; `bd create -t bug` ranked by exposure.'
  printf '%s\n' '- Kill rates are informational: no threshold, no gate.'
  printf '\n'
  printf '## Rotation\n'
  printf '\n'
  printf '```\n'
  if is_present "$RATIONALE"; then
    cat "$RATIONALE"
  else
    printf 'MISSING ROTATION RATIONALE (infra failure)\n'
  fi
  printf '```\n'
  printf '\n'
  printf '%s\n' 'Next window: minimal PR editing `.github/assurance-monthly.json`'
  printf '%s\n' '(mutants_rotating_file from next_candidates + rotate rotated_on).'
  config_file="$ARTIFACTS_ROOT/.github/assurance-monthly.json"
  if [[ -f "$config_file" && -r "$config_file" ]]; then
    candidates="$(jq_q '.next_candidates | join(", ")' "$config_file")"
    printf 'Candidates: %s\n' "$candidates"
  else
    printf 'Candidates: (no .github/assurance-monthly.json — rotation config unreadable)\n'
  fi
} > "$OUT_BODY"

# Machine-readable survivor set for NEXT month's diff, between HTML comment
# markers (robust to markdown renderers). Same .name form as the display block
# so diffs are stable.
{
  printf '<!--baseline-data-start-->\n'
  if is_present "$OUT_BASE"; then
    jq_q '.outcomes[] | select(.summary=="MissedMutant")
      | .scenario.Mutant.name' "$OUT_BASE" | sort -u
  fi
  printf '<!--baseline-data-end-->\n'
} >> "$OUT_BODY"

# --- Env file (subsequent-step semantics) -----------------------------------
# rotating is evidence as a unit: outcomes *and* rationale.
INFRA_MISSING=""
if ! is_present "$OUT_BASE"; then
  INFRA_MISSING="${INFRA_MISSING:+$INFRA_MISSING }baseline"
fi
if ! is_present "$OUT_ROT" || ! is_present "$RATIONALE"; then
  INFRA_MISSING="${INFRA_MISSING:+$INFRA_MISSING }rotating"
fi
if ! is_present "$FUZZF"; then
  INFRA_MISSING="${INFRA_MISSING:+$INFRA_MISSING }fuzz"
fi

{
  printf 'OUT_BASE=%s\n' "$OUT_BASE"
  printf 'OUT_ROT=%s\n' "$OUT_ROT"
  printf 'FUZZF=%s\n' "$FUZZF"
  printf 'STAMP=%s\n' "$STAMP"
  printf 'INFRA_MISSING=%s\n' "$INFRA_MISSING"
} >> "$OUT_ENV"
