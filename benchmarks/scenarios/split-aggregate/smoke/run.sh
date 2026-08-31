#!/usr/bin/env bash
# split-aggregate smoke test (OpenSpec change bench-missing-cells,
# task 2.3). Bring-up smoke for the two rust artifacts — NOT a
# substitute for the full harness. Verifies, per artifact:
#   - exactly ONE marker `BENCH_ROUTE_READY items=100`
#   - exactly ONE `BENCH_INPUT_SHA256=<digest>` line equal to the
#     canonical array golden digest
# Runs:
#   1. lib fixture   (programmatic routes, Pair A)
#   2. CLI artifact  via `camel run` + routes/split-aggregate.yaml
#                    (no template tokens — the array is fixed)
#   3. Node fixtures OPTIONAL (bench-node task 2.3): node-native
#                    (needs a node binary) and node-fastify (also
#                    needs node_modules — run `npm ci --omit=dev` in
#                    its dir first). Same greps — the fixed array
#                    makes INPUT parity trivial.
# Exit 0 on full pass; 1 on any failure.
#
# Usage: bash benchmarks/scenarios/split-aggregate/smoke/run.sh
#        (from the scenario dir: bash smoke/run.sh)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCENARIO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# split-aggregate/smoke -> split-aggregate -> scenarios -> benchmarks -> <WORKTREE>
WORKTREE="$(cd "$SCENARIO_DIR/../../.." && pwd)"

GOLDEN_ARRAY="123444b475c48473309ed966eb69896c6725429021a5a5d2e0eaa0a77a159316"
MARKER_TARGET="BENCH_ROUTE_READY items=100"

PASS=0
FAIL=0

# ── Toolchain resolution ─────────────────────────────────────────────
# Debug binaries match the `cargo build --workspace` gate artifacts.
# A host CARGO_TARGET_DIR would move them — unset it for every cargo
# call (same gotcha as the .cargo/config.toml fixtures document).

LIB_BIN="$WORKTREE/target/debug/split-aggregate"
if [[ ! -x "$LIB_BIN" ]]; then
    echo "--- building lib fixture (debug, worktree root) ---"
    (cd "$WORKTREE" && env -u CARGO_TARGET_DIR cargo build -p split-aggregate-rust-camel-lib --bin split-aggregate) || {
        echo "FAIL: cargo build -p split-aggregate-rust-camel-lib"
        exit 1
    }
fi

CAMEL_BIN="${CAMEL_BIN:-}"
if [[ -z "$CAMEL_BIN" ]]; then
    if [[ -x "$WORKTREE/target/release/camel" ]]; then
        CAMEL_BIN="$WORKTREE/target/release/camel"
    elif [[ -x "$WORKTREE/target/debug/camel" ]]; then
        CAMEL_BIN="$WORKTREE/target/debug/camel"
    else
        echo "--- building camel-cli (release, worktree root) ---"
        (cd "$WORKTREE" && env -u CARGO_TARGET_DIR cargo build -p camel-cli --release) || {
            echo "FAIL: cargo build -p camel-cli"
            exit 1
        }
        CAMEL_BIN="$WORKTREE/target/release/camel"
    fi
fi

# ── Helpers ──────────────────────────────────────────────────────────

