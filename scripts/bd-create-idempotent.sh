#!/usr/bin/env bash
# bd-create-idempotent.sh — retry-safe wrapper around `bd create`.
#
# Contract:
#   bd-create-idempotent.sh <title> [bd-create flags...]
#     stdout: the created-or-reused issue as JSON with an extra boolean
#             field `reused` (true = an existing issue was returned, no
#             write happened; false = a new issue was created).
#     stderr: on reuse, names the reused id and its created_at.
#     exit:   propagates `bd create`'s status on the create path; 0 on reuse;
#             2 on usage errors; non-zero if the dedup pre-check fails.
#   bd-create-idempotent.sh --self-test
#     Runs the M1-M6 verification matrix against an isolated embedded-Dolt
#     database in a temp dir and cleans it up. Never touches the fleet DB.
#
# Dedup rule: an OPEN issue whose title is byte-identical (case-sensitive)
# and which was created within BD_CREATE_IDEMPOTENT_WINDOW seconds
# (default 600) is reused; when several match, the oldest by created_at wins.
#
# Fail-closed: if the pre-check itself fails (bd list errors, missing
# jq/bd/date, unparseable list JSON) the script aborts non-zero. It never
# falls through to a blind create, because a blind create is exactly the
# duplicate-generating failure this wrapper exists to prevent.
#
# Context: `bd create` is not idempotent; agent retries after a timeout or a
# crashed tool call created duplicate issues (see bd rc-bwi7g).
set -euo pipefail

usage() {
  printf 'usage: %s <title> [bd-create flags...]\n' "$0" >&2
  printf '       %s --self-test\n' "$0" >&2
}

