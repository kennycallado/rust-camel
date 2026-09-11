#!/usr/bin/env bash
# Weekly local verification of the 8 Ollama `slow test:` cases (bd rc-gfum).
#
# Runs the full suite against the real qwen3.5:4b + embeddinggemma models that
# already live on the developer host. Invoked by the `camel-ollama-live.timer`
# systemd --user unit. Records one JSON line per run to the fleet event log the
# agent fleet already monitors, and files a P1 bd bug on failure so a human sees
# it via `bd ready`.
#
# MUST run in a feature worktree, never the cold main ./target (AGENTS.md hard
# rule: an agent build in main re-heats the shared 75G target for hours).
set -u

# --- configuration -----------------------------------------------------------
WORKTREE="${CAMEL_OLLAMA_WORKTREE:-/home/shared/rust-camel-worktrees/l2}"
EVENTS_LOG="${CAMEL_FLEET_EVENTS:-$HOME/dev/rust-camel/.opencode/fleet/events.log}"
OLLAMA_URL="${OLLAMA_BASE_URL:-http://localhost:11434}"

fail_hard() {
  # Infrastructure failure (not a test failure): record and exit non-zero so
  # systemd OnFailure= fires, but do not file a test-regression bug.
  local reason="$1"
  printf '{"kind":"ollama-live","ts":"%s","result":"infra-error","reason":"%s"}\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$reason" >>"$EVENTS_LOG" 2>/dev/null || true
  echo "ollama-live: infra error: $reason" >&2
  exit 2
}

# --- preflight ---------------------------------------------------------------
[ -d "$WORKTREE" ] || fail_hard "worktree $WORKTREE not found"
command -v ollama >/dev/null 2>&1 || fail_hard "ollama CLI not on PATH"

# Ollama reachable?
if ! curl -fsS "$OLLAMA_URL/api/tags" >/dev/null 2>&1; then
  fail_hard "ollama not reachable at $OLLAMA_URL"
fi

# Required models present? (qwen3.5:4b for 7 tests, embeddinggemma for embed)
models="$(ollama list 2>/dev/null || true)"
echo "$models" | grep -q 'qwen3.5:4b'    || fail_hard "qwen3.5:4b model not pulled"
echo "$models" | grep -q 'embeddinggemma' || fail_hard "embeddinggemma model not pulled"

# --- run the suite in the worktree -------------------------------------------
start_ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
start_s="$(date +%s)"

# Capture stdout+stderr; parse the libtest summary line for pass/fail counts.
out="$(cd "$WORKTREE" && cargo test -p camel-component-llm --test ollama_live \
  --features ollama -- --ignored 2>&1)"
rc=$?

end_s="$(date +%s)"
duration=$((end_s - start_s))

# Extract the names of any FAILED tests from libtest output.
failed_json="$(printf '%s\n' "$out" \
  | sed -n 's/^test \(.*\) \.\.\. FAILED$/\1/p' \
  | paste -sd, - \
  | sed 's/\([^,][^,]*\)/"\1"/g')"
[ -z "$failed_json" ] && failed_json="" # empty list when none

if [ "$rc" -eq 0 ]; then
  result="pass"
else
  result="fail"
fi

# --- record to the fleet event log (the agents already tail this) ------------
printf '{"kind":"ollama-live","ts":"%s","result":"%s","duration_s":%s,"failed":[%s]}\n' \
  "$start_ts" "$result" "$duration" "$failed_json" >>"$EVENTS_LOG" 2>/dev/null \
  || echo "ollama-live: WARNING could not append to $EVENTS_LOG" >&2

echo "ollama-live: $result in ${duration}s (rc=$rc)"

# --- escalate a genuine test regression to a human via bd --------------------
if [ "$rc" -ne 0 ]; then
  if command -v bd >/dev/null 2>&1; then
    bd create "ollama-live weekly FAILED: [$failed_json]" \
      --description="Weekly systemd timer run of the 8 Ollama slow tests failed on $start_ts (duration ${duration}s). Failed: [$failed_json]. See .opencode/fleet/events.log and re-run: cd $WORKTREE && cargo test -p camel-component-llm --test ollama_live --features ollama -- --ignored" \
      -t bug -p 1 --deps discovered-from:rc-gfum >/dev/null 2>&1 \
      || echo "ollama-live: WARNING bd create failed" >&2
  fi
  exit 1
fi

exit 0
