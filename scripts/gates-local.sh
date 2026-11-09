#!/usr/bin/env bash
# gates-local.sh — run the local subset of the AGENTS.md QUALITY GATES registry.
#
# Why this exists (2026-11-08, v0.66.0): the registry is the single source of
# truth, but in practice the conductor ran a partial, hand-copied subset — once
# per mission, not on the merge result. Two recurring failures came from gates
# nobody ran locally: `lint-secrets` (a fixture literal) and
# `lint-unbounded-wait` (a bare channel receive in a new test). Both are cheap.
#
# This script PARSES the registry out of AGENTS.md instead of copying it, so a
# new gate is picked up here the moment it lands there. Gates that need the
# network or infra are skipped by name and left to CI.
#
# Usage:
#   bash scripts/gates-local.sh                 # run everything not skipped
#   GATES_ONLY="fmt check lint-unwrap" bash scripts/gates-local.sh
#   GATES_SKIP="" bash scripts/gates-local.sh   # also run the skipped ones
#
# The summary reports `passed:`, `failed:`, `skipped:` and `ran: X of Y`, so a
# sweep always states how much of the registry it covered. A partial sweep
# (GATES_ONLY non-empty) is labeled PARTIAL in the divider and names the
# `not selected:` gates, so an incomplete report can never read as a full one.
#
# Exit: 0 only when every executed gate passed.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)"
AGENTS="$ROOT/AGENTS.md"
# Network / infra / interactive gates: CI owns them.
#   lint-commits        -> git fetch origin main (network)
#   doc-guide-test      -> needs mdbook + docs toolchain
#   cargo-audit         -> advisory DB download
#   audit               -> cargo-audit wrapper, same DB
#   lint-bridge-releases -> GitHub release API (published tag and assets)
SKIP="${GATES_SKIP-lint-commits doc-guide-test cargo-audit audit lint-bridge-releases}"
ONLY="${GATES_ONLY:-}"

[[ -f "$AGENTS" ]] || { echo "gates-local: no AGENTS.md at $AGENTS" >&2; exit 2; }

# python3 parses the registry block; keep the failure message actionable.
PY_BIN="$(command -v python3 || command -v python || true)"
[[ -n "$PY_BIN" ]] || {
    echo "gates-local: python3 (or python) must be on PATH to parse AGENTS.md" >&2
    exit 2
}

mapfile -t GATES < <("$PY_BIN" - "$AGENTS" <<'PY'
import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
start = text.index("## QUALITY GATES")
block = text[start:]
open_fence = block.index("```") + 3
close_fence = block.index("```", open_fence)
body = block[open_fence:close_fence]

gates = []
name = None
run = []
run_sep = " "


def strip_comment(value):
    """Drop a YAML inline comment: `name: x   # note` -> `x`."""
    marker = value.find("  #")
    if marker != -1:
        value = value[:marker]
    return value.rstrip()


def flush():
    if name:
        gates.append((name, run_sep.join(run).strip()))


for line in body.splitlines():
    m = re.match(r"^- name:\s*(.+?)\s*$", line)
    if m:
        flush()
        name, run, run_sep = strip_comment(m.group(1)).strip(), [], " "
        continue
    m = re.match(r"^\s+run:\s*(.*)$", line)
    if m:
        value = m.group(1)
        if value[:1] in ("|", ">"):
            # Literal/folded block: one shell command per line.
            run, run_sep = [], "\n"
        else:
            # Plain scalar wrapped over several lines: YAML folds with spaces.
            run, run_sep = [strip_comment(value)], " "
        continue
    if name and line.startswith("    "):
        continuation = line.strip()
        if continuation:
            run.append(continuation)
flush()

for gate_name, command in gates:
    print(gate_name.replace("\t", " ") + "\t" + command.replace("\n", "\\n"))
PY
)

[[ ${#GATES[@]} -gt 0 ]] || { echo "gates-local: parsed 0 gates from AGENTS.md" >&2; exit 2; }

failed=()
passed=()
skipped=()
not_selected=()

if [[ -n "$ONLY" ]]; then
    printf 'gates-local: PARTIAL sweep — GATES_ONLY="%s"\n' "$ONLY"
fi

for entry in "${GATES[@]}"; do
    name="${entry%%$'\t'*}"
    command="${entry#*$'\t'}"
    command="${command//\\n/$'\n'}"

    if [[ " $SKIP " == *" $name "* ]]; then
        skipped+=("$name")
        continue
    fi
    if [[ -n "$ONLY" && " $ONLY " != *" $name "* ]]; then
        not_selected+=("$name")
        continue
    fi

    printf '\n=== %s\n' "$name"
    if ( cd "$ROOT" && bash -o pipefail -c "$command" ); then
        printf 'PASS %s\n' "$name"
        passed+=("$name")
    else
        printf 'FAIL %s\n' "$name"
        failed+=("$name")
    fi
done

ran=$(( ${#passed[@]} + ${#failed[@]} ))
total=${#GATES[@]}

if [[ -n "$ONLY" ]]; then
    printf '\n──── gates-local summary (PARTIAL — GATES_ONLY set) ────\n'
else
    printf '\n──────── gates-local summary ────────\n'
fi
printf 'passed:  %d\n' "${#passed[@]}"
printf 'failed:  %d%s\n' "${#failed[@]}" "${failed:+ — ${failed[*]}}"
printf 'skipped: %d%s\n' "${#skipped[@]}" "${skipped:+ — ${skipped[*]}}"
if [[ -n "$ONLY" ]]; then
    printf 'not selected: %d%s\n' "${#not_selected[@]}" "${not_selected:+ — ${not_selected[*]}}"
fi
printf 'ran:     %d of %d\n' "$ran" "$total"

[[ ${#failed[@]} -eq 0 ]]
