#!/usr/bin/env bash
# Host-side entry point. Resolves the digest-pinned runner image, then
# runs the harness inside the benchmark-runner container with proper
# mounts.
#
# Usage: bash benchmarks/harness/run-all.sh [harness args...]
# Example: bash benchmarks/harness/run-all.sh --metric=m1+m2 --n=50
#
# NO SUBSETS (2026-08-31, owner ruling): the default run measures
# EVERY active scenario × every contender (auto-discovery of
# benchmarks/scenarios/ minus spike-*). `--scenarios=` remains a
# harness-level developer/test knob only — never part of the
# owner-facing surface (`bash benchmarks/bench run-all`).
#
# Digest identity (bench-era-2): without IMAGE_NAME the runner image
# is taken from benchmarks/runner/DIGEST (pin.sh output). Local
# builds record a bare `sha256:<id>` (config digest — `docker run
# sha256:<id>` resolves the local image); after a registry push,
# re-run pin.sh so a `repo@sha256:<id>` manifest digest replaces it.
# An explicit IMAGE_NAME override must be a digest reference
# (`sha256:<64hex>` or `repo@sha256:<64hex>`) — any mutable tag or
# malformed reference exits 1 pointing at pin.sh. A digest-pinned
# run NEVER auto-builds — a rebuilt image would be a different
# digest.
#
# Results redirect (bench-era-2): the container harness writes raw
# artifacts under benchmarks/harness/out/<ts>/ (gitignored, via
# BENCH_RESULTS_ROOT) instead of the tracked benchmarks/results zone,
# and a launch-time meta.json (digest, git commit, quiet-host load
# snapshot, protocol) lands beside it for `bench summarize --meta`.
#
# Host requirements: docker only. No JDK, cargo, or Maven needed.

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"

# === Step 0: argument passthrough (no subset concept — see header) ===
HARNESS_ARGS=("$@")

# === Digest-identity guard (ordered BEFORE any docker call): an
# explicitly overridden IMAGE_NAME must be a digest reference —
# `sha256:<64hex>` (config digest, local builds) or
# `repo@sha256:<64hex>` (manifest digest, valid after a registry
# push). Mutable tags and malformed references exit 1 pointing at
# pin.sh. ===
is_digest_ref() {
    [[ "$1" =~ ^(sha256:[0-9a-f]{64}|[^@]+@sha256:[0-9a-f]{64})$ ]]
}

if [[ -n "${IMAGE_NAME:-}" ]]; then
    if ! is_digest_ref "$IMAGE_NAME"; then
        echo "error: IMAGE_NAME='$IMAGE_NAME' is not a digest-pinned reference." >&2
        echo "       Canonical runs run by digest. Build and pin first:" >&2
        echo "         bash benchmarks/runner/pin.sh" >&2
        exit 1
    fi
fi

# === Step 0b: default image = the pinned digest from runner/DIGEST ===
DIGEST_FILE="$REPO_ROOT/benchmarks/runner/DIGEST"
if [[ -z "${IMAGE_NAME:-}" ]]; then
    if [[ ! -s "$DIGEST_FILE" ]]; then
        echo "error: benchmarks/runner/DIGEST is missing or empty." >&2
        echo "       Canonical runs run by digest; build and pin first:" >&2
        echo "         bash benchmarks/runner/pin.sh" >&2
        exit 1
    fi
    IMAGE_NAME="$(tr -d '[:space:]' < "$DIGEST_FILE")"
    if ! is_digest_ref "$IMAGE_NAME"; then
        echo "error: benchmarks/runner/DIGEST holds a malformed reference: '$IMAGE_NAME'" >&2
        echo "       Expected 'sha256:<64hex>' or 'repo@sha256:<64hex>' (see pin.sh)." >&2
        exit 1
    fi
fi
echo "=== runner image (digest-pinned): $IMAGE_NAME ==="

# === Step 1: the pinned image must already exist (pin.sh builds it) ===
if ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
    echo "error: image $IMAGE_NAME is not present on this daemon." >&2
    echo "       Build and pin it first: bash benchmarks/runner/pin.sh" >&2
    exit 1
fi

# === Step 2b: launch-time meta + out-of-tree results root ===
TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_ROOT="$REPO_ROOT/benchmarks/harness/out/$TS"
mkdir -p "$OUT_ROOT"
export BENCH_RESULTS_ROOT="$OUT_ROOT"

