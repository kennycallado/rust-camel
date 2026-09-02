#!/usr/bin/env bash
# Build all benchmark artifacts inside the container.
# Called by run-all.sh before invoking the harness.
#
# Builds:
#   - Rust: rust-camel-lib (consolidated fixture, one crate) + camel-cli
#     + loadgen + bridge
#   - Node: shared node_modules at benchmarks/contenders/node (one
#     package.json serves all 14 node cells)
#   - Maven: camel-standalone-{dsl,yaml} jars per scenario
#   - Quarkus native: deferred to the harness (build_native_artifact)
#
# Idempotent: cargo and mvn skip up-to-date builds.

set -euo pipefail
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/../../.." && pwd)}"
cd "$REPO_ROOT"

echo "=== Building Rust artifacts ==="

# rust-camel-lib: ONE consolidated crate builds every scenario's route
# builder (change bench-consol-tick task 1.2). The crate's
# .cargo/config.toml pins a fixture-local target dir, so build from
# inside the crate dir (cargo config discovery is cwd-based) with
# CARGO_TARGET_DIR unset — otherwise the host's shared-cache override
# would send the binary to the wrong place.
echo "  → rust-camel-lib-fixture (consolidated)"
(cd "$REPO_ROOT"/benchmarks/contenders/rust-camel-lib \
    && env -u CARGO_TARGET_DIR cargo build --release -p rust-camel-lib-fixture 2>&1 | tail -3)

# Shared rust-camel-cli binary (used by all scenarios via route YAML).
echo "  → camel-cli (shared)"
(cd "$REPO_ROOT" && env -u CARGO_TARGET_DIR cargo build --release -p camel-cli 2>&1 | tail -3)

# Load generator (M2 Protocol A).
echo "  → bench-loadgen"
(cd "$REPO_ROOT" && env -u CARGO_TARGET_DIR cargo build --release -p bench-loadgen 2>&1 | tail -3) || true

# XML bridge binary (T4a/T4b — bridges/xml).
if [[ -d "$REPO_ROOT/bridges/xml" ]]; then
    echo "  → bridges/xml"
    # Bridge may already be built (104MB native binary); skip if present.
    BRIDGE_BIN="$REPO_ROOT/bridges/xml/build/native/xml-bridge"
    if [[ ! -x "$BRIDGE_BIN" ]]; then
        echo "    (bridge binary not found — build may require manual setup)"
    else
        echo "    (bridge binary already present, skipping)"
    fi
fi

echo ""
echo "=== Building Node runtime (shared) ==="

# All node contenders (node-native + node-fastify) share ONE
# package.json at benchmarks/contenders/node/ (bench-consol-tick
# task 1.3): a single npm ci materializes the shared node_modules
# that serves all 14 node cells. package-lock.json is committed, so
# the install is pinned. Idempotent like cargo/mvn: npm ci is a
# no-op-cost reinstall of the same pinned tree.
if [[ -f "$REPO_ROOT/benchmarks/contenders/node/package.json" ]]; then
    echo "  → node runtime (shared node_modules)"
    npm ci --omit=dev --prefix "$REPO_ROOT/benchmarks/contenders/node"
fi

echo ""
echo "=== Building Maven artifacts ==="

# camel-standalone per-scenario jars.
for standalone_dir in "$REPO_ROOT"/benchmarks/scenarios/*/camel-standalone; do
    [[ -d "$standalone_dir" ]] || continue
    scenario=$(basename "$(dirname "$standalone_dir")")
    for sub in "$standalone_dir"/*/; do
        [[ -d "$sub" ]] || continue
        sub_name=$(basename "$sub")
        # Locate this module's packaged standalone jar (task 3.3 / rc-ld1o).
        jar=$(ls "$sub"/target/*-jar-with-dependencies.jar 2>/dev/null | head -n1)
        if [[ -n "$jar" ]]; then
            # Jar exists: rebuild only when a source/resource under src/ is
            # newer than it — otherwise a stale jar silently survives a
            # fixture edit (stale 1605/1632ms markers, rc-ld1o). Skip only
            # when fresh.
            if [[ -z "$(find "$sub/src" -newer "$jar" -print -quit 2>/dev/null)" ]]; then
                echo "  → $scenario/$sub_name (fresh, skipping)"
                continue
            fi
            echo "  → $scenario/$sub_name (stale, rebuilding)"
        else
            echo "  → $scenario/$sub_name (jar missing, building)"
        fi
        (cd "$sub" && mvn package -DskipTests -q 2>&1 | tail -3)
    done
done

echo ""
echo "=== Build summary ==="
echo "Rust binaries:"
# (find || true): find exits nonzero if ANY scenario lacks target/release
# (never-built fixture); under `set -euo pipefail` that silently killed
# the script mid-summary (first container run 2026-08-31). Listing
# partial results is correct here — the harness fails loud later if a
# cell's binary is actually missing.
(find "$REPO_ROOT"/benchmarks/contenders/rust-camel-lib/target/release -maxdepth 1 -type f -executable 2>/dev/null || true) | while read -r f; do
    echo "  $(basename "$f") — $(du -h "$f" | cut -f1)"
done
echo "camel-cli: $(du -h "$REPO_ROOT/target/release/camel" 2>/dev/null | cut -f1 || echo 'NOT FOUND')"
echo ""
echo "Maven jars:"
(find "$REPO_ROOT"/benchmarks/scenarios/*/camel-standalone/*/target -name '*-jar-with-dependencies.jar' 2>/dev/null || true) | while read -r f; do
    echo "  $(basename "$f") — $(du -h "$f" | cut -f1)"
done
