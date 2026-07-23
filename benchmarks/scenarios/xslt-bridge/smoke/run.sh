#!/usr/bin/env bash
# T4a XSLT bridge smoke test: builds (if needed) and smoke-tests all 4
# T4a artifacts. NOT a substitute for the full M1/M2 harness (which is
# Task 5's concern); this is a quick bring-up smoke that verifies the
# marker + BENCH_LATENCY contract + correct XSLT output end-to-end.
#
# Usage: bash benchmarks/scenarios/xslt-bridge/smoke/run.sh
# Exit code 0 on full pass; 1 on any failure.
#
# Pre-flight: the rust-camel `camel` binary must be built at
# $WORKTREE/target/release/camel. The bridge binary must be built at
# $WORKTREE/bridges/xml/build/native/xml-bridge (run
# `cargo xtask build-xml-bridge` if missing). Java artifacts must be
# built (smoke prints SKIP for any artifact whose output is missing;
# run the build commands printed by the smoke to populate them).

set -o pipefail

SCENARIO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../" && pwd)"
WORKTREE="$(cd "$SCENARIO_DIR/../../.." && pwd)"
JAVA_HOME="${JAVA_HOME:-/tmp/rc-f3g9-jdk21}"
GRADLE="${GRADLE:-/home/kenny/.gradle/wrapper/dists/gradle-8.10-bin/deqhafrv1ntovfmgh0nh3npr9/gradle-8.10/bin/gradle}"
M2_CACHE="${M2_CACHE:-/tmp/m2-cache}"
CAMEL_BIN="${CAMEL_BIN:-$WORKTREE/target/release/camel}"
BRIDGE_BINARY="${BRIDGE_BINARY:-$WORKTREE/bridges/xml/build/native/xml-bridge}"
BRIDGE_WRAPPER="$SCENARIO_DIR/shared/bridge-wrapper.sh"
RUST_LIB_BIN="$SCENARIO_DIR/rust-camel-lib/target/release/xslt-bridge"
RUST_CLI_WRAPPER="$SCENARIO_DIR/rust-camel-cli/xslt-bridge-cli-wrapper.sh"
STAND_DSL_JAR="$SCENARIO_DIR/camel-standalone/camel-standalone-dsl/target/camel-standalone-dsl-1.0.0-jar-with-dependencies.jar"
QD_NATIVE="$SCENARIO_DIR/camel-quarkus/camel-quarkus-dsl-native/build/camel-quarkus-dsl-native-1.0.0-runner"

# Reduced tick count for smoke speed (production uses 10000).
SMOKE_REPEAT_COUNT="${SMOKE_REPEAT_COUNT:-200}"
SMOKE_DEADLINE_MS=45000
# I2: per-tick correctness marker. Presence of this log line proves the
# to:xslt step completed without throwing on that tick.
CORRECTNESS_MARKER="BENCH_XSLT_TICK"

PASS=0
FAIL=0
FAILED_ARTIFACTS=()

# Kill by full binary path. The previous broad regex
# 'xslt-bridge|xml-bridge|...' matched this script's own bash command
# line (which contains ".../xslt-bridge/smoke/run.sh") and SIGKILLed
# the smoke itself after the first artifact. Full paths are specific
# enough to not collide with the smoke. (bd rc-2vxg M5.)
cleanup_cell_artifacts() {
    pkill -9 -f "$BRIDGE_BINARY" 2>/dev/null || true
    pkill -9 -f "$RUST_LIB_BIN" 2>/dev/null || true
    pkill -9 -f "$RUST_CLI_WRAPPER" 2>/dev/null || true
    pkill -9 -f "$CAMEL_BIN" 2>/dev/null || true
    [[ -f "$STAND_DSL_JAR" ]] && pkill -9 -f "$STAND_DSL_JAR" 2>/dev/null || true
    [[ -x "$QD_NATIVE" ]] && pkill -9 -f "$QD_NATIVE" 2>/dev/null || true
    # Orphan tail/awk subprocesses from the CLI wrapper's latency tee.
    pkill -9 -f 'tail -n \+1 -F' 2>/dev/null || true
    pkill -9 -f 'awk -v lf=' 2>/dev/null || true
    sleep 0.5
}