# Order seed: generated on the host ONCE and exported into the
# container, so meta.json records the same seed the harness used
# (run.sh honors BENCH_SEED for the M3/M4 randomized-block order).
ORDER_SEED="${BENCH_SEED:-$(date +%s)}"
export BENCH_SEED="$ORDER_SEED"

# Native build mode: the runner image ships NO in-image native-image
# (runner/Dockerfile header) — native cells build through
# $QUARKUS_NATIVE_BUILDER_IMAGE, which requires the host docker
# socket inside the container. Default docker, matching the image
# contract (see RUNBOOK.md §docker socket).
BENCH_NATIVE_MODE="${BENCH_NATIVE_MODE:-docker}"
DOCKER_SOCK_ARGS=()
if [[ "$BENCH_NATIVE_MODE" == "docker" ]]; then
    # The container user is the host UID without the host's supplementary
    # groups; the docker socket is root:<docker-gid> 0660. Without
    # --group-add the socket mount is unusable and the first native build
    # dies with permission denied (found 2026-08-31 while hardening the
    # first container run). Resolve the gid from the socket itself.
    DOCKER_SOCK_GID="$(stat -c %g /var/run/docker.sock)"
    DOCKER_SOCK_ARGS=(-v /var/run/docker.sock:/var/run/docker.sock
        --group-add "$DOCKER_SOCK_GID")
fi

# Quiet-host snapshot (records/SCHEMA.md host_provenance.load).
read -r LOAD_ONE LOAD_FIVE LOAD_FIFTEEN _ < /proc/loadavg
GIT_COMMIT="$(git rev-parse HEAD)"
CPU_MODEL="$(lscpu 2>/dev/null | sed -n 's/^Model name:[[:space:]]*//p' | head -1)"
CPU_MODEL="${CPU_MODEL:-unknown}"
HOST_CORES="$(nproc)"
KERNEL="$(uname -r)"
# ~3h wall-clock estimate recorded at launch (override if known).
PROTOCOL_DURATION="${BENCH_PROTOCOL_DURATION_SECS:-10800}"

# Scenario vocabulary for meta: the auto-discovery set run.sh
# resolves (scenario dirs minus spike-*). meta records `scenarios`
# (what the run covers); the old `subset` field is retired
# (2026-08-31 owner ruling — no named subsets).
META_SCENARIOS="$(find benchmarks/scenarios -mindepth 1 -maxdepth 1 -type d \
    ! -name 'spike-*' -printf '%f\n' | LC_ALL=C sort | paste -sd, -)"

# Record identity (2026-08-31 owner ruling): run_id is the launch
# timestamp — plain, chronological, no sequence numbering. The old
# `run_seq` / `<YYYYMMDD>-v<N>` composition is retired for new runs
# (summarize.py still understands run_seq in legacy metas).

# SCHEMA.md container_digest is the bare `sha256:<id>` — strip a
# push-qualified repo prefix if pin.sh recorded one.
META_DIGEST="${IMAGE_NAME##*@}"

# Launch-time meta snapshot for `bench summarize --meta`
# (summarize.py build_record requires era/protocol/host_provenance/
# git_commit; extra keys like captured_at are ignored).
cat > "$OUT_ROOT/meta.json" <<EOF
{
  "era": "2",
  "run_id": "$TS",
  "scenarios": "$META_SCENARIOS",
  "git_commit": "$GIT_COMMIT",
  "container_digest": "$META_DIGEST",
  "captured_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "host_provenance": {
    "cpu_model": "$CPU_MODEL",
    "cores": $HOST_CORES,
    "kernel": "$KERNEL",
    "containerized": true,
    "load": {
      "one": $LOAD_ONE,
      "five": $LOAD_FIVE,
      "fifteen": $LOAD_FIFTEEN
    }
  },
  "protocol": {
    "rounds": 5,
    "duration_secs": $PROTOCOL_DURATION,
    "warmup_secs": 30,
    "order_seed": $ORDER_SEED
  }
}
EOF
echo "=== meta snapshot: $OUT_ROOT/meta.json ==="

# === Step 2c: harness args passthrough (no subset prepending) ===

# === Step 2d: prepare cache directories (bind-mounted, host-UID-owned) ===
# Named Docker volumes end up root-owned; chmod in Dockerfile doesn't
# propagate to mounted volumes. Bind-mounting under the repo tree
# means the host UID owns them → no permission errors.
CACHE_DIR="$REPO_ROOT/benchmarks/.cache"
mkdir -p "$CACHE_DIR/cargo" "$CACHE_DIR/m2" "$CACHE_DIR/gradle"

