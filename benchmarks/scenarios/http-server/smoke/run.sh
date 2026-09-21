#!/usr/bin/env bash
# T3 smoke test: builds (if needed) and smoke-tests the T3 artifacts.
# NOT a substitute for the full M1/M2 harness (which is the Task 5
# concern); this is a quick bring-up smoke that verifies the marker +
# 200/pong contract end-to-end for each fixture.
#
# Assertion policy (e_opus ruling D2, 2026-09-16; bd rc-h42s6): the
# HARD assertions are the BENCH_ROUTE_READY marker and the 200/pong
# response body. The per-request `id=1` check is WARN-only
# observability: the fixtures are minimal-bare (e_opus ruling D1) and
# no longer emit per-request stdout lines at all. The previously
# committed smoke logs were deleted for exactly this reason — they
# showed `received`/`id=1` lines the aligned fixtures can no longer
# produce; fresh smoke evidence is regenerated against live cells as
# part of the era-3 gate.
#
# Usage: bash benchmarks/scenarios/http-server/smoke/run.sh [artifact]
#   [artifact]  optional filter — run ONLY the matching artifact's
#               case (e.g. axum-bare); with no argument the full
#               artifact set runs exactly as before.
# Exit code 0 on full pass; 1 on any failure.

set -uo pipefail

