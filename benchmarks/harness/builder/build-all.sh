#!/usr/bin/env bash
# Build all benchmark artifacts inside the container.
# Called by run-all.sh before invoking the harness.
#
# Builds:
#   - Rust: rust-camel-lib × 5 scenarios + camel-cli + loadgen + bridge
#   - Maven: camel-standalone-{dsl,yaml} jars per scenario
#   - Quarkus native: deferred to the harness (build_native_artifact)
#
# Idempotent: cargo and mvn skip up-to-date builds.

set -euo pipefail
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
cd "$REPO_ROOT"

echo "=== Building Rust artifacts ==="

# rust-camel-lib per-scenario binaries (5 scenarios).
for scenario_dir in "$REPO_ROOT"/benchmarks/scenarios/*/rust-camel-lib; do
    [[ -d "$scenario_dir" ]] || continue
    echo "  → $(basename "$(dirname "$scenario_dir")")/rust-camel-lib"
    (cd "$scenario_dir" && env -u CARGO_TARGET_DIR cargo build --release 2>&1 | tail -1)
done

# Shared rust-camel-cli binary (used by all scenarios via route YAML).
echo "  → camel-cli (shared)"
(cd "$REPO_ROOT" && env -u CARGO_TARGET_DIR cargo build --release -p camel-cli 2>&1 | tail -1)

# Load generator (M2 Protocol A).
echo "  → bench-loadgen"
(cd "$REPO_ROOT" && env -u CARGO_TARGET_DIR cargo build --release -p bench-loadgen 2>&1 | tail -1) || true

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
echo "=== Building Maven artifacts ==="

# camel-standalone per-scenario jars.
for standalone_dir in "$REPO_ROOT"/benchmarks/scenarios/*/camel-standalone; do
    [[ -d "$standalone_dir" ]] || continue
    scenario=$(basename "$(dirname "$standalone_dir")")
    for sub in "$standalone_dir"/*/; do
        [[ -d "$sub" ]] || continue
        sub_name=$(basename "$sub")
        # Skip if jar already built.
        if ls "$sub"/target/*-jar-with-dependencies.jar >/dev/null 2>&1; then
            echo "  → $scenario/$sub_name (jar exists, skipping)"
            continue
        fi
        echo "  → $scenario/$sub_name"
        (cd "$sub" && mvn package -DskipTests -q 2>&1 | tail -1)
    done
done

echo ""
echo "=== Build summary ==="
echo "Rust binaries:"
find "$REPO_ROOT"/benchmarks/scenarios/*/rust-camel-lib/target/release -maxdepth 1 -type f -executable 2>/dev/null | while read -r f; do
    echo "  $(basename "$f") — $(du -h "$f" | cut -f1)"
done
echo "camel-cli: $(du -h "$REPO_ROOT/target/release/camel" 2>/dev/null | cut -f1 || echo 'NOT FOUND')"
echo ""
echo "Maven jars:"
find "$REPO_ROOT"/benchmarks/scenarios/*/camel-standalone/*/target -name '*-jar-with-dependencies.jar' 2>/dev/null | while read -r f; do
    echo "  $(basename "$f") — $(du -h "$f" | cut -f1)"
done
