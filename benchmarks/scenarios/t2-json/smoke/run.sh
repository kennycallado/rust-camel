#!/usr/bin/env bash
# t2-json smoke test (OpenSpec change bench-missing-cells, task 2.1).
# Bring-up smoke for the two rust artifacts — NOT a substitute for the
# full harness. Verifies, per run:
#   - exactly ONE marker `BENCH_ROUTE_READY bytes=<expected>` where
#     expected = BENCH_PAYLOAD_BYTES + 13
#   - the `BENCH_INPUT_SHA256=<digest>` line equals the canonical golden
#     digest for (size, 0)
# Runs:
#   1. lib fixture   @ 32768  (bytes=32781, golden a0db69e1...)
#   2. CLI artifact  @ 32768  via TEMPLATE COPY of routes/t2-json.yaml
#                             (same greps; both digests cross-checked)
#   3. lib fixture   @ 1024   per-class marker (bytes=1037)
#   4. JVM artifacts @ 32768  OPTIONAL — the task-2.4
#                             `cross_runtime_digest_equality` leg. Every
#                             runnable JVM artifact participates:
#                             camel-standalone-{dsl,yaml} fat jars and
#                             camel-quarkus-{dsl,yaml} fast-jar runners.
#                             Absent artifacts print "SKIP (not built)";
#                             the rust pair above stays MANDATORY.
# Exit 0 on full pass; 1 on any failure.
#
# Usage: bash benchmarks/scenarios/t2-json/smoke/run.sh
#        (from the scenario dir: bash smoke/run.sh)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCENARIO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# t2-json/smoke -> t2-json -> scenarios -> benchmarks -> <WORKTREE>
WORKTREE="$(cd "$SCENARIO_DIR/../../.." && pwd)"

GOLDEN_32768="a0db69e1146a29b0b25ca22435e51f39e271ecb1ac4ec1cee0ead3212eae10e9"
GOLDEN_1024="5abe5f00068356cad4e72f4d5e5e0a5d15d4a5cc9df8d0f22e22bf1448891b0f"
GOLDEN_262144="02adf20f21dc63217c9dc2e26b82101f96dbf311af5fbbf86e818e63d7171e27"
GOLDEN_1048576="9d4da9b244b6d12bed15d624ce426099da3126422285ecc584b9d3fff93a3abd"
PAYLOAD_BYTES="${BENCH_PAYLOAD_BYTES:-32768}"

# Golden digest for (PAYLOAD_BYTES, 0) — pure function of the size.
case "$PAYLOAD_BYTES" in
    1024)     GOLDEN_MAIN="$GOLDEN_1024" ;;
    32768)    GOLDEN_MAIN="$GOLDEN_32768" ;;
    262144)   GOLDEN_MAIN="$GOLDEN_262144" ;;
    1048576)  GOLDEN_MAIN="$GOLDEN_1048576" ;;
    *)
        echo "FAIL: BENCH_PAYLOAD_BYTES=$PAYLOAD_BYTES not in {1024,32768,262144,1048576}"
        exit 1
        ;;
esac

PASS=0
FAIL=0

# ── Toolchain resolution ─────────────────────────────────────────────
# Debug binaries match the `cargo build --workspace` gate artifacts.
# A host CARGO_TARGET_DIR would move them — unset it for every cargo
# call (same gotcha as the .cargo/config.toml fixtures document).

