#!/usr/bin/env bash
# T3 HTTP server fixture — rust-camel-cli (Pair B partner, bd Task 3).
#
# Wrapper that:
# 1. Spawns the shared `camel run` binary with the T3 YAML route
#    (Pair B measurement target — the actual `camel run`).
# 2. Emits the listener-bound `BENCH_ROUTE_READY <unix_ms>` marker
#    once the child prints "CamelContext started" on stdout — the
#    deterministic rc-w1u9 post-bind signal (see "# Marker
#    mechanism" below).
# 3. Forwards child stdout to wrapper stdout so per-request
#    `BENCH_HTTP_REQUEST` lines are visible to the harness.
# 4. On exit (any catchable signal): forwards SIGTERM to the
#    camel child, then SIGKILL after a grace window, then
#    reaps the child + removes the temp dir.
#
# # Marker mechanism (rc-w1u9 production handshake)
#
# Under the rc-w1u9 production handshake (`ConsumerStartupMode::Explicit`),
# the child's `CamelContext::start()` blocks until `HttpConsumer` calls
# `ctx.mark_ready()` (after TcpListener::bind + axum spawn + route
# registration) or `mark_failed()` (bind error such as EADDRINUSE).
# The child's tracing subscriber (installed by `camel_config::context_ext`)
# emits "CamelContext started" on stdout AFTER `ctx.start()` returns
# Ok, and "Failed to start CamelContext: …" + exit code 1 on Err.
#
# Therefore:
# - Seeing "CamelContext started" on the child's stdout is a 1:1 signal
#   that THIS child's listener is bound and accepting — not an orphan,
#   not a sibling timer. The wrapper greps the child's stdout for this
#   string and emits the marker as soon as it appears.
# - If the child exits before printing that line (e.g. EADDRINUSE
#   because a stale orphan holds 8080), the wrapper exits with an
#   error and emits NO marker → the harness sees a hard failure
#   (the correct outcome, no silent test corruption).
#
# This replaces the pre-rc-w1u9 wrapper's UNGATED TCP probe, which
# could silently succeed against an orphan while the child died with
# EADDRINUSE (see the T3 report's "Review fixes — BLOCKED on missing
# listener-bound hook" section for the failure mode). It also
# replaces the obvious-but-racy "TCP probe + child liveness check"
# combination, which suffers a brief startup window where the child
# is alive but its bind has not yet failed — during that window the
# probe could hit the orphan. Grepping for the child's own
# post-bind log line eliminates the race entirely.
#
# # Why not rely on the wrapper's TCP probe at all
#
# The wrapper cannot distinguish its own child's listener from a
# stale orphan's listener via TCP probe alone — both accept
# connections on 127.0.0.1:8080. The rc-w1u9 handshake moves the
# distinction into the child's own process state: only the child
# that successfully bound will print "CamelContext started". The
# wrapper reads that signal from the child's stdout, which is
# immune to the orphan-port race.
#
# # Why bash (not a Rust binary)
# An earlier Rust wrapper using tokio's signal handling did
# not fire reliably on SIGTERM before the runtime's
# default-action terminate took over. Bash traps are simpler
# and well-understood: TERM/INT/HUP/EXIT always propagate
# when installed before the signal arrives.
#
# # Harness kill strategy (resolved — bd rc-8ysn)
# The harness uses `_kill_process_tree_recursive` at every kill
# point (M1 post-marker, M2 post-window, kill_contender, cleanup
# trap). This walks /proc/<pid>/task/<tid>/children recursively
# and kills descendants deepest-first, so the setsid'd camel child
# is reached even though the wrapper's EXIT trap cannot catch
# SIGKILL. The wrapper's own cleanup function (trap on EXIT/INT/
# TERM/HUP) remains as a fallback for graceful termination paths
# (e.g. the child exits on its own, or the harness sends SIGTERM).
#
# # Harness argv
# The harness runs this script with:
#   <bin>/http-server-cli-wrapper.sh \
#     --camel-bin <path/to/camel> \
#     --config <fixture>/Camel.toml \
#     --routes <fixture>/routes/http-server.yaml

set -o pipefail