# Pre-flight: `nc` (netcat) is required to POST /bench against
# the running artifacts. Smoke fails fast with a clear message
# if it's missing (NixOS hosts don't always ship netcat by
# default; `nix-shell -p ncat` or `nix-shell -p netcat` is the
# usual fix).
command -v nc >/dev/null 2>&1 || {
    echo "FAIL: nc (netcat) required for smoke test — install via 'nix-shell -p ncat' or your distro's netcat package"
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCENARIO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
# http-server/smoke -> http-server -> scenarios -> benchmarks -> <WORKTREE>
WORKTREE="$(cd "$SCENARIO_DIR/../../.." && pwd)"

# Optional first-arg artifact filter (change bench-axum-bare task 4.1):
# when set, only the matching artifact case runs; with no argument
# every case runs as before (the dispatch below is wrapped, not
# replaced).
ARTIFACT_FILTER="${1:-}"
filter_allows() {
    [[ -z "$ARTIFACT_FILTER" || "$1" == "$ARTIFACT_FILTER" ]]
}

# Toolchain: assumes the same env as the v1 / T2 harness
# (JAVA_HOME=/tmp/rc-f3g9-jdk21, GRADLE in ~/.gradle/wrapper).
JAVA_HOME="${JAVA_HOME:-/tmp/rc-f3g9-jdk21}"
GRADLE="${GRADLE:-/home/kenny/.gradle/wrapper/dists/gradle-8.10-bin/deqhafrv1ntovfmgh0nh3npr9/gradle-8.10/bin/gradle}"
M2_CACHE="${M2_CACHE:-/tmp/m2-cache}"
DOCKER_MVN_IMAGE="${DOCKER_MVN_IMAGE:-maven:3.9-eclipse-temurin-21}"
CAMEL_BIN="${CAMEL_BIN:-$WORKTREE/target/release/camel}"
RUST_LIB_BIN="$WORKTREE/benchmarks/contenders/rust-camel-lib/target/release/rust-camel-lib-fixture"
# axum-bare reference contender (change bench-axum-bare task 4.1):
# fixture-local target pin, resolved relative to the worktree root.
AXUM_BARE_BIN="$WORKTREE/benchmarks/contenders/axum-bare/target/release/axum-bare-fixture"
# Node binary: same resolution chain as the harness (bench-node task
# 1.1/1.2) — NODE_BIN env override, runner install path, PATH.
if [[ -n "${NODE_BIN:-}" ]]; then
    :
elif [[ -x /opt/node/bin/node ]]; then
    NODE_BIN=/opt/node/bin/node
else
    NODE_BIN="$(command -v node 2>/dev/null || echo "<missing:node>")"
fi
# Consolidated node contender tree (change bench-consol-tick task
# 1.3): per-scenario entry scripts, shared node_modules at its root.
NODE_CONTENDER_DIR="$WORKTREE/benchmarks/contenders/node"
RUST_CLI_WRAPPER="$SCENARIO_DIR/rust-camel-cli/http-server-cli-wrapper.sh"
STAND_DSL_JAR="$SCENARIO_DIR/camel-standalone/camel-standalone-dsl/target/camel-standalone-dsl-1.0.0-jar-with-dependencies.jar"
STAND_YAML_JAR="$SCENARIO_DIR/camel-standalone/camel-standalone-yaml/target/camel-standalone-yaml-1.0.0-jar-with-dependencies.jar"
QD_JAR="$SCENARIO_DIR/camel-quarkus/camel-quarkus-dsl/build/quarkus-app/quarkus-run.jar"
QY_JAR="$SCENARIO_DIR/camel-quarkus/camel-quarkus-yaml/build/quarkus-app/quarkus-run.jar"
QD_NATIVE="$SCENARIO_DIR/camel-quarkus/camel-quarkus-dsl-native/build/camel-quarkus-dsl-native-1.0.0-runner"
QY_NATIVE="$SCENARIO_DIR/camel-quarkus/camel-quarkus-yaml-native/build/camel-quarkus-yaml-native-1.0.0-runner"

PASS=0
FAIL=0
FAILED_ARTIFACTS=()
# Raw response of the last post_smoke_port call (axum-bare case);
# the case body assembles its transcript from it.
LAST_SMOKE_RESPONSE=""

# Pre-flight: free port 8080. Kill any existing process and
# wait until the port is actually released (TIME_WAIT can
# hold it for 30-60s after a TCP server dies; SO_REUSEADDR
# helps, but for the smoke test we just wait it out).
free_port_8080() {
    pkill -9 -f 'camel\|http-server\|jetty\|quarkus' 2>/dev/null || true
    for _ in $(seq 1 100); do
        if ! ss -tlnp 2>/dev/null | grep -q ":8080"; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}
free_port_8080

# Pick a free TCP port (axum-bare case): random high port with no
# listener, same ss idiom as free_port_8080. The fixture's bind is
# the real arbiter — if the picked port is sniped in the race
# window the fixture exits 1 and the marker wait fails loud.
pick_free_port() {
    local port
    while :; do
        port=$(( (RANDOM % 20000) + 30000 ))
        if ! ss -tln 2>/dev/null | grep -q ":${port} "; then
            printf '%s\n' "$port"
            return 0
        fi
    done
}

post_smoke() {
    local artifact="$1"
    local pid="$2"

    if ! kill -0 "$pid" 2>/dev/null; then
        echo "  FAIL: $artifact exited unexpectedly"
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$artifact")
        return 1
    fi

    # POST /bench body=ping
    local resp
    resp=$(printf 'POST /bench HTTP/1.1\r\nHost: 127.0.0.1:8080\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping' \
        | timeout 2 nc 127.0.0.1 8080 2>/dev/null \
        | tail -1)
    if [[ "$resp" != "pong" ]]; then
        echo "  FAIL: $artifact POST /bench returned '$resp' (expected 'pong')"
        kill -9 "$pid" 2>/dev/null
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$artifact")
        return 1
    fi
    echo "  PASS: $artifact POST /bench → 200/pong"
    return 0
}

# Port-parameterized post_smoke variant (axum-bare reference case,
# change bench-axum-bare task 4.1): the fixture binds a picked free
# port instead of the fixed 8080, so the POST target is a parameter.
# post_smoke() and its call sites stay untouched. The raw response is
# kept in LAST_SMOKE_RESPONSE for the caller, which assembles the
# axum-bare.log transcript.
post_smoke_port() {
    local artifact="$1"
    local pid="$2"
    local port="$3"

    if ! kill -0 "$pid" 2>/dev/null; then
        echo "  FAIL: $artifact exited unexpectedly"
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$artifact")
        return 1
    fi

    # POST /bench body=ping
    local resp
    resp=$(printf 'POST /bench HTTP/1.1\r\nHost: 127.0.0.1:%s\r\nContent-Length: 4\r\nConnection: close\r\n\r\nping' "$port" \
        | timeout 2 nc 127.0.0.1 "$port" 2>/dev/null)
    LAST_SMOKE_RESPONSE="$resp"
    local status_line body_line
    status_line="$(head -n1 <<<"$resp")"
    body_line="$(tail -n1 <<<"$resp")"
    if [[ "$status_line" != *"200"* || "$body_line" != *"pong"* ]]; then
        echo "  FAIL: $artifact POST /bench on port $port returned '$status_line'/'$body_line' (expected 200/pong)"
        kill -9 "$pid" 2>/dev/null
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$artifact")
        return 1
    fi
    echo "  PASS: $artifact POST /bench → 200/pong"
    return 0
}

# Per-request id observability (e_opus ruling D2, 2026-09-16; bd
# rc-h42s6): under the minimal-bare fixture shape (D1) NO fixture
# emits per-request stdout lines, so this check is WARN-only for
# every artifact — a missing `id=1` never increments FAIL or
# FAILED_ARTIFACTS; a present `id=1` gets an informational PASS line.
# (The old per-artifact distinction — rust-camel-cli partial pass on
# the static `received` line — is moot: nothing emits `received`
# anymore.) The HARD assertions stay in post_smoke: marker presence +
# the 200/pong body.
verify_request_id() {
    local label="$1"
    local log="$SCRIPT_DIR/${label}.log"

    if ! grep -qE "BENCH_HTTP_REQUEST id=1\$" "$log" 2>/dev/null; then
        echo "  WARN: $label log has no 'BENCH_HTTP_REQUEST id=1' (expected under minimal-bare fixtures, e_opus D2 — observability only)"
        return 0
    fi
    echo "  PASS: $label log contains 'BENCH_HTTP_REQUEST id=1'"
    return 0
}

smoke_artifact() {
    local label="$1"
    local pid
    local log="$SCRIPT_DIR/${label}.log"

    case "$label" in
        rust-camel-lib)
            "$RUST_LIB_BIN" http-server > "$log" 2>&1 &
            pid=$!
            ;;
        rust-camel-cli)
            "$RUST_CLI_WRAPPER" \
                --camel-bin "$CAMEL_BIN" \
                --config "$SCENARIO_DIR/rust-camel-cli/Camel.toml" \
                --routes "$SCENARIO_DIR/rust-camel-cli/routes/http-server.yaml" \
                > "$log" 2>&1 &
            pid=$!
            ;;
        camel-standalone-dsl)
            "$JAVA_HOME/bin/java" -jar "$STAND_DSL_JAR" > "$log" 2>&1 &
            pid=$!
            ;;
        camel-standalone-yaml)
            "$JAVA_HOME/bin/java" -jar "$STAND_YAML_JAR" > "$log" 2>&1 &
            pid=$!
            ;;
        camel-quarkus-dsl)
            "$JAVA_HOME/bin/java" -jar "$QD_JAR" > "$log" 2>&1 &
            pid=$!
            ;;
        camel-quarkus-yaml)
            "$JAVA_HOME/bin/java" -jar "$QY_JAR" > "$log" 2>&1 &
            pid=$!
            ;;
        camel-quarkus-dsl-native)
            "$QD_NATIVE" > "$log" 2>&1 &
            pid=$!
            ;;
        camel-quarkus-yaml-native)
            "$QY_NATIVE" > "$log" 2>&1 &
            pid=$!
            ;;
        node-native)
            "$NODE_BIN" "$NODE_CONTENDER_DIR/node-native/http-server.mjs" > "$log" 2>&1 &
            pid=$!
            ;;
        node-fastify)
            "$NODE_BIN" "$NODE_CONTENDER_DIR/node-fastify/http-server.mjs" > "$log" 2>&1 &
            pid=$!
            ;;
        *)
            echo "  FAIL: unknown artifact '$label'"
            FAIL=$((FAIL+1))
            return 1
            ;;
    esac

    # Wait for BENCH_ROUTE_READY (up to 30s)
    for _ in $(seq 1 300); do
        if grep -qF "BENCH_ROUTE_READY" "$log" 2>/dev/null; then
            break
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "  FAIL: $label process died before emitting marker"
            tail -20 "$log"
            FAIL=$((FAIL+1))
            FAILED_ARTIFACTS+=("$label")
            return 1
        fi
        sleep 0.1
    done

    if ! grep -qF "BENCH_ROUTE_READY" "$log" 2>/dev/null; then
        echo "  FAIL: $label did not emit BENCH_ROUTE_READY within 30s"
        kill -9 "$pid" 2>/dev/null
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$label")
        return 1
    fi

    local marker_count
    marker_count=$(grep -cF "BENCH_ROUTE_READY" "$log")
    if [[ "$marker_count" -ne 1 ]]; then
        echo "  FAIL: $label emitted $marker_count markers (expected 1)"
        kill -9 "$pid" 2>/dev/null
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("$label")
        return 1
    fi
    echo "  PASS: $label marker count = 1"

    if post_smoke "$label" "$pid"; then
        PASS=$((PASS+1))
    fi
    # Per-request id observability (e_opus ruling D2): WARN-only,
    # never fails. Runs after post_smoke so any emitted line would
    # already be on disk.
    verify_request_id "$label"
    # Aggressive cleanup: kill all processes whose cmdline
    # matches the artifact binary, then the original pid,
    # then anything left on 8080. The JVM and Quarkus
    # native processes spawn many threads, and the Jetty
    # accept thread holds the port — pkill -P doesn't
    # recurse, and a single SIGKILL on the main process
    # doesn't always release the port immediately. We use
    # the artifact's own cmdline as the kill pattern.
    case "$label" in
        rust-camel-lib)
            pkill -9 -f "$RUST_LIB_BIN" 2>/dev/null || true
            ;;
        rust-camel-cli)
            pkill -9 -f 'http-server-cli-wrapper.sh' 2>/dev/null || true
            pkill -9 -f 'target/release/camel' 2>/dev/null || true
            ;;
        camel-standalone-dsl)
            pkill -9 -f 'camel-standalone-dsl-1.0.0-jar-with-dependencies.jar' 2>/dev/null || true
            ;;
        camel-standalone-yaml)
            pkill -9 -f 'camel-standalone-yaml-1.0.0-jar-with-dependencies.jar' 2>/dev/null || true
            ;;
        camel-quarkus-dsl|camel-quarkus-dsl-native)
            pkill -9 -f 'camel-quarkus-dsl' 2>/dev/null || true
            ;;
        camel-quarkus-yaml|camel-quarkus-yaml-native)
            pkill -9 -f 'camel-quarkus-yaml' 2>/dev/null || true
            ;;
        node-native)
            pkill -9 -f 'node-native/http-server.mjs' 2>/dev/null || true
            ;;
        node-fastify)
            pkill -9 -f 'node-fastify/http-server.mjs' 2>/dev/null || true
            ;;
    esac
    kill -9 "$pid" 2>/dev/null || true
    # Wait for the main process to exit.
    for _ in $(seq 1 50); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done
    # Wait for port 8080 to be released.
    free_port_8080
    return 0
}