LIB_BIN="$WORKTREE/target/debug/t2-json"
if [[ ! -x "$LIB_BIN" ]]; then
    echo "--- building lib fixture (debug, worktree root) ---"
    (cd "$WORKTREE" && env -u CARGO_TARGET_DIR cargo build -p t2-json-rust-camel-lib --bin t2-json) || {
        echo "FAIL: cargo build -p t2-json-rust-camel-lib"
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

# run_and_check <label> <log> <expected_bytes> <expected_digest> <pid>
# Assumes the process is already running with output to <log>. Waits up
# to 30s for the marker, verifies exactly-one marker + digest, kills.
run_and_check() {
    local label="$1" log="$2" expected_bytes="$3" expected_digest="$4" pid="$5"
    local found=0
    local target="BENCH_ROUTE_READY bytes=${expected_bytes}"
    for _ in $(seq 1 300); do
        if grep -qF "$target" "$log" 2>/dev/null; then
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
        echo "FAIL: $label did not emit '$target' within 30s; log tail:"
        tail -5 "$log"
        FAIL=$((FAIL+1))
        return 1
    fi

    local marker_count
    marker_count=$(grep -cF "$target" "$log")
    if [[ "$marker_count" -ne 1 ]]; then
        echo "FAIL: $label emitted $marker_count '$target' markers (expected 1)"
        FAIL=$((FAIL+1))
        return 1
    fi

    local sha_count
    sha_count=$(grep -cF "BENCH_INPUT_SHA256=${expected_digest}" "$log")
    if [[ "$sha_count" -ne 1 ]]; then
        echo "FAIL: $label BENCH_INPUT_SHA256=${expected_digest} count=$sha_count (expected 1)"
        FAIL=$((FAIL+1))
        return 1
    fi

    echo "PASS: $label marker=$target sha=${expected_digest:0:12}..."
    PASS=$((PASS+1))
    return 0
}

# ── 1. lib fixture @ BENCH_PAYLOAD_BYTES (default 32768) ────────────
echo "=== t2-json smoke (payload=$PAYLOAD_BYTES) ==="
echo "--- rust-camel-lib @ $PAYLOAD_BYTES ---"
LIB_LOG="$SCRIPT_DIR/rust-camel-lib-$PAYLOAD_BYTES.log"
env BENCH_PAYLOAD_BYTES="$PAYLOAD_BYTES" "$LIB_BIN" > "$LIB_LOG" 2>&1 &
LIB_PID=$!
run_and_check "rust-camel-lib@$PAYLOAD_BYTES" "$LIB_LOG" \
    "$((PAYLOAD_BYTES + 13))" "$GOLDEN_MAIN" "$LIB_PID"

# ── 2. CLI artifact @ BENCH_PAYLOAD_BYTES via TEMPLATE COPY ─────────
if [[ "$PAYLOAD_BYTES" != "32768" ]]; then
    echo "SKIP: rust-camel-cli smoke is pinned to 32768 (got BENCH_PAYLOAD_BYTES=$PAYLOAD_BYTES)"
else
    echo "--- rust-camel-cli @ 32768 (template copy) ---"
    TMP_YAML="$(mktemp /tmp/t2-json-routes-XXXXXX.yaml)"
    sed -e "s/SIZE/32768/g" -e "s/GOLDEN/${GOLDEN_32768}/g" \
        "$SCENARIO_DIR/rust-camel-cli/routes/t2-json.yaml" > "$TMP_YAML"
    CLI_LOG="$SCRIPT_DIR/rust-camel-cli-32768.log"
    "$CAMEL_BIN" run --config "$SCENARIO_DIR/rust-camel-cli/Camel.toml" \
        --routes "$TMP_YAML" --no-watch > "$CLI_LOG" 2>&1 &
    CLI_PID=$!
    run_and_check "rust-camel-cli@32768" "$CLI_LOG" 32781 "$GOLDEN_32768" "$CLI_PID"
    rm -f "$TMP_YAML"

    # Cross-check: both artifacts reported the SAME input digest.
    lib_sha=$(grep -oF "BENCH_INPUT_SHA256=${GOLDEN_32768}" "$LIB_LOG" | head -1)
    cli_sha=$(grep -oF "BENCH_INPUT_SHA256=${GOLDEN_32768}" "$CLI_LOG" | head -1)
    if [[ -n "$lib_sha" && "$lib_sha" == "$cli_sha" ]]; then
        echo "PASS: lib and CLI input digests identical (${GOLDEN_32768:0:12}...)"
        PASS=$((PASS+1))
    else
        echo "FAIL: input digest cross-check (lib='${lib_sha:0:12}' cli='${cli_sha:0:12}')"
        FAIL=$((FAIL+1))
    fi
fi

# ── 3. lib fixture @ 1024 (per-class marker) ────────────────────────
echo "--- rust-camel-lib @ 1024 (per-class) ---"
LIB_LOG_1024="$SCRIPT_DIR/rust-camel-lib-1024.log"
env BENCH_PAYLOAD_BYTES=1024 "$LIB_BIN" > "$LIB_LOG_1024" 2>&1 &
LIB_PID_1024=$!
run_and_check "rust-camel-lib@1024" "$LIB_LOG_1024" 1037 "$GOLDEN_1024" "$LIB_PID_1024"

# ── 4. JVM artifacts @ 32768 (cross_runtime_digest_equality) ─────────
# OPTIONAL legs (task 2.4): every runnable JVM artifact participates;
# the rust pair in sections 1-3 remains MANDATORY. Pinned to 32768 so
# every runtime's digest lands on the same golden.
JAVA_BIN="$(command -v java 2>/dev/null || true)"
if [[ "$PAYLOAD_BYTES" != "32768" ]]; then
    echo "SKIP: JVM legs pinned to 32768 (got BENCH_PAYLOAD_BYTES=$PAYLOAD_BYTES)"
elif [[ -z "$JAVA_BIN" ]]; then
    echo "SKIP (no java on PATH): JVM legs skipped"
else
    declare -a JVM_LABELS=() JVM_JARS=() JVM_LOGS=()

    # camel-standalone: assembly-plugin fat jar from a prior
    # `mvn -q package` (glob — artifactId varies, version fixed 1.0.0).
    for name in camel-standalone-dsl camel-standalone-yaml; do
        jar="$(ls "$SCENARIO_DIR/camel-standalone/$name/target/"*-jar-with-dependencies.jar 2>/dev/null | head -1 || true)"
        if [[ -n "$jar" ]]; then
            JVM_LABELS+=("$name@32768 (standalone)")
            JVM_JARS+=("$jar")
            JVM_LOGS+=("$SCRIPT_DIR/$name-32768.log")
        else
            echo "SKIP (not built): camel-standalone $name jar-with-dependencies"
        fi
    done

    # camel-quarkus JVM mode: fast-jar launcher from a prior
    # `./gradlew :camel-quarkus-<x>:build` (quarkus-app/quarkus-run.jar;
    # the thin launcher runs the app in-process, so PID kill is scoped).
    for name in camel-quarkus-dsl camel-quarkus-yaml; do
        runner="$SCENARIO_DIR/camel-quarkus/$name/build/quarkus-app/quarkus-run.jar"
        if [[ -f "$runner" ]]; then
            JVM_LABELS+=("$name@32768 (quarkus-jvm)")
            JVM_JARS+=("$runner")
            JVM_LOGS+=("$SCRIPT_DIR/$name-32768.log")
        else
            echo "SKIP (not built): camel-quarkus $name quarkus-app/quarkus-run.jar"
        fi
    done

    if [[ "${#JVM_LABELS[@]}" -eq 0 ]]; then
        echo "SKIP: no runnable JVM artifacts (rust pair only)"
    else
        echo "--- JVM legs: ${#JVM_LABELS[@]} runnable artifact(s) @ 32768 ---"
        for i in "${!JVM_LABELS[@]}"; do
            echo "--- ${JVM_LABELS[$i]} ---"
            env BENCH_PAYLOAD_BYTES=32768 "$JAVA_BIN" -jar "${JVM_JARS[$i]}" \
                > "${JVM_LOGS[$i]}" 2>&1 &
            JVM_PID=$!
            run_and_check "${JVM_LABELS[$i]}" "${JVM_LOGS[$i]}" \
                32781 "$GOLDEN_32768" "$JVM_PID"
        done
    fi
fi

# ── Cleanup + summary ────────────────────────────────────────────────
# Stray runtime processes hold nothing the next run needs, but the CLI
# binary may linger after SIGKILL of the parent — sweep it. 2>/dev/null || true 2>/dev/null || true

echo
echo "=== t2-json smoke summary: $PASS pass, $FAIL fail ==="
if [[ "$FAIL" -gt 0 ]]; then
    exit 1
fi
exit 0