# === Step 3: launch the runner container with the harness invocation ===
# Mount layout:
#   - $REPO_ROOT → $REPO_ROOT (MIRRORED host path, read-write for build
#     outputs). The mirror is load-bearing: native builds delegate to the
#     Mandrel builder through the HOST docker daemon (docker-out-of-docker),
#     and the daemon can only bind-mount HOST paths. If the repo sat at a
#     container-only path (/work), every builder mount would resolve to
#     nothing on the host and quarkusAppPartsBuild would die with
#     NoSuchFileException /project/... (first container run 2026-08-31).
#   - caches live under $REPO_ROOT/benchmarks/.cache (cargo, m2,
#     gradle) and are env-pointed there — same mirrored-path rationale.
#
# CARGO_HOME / GRADLE_USER_HOME / MAVEN repo.local all point under
# mirrored .cache/ so the tools write to the bind-mounted dirs.
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
#
# Docker socket:
#   Mounted when BENCH_NATIVE_MODE=docker (the default): native
#   cells delegate to $QUARKUS_NATIVE_BUILDER_IMAGE through the
#   host daemon. Without the mount every Quarkus native cell fails
#   (see RUNBOOK.md).

# Forward-if-set (bench-consol-tick task 1.4): the RED-proof
# BENCH_DEBUG_DROP_CELL hook must reach the container when the host
# exported it; when absent on the host, nothing is forwarded.
# BENCH_DEBUG_SILENCE_CELL (task 3.2, rc-tpig dead-cell proof) rides
# the same mechanism: it silences ONE cell's BENCH_LATENCY file so the
# m2 record check can be proven to hard-fail in-container.
DEBUG_DROP_CELL_ARGS=()
if [[ -n "${BENCH_DEBUG_DROP_CELL:-}" ]]; then
    DEBUG_DROP_CELL_ARGS+=(-e "BENCH_DEBUG_DROP_CELL=$BENCH_DEBUG_DROP_CELL")
fi
if [[ -n "${BENCH_DEBUG_SILENCE_CELL:-}" ]]; then
    DEBUG_DROP_CELL_ARGS+=(-e "BENCH_DEBUG_SILENCE_CELL=$BENCH_DEBUG_SILENCE_CELL")
fi

echo "=== Launching $IMAGE_NAME ==="
# Two-phase invocation inside the container:
#   Phase 1: build all Rust + Maven artifacts (Quarkus native deferred to harness)
#   Phase 2: run the measurement harness
exec docker run --rm \
    -v "$REPO_ROOT:$REPO_ROOT" \
    "${DOCKER_SOCK_ARGS[@]}" \
    -w "$REPO_ROOT" \
    --user "$(id -u):$(id -g)" \
    -e HOME=/tmp \
    -e LC_ALL=C \
    -e CARGO_HOME="$REPO_ROOT/benchmarks/.cache/cargo" \
    -e RUSTUP_HOME="$REPO_ROOT/benchmarks/.cache/cargo/rustup" \
    -e GRADLE_USER_HOME="$REPO_ROOT/benchmarks/.cache/gradle" \
    -e MAVEN_ARGS="-Dmaven.repo.local=$REPO_ROOT/benchmarks/.cache/m2" \
    --network host \
    -e BENCH_HTTP_URL="${BENCH_HTTP_URL:-http://127.0.0.1:8080/bench}" \
    -e BENCH_NATIVE_MODE="$BENCH_NATIVE_MODE" \
    -e QUARKUS_NATIVE_BUILDER_IMAGE \
    -e BENCH_SEED="$ORDER_SEED" \
    -e BENCH_RESULTS_ROOT="$OUT_ROOT" \
    -e BENCH_SCRATCH_DIR="$OUT_ROOT/scratch" \
    "${DEBUG_DROP_CELL_ARGS[@]}" \
    -e NATIVE_ZLIB_LINK="" \
    -e GRADLE_BIN=/opt/gradle/bin/gradle \
    "$IMAGE_NAME" \
    bash -c '
        set -e
        echo "--- Phase 1: build artifacts ---"
        bash benchmarks/harness/builder/build-all.sh
        echo ""
        echo "--- Phase 2: run harness ---"
        bash benchmarks/harness/run.sh "$@"
    ' \
    _ "${HARNESS_ARGS[@]}"
# Note: the `_` placeholder occupies $0 inside the container's bash -c,
# so "$@" correctly forwards the original harness args.