# axum-bare reference-contender case (change bench-axum-bare task 4.1).
# Self-contained: the fixture binds a picked free port (not 8080), the
# POST goes through post_smoke_port, and the transcript assembles here
# (fixture stdout + the POST response status line and body). Liveness
# evidence only — the log carries markers, never timing-like numbers.
smoke_axum_bare() {
    local log="$SCRIPT_DIR/axum-bare.log"
    local port pid

    port="$(pick_free_port)"
    BENCH_AXUM_BARE_PORT="$port" "$AXUM_BARE_BIN" > "$log" 2>&1 &
    pid=$!

    # Wait for BENCH_ROUTE_READY (up to 30s) — same wait-loop idiom
    # as smoke_artifact.
    for _ in $(seq 1 300); do
        if grep -qF "BENCH_ROUTE_READY" "$log" 2>/dev/null; then
            break
        fi
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "  FAIL: axum-bare process died before emitting marker"
            tail -20 "$log"
            FAIL=$((FAIL+1))
            FAILED_ARTIFACTS+=("axum-bare")
            return 1
        fi
        sleep 0.1
    done

    if ! grep -qF "BENCH_ROUTE_READY" "$log" 2>/dev/null; then
        echo "  FAIL: axum-bare did not emit BENCH_ROUTE_READY within 30s"
        kill -9 "$pid" 2>/dev/null
        FAIL=$((FAIL+1))
        FAILED_ARTIFACTS+=("axum-bare")
        return 1
    fi
    echo "  PASS: axum-bare BENCH_ROUTE_READY marker"

    if post_smoke_port axum-bare "$pid" "$port"; then
        PASS=$((PASS+1))
    fi

    # Per-request stdout lines: WARN-only observability (e_opus
    # ruling D2) — the minimal-bare fixture (D1) emits none. Same
    # policy as verify_request_id; inline because the axum-bare case
    # bypasses post_smoke.
    if grep -qF "BENCH_HTTP_REQUEST received" "$log" \
        && grep -qE "BENCH_HTTP_REQUEST id=1\$" "$log"; then
        echo "  PASS: axum-bare emitted 'BENCH_HTTP_REQUEST received' + 'id=1'"
    else
        echo "  WARN: axum-bare emitted no per-request lines (expected under minimal-bare, e_opus D2 — observability only)"
    fi

    kill -9 "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do
        if ! kill -0 "$pid" 2>/dev/null; then
            break
        fi
        sleep 0.1
    done

    # Transcript: append the POST response status line + body. Full
    # headers are dropped (the Date stamp is a time-shaped value the
    # liveness log must not carry).
    {
        head -n1 <<<"${LAST_SMOKE_RESPONSE:-}"
        tail -n1 <<<"${LAST_SMOKE_RESPONSE:-}"
    } >> "$log"
    return 0
}