# Literal string the child prints on stdout AFTER ctx.start()
# returns Ok (rc-w1u9: only after HttpConsumer's mark_ready fires,
# i.e. after TcpListener::bind + axum spawn + route registration).
# Emitted by `camel_core::lifecycle::application::context_lifecycle`,
# which writes via the tracing subscriber installed in
# `camel_config::context_ext` (writer = std::io::stdout).
CHILD_READY_LINE="CamelContext started"
# Failure signature: child prints this via tracing::error! when
# ctx.start() returns Err (e.g. EADDRINUSE). Used only as a
# diagnostic in the wrapper's error message.
CHILD_FAIL_LINE="Failed to start CamelContext"

MARKER_DEADLINE_MS=30000
POLL_INTERVAL_S=0.01

camel_bin=""
config=""
routes=""

usage() {
    cat <<'EOF' >&2
usage: http-server-cli-wrapper.sh --camel-bin <path> --config <Camel.toml> --routes <routes.yaml>
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
        # Forward a graceful SIGTERM first so the child can do
        # orderly cleanup (camel-cli closes the listener and
        # drops in-flight exchanges). Then SIGKILL after a
        # 1s grace window if the child hasn't exited.
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

# Spawn the child. stdout+stderr go to temp files (not a
# pipe) so we can tail-follow them in background tasks
# without blocking the child. A pipe would block the child
# when the kernel's pipe buffer fills, which would add
# jitter to the route's log latency.
#
# We exec `setsid` so the camel child becomes the leader of
# its own session. This means the wrapper's process group
# and the child's process group are distinct — the harness's
# TERM-then-KILL pattern can therefore target the wrapper
# without cascading to the child.
work_dir="$(mktemp -d -t t3-cli-wrapper-XXXXXX)"
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

# Forward child stderr + stdout to the wrapper's streams
# immediately so the harness sees the child's tracing output
# (including the eventual "CamelContext started" line). These
# forwarders run in the background for the lifetime of the
# wrapper; they get EOF on the temp files when the child
# exits and terminate on their own.
(
    tail -n +1 -F "$child_stderr" 2>/dev/null
) &
stderr_tail_pid=$!
(
    tail -n +1 -F "$child_stdout" 2>/dev/null
) &
stdout_tail_pid=$!

# Wait for the child's "CamelContext started" line on stdout
# (the rc-w1u9 post-bind signal) OR for the child to exit
# before printing it (which under rc-w1u9 means ctx.start()
# returned Err — typically EADDRINUSE because a stale orphan
# holds 8080). Either branch breaks the loop; the marker is
# emitted ONLY in the success branch.
deadline_start_ms=$(( $(date +%s%N) / 1000000 ))
marker_emitted=0
child_failed_diag=""
while :; do
    if grep -qF "$CHILD_READY_LINE" "$child_stdout" 2>/dev/null; then
        # Child printed "CamelContext started" → under rc-w1u9,
        # this child's listener is bound and accepting. Emit
        # the marker; the harness's M1 clock stops here.
        unix_ms=$(date +%s%3N)
        echo "BENCH_ROUTE_READY $unix_ms"
        marker_emitted=1
        break
    fi
    if ! kill -0 "$child_pid" 2>/dev/null; then
        # Child exited before printing the ready line.
        # Surface the failure signature (if present) to make
        # the EADDRINUSE / bind-error diagnosis obvious.
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
        echo "error: BENCH_ROUTE_READY marker NOT emitted within ${MARKER_DEADLINE_MS}ms — child did not print '${CHILD_READY_LINE}' on stdout" >&2
        echo "--- child stderr ---" >&2
        cat "$child_stderr" >&2 2>/dev/null || true
        break
    fi
    sleep "$POLL_INTERVAL_S"
done

if [[ "$marker_emitted" -ne 1 ]]; then
    # Cleanup will fire via the EXIT trap.
    exit 1
fi

# Wait for the child. When the child exits, the tail
# forwarders will get EOF and exit on their own; the cleanup
# trap kills the child + removes the temp dir.
wait "$child_pid"
exit_code=$?

exit "$exit_code"