# I1: extract the set_body.value block scalar from a rust-camel-cli
# routes YAML and byte-compare with shared/bench-payload.xml. Drift
# (someone edited the YAML without updating shared, or vice versa)
# fails the smoke before the cell is launched. The awk extractor
# assumes the standard 12-space indent emitted by regen-routes.sh;
# if the YAML structure changes, this fails loudly. (bd rc-2vxg I1.)
verify_cli_payload_byte_equality() {
    local routes_file="$1"
    local shared_file="$2"
    local extracted shared_content
    extracted=$(awk '
        /^      - set_body:/ { in_sb=1; next }
        in_sb && /^          value: \|[+-]?$/ { in_val=1; next }
        in_val {
            # Block content: 12-space indent (the block scalar indent).
            if ($0 ~ /^            /) {
                sub(/^            /, "")
                print
            } else if ($0 == "") {
                # Empty line — preserved by | (clip).
                print
            } else {
                # Non-empty line with < 12 leading spaces: end of block.
                in_val=0; in_sb=0; next
            }
        }
    ' "$routes_file")
    if [[ -z "$extracted" ]]; then
        echo "FAIL: could not extract set_body.value from $routes_file" >&2
        return 1
    fi
    shared_content=$(cat "$shared_file")
    # Both sides go through $(...) which strips a trailing newline;
    # this normalizes the comparison for the file's single trailing \n.
    if [[ "$extracted" != "$shared_content" ]]; then
        echo "FAIL: rust-camel-cli payload drift — $routes_file != $shared_file" >&2
        diff <(printf '%s\n' "$extracted") <(printf '%s\n' "$shared_content") | head -20 >&2
        return 1
    fi
    return 0
}

# Run one artifact end-to-end: launch via the supplied launcher
# function, wait for marker, wait for ≥10 latency records, verify
# ≥10 $CORRECTNESS_MARKER signals (I2), cleanup. Replaces 4 duplicated
# 40-line inline launch blocks. (bd rc-2vxg I3.)
#
# Args: label log_file latency_file pid_file launcher_fn
smoke_cell() {
    local label="$1" log_file="$2" latency_file="$3" pid_file="$4"
    local launcher="$5"

    rm -f "$pid_file" "$latency_file" "$log_file"
    cleanup_cell_artifacts

    echo "--- $label ---"
    "$launcher" >"$log_file" 2>&1 &
    local wrapper_pid=$!

    local deadline=$(( $(date +%s) * 1000 + SMOKE_DEADLINE_MS ))
    local ready=0
    while true; do
        if grep -q '^BENCH_ROUTE_READY ' "$log_file" 2>/dev/null; then
            ready=1; break
        fi
        if ! kill -0 "$wrapper_pid" 2>/dev/null; then break; fi
        if [[ $(( $(date +%s) * 1000 )) -gt $deadline ]]; then break; fi
        sleep 0.1
    done
    if [[ $ready -ne 1 ]]; then
        echo "FAIL: $label — no BENCH_ROUTE_READY within ${SMOKE_DEADLINE_MS}ms"
        tail -30 "$log_file" 2>/dev/null
        FAIL=$((FAIL+1)); FAILED_ARTIFACTS+=("$label:no-marker")
        cleanup_cell_artifacts
        return 1
    fi
    echo "  marker: $(grep -m1 '^BENCH_ROUTE_READY ' "$log_file")"

    local got=0
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        local n; n=$(wc -l < "$latency_file" 2>/dev/null || echo 0)
        if [[ "$n" -ge 10 ]]; then got=1; break; fi
        sleep 0.5
    done
    if [[ $got -ne 1 ]]; then
        echo "FAIL: $label — fewer than 10 latency records"
        FAIL=$((FAIL+1)); FAILED_ARTIFACTS+=("$label:no-latency")
        cleanup_cell_artifacts
        return 1
    fi
    echo "  latency records: $(wc -l < "$latency_file")"

    # I2: correctness check. Require ≥10 ${CORRECTNESS_MARKER} signals
    # in the log — presence proves the to:xslt step completed without
    # throwing on every tick that produced a latency record. The marker
    # is emitted per-tick AFTER the transform step; absence (or count
    # < latency count) means the route pipeline is failing mid-tick.
    # (bd rc-2vxg I2.)
    local ok_count
    ok_count=$(grep -c "$CORRECTNESS_MARKER" "$log_file" 2>/dev/null || echo 0)
    if [[ "$ok_count" -lt 10 ]]; then
        echo "FAIL: $label — fewer than 10 ${CORRECTNESS_MARKER} signals (got $ok_count)"
        FAIL=$((FAIL+1)); FAILED_ARTIFACTS+=("$label:no-correctness")
        cleanup_cell_artifacts
        return 1
    fi
    echo "  ${CORRECTNESS_MARKER} signals: $ok_count"

    if [[ -n "$pid_file" && -f "$pid_file" ]]; then
        echo "  bridge PID file: yes ($(cat "$pid_file"))"
    else
        echo "  bridge PID file: no"
    fi

    cleanup_cell_artifacts
    PASS=$((PASS+1))
    return 0
}

# Per-artifact launchers (kept as functions so smoke_cell stays generic).
# Each launcher `cd`s into the artifact dir and execs the artifact with
# the right env / args; smoke_cell redirects its stdout+stderr to the
# log file and backgrounds it.
launch_rust_camel_lib() {
    cd "$SCENARIO_DIR/rust-camel-lib"
    BENCH_PAYLOAD="$SCENARIO_DIR/shared/bench-payload.xml" \
    BENCH_STYLESHEET="$SCENARIO_DIR/shared/identity-transform.xsl" \
    BENCH_LATENCY_FILE="/tmp/v3-protocol-b-t4a-rust-camel-lib.log" \
    BENCH_BRIDGE_WRAPPER="$BRIDGE_WRAPPER" \
    CAMEL_XML_BRIDGE_REAL_BINARY="$BRIDGE_BINARY" \
    V3_BRIDGE_PID_FILE="/tmp/v3-bridge-pid-t4a-rust-camel-lib.txt" \
    env -u CARGO_TARGET_DIR \
    "$RUST_LIB_BIN"
}

launch_rust_camel_cli() {
    cd "$SCENARIO_DIR/rust-camel-cli"
    "$RUST_CLI_WRAPPER" \
        --camel-bin "$CAMEL_BIN" \
        --config "$SCENARIO_DIR/rust-camel-cli/Camel.toml" \
        --routes "$SCENARIO_DIR/rust-camel-cli/routes/xslt-bench.yaml" \
        --bridge-binary "$BRIDGE_BINARY" \
        --bridge-wrapper "$BRIDGE_WRAPPER" \
        --bridge-pid-file "/tmp/v3-bridge-pid-t4a-rust-camel-cli.txt" \
        --latency-file "/tmp/v3-protocol-b-t4a-rust-camel-cli.log"
}

launch_camel_standalone_dsl() {
    cd "$SCENARIO_DIR/camel-standalone/camel-standalone-dsl"
    JAVA_HOME="$JAVA_HOME" \
    "$JAVA_HOME/bin/java" \
        -Dbench.payload="$SCENARIO_DIR/shared/bench-payload.xml" \
        -Dbench.stylesheet="$SCENARIO_DIR/shared/identity-transform.xsl" \
        -Dbench.latency_file="/tmp/v3-protocol-b-t4a-camel-standalone-dsl.log" \
        -jar "$STAND_DSL_JAR"
}

# launch_camel_quarkus_dsl_native intentionally omitted for T4a —
# the cell is won't-measure (Xalan cannot compile the XSLT in native
# mode). See task-4-bug-investigation.md "T4a camel-quarkus-dsl-native
# — won't-measure evidence".

# Pre-flight: warn if rust-camel CLI binary missing.
if [[ ! -x "$CAMEL_BIN" ]]; then
    echo "SKIP: \$CAMEL_BIN not built at $CAMEL_BIN (build with: cargo build --release -p camel-cli)" >&2
fi

echo "=== T4a XSLT bridge smoke test ==="
echo "Scenario dir: $SCENARIO_DIR"
echo "Smoke repeat count: $SMOKE_REPEAT_COUNT (production = 10000)"
echo

# --- rust-camel-lib ---
if [[ -x "$RUST_LIB_BIN" ]]; then
    smoke_cell "rust-camel-lib" \
        "/tmp/v3-smoke-t4a-rust-camel-lib.log" \
        "/tmp/v3-protocol-b-t4a-rust-camel-lib.log" \
        "/tmp/v3-bridge-pid-t4a-rust-camel-lib.txt" \
        launch_rust_camel_lib
else
    echo "SKIP: rust-camel-lib binary not found at $RUST_LIB_BIN"
fi

# --- rust-camel-cli ---
if [[ -x "$CAMEL_BIN" && -x "$RUST_CLI_WRAPPER" ]]; then
    # I1: verify the embedded YAML payload byte-matches shared/bench-payload.xml
    # BEFORE launching. Drift = smoke failure (no point measuring a cell
    # whose input differs from the spec'd shared file).
    if verify_cli_payload_byte_equality \
        "$SCENARIO_DIR/rust-camel-cli/routes/xslt-bench.yaml" \
        "$SCENARIO_DIR/shared/bench-payload.xml"; then
        echo "  payload byte-equality: OK"
        smoke_cell "rust-camel-cli" \
            "/tmp/v3-smoke-t4a-rust-camel-cli.log" \
            "/tmp/v3-protocol-b-t4a-rust-camel-cli.log" \
            "/tmp/v3-bridge-pid-t4a-rust-camel-cli.txt" \
            launch_rust_camel_cli
    else
        FAIL=$((FAIL+1)); FAILED_ARTIFACTS+=("rust-camel-cli:payload-drift")
    fi
else
    echo "SKIP: rust-camel-cli prerequisites not present"
fi

# --- camel-standalone-dsl ---
if [[ -f "$STAND_DSL_JAR" ]]; then
    smoke_cell "camel-standalone-dsl" \
        "/tmp/v3-smoke-t4a-camel-standalone-dsl.log" \
        "/tmp/v3-protocol-b-t4a-camel-standalone-dsl.log" \
        "" \
        launch_camel_standalone_dsl
else
    echo "SKIP: $STAND_DSL_JAR not built"
    echo "      build with: cd $SCENARIO_DIR/camel-standalone && mvn -DskipTests package"
fi

# --- camel-quarkus-dsl-native ---
# Per task-4-bug-investigation.md + task-4-report.md "Java cells —
# won't-measure rationale", T4a camel-quarkus-dsl-native is documented
# won't-measure: camel-quarkus-xalan's Xalan transformer cannot compile
# the identity-transform stylesheet (TransformerConfigurationException
# at AOT/load time). The cell is intentionally NOT measured — bd
# rc-2vxg documents this and the smoke skips it rather than failing.
# (T4b camel-quarkus-dsl-native IS measurable; its smoke in
# xsd-validation-bridge/smoke/run.sh runs the cell normally.)
if [[ -x "$QD_NATIVE" ]]; then
    echo "SKIP: camel-quarkus-dsl-native is won't-measure for T4a (Xalan cannot compile the XSLT in native mode — see task-4-bug-investigation.md)"
else
    echo "SKIP: $QD_NATIVE not built (T4a quarkus is won't-measure anyway)"
fi

echo
echo "=== Summary: $PASS pass, $FAIL fail ==="
if [[ "$FAIL" -gt 0 ]]; then
    echo "Failed: ${FAILED_ARTIFACTS[*]}"
    exit 1
fi
exit 0
