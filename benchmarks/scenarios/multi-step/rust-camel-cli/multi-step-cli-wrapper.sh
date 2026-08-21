#!/usr/bin/env bash
# Multi-step benchmark fixture — rust-camel-cli wrapper
# (loadgen-connections-multistep-bench, Task 2.1).
#
# Wrapper that:
# 1. Spawns the shared `camel run` binary with the multi-step YAML
#    route (the measurement target).
# 2. Waits for the child's "CamelContext started" stdout line — the
#    deterministic rc-w1u9 post-bind signal (CHILD-ready, NOT the
#    public marker; see http-server-cli-wrapper.sh for the full
#    handshake rationale this mirrors).
# 3. BEFORE emitting anything public, runs the PREFLIGHT: one POST
#    `ping` against the route, asserting HTTP 200, header
#    `X-Bench-Stage: two`, and body exactly `PING-M1-M2`. Only
#    complete, ordered execution of every route step can produce
#    that triple (see routes/multi-step.yaml header). `fail()` kills
#    the child and exits nonzero — no marker is emitted on failure.
# 4. ONLY after the preflight passes, prints
#    `BENCH_ROUTE_READY <unix_ms>` (unix epoch milliseconds).
# 5. Forwards child stdout/stderr to wrapper stdout/stderr.
# 6. On exit (any catchable signal): forwards SIGTERM to the camel
#    child, then SIGKILL after a grace window, then reaps the child
#    + removes the temp dir.
#
# # Harness argv
#   <fixture>/multi-step-cli-wrapper.sh \
#     --camel-bin <path/to/camel> \
#     --config <fixture>/Camel.toml \
#     --routes <fixture>/routes/multi-step.yaml

set -o pipefail

# Literal string the child prints on stdout AFTER ctx.start()
# returns Ok (rc-w1u9: only after HttpConsumer's mark_ready fires,
# i.e. after TcpListener::bind + axum spawn + route registration).
CHILD_READY_LINE="CamelContext started"
# Failure signature: child prints this via tracing::error! when
# ctx.start() returns Err (e.g. EADDRINUSE). Diagnostic only.
CHILD_FAIL_LINE="Failed to start CamelContext"

MARKER_DEADLINE_MS=30000
POLL_INTERVAL_S=0.01

camel_bin=""
config=""
routes=""

usage() {
    cat <<'EOF' >&2
usage: multi-step-cli-wrapper.sh --camel-bin <path> --config <Camel.toml> --routes <routes.yaml>
EOF
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --camel-bin) camel_bin="${2:-}"; shift 2 ;;
        --config)    config="${2:-}";    shift 2 ;;
        --routes)    routes="${2:-}";    shift 2 ;;
        -h|--help)   usage ;;
        *)           echo "error: unexpected argument: $1" >&2; usage ;;
    esac
done

if [[ -z "$camel_bin" || -z "$config" || -z "$routes" ]]; then
    echo "error: --camel-bin, --config, and --routes are all required" >&2
    usage
fi

if [[ ! -x "$camel_bin" ]]; then
    echo "error: --camel-bin path is not executable: $camel_bin" >&2
    exit 2
fi

# State for the cleanup handler. Declared BEFORE the trap is
# installed (the trap body references these).
work_dir=""
child_pid=""
stdout_tail_pid=""
stderr_tail_pid=""

cleanup() {
    if [[ -n "$child_pid" ]]; then
        # Graceful SIGTERM first, SIGKILL after a 1s grace window.
        kill -TERM "$child_pid" 2>/dev/null || true
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            if ! kill -0 "$child_pid" 2>/dev/null; then
                break
            fi
            sleep 0.1
        done
        if kill -0 "$child_pid" 2>/dev/null; then
            kill -KILL "$child_pid" 2>/dev/null || true
        fi
        # Reap the child to avoid zombies.
        wait "$child_pid" 2>/dev/null || true
    fi
    [[ -n "$stdout_tail_pid" ]] && kill -TERM "$stdout_tail_pid" 2>/dev/null || true
    [[ -n "$stderr_tail_pid" ]] && kill -TERM "$stderr_tail_pid" 2>/dev/null || true
    if [[ -n "$work_dir" && -d "$work_dir" ]]; then
        rm -rf "$work_dir"
    fi
}
trap cleanup EXIT INT TERM HUP

