#!/usr/bin/env bash
# bridge-lifetime-evidence.sh — capture bridge child-lifetime evidence.
#
# `before` reproduces CI run 39578427824: `cargo test ... 2>&1 | cat` where the
# bridge child (and its grandchild) inherit the cargo test binary's stderr. The
# `cat` consumer cannot see EOF while an orphan holds that write end, so the
# pipeline hangs until `timeout` cancels it (exit 124).
#
# `after` runs the SAME pipeline over the full `--ignored` evidence suite
# (normal stop, panic, SIGTERM/SIGINT/SIGKILL parent death, death inside the
# teardown window, early watchdog death, retired caller runtime, concurrent
# spawns). Post-fix every writer exits, so the pipeline completes and no
# fixture member survives.
#
# Usage:
#   scripts/bridge-lifetime-evidence.sh <before|after> <out.md>
#
# Verdicts:
#   HANG REPRODUCED  before: pipeline timed out AND live fake-bridge orphans exist
#   CLEAN            pipeline completed AND no running fake-bridge orphan
#   BUILD/RUN FAILURE anything else (compile/launch/test failure — not evidence)
set -uo pipefail

mode=${1:-}
out=${2:-}
if [[ $mode != before && $mode != after ]]; then
  printf 'usage: %s <before|after> <out.md>\n' "$0" >&2
  exit 2
fi
if [[ -z $out ]]; then
  printf 'usage: %s <before|after> <out.md>\n' "$0" >&2
  exit 2
fi

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root" || exit 1
mkdir -p "$(dirname "$out")"

sha=$(git rev-parse --short HEAD)
stamp=$(date -u +%Y-%m-%dT%H:%M:%SZ)
started=$(date +%s)

# One token per invocation: the fixture embeds it in every fake-bridge and
# grandchild command line, so the verdict count and all cleanup are scoped to
# THIS run. Concurrent worktrees or stale fixtures can never contaminate the
# verdict, and cleanup cannot kill another run's processes.
evtoken="m390-$(date +%s%N)"
export BRIDGE_EVIDENCE_TOKEN="$evtoken"

if [[ $mode == before ]]; then
  command='cargo test -p camel-bridge --test bridge_exit_path_evidence leak_parent_then_exit -- --ignored --nocapture 2>&1 | cat'
  limit=90
else
  command='cargo test -p camel-bridge --test bridge_exit_path_evidence -- --ignored --nocapture 2>&1 | cat'
  limit=300
fi
log=$(mktemp)

# The pipeline deliberately is NOT given the orphan's death: with inherited
# stderr alive, `cat` never reaches EOF and timeout cancels at `limit`. pipefail
# is enabled INSIDE the inner shell too, so a cargo compile/test failure cannot
# be masked by `cat`'s status and slip into the CLEAN branch.
timeout "$limit" bash -o pipefail -c "$command" >"$log" 2>&1
rc=$?

# Snapshot at cancel, BEFORE any cleanup, so the verdict reflects what timeout
# left behind.
orphans_raw=$(pgrep -af "$evtoken" 2>/dev/null || true)
live_pids=()
while read -r pid _rest; do
  [[ -n ${pid:-} ]] || continue
  state=$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)
  if [[ -n $state && $state != Z ]]; then
    live_pids+=("$pid")
  fi
done <<<"$orphans_raw"
live_count=${#live_pids[@]}

elapsed=$(( $(date +%s) - started ))

if [[ $mode == before ]]; then
  if ((rc == 124 && live_count > 0)); then
    verdict='HANG REPRODUCED'
    pipeline="TIMED OUT after ${limit}s (exit 124)"
  elif ((rc == 0 && live_count == 0)); then
    verdict='CLEAN'
    pipeline="completed (exit $rc)"
  else
    verdict='BUILD/RUN FAILURE'
    pipeline="completed (exit $rc)"
  fi
else
  if ((rc == 0 && live_count == 0)); then
    verdict='CLEAN'
    pipeline="completed (exit $rc)"
  elif ((rc == 124)); then
    verdict='BUILD/RUN FAILURE'
    pipeline="TIMED OUT after ${limit}s (exit 124)"
  else
    verdict='BUILD/RUN FAILURE'
    pipeline="completed (exit $rc)"
  fi
fi

test_results=$(grep -E '^test .+ \.\.\. (ok|FAILED|ignored)' "$log" 2>/dev/null || true)
if [[ -z $test_results ]]; then
  test_results='(no per-test result lines captured)'
fi
tail_output=$(tail -n 40 "$log" 2>/dev/null || true)

{
  printf '# Bridge child lifetime evidence — %s\n\n' "$mode"
  printf -- '- date: %s\n' "$stamp"
  printf -- '- commit: `%s`\n' "$sha"
  printf -- '- mode: `%s`\n' "$mode"
  printf -- '- evidence token: `%s`\n' "$evtoken"
  printf -- '- command: `%s`\n' "$command"
  printf -- '- pipeline: %s\n' "$pipeline"
  printf -- '- timeout exit code: %s\n' "$rc"
  printf -- '- wall time: %ss\n' "$elapsed"
  printf -- '- live token orphans at cancel: %s\n\n' "$live_count"
  if [[ $mode == after ]]; then
    printf '## Per-test results\n\n```text\n%s\n```\n\n' "$test_results"
    printf '## Post-run fixtures (`pgrep -af %s`)\n\n```text\n%s\n```\n\n' "$evtoken" "$orphans_raw"
  else
    printf '## Orphans at cancel (`pgrep -af %s`)\n\n```text\n%s\n```\n\n' "$evtoken" "$orphans_raw"
  fi
  printf '## Captured output (tail)\n\n```text\n%s\n```\n\n' "$tail_output"
  printf '## Verdict\n\n**%s**\n' "$verdict"
} >"$out"

# Cleanup: kill only THIS invocation's fixtures (the live pids captured above
# plus any remaining token match), then let the pipeline's consumers see EOF.
# This never changes the verdict recorded above.
for pid in "${live_pids[@]:-}"; do
  [[ -n $pid ]] && kill -KILL "$pid" 2>/dev/null || true
done
pkill -f "$evtoken" 2>/dev/null || true
rm -f "$log"

printf 'evidence written to %s\n' "$out"
printf 'verdict: %s\n' "$verdict"
exit 0