# ---------------------------------------------------------------------------
# --self-test: isolated DB + M1-M6 matrix.
# ---------------------------------------------------------------------------
self_test() {
  local script_path out rc flags failures
  local title_a title_b id1 id2 id3 reused count

  # `tmp` is deliberately global: the EXIT trap below runs after this
  # function returns, when a `local` would already be unbound under set -u.
  tmp="$(mktemp -d)"
  trap 'cd / && rm -rf "$tmp"' EXIT

  script_path="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
  flags=(-t task -p 2)
  failures=0

  # This repository's bd is configured with a remote metrics endpoint that
  # blocks init on this fleet, and a TTY run may prompt for agent setup.
  # The self-test must be non-interactive and off the network, so pin both.
  export DO_NOT_TRACK=1 BD_METRICS_DISABLED=1 BD_NON_INTERACTIVE=1

  cd "$tmp"
  if ! bd init --non-interactive >/dev/null 2>&1; then
    printf 'FAIL setup: bd init failed in %s\n' "$tmp"
    printf 'self-test: 0/6 passed\n'
    return 1
  fi

  title_a="bdidem self-test alpha $$"
  title_b="bdidem self-test beta $$"

  # M1 — create-new: reused=false, non-empty id.
  if out="$("$script_path" "$title_a" "${flags[@]}")"; then
    reused="$(jq -r '.reused' <<<"$out" 2>/dev/null || true)"
    id1="$(jq -r '.id // empty' <<<"$out" 2>/dev/null || true)"
    if [[ "$reused" == "false" && -n "$id1" ]]; then
      printf 'PASS M1\n'
    else
      printf 'FAIL M1: expected reused=false with non-empty id, got reused=%s id=%s\n' \
        "${reused:-<none>}" "${id1:-<none>}"
      failures=$((failures + 1))
    fi
  else
    rc=$?
    printf 'FAIL M1: wrapper create exited %s\n' "$rc"
    failures=$((failures + 1))
  fi

  # M2 — identical retry: same id, reused=true, no duplicate row.
  if out="$("$script_path" "$title_a" "${flags[@]}")"; then
    reused="$(jq -r '.reused' <<<"$out" 2>/dev/null || true)"
    id2="$(jq -r '.id // empty' <<<"$out" 2>/dev/null || true)"
    count="$(bd list --json 2>/dev/null | jq --arg t "$title_a" \
      '[.[] | select(.title == $t)] | length' 2>/dev/null || echo -1)"
    if [[ "$reused" == "true" && -n "$id1" && "$id2" == "$id1" && "$count" == "1" ]]; then
      printf 'PASS M2\n'
    else
      printf 'FAIL M2: expected id=%s reused=true count=1, got id=%s reused=%s count=%s\n' \
        "${id1:-<none>}" "${id2:-<none>}" "${reused:-<none>}" "$count"
      failures=$((failures + 1))
    fi
  else
    rc=$?
    printf 'FAIL M2: wrapper retry exited %s\n' "$rc"
    failures=$((failures + 1))
  fi

  # M3 — different title: new, distinct id, reused=false.
  if out="$("$script_path" "$title_b" "${flags[@]}")"; then
    reused="$(jq -r '.reused' <<<"$out" 2>/dev/null || true)"
    id3="$(jq -r '.id // empty' <<<"$out" 2>/dev/null || true)"
    if [[ "$reused" == "false" && -n "$id3" && "$id3" != "$id1" ]]; then
      printf 'PASS M3\n'
    else
      printf 'FAIL M3: expected new distinct id (M1=%s), got id=%s reused=%s\n' \
        "${id1:-<none>}" "${id3:-<none>}" "${reused:-<none>}"
      failures=$((failures + 1))
    fi
  else
    rc=$?
    printf 'FAIL M3: wrapper create exited %s\n' "$rc"
    failures=$((failures + 1))
  fi

  # M4 — usage guards: no args and flag-first title both exit 2.
  local rc_noargs=0 rc_flagfirst=0
  "$script_path" >/dev/null 2>&1 || rc_noargs=$?
  "$script_path" -p 1 >/dev/null 2>&1 || rc_flagfirst=$?
  if [[ "$rc_noargs" == "2" && "$rc_flagfirst" == "2" ]]; then
    printf 'PASS M4\n'
  else
    printf 'FAIL M4: expected exit 2 for both guards, got no-args=%s flag-first=%s\n' \
      "$rc_noargs" "$rc_flagfirst"
    failures=$((failures + 1))
  fi

  # M5 — closed-exclusion: a CLOSED bd with the alpha title must not be
  # reused; the wrapper must create a fresh one.
  local rc_close=0 id5=""
  bd close "$id1" --reason "self-test" >/dev/null 2>&1 || rc_close=$?
  if [[ "$rc_close" != "0" ]]; then
    printf 'FAIL M5: bd close %s exited %s\n' "${id1:-<none>}" "$rc_close"
    failures=$((failures + 1))
  elif out="$("$script_path" "$title_a" "${flags[@]}")"; then
    reused="$(jq -r '.reused' <<<"$out" 2>/dev/null || true)"
    id5="$(jq -r '.id // empty' <<<"$out" 2>/dev/null || true)"
    if [[ "$reused" == "false" && -n "$id5" && "$id5" != "$id1" ]]; then
      printf 'PASS M5\n'
    else
      printf 'FAIL M5: expected new id != closed %s, got id=%s reused=%s\n' \
        "${id1:-<none>}" "${id5:-<none>}" "${reused:-<none>}"
      failures=$((failures + 1))
    fi
  else
    rc=$?
    printf 'FAIL M5: wrapper create-after-close exited %s\n' "$rc"
    failures=$((failures + 1))
  fi

  # M6 — oldest-wins: with two identical open titles seeded directly via
  # bd, the wrapper must reuse the OLDER by created_at.
  local title_c c_old c_new id6=""
  title_c="bdidem self-test gamma $$"
  bd create "$title_c" "${flags[@]}" --json >/dev/null 2>&1 || true
  bd create "$title_c" "${flags[@]}" --json >/dev/null 2>&1 || true
  c_old="$(bd list --json 2>/dev/null | jq -r --arg t "$title_c" \
    '[.[] | select(.title == $t)] | sort_by(.created_at) | (.[0].id // empty)' 2>/dev/null || true)"
  c_new="$(bd list --json 2>/dev/null | jq -r --arg t "$title_c" \
    '[.[] | select(.title == $t)] | sort_by(.created_at) | (.[1].id // empty)' 2>/dev/null || true)"
  if [[ -z "$c_old" || -z "$c_new" || "$c_old" == "$c_new" ]]; then
    printf 'FAIL M6: could not seed/read two identical titles (old=%s new=%s)\n' \
      "${c_old:-<none>}" "${c_new:-<none>}"
    failures=$((failures + 1))
  elif out="$("$script_path" "$title_c" "${flags[@]}")"; then
    reused="$(jq -r '.reused' <<<"$out" 2>/dev/null || true)"
    id6="$(jq -r '.id // empty' <<<"$out" 2>/dev/null || true)"
    if [[ "$reused" == "true" && "$id6" == "$c_old" ]]; then
      printf 'PASS M6\n'
    else
      printf 'FAIL M6: expected reuse of oldest %s, got id=%s reused=%s\n' \
        "$c_old" "${id6:-<none>}" "${reused:-<none>}"
      failures=$((failures + 1))
    fi
  else
    rc=$?
    printf 'FAIL M6: wrapper exited %s\n' "$rc"
    failures=$((failures + 1))
  fi

  if ((failures == 0)); then
    printf 'self-test: 6/6 passed\n'
    return 0
  fi
  printf 'self-test: %d/6 failed\n' "$failures"
  return 1
}

