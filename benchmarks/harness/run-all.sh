#!/usr/bin/env bash
# Host-side entry point. Builds the runner image if missing, then runs
# the harness inside the benchmark-runner container with proper mounts.
#
# Usage: bash benchmarks/harness/run-all.sh [harness args...]
# Example: bash benchmarks/harness/run-all.sh --metric=m1+m2 \
#            --scenarios=startup-minimal,http-server --n=50
#
# Host requirements: docker only. No JDK, cargo, or Maven needed.

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

IMAGE_NAME="${IMAGE_NAME:-benchmark-runner:latest}"
RESULTS_DIR="${RESULTS_DIR:-$REPO_ROOT/benchmarks/results}"
mkdir -p "$RESULTS_DIR"

# === Step 1: ensure runner image is built ===
if ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    echo "=== Building $IMAGE_NAME (one-time, ~3-5 min) ==="
    docker build -t "$IMAGE_NAME" -f benchmarks/runner/Dockerfile .
fi

# === Step 2: prepare cache directories (bind-mounted, host-UID-owned) ===
# Named Docker volumes end up root-owned; chmod in Dockerfile doesn't
# propagate to mounted volumes. Bind-mounting under the repo tree
# means the host UID owns them → no permission errors.
CACHE_DIR="$REPO_ROOT/benchmarks/.cache"
mkdir -p "$CACHE_DIR/cargo" "$CACHE_DIR/m2" "$CACHE_DIR/gradle"

# === Step 3: launch the runner container with the harness invocation ===
# Mount layout:
#   - $REPO_ROOT → /work (the entire repo, read-write for build outputs)
#   - $CACHE_DIR/cargo  → /work/.cache/cargo  (Rust crate cache, bind-mounted)
#   - $CACHE_DIR/m2     → /work/.cache/m2     (Maven dep cache, bind-mounted)
#   - $CACHE_DIR/gradle → /work/.cache/gradle (Gradle dep cache, bind-mounted)
#
# CARGO_HOME / GRADLE_USER_HOME / MAVEN repo.local all point under
# /work/.cache/ so the tools write to the bind-mounted dirs.
#
# UID/GID:
#   Run as the host user so build outputs (target/, build/) are
#   writable by both the container process AND the host user.
#
# Network:
#   --network host so Protocol A (loadgen → 127.0.0.1:8080) works
#   without port mapping gymnastics. NOTE: contenders bind 0.0.0.0:8080
#   (not 127.0.0.1), so the port IS exposed on all host interfaces
#   during the ~seconds-long test window. Acceptable on isolated CI
#   hosts; do NOT run on a hostile LAN. macOS/Windows Docker Desktop
#   users: --network host is a no-op (VM-based); needs different bind
#   handling, out of scope for v3.5.
#
# HOME:
#   Set HOME=/tmp (writable tmpfs) so Maven/Gradle/cargo resolve ~
#   correctly when running as non-root UID without a passwd entry.

echo "=== Launching $IMAGE_NAME ==="
# Two-phase invocation inside the container:
#   Phase 1: build all Rust + Maven artifacts (Quarkus native deferred to harness)
#   Phase 2: run the measurement harness
exec docker run --rm \
    -v "$REPO_ROOT:/work" \
    -w /work \
    --user "$(id -u):$(id -g)" \
    -e HOME=/tmp \
    -e LC_ALL=C \
    -e CARGO_HOME=/work/benchmarks/.cache/cargo \
    -e RUSTUP_HOME=/work/benchmarks/.cache/cargo/rustup \
    -e GRADLE_USER_HOME=/work/benchmarks/.cache/gradle \
    -e MAVEN_ARGS="-Dmaven.repo.local=/work/benchmarks/.cache/m2" \
    --network host \
    -e BENCH_HTTP_URL="${BENCH_HTTP_URL:-http://127.0.0.1:8080/bench}" \
    -e BENCH_NATIVE_MODE=local \
    -e NATIVE_ZLIB_LINK="" \
    -e GRADLE_BIN=/opt/gradle/bin/gradle \
    "$IMAGE_NAME" \
    bash -c '
        set -e
        echo "--- Phase 1: build artifacts ---"
        bash benchmarks/builder/build-all.sh
        echo ""
        echo "--- Phase 2: run harness ---"
        bash benchmarks/harness/run.sh "$@"
    ' \
    _ "$@"
# Note: the `_` placeholder occupies $0 inside the container's bash -c,
# so "$@" correctly forwards the original harness args.