echo "=== T3 HTTP server smoke test ==="
echo "Scenario dir: $SCENARIO_DIR"
echo

if [[ -x "$RUST_LIB_BIN" ]]; then
    if filter_allows rust-camel-lib; then
        echo "--- rust-camel-lib ---"
        smoke_artifact rust-camel-lib
    fi
else
    echo "SKIP: rust-camel-lib binary not found at $RUST_LIB_BIN"
fi

if [[ -x "$CAMEL_BIN" && -x "$RUST_CLI_WRAPPER" && -f "$SCENARIO_DIR/rust-camel-cli/Camel.toml" ]]; then
    if filter_allows rust-camel-cli; then
        echo "--- rust-camel-cli (via wrapper) ---"
        smoke_artifact rust-camel-cli
    fi
else
    echo "SKIP: rust-camel-cli prerequisites not present"
fi

if [[ -f "$STAND_DSL_JAR" ]]; then
    if filter_allows camel-standalone-dsl; then
        echo "--- camel-standalone-dsl ---"
        smoke_artifact camel-standalone-dsl
    fi
else
    echo "SKIP: $STAND_DSL_JAR not built"
fi

if [[ -f "$STAND_YAML_JAR" ]]; then
    if filter_allows camel-standalone-yaml; then
        echo "--- camel-standalone-yaml ---"
        smoke_artifact camel-standalone-yaml
    fi