# ---------------------------------------------------------------------------
# Dispatch.
# ---------------------------------------------------------------------------
if [[ "${1:-}" == "--self-test" ]]; then
  [[ $# -eq 1 ]] || {
    printf 'error: --self-test takes no arguments\n' >&2
    usage
    exit 2
  }
  if self_test; then exit 0; else exit 1; fi
fi

if [[ $# -eq 0 ]]; then
  printf 'error: <title> is required\n' >&2
  usage
  exit 2
fi

title="$1"
shift
if [[ -z "$title" || "$title" == -* ]]; then
  printf 'error: <title> must be non-empty and must not start with "-"\n' >&2
  usage
  exit 2
fi

for cmd in bd jq date; do
  if ! command -v "$cmd" >/dev/null 2>&1; then
    printf 'error: required command not found: %s\n' "$cmd" >&2
    exit 1
  fi
done

window="${BD_CREATE_IDEMPOTENT_WINDOW:-600}"
if ! [[ "$window" =~ ^[0-9]+$ ]]; then
  printf 'error: BD_CREATE_IDEMPOTENT_WINDOW must be a non-negative integer, got %s\n' "$window" >&2
  exit 1
fi

now="$(date +%s)"
cutoff_epoch=$((now - window))
if ! cutoff="$(date -u -d "@$cutoff_epoch" +"%Y-%m-%dT%H:%M:%SZ")"; then
  printf 'error: could not compute RFC3339 cutoff from epoch %s\n' "$cutoff_epoch" >&2
  exit 1
fi

# Pre-check: fail-closed, never a blind create.
if ! list_json="$(bd list --created-after "$cutoff" --json)"; then
  printf 'error: bd list pre-check failed; refusing to create blindly\n' >&2
  exit 1
fi

if ! oldest_id="$(jq -r --arg t "$title" '
      map(select((.status // "open") != "closed" and .title == $t))
      | sort_by(.created_at)
      | (.[0].id // empty)
    ' <<<"$list_json")"; then
  printf 'error: could not parse bd list JSON; refusing to create blindly\n' >&2
  exit 1
fi

if [[ -n "$oldest_id" ]]; then
  if ! show_json="$(bd show "$oldest_id" --json)"; then
    printf 'error: bd show failed for reused candidate %s\n' "$oldest_id" >&2
    exit 1
  fi
  if ! reused_json="$(jq 'if type == "array" then (.[0] // error("empty array")) else . end | . + {reused: true}' <<<"$show_json")"; then
    printf 'error: could not parse bd show JSON for %s\n' "$oldest_id" >&2
    exit 1
  fi
  created_at="$(jq -r '.created_at // "unknown"' <<<"$reused_json")"
  printf 'reusing %s (identical open title created %s) — no new bd\n' "$oldest_id" "$created_at" >&2
  printf '%s\n' "$reused_json"
  exit 0
fi

# Create path: append --json unless the caller already asked for it.
args=(create "$title")
need_json=1
for arg in "$@"; do
  [[ "$arg" == "--json" ]] && need_json=0
  args+=("$arg")
done
if ((need_json)); then
  args+=(--json)
fi

set +e
create_out="$(bd "${args[@]}")"
rc=$?
set -e
if ((rc != 0)); then
  exit "$rc"
fi

if ! created_json="$(jq '. + {reused: false}' <<<"$create_out")"; then
  printf 'error: bd create succeeded but its output was not parseable JSON\n' >&2
  exit 1
fi
printf '%s\n' "$created_json"
exit 0