# Preflight failure: report the response for diagnosis, kill the
# child (via the EXIT trap's cleanup — TERM then KILL), exit nonzero.
# The public marker is NEVER emitted on this path.
fail() {
    echo "error: multi-step preflight FAILED — killing child, no BENCH_ROUTE_READY emitted" >&2
    if [[ -n "${RESP:-}" ]]; then
        echo "--- preflight response ---" >&2
        printf '%s\n' "$RESP" >&2
    fi
    exit 1
}

# Spawn the child. stdout+stderr go to temp files (not a pipe) so we
# can tail-follow them in background tasks without blocking the child.
# setsid gives the child its own session/process group (harness can
# TERM the wrapper without cascading, and its recursive /proc walker
# still reaches the child — bd rc-8ysn).
work_dir="$(mktemp -d -t multistep-cli-wrapper-XXXXXX)"
child_stdout="$work_dir/child.stdout"
child_stderr="$work_dir/child.stderr"
: >"$child_stdout"
: >"$child_stderr"

setsid "$camel_bin" run \
    --config "$config" \
    --routes "$routes" \
    --no-watch \
    >"$child_stdout" 2>"$child_stderr" &
child_pid=$!

echo "info: spawned child 'camel run' with pid $child_pid" >&2

# Forward child stderr + stdout to the wrapper's streams immediately.
(
    tail -n +1 -F "$child_stderr" 2>/dev/null
) &
stderr_tail_pid=$!
(
    tail -n +1 -F "$child_stdout" 2>/dev/null
) &
stdout_tail_pid=$!

# Wait for the child's "CamelContext started" line on stdout (the
# CHILD-ready signal — NOT the public marker) OR for the child to
# exit before printing it, OR for the deadline to expire.
deadline_start_ms=$(( $(date +%s%N) / 1000000 ))
child_ready=0
child_failed_diag=""
while :; do
    if grep -qF "$CHILD_READY_LINE" "$child_stdout" 2>/dev/null; then
        child_ready=1
        break
    fi
    if ! kill -0 "$child_pid" 2>/dev/null; then
        child_failed_diag=$(grep -F "$CHILD_FAIL_LINE" "$child_stderr" "$child_stdout" 2>/dev/null | head -1 || true)
        echo "error: child exited before printing '${CHILD_READY_LINE}' on stdout" >&2
        if [[ -n "$child_failed_diag" ]]; then
            echo "error: child failure signature: ${child_failed_diag}" >&2
        fi
        echo "--- child stderr ---" >&2
        cat "$child_stderr" >&2 2>/dev/null || true
        break
    fi
    now_ms=$(( $(date +%s%N) / 1000000 ))
    elapsed_ms=$(( now_ms - deadline_start_ms ))
    if (( elapsed_ms > MARKER_DEADLINE_MS )); then
        echo "error: child did not print '${CHILD_READY_LINE}' within ${MARKER_DEADLINE_MS}ms" >&2
        echo "--- child stderr ---" >&2
        cat "$child_stderr" >&2 2>/dev/null || true
        break
    fi
    sleep "$POLL_INTERVAL_S"
done

if [[ "$child_ready" -ne 1 ]]; then
    # Cleanup will fire via the EXIT trap.
    exit 1
fi

# ── PREFLIGHT (before anything public) ────────────────────────────
# One POST `ping`; only complete, ordered execution of every route
# step yields 200 + X-Bench-Stage: two + body PING-M1-M2.
# Note: hyper emits header NAMES lowercase on the wire
# (`x-bench-stage: two`); the header assertion is case-insensitive
# per HTTP semantics (RFC 9110 §5.1 field names are case-insensitive).
RESP="$(curl -sS --max-time 10 -i -X POST --data ping http://127.0.0.1:8081/bench-multi)" || fail
echo "$RESP" | grep -q '^HTTP/.* 200' || fail
echo "$RESP" | grep -qi 'x-bench-stage: two' || fail
[ "$(echo "$RESP" | tail -1)" = "PING-M1-M2" ] || fail

# Preflight passed — NOW emit the public marker. The harness's clock
# and the loadgen readiness gate stop here.
unix_ms=$(date +%s%3N)
echo "BENCH_ROUTE_READY $unix_ms"

# Wait for the child. When it exits, the tail forwarders get EOF and
# exit on their own; the cleanup trap kills the child + removes the
# temp dir.
wait "$child_pid"
exit_code=$?

exit "$exit_code"