# run_and_check <label> <log> <pid>
# Assumes the process is already running with output to <log>. Waits up
# to 30s for the marker, verifies exactly-one marker + digest, kills
# (PID-scoped only — no wide pkill).
run_and_check() {
    local label="$1" log="$2" pid="$3"
    local found=0
    for _ in $(seq 1 300); do
        if grep -qF "$MARKER_TARGET" "$log" 2>/dev/null; then
            found=1
            break
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "FAIL: $label process died before emitting the marker; log tail:"
            tail -5 "$log"
            FAIL=$((FAIL+1))
            return 1
        fi
        sleep 0.1
    done
    kill -9 "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true

    if [[ "$found" -ne 1 ]]; then
        echo "FAIL: $label did not emit '$MARKER_TARGET' within 30s; log tail:"
        tail -5 "$log"
        FAIL=$((FAIL+1))
        return 1
    fi

    local marker_count
    marker_count=$(grep -cF "$MARKER_TARGET" "$log")
    if [[ "$marker_count" -ne 1 ]]; then
        echo "FAIL: $label emitted $marker_count '$MARKER_TARGET' markers (expected 1)"
        FAIL=$((FAIL+1))
        return 1
    fi

    local sha_count
    sha_count=$(grep -cF "BENCH_INPUT_SHA256=${GOLDEN_ARRAY}" "$log")
    if [[ "$sha_count" -ne 1 ]]; then
        echo "FAIL: $label BENCH_INPUT_SHA256=${GOLDEN_ARRAY} count=$sha_count (expected 1)"
        FAIL=$((FAIL+1))
        return 1
    fi

    echo "PASS: $label marker=$MARKER_TARGET sha=${GOLDEN_ARRAY:0:12}..."
    PASS=$((PASS+1))
    return 0
}

# ── 1. lib fixture ───────────────────────────────────────────────────
echo "=== split-aggregate smoke ==="
echo "--- rust-camel-lib ---"
LIB_LOG="$SCRIPT_DIR/rust-camel-lib.log"
"$LIB_BIN" > "$LIB_LOG" 2>&1 &
LIB_PID=$!
run_and_check "rust-camel-lib" "$LIB_LOG" "$LIB_PID"

# ── 2. CLI artifact ──────────────────────────────────────────────────
echo "--- rust-camel-cli ---"
CLI_LOG="$SCRIPT_DIR/rust-camel-cli.log"
"$CAMEL_BIN" run --config "$SCENARIO_DIR/rust-camel-cli/Camel.toml" \
    --routes "$SCENARIO_DIR/rust-camel-cli/routes/split-aggregate.yaml" \
    --no-watch > "$CLI_LOG" 2>&1 &
CLI_PID=$!
run_and_check "rust-camel-cli" "$CLI_LOG" "$CLI_PID"

# ── 3. Node fixtures (bench-node task 2.3) ───────────────────────────
# OPTIONAL legs: need a node binary (NODE_BIN override, runner install
# path, PATH — same resolution chain as the harness); node-fastify
# additionally needs node_modules (npm ci --omit=dev in its dir). The
# input array is fixed, so no size env is passed and the log names
# carry no size suffix (scenario convention).
if [[ -n "${NODE_BIN:-}" ]]; then
    :
elif [[ -x /opt/node/bin/node ]]; then
    NODE_BIN=/opt/node/bin/node
else
    NODE_BIN="$(command -v node 2>/dev/null || echo "")"
fi
echo "--- node legs (node: ${NODE_BIN:-<missing>}) ---"
if [[ ! -x "$NODE_BIN" ]]; then
    echo "SKIP (no node binary): node legs skipped"
else
    echo "--- node-native ---"
    NODE_LOG="$SCRIPT_DIR/node-native.log"
    (cd "$SCENARIO_DIR/node-native" \
        && "$NODE_BIN" route.mjs) > "$NODE_LOG" 2>&1 &
    NODE_PID=$!
    run_and_check "node-native" "$NODE_LOG" "$NODE_PID"

    if [[ -d "$SCENARIO_DIR/node-fastify/node_modules" ]]; then
        echo "--- node-fastify ---"
        FASTIFY_LOG="$SCRIPT_DIR/node-fastify.log"
        (cd "$SCENARIO_DIR/node-fastify" \
            && "$NODE_BIN" route.mjs) > "$FASTIFY_LOG" 2>&1 &
        FASTIFY_PID=$!
        run_and_check "node-fastify" "$FASTIFY_LOG" "$FASTIFY_PID"
    else
        echo "SKIP: node-fastify prerequisites not present (need npm ci --omit=dev in $SCENARIO_DIR/node-fastify)"
    fi
fi

# ── Cleanup + summary ────────────────────────────────────────────────
# PID-scoped kills only (run_and_check); no wide pkill — a stray
# process holding nothing the next run needs is left alone.

echo
echo "=== split-aggregate smoke summary: $PASS pass, $FAIL fail ==="
if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
exit 0
