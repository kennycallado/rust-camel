#!/usr/bin/env bash
# Host-side entry point. Resolves the digest-pinned runner image, then
# runs the harness inside the benchmark-runner container with proper
# mounts.
#
# Usage: bash benchmarks/harness/run-all.sh [harness args...]
# Example: bash benchmarks/harness/run-all.sh --metric=m1+m2 \
#            --scenarios=startup-minimal,http-server --n=50
#
# Subset pinning (bench-era-2): BENCH_SUBSET=v1 pins the run to the
# four validated families (startup-minimal, http-server, t2-json,
# split-aggregate). `--print-subset` prints the resolved scenario
# list and exits 0 without touching docker.
#
# Digest identity (bench-era-2): without IMAGE_NAME the runner image
# is taken from benchmarks/runner/DIGEST (pin.sh output). Local
# builds record a bare `sha256:<id>` (config digest — `docker run
# sha256:<id>` resolves the local image); after a registry push,
# re-run pin.sh so a `repo@sha256:<id>` manifest digest replaces it.
# An explicit IMAGE_NAME override must be a digest reference
# (`sha256:<64hex>` or `repo@sha256:<64hex>`) — any mutable tag or
# malformed reference exits 1 pointing at pin.sh. The guard runs
# BEFORE any docker call, so `--print-subset` never reaches the
# daemon. A digest-pinned run NEVER auto-builds — a rebuilt image
# would be a different digest.
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

# === Step 0: v1 subset mapping + --print-subset interception ===
BENCH_SUBSET="${BENCH_SUBSET:-}"
case "$BENCH_SUBSET" in
    "") SUBSET_SCENARIOS="" ;;
    v1)
        # The validated families (bench-era-2; spec "Canonical v1
        # baseline run").
        SUBSET_SCENARIOS="startup-minimal,http-server,t2-json,split-aggregate"
        ;;
    *)
        echo "error: unknown BENCH_SUBSET '$BENCH_SUBSET' (supported: v1)" >&2
        exit 2
        ;;
esac

PRINT_SUBSET=false
HARNESS_ARGS=()
for arg in "$@"; do
    if [[ "$arg" == "--print-subset" ]]; then
        PRINT_SUBSET=true
    else
        HARNESS_ARGS+=("$arg")
    fi
done

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

if [[ "$PRINT_SUBSET" == "true" ]]; then
    if [[ -n "$SUBSET_SCENARIOS" ]]; then
        echo "$SUBSET_SCENARIOS"
    else
        echo "default: auto-discovery of every scenario under benchmarks/scenarios/"
    fi
    exit 0
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
    DOCKER_SOCK_ARGS=(-v /var/run/docker.sock:/var/run/docker.sock)
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

# subset string for meta: the explicit subset, else the same
# auto-discovery set run.sh resolves (scenario dirs minus spike-*).
if [[ -n "$SUBSET_SCENARIOS" ]]; then
    META_SUBSET="$SUBSET_SCENARIOS"
else
    META_SUBSET="$(find benchmarks/scenarios -mindepth 1 -maxdepth 1 -type d \
        ! -name 'spike-*' -printf '%f\n' | LC_ALL=C sort | paste -sd, -)"
fi

# summarize.py composes run_id "<YYYYMMDD>-v<seq>" from run_seq
# (no --run-id/--run-seq CLI flags exist; the meta document carries
# the sequence).
RUN_SEQ_JSON=""
if [[ -n "${BENCH_RUN_SEQ:-}" ]]; then
    if [[ ! "$BENCH_RUN_SEQ" =~ ^[0-9]+$ ]]; then
        echo "error: BENCH_RUN_SEQ='$BENCH_RUN_SEQ' must be a non-negative integer" >&2
        echo "       (the run sequence that composes the record id <YYYYMMDD>-v<N>)." >&2
        exit 1
    fi
    RUN_SEQ_JSON=",
  \"run_seq\": ${BENCH_RUN_SEQ}"
fi

# SCHEMA.md container_digest is the bare `sha256:<id>` — strip a
# push-qualified repo prefix if pin.sh recorded one.
META_DIGEST="${IMAGE_NAME##*@}"

# Launch-time meta snapshot for `bench summarize --meta`
# (summarize.py build_record requires era/protocol/host_provenance/
# git_commit; extra keys like captured_at are ignored).
cat > "$OUT_ROOT/meta.json" <<EOF
{
  "era": "2",
  "subset": "$META_SUBSET",
  "git_commit": "$GIT_COMMIT",
  "container_digest": "$META_DIGEST",
  "captured_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"$RUN_SEQ_JSON,
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

# === Step 2c: subset args prepended ahead of any explicit harness args ===
SUBSET_ARGS=()
if [[ -n "$SUBSET_SCENARIOS" ]]; then
    SUBSET_ARGS=(--scenarios="$SUBSET_SCENARIOS")
fi

# === Step 2d: prepare cache directories (bind-mounted, host-UID-owned) ===
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
#
# Docker socket:
#   Mounted when BENCH_NATIVE_MODE=docker (the default): native
#   cells delegate to $QUARKUS_NATIVE_BUILDER_IMAGE through the
#   host daemon. Without the mount every Quarkus native cell fails
#   (see RUNBOOK.md).

echo "=== Launching $IMAGE_NAME ==="
# Two-phase invocation inside the container:
#   Phase 1: build all Rust + Maven artifacts (Quarkus native deferred to harness)
#   Phase 2: run the measurement harness
exec docker run --rm \
    -v "$REPO_ROOT:/work" \
    "${DOCKER_SOCK_ARGS[@]}" \
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
    -e BENCH_NATIVE_MODE="$BENCH_NATIVE_MODE" \
    -e QUARKUS_NATIVE_BUILDER_IMAGE \
    -e BENCH_SEED="$ORDER_SEED" \
    -e BENCH_RESULTS_ROOT="$OUT_ROOT" \
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
    _ "${SUBSET_ARGS[@]}" "${HARNESS_ARGS[@]}"
# Note: the `_` placeholder occupies $0 inside the container's bash -c,
# so "$@" correctly forwards the subset + original harness args.
