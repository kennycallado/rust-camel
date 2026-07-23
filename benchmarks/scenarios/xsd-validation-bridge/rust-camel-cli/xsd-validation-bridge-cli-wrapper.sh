#!/usr/bin/env bash
# T4b XSD validation bridge fixture — rust-camel-cli (Pair B partner, bd Task 4).
#
# Wrapper that:
# 1. Spawns the shared `camel run` binary with the T4a YAML route
#    (Pair B measurement target).
# 2. Configures camel-bridge to spawn the bridge via shared/
#    bridge-wrapper.sh, which writes the bridge PID to
#    $V3_BRIDGE_PID_FILE before exec'ing the real binary (spec §4.11).
# 3. Emits `BENCH_ROUTE_READY <unix_ms>` once the child prints
#    "CamelContext started" on stdout (rc-w1u9 post-start signal —
#    same pattern as Task 3's http-server-cli-wrapper.sh).
# 4. The CLI's bench_instrument module (gated on BENCH_LATENCY_FILE
#    env var) writes `BENCH_LATENCY <id> <duration_ns>` directly to the
#    tmpfs latency file. The wrapper only forwards stdout for tracing.
# 5. On exit: SIGTERM→SIGKILL the camel child + bridge subprocess,
#    reap, clean up temp dir. The harness uses tree-aware kill
#    (_kill_process_tree_recursive) so the setsid'd camel child
#    is reached even under SIGKILL (bd rc-8ysn).
#
# # How BENCH_LATENCY is emitted
# The CLI's bench_instrument module (crates/camel-cli/src/commands/
# bench_instrument.rs), gated on BENCH_LATENCY_FILE env var, wraps
# every top-level `.to()` step with Instant-bracketing processors.
# The post-`.to()` processor writes `BENCH_LATENCY <id> <ns>` directly
# to the file. No wrapper-side awk or id assignment — the CLI owns the
# full contract (resolves bd rc-5gcu).
#
# # Harness argv
# The harness / smoke runs this script with:
#   xsd-validation-bridge-cli-wrapper.sh \
#     --camel-bin <path/to/camel> \
#     --config <fixture>/Camel.toml \
#     --routes <fixture>/routes/xsd-bench.yaml \
#     --bridge-binary <worktree>/bridges/xml/build/native/xml-bridge \
#     --bridge-wrapper <scenario>/shared/bridge-wrapper.sh \
#     --bridge-pid-file /tmp/v3-bridge-pid-t4b-rust-camel-cli.txt \
#     --latency-file /tmp/v3-protocol-b-t4b-rust-camel-cli.log

set -o pipefail

CHILD_READY_LINE="CamelContext started"
CHILD_FAIL_LINE="Failed to start CamelContext"

MARKER_DEADLINE_MS=30000
POLL_INTERVAL_S=0.01

camel_bin=""
config=""
routes=""
bridge_binary=""
bridge_wrapper=""
bridge_pid_file=""
latency_file="/tmp/v3-protocol-b-t4b-rust-camel-cli.log"

usage() {
    cat <<'EOF' >&2
usage: xsd-validation-bridge-cli-wrapper.sh
  --camel-bin <path/to/camel>
  --config <fixture>/Camel.toml
  --routes <fixture>/routes/xsd-bench.yaml
  --bridge-binary <worktree>/bridges/xml/build/native/xml-bridge
  --bridge-wrapper <scenario>/shared/bridge-wrapper.sh
  --bridge-pid-file /tmp/v3-bridge-pid-<cell>.txt
  --latency-file /tmp/v3-protocol-b-<cell>.log
EOF
    exit 2
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --camel-bin)        camel_bin="${2:-}"; shift 2 ;;
        --config)           config="${2:-}"; shift 2 ;;
        --routes)           routes="${2:-}"; shift 2 ;;
        --bridge-binary)    bridge_binary="${2:-}"; shift 2 ;;
        --bridge-wrapper)   bridge_wrapper="${2:-}"; shift 2 ;;
        --bridge-pid-file)  bridge_pid_file="${2:-}"; shift 2 ;;
        --latency-file)     latency_file="${2:-}"; shift 2 ;;
        -h|--help)          usage ;;
        *)                  echo "error: unexpected argument: $1" >&2; usage ;;
    esac
done

if [[ -z "$camel_bin" || -z "$config" || -z "$routes" ]]; then
    echo "error: --camel-bin, --config, and --routes are required" >&2
    usage
fi

if [[ ! -x "$camel_bin" ]]; then
    echo "error: --camel-bin path is not executable: $camel_bin" >&2
    exit 2
fi

# Pre-create / truncate the latency file.
: >"$latency_file" 2>/dev/null || true
if [[ -n "$bridge_pid_file" ]]; then
    rm -f "$bridge_pid_file" 2>/dev/null || true