else
    echo "SKIP: $STAND_YAML_JAR not built"
fi

if [[ -f "$QD_JAR" ]]; then
    if filter_allows camel-quarkus-dsl; then
        echo "--- camel-quarkus-dsl ---"
        smoke_artifact camel-quarkus-dsl
    fi
else
    echo "SKIP: $QD_JAR not built"
fi

if [[ -f "$QY_JAR" ]]; then
    if filter_allows camel-quarkus-yaml; then
        echo "--- camel-quarkus-yaml ---"
        smoke_artifact camel-quarkus-yaml
    fi
else
    echo "SKIP: $QY_JAR not built"
fi

if [[ -x "$QD_NATIVE" ]]; then
    if filter_allows camel-quarkus-dsl-native; then
        echo "--- camel-quarkus-dsl-native ---"
        smoke_artifact camel-quarkus-dsl-native
    fi
else
    echo "SKIP: $QD_NATIVE not built"
fi

if [[ -x "$QY_NATIVE" ]]; then
    if filter_allows camel-quarkus-yaml-native; then
        echo "--- camel-quarkus-yaml-native ---"
        smoke_artifact camel-quarkus-yaml-native
    fi
else
    echo "SKIP: $QY_NATIVE not built"
fi

if [[ -x "$NODE_BIN" && -f "$NODE_CONTENDER_DIR/node-native/http-server.mjs" ]]; then
    if filter_allows node-native; then
        echo "--- node-native ---"
        smoke_artifact node-native
    fi
else
    echo "SKIP: node binary or node-native fixture not present ($NODE_BIN)"
fi

if [[ -x "$NODE_BIN" && -f "$NODE_CONTENDER_DIR/node-fastify/http-server.mjs" && -d "$NODE_CONTENDER_DIR/node_modules" ]]; then
    if filter_allows node-fastify; then
        echo "--- node-fastify ---"
        smoke_artifact node-fastify
    fi
else
    echo "SKIP: node-fastify prerequisites not present (need node + npm ci)"
fi

if [[ -x "$AXUM_BARE_BIN" ]]; then
    if filter_allows axum-bare; then
        echo "--- axum-bare ---"
        smoke_axum_bare
    fi
else
    echo "SKIP: axum-bare binary not found at $AXUM_BARE_BIN"
fi

# Guard against a typo'd filter arg matching no case: a nonempty
# filter with zero PASS/FAIL would otherwise exit 0 vacuously.
if [[ -n "$ARTIFACT_FILTER" && "$PASS" -eq 0 && "$FAIL" -eq 0 ]]; then
    echo "error: no case matched filter '$ARTIFACT_FILTER'" >&2
    exit 1
fi

echo
echo "=== Summary: $PASS pass, $FAIL fail ==="
if [[ "$FAIL" -gt 0 ]]; then
    echo "Failed: ${FAILED_ARTIFACTS[*]}"
    exit 1
fi
exit 0
