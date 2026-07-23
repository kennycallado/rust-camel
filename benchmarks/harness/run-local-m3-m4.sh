#!/usr/bin/env bash
# Local M3+M4 measurement runner — wraps the Mandrel-único container.
#
# Builds all artifacts inside the container, then runs the harness
# against the http-server scenario (--metric=m3+m4). All 6 T3 cells
# are measured: camel-standalone-dsl, camel-standalone-yaml,
# camel-quarkus-dsl-native, camel-quarkus-yaml-native, rust-camel-lib,
# rust-camel-cli.
#
# First run: ~30-45 min (dep downloads + native-image + measurement).
# Subsequent runs with --cache-dir: ~25 min (artifacts cached on host).
#
# Usage:
#   cd /home/kenny/dev/rust-camel/.worktrees/rc-f3g9-startup-benchmark
#   bash benchmarks/harness/run-local-m3-m4.sh
#
# Optional env overrides:
#   M3_ROUNDS=3 bash benchmarks/harness/run-local-m3-m4.sh  # fewer rounds
#   M3_DURATION_SECS=20 ...                                  # shorter window
#   CACHE_DIR=$PWD/.docker-cache ...                         # persist deps
set -euo pipefail

WORKTREE_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="benchmark-runner:latest"

# Verify the image exists.
if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
    echo "error: $IMAGE not found. Build it first:"
    echo "  docker build -t $IMAGE -f benchmarks/runner/Dockerfile ."
    exit 1
fi

# Optional host-side cache dir (bind-mount → persists across runs).
# Default: no cache (container uses ephemeral /tmp).
CACHE_DIR="${CACHE_DIR:-}"
CACHE_ARGS=()
if [[ -n "$CACHE_DIR" ]]; then
    mkdir -p "$CACHE_DIR/maven" "$CACHE_DIR/gradle" "$CACHE_DIR/cargo-registry"
    CACHE_ARGS=(
        -v "$CACHE_DIR/maven:/tmp/maven-cache"
        -v "$CACHE_DIR/gradle:/tmp/gradle-cache"
        -v "$CACHE_DIR/cargo-registry:/tmp/cargo-registry"
    )
    MAVEN_REPO="/tmp/maven-cache/repository"
    GRADLE_HOME="/tmp/gradle-cache"
    CARGO_REG="/tmp/cargo-registry"
else
    MAVEN_REPO="/tmp/.m2/repository"
    GRADLE_HOME="/tmp/.gradle"
    CARGO_REG="/tmp/.cargo/registry"
fi

echo "=== Local M3+M4 benchmark ==="
echo "image:   $IMAGE"
echo "workdir: $WORKTREE_ROOT"
echo "cache:   ${CACHE_DIR:-<ephemeral>}"
echo "metric:  m3+m4 (sustained throughput + RSS growth, T3 http-server only)"
echo ""

docker run --rm \
    --user "$(id -u):$(id -g)" \
    -v "$WORKTREE_ROOT:/work" \
    "${CACHE_ARGS[@]}" \
    -w /work \
    -e HOME=/tmp \
    -e MAVEN_OPTS="-Dmaven.repo.local=$MAVEN_REPO" \
    -e GRADLE_USER_HOME="$GRADLE_HOME" \
    -e CARGO_HOME=/tmp/.cargo \
    -e M3_ROUNDS="${M3_ROUNDS:-5}" \
    -e M3_DURATION_SECS="${M3_DURATION_SECS:-50}" \
    -e M3_WARMUP_SECS="${M3_WARMUP_SECS:-10}" \
    -e BENCH_GIT_COMMIT="$(git -C "$WORKTREE_ROOT" rev-parse HEAD 2>/dev/null || echo unknown)" \
    -e BENCH_GIT_DIRTY="$(git -C "$WORKTREE_ROOT" diff --quiet HEAD 2>/dev/null && echo false || echo true)" \
    -e BENCH_DOCKER_IMAGE="$IMAGE" \
    "$IMAGE" \
    bash -c "
        set -euo pipefail
        mkdir -p '$MAVEN_REPO' '$GRADLE_HOME' '$CARGO_REG'
        echo '--- pre-build: bench-loadgen + bench-devnull ---'
        cargo build --release -p bench-loadgen --bins
        echo '--- pre-build: camel CLI (rust-camel-cli) ---'
        cargo build --release -p camel-cli
        echo '--- pre-build: rust-camel-lib fixture (http-server) ---'
        (cd benchmarks/scenarios/http-server/rust-camel-lib && cargo build --release)
        echo '--- pre-build: camel-standalone jars (Maven) ---'
        (cd benchmarks/scenarios/http-server/camel-standalone && \
         mvn package -q -DskipTests -Dmaven.repo.local='$MAVEN_REPO')
        echo ''
        echo '--- launching harness ---'
        exec bash benchmarks/harness/run.sh \
            --scenarios=http-server \
            --metric=m3+m4
    "