fi

# Export env vars read by camel-bridge (CAMEL_XML_BRIDGE_BINARY_PATH)
# and by bridge-wrapper.sh (CAMEL_XML_BRIDGE_REAL_BINARY,
# V3_BRIDGE_PID_FILE). The bridge spawns lazily on the first xslt:
# invocation; it reads these env vars at spawn time.
if [[ -n "$bridge_wrapper" ]]; then
    if [[ ! -x "$bridge_wrapper" ]]; then
        echo "error: --bridge-wrapper not executable: $bridge_wrapper" >&2
        exit 2
    fi
    export CAMEL_XML_BRIDGE_BINARY_PATH="$bridge_wrapper"
fi
if [[ -n "$bridge_binary" ]]; then
    if [[ ! -x "$bridge_binary" ]]; then
        echo "error: --bridge-binary not executable: $bridge_binary" >&2
        exit 2
    fi
    export CAMEL_XML_BRIDGE_REAL_BINARY="$bridge_binary"
fi
if [[ -n "$bridge_pid_file" ]]; then
    export V3_BRIDGE_PID_FILE="$bridge_pid_file"
fi

# State for the cleanup handler.
work_dir=""
child_pid=""
stdout_tail_pid=""
stderr_tail_pid=""

cleanup() {
    if [[ -n "$child_pid" ]]; then
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
        wait "$child_pid" 2>/dev/null || true
    fi
    # Kill the bridge subprocess if its PID was recorded.
    if [[ -n "$bridge_pid_file" && -f "$bridge_pid_file" ]]; then
        local bp
        bp=$(cat "$bridge_pid_file" 2>/dev/null || true)
        if [[ -n "$bp" ]]; then
            kill -TERM "$bp" 2>/dev/null || true
            sleep 0.2
            kill -KILL "$bp" 2>/dev/null || true
        fi
    fi
    [[ -n "$stdout_tail_pid" ]] && kill -TERM "$stdout_tail_pid" 2>/dev/null || true
    [[ -n "$stderr_tail_pid" ]] && kill -TERM "$stderr_tail_pid" 2>/dev/null || true
    if [[ -n "$work_dir" && -d "$work_dir" ]]; then
        rm -rf "$work_dir"
    fi
}
trap cleanup EXIT INT TERM HUP

# Spawn the child. stdout+stderr go to temp files so we can
# tail-follow them without blocking the child (pipe buffer fill
# would add jitter to per-tick latency).
work_dir="$(mktemp -d -t t4b-cli-wrapper-XXXXXX)"
child_stdout="$work_dir/child.stdout"
child_stderr="$work_dir/child.stderr"
: >"$child_stdout"
: >"$child_stderr"

# Export BENCH_LATENCY_FILE so the CLI's bench_instrument module opens
# this file and writes BENCH_LATENCY <id> <ns> records directly (no awk).
export BENCH_LATENCY_FILE="$latency_file"

setsid "$camel_bin" run \
    --config "$config" \
    --routes "$routes" \
    --no-watch \
    >"$child_stdout" 2>"$child_stderr" &
child_pid=$!

echo "info: spawned child 'camel run' with pid $child_pid" >&2

# Forward child stderr to wrapper stderr (best-effort visibility).
(
    tail -n +1 -F "$child_stderr" 2>/dev/null
) &
stderr_tail_pid=$!

# Forward child stdout verbatim to wrapper stdout (so harness / smoke
# sees tracing output). The CLI writes BENCH_LATENCY records directly
# to $BENCH_LATENCY_FILE via the bench_instrument module — no awk
# processing needed here.
(
    tail -n +1 -F "$child_stdout" 2>/dev/null
) &
stdout_tail_pid=$!

# Wait for the child's "CamelContext started" line on stdout.
deadline_start_ms=$(( $(date +%s%N) / 1000000 ))
marker_emitted=0
child_failed_diag=""
while :; do
    if grep -qF "$CHILD_READY_LINE" "$child_stdout" 2>/dev/null; then
        unix_ms=$(date +%s%3N)
        echo "BENCH_ROUTE_READY $unix_ms"
        marker_emitted=1
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
        echo "error: BENCH_ROUTE_READY marker NOT emitted within ${MARKER_DEADLINE_MS}ms" >&2
        echo "--- child stderr ---" >&2
        cat "$child_stderr" >&2 2>/dev/null || true
        break
    fi
    sleep "$POLL_INTERVAL_S"
done

if [[ "$marker_emitted" -ne 1 ]]; then
    exit 1
fi

# Wait for the child. Cleanup fires via EXIT trap.
wait "$child_pid"
exit_code=$?

exit "$exit_code"
