#!/usr/bin/env bash
# Shell-less container guard for the JMS bridge lifecycle.
#
# Runs `camel run` on a minimal JMS route inside the production (scratch) and
# gnu (distroless) images with a static fake bridge. Invariant: no shell, no
# usable /tmp — enforced by a read-only empty-dir shadow mounted over /tmp
# (the gnu image itself ships a writable /tmp). Asserts that the bridge starts
# (the context reaches its started marker, which the JMS consumer pre-flight
# gates on bridge readiness) and that SIGTERM tears the container down within the stop timeout with exit 0 or 143.
#
# Each image is exercised twice:
#   default          - writable rootfs, default user (root)
#   read-only-65532  - --read-only --user 65532 + writable /cache volume
#                      (XDG_CACHE_HOME and HOME point into the volume)
#
# The two images must already exist (the CI job builds them) or be buildable
# from DOCKER_CONTEXT_DIR with CAMEL_MUSL_BIN / CAMEL_GNU_BIN (auto mode).
#
# Exit status: 0 when every case passes, 1 otherwise.
set -euo pipefail

PRODUCTION_IMAGE=${PRODUCTION_IMAGE:-camel-smoke:production}
GNU_IMAGE=${GNU_IMAGE:-camel-smoke:gnu}
DOCKER_CONTEXT_DIR=${DOCKER_CONTEXT_DIR:-context}
CAMEL_MUSL_BIN=${CAMEL_MUSL_BIN:-target/x86_64-unknown-linux-musl/release/camel}
CAMEL_GNU_BIN=${CAMEL_GNU_BIN:-target/release/camel}
FAKE_BRIDGE_BIN=${FAKE_BRIDGE_BIN:-target/x86_64-unknown-linux-musl/release/camel-fake-bridge}
BUILD_IMAGES=${BUILD_IMAGES:-auto}
READY_TIMEOUT_SECS=${READY_TIMEOUT_SECS:-60}
STOP_TIMEOUT_SECS=${STOP_TIMEOUT_SECS:-10}
# Used only to pre-own the writable volume of the unprivileged variant.
ALPINE_IMAGE=${ALPINE_IMAGE:-alpine:3.21}

# The context reaches this line only after every auto-start route started; the
# JMS consumer's start() awaits the bridge slot, so this is the observable
# bridge-ready marker (pre-fix camel-jms publishes Ready without an INFO line).
READY_MARKER=${READY_MARKER:-"camel-cli: context started"}
# Sanity marker proving the JMS component actually attempted a bridge launch,
# so a guard pass can never come from a route that never touched the bridge.
ATTEMPT_MARKER=${ATTEMPT_MARKER:-"Starting JMS bridge process for"}

EXIT_CODE=0
WORK=""
LAST_LOGS=""

CACHE_VOLUMES=()
CASE_CONTAINERS=()

log() { printf '%s\n' "$*"; }

cleanup() {
    local vol name
    # Remove any case container left behind by an aborted setup/launch.
    for name in "${CASE_CONTAINERS[@]:-}"; do
        [[ -n "$name" ]] && docker rm -f "$name" >/dev/null 2>&1 || true
    done
    for vol in "${CACHE_VOLUMES[@]:-}"; do
        [[ -n "$vol" ]] && docker volume rm -f "$vol" >/dev/null 2>&1 || true
    done
    if [[ -n "$WORK" && -d "$WORK" ]]; then
        rm -rf "$WORK"
    fi
}
trap cleanup EXIT

# docker image inspect <image>; true when present locally.
image_present() {
    docker image inspect "$1" >/dev/null 2>&1
}

# Builds one target image from DOCKER_CONTEXT_DIR when it is missing (auto) or
# always requested. Mirrors the CI job's recipe: copy the matching binary to
# context/camel-amd64, then `docker build --target <target>`.
ensure_image() {
    local image=$1 target=$2 binary=$3
    if [[ "$BUILD_IMAGES" == "never" ]]; then
        return 0
    fi
    if [[ "$BUILD_IMAGES" != "always" ]] && image_present "$image"; then
        return 0
    fi
    if [[ ! -f "$DOCKER_CONTEXT_DIR/Dockerfile" ]]; then
        log "ERROR: $DOCKER_CONTEXT_DIR/Dockerfile is missing; cannot build $image" >&2
        return 1
    fi
    if [[ ! -f "$binary" ]]; then
        log "ERROR: binary $binary is missing; build it first or set BUILD_IMAGES=never" >&2
        return 1
    fi
    cp "$binary" "$DOCKER_CONTEXT_DIR/camel-amd64"
    chmod +x "$DOCKER_CONTEXT_DIR/camel-amd64"
    docker build --target "$target" -t "$image" "$DOCKER_CONTEXT_DIR"
}

# Writes the guard inputs (Camel.toml + one JMS consumer route) under WORK.
prepare_inputs() {
    local routes_dir="$WORK/routes"
    mkdir -p "$routes_dir"

    cat >"$routes_dir/Camel.toml" <<'EOF'
[default]
routes = ["/routes/*.yaml"]
log_level = "INFO"
watch = false

[default.components.jms]
max_bridges = 1

[default.components.jms.brokers.main]
broker_url = "tcp://127.0.0.1:61616"
broker_type = "activemq"
EOF

    cat >"$routes_dir/guard.yaml" <<'EOF'
routes:
  - id: "bridge-guard-consumer"
    from: "jms:queue:guard?broker=main"
    steps:
      - log: "message=bridge guard message"
EOF
}

# Polls the container logs for READY_MARKER. Returns 0 on readiness, 1 when the
# marker did not appear or the container died first. Copies the observed logs to
# LAST_LOGS either way.
poll_ready() {
    local name=$1 deadline logs running
    deadline=$((SECONDS + READY_TIMEOUT_SECS))
    while ((SECONDS < deadline)); do
        logs=$(docker logs "$name" 2>&1 || true)
        running=$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo false)
        if grep -qF "$READY_MARKER" <<<"$logs"; then
            printf '%s' "$logs" >"$LAST_LOGS"
            # A marker printed just before a crash is not readiness: the
            # container must still be running when the marker is observed.
            [[ "$running" == "true" ]] && return 0
            return 1
        fi
        if [[ "$running" != "true" ]]; then
            printf '%s' "$logs" >"$LAST_LOGS"
            return 1
        fi
        sleep 1
    done
    docker logs "$name" >"$LAST_LOGS" 2>&1 || true
    return 1
}

# Removes the case container and its writable cache volume (if any).
release_case() {
    local name=$1 cache_vol=$2
    docker rm -f "$name" >/dev/null 2>&1 || true
    if [[ -n "$cache_vol" ]]; then
        docker volume rm -f "$cache_vol" >/dev/null 2>&1 || true
    fi
}

# Emits the per-case FAIL line, releases the case resources, and records the
# failure. Always returns 0 so `set -e` never aborts the remaining cases.
fail_case() {
    local name=$1 cache_vol=$2 image=$3 variant=$4 detail=$5
    log "GUARD $image $variant: FAIL $detail"
    release_case "$name" "$cache_vol"
    EXIT_CODE=1
}

# run_case <image> <variant> <extra docker args...>
run_case() {
    local image=$1 variant=$2
    shift 2
    local name="camel-bridge-guard-${variant//[^a-zA-Z0-9_.-]/-}-$$"
    local extra=("$@")
    local exit_code attempt detail cache_vol=""

    CASE_CONTAINERS+=("$name")

    if [[ "$variant" == "read-only-65532" ]]; then
        cache_vol="camel-bridge-guard-cache-${variant}-$$"
        docker volume rm -f "$cache_vol" >/dev/null 2>&1 || true
        if ! docker volume create "$cache_vol" >/dev/null; then
            fail_case "$name" "$cache_vol" "$image" "$variant" \
                "could not create cache volume $cache_vol"
            return 0
        fi
        CACHE_VOLUMES+=("$cache_vol")
        # A fresh volume is root-owned 0755; uid 65532 must be able to write
        # the TLS material and attempt-journal trees.
        if ! docker run --rm --user 0 -v "$cache_vol:/cache" "$ALPINE_IMAGE" \
            chown 65532:65532 /cache >/dev/null 2>&1; then
            fail_case "$name" "$cache_vol" "$image" "$variant" \
                "could not chown cache volume $cache_vol for uid 65532"
            return 0
        fi
        extra+=(-v "$cache_vol:/cache")
    fi

    docker rm -f "$name" >/dev/null 2>&1 || true
    LAST_LOGS="$WORK/${variant}.log"
    : >"$LAST_LOGS"

    if ! docker run -d --name "$name" \
        -v "$WORK/fixtures/fake-bridge:/fixtures/fake-bridge:ro" \
        -v "$WORK/routes:/routes:ro" \
        -v "$WORK/empty-tmp:/tmp:ro" \
        -e CAMEL_JMS_BRIDGE_BINARY_PATH=/fixtures/fake-bridge \
        -e CAMEL_CONFIG_FILE=/routes/Camel.toml \
        "${extra[@]}" \
        "$image" run --routes '/routes/*.yaml' >/dev/null 2>"$WORK/${variant}.run.err"; then
        detail="docker run failed: $(tr '\n' ' ' <"$WORK/${variant}.run.err" | tail -c 300)"
        fail_case "$name" "$cache_vol" "$image" "$variant" "$detail"
        return 0
    fi

    if ! poll_ready "$name"; then
        attempt="missing"
        grep -qF "$ATTEMPT_MARKER" "$LAST_LOGS" && attempt="seen"
        exit_code=$(docker inspect -f '{{.State.ExitCode}}' "$name" 2>/dev/null || echo unknown)
        detail="readiness marker '$READY_MARKER' not observed within ${READY_TIMEOUT_SECS}s (bridge attempt: $attempt, exit code: $exit_code)"
        log "GUARD $image $variant: FAIL $detail"
        log "----- docker logs $name -----"
        cat "$LAST_LOGS"
        log "----- end docker logs $name -----"
        release_case "$name" "$cache_vol"
        EXIT_CODE=1
        return 0
    fi

    attempt="missing"
    grep -qF "$ATTEMPT_MARKER" "$LAST_LOGS" && attempt="seen"
    if [[ "$attempt" != "seen" ]]; then
        fail_case "$name" "$cache_vol" "$image" "$variant" \
            "readiness observed but bridge attempt marker '$ATTEMPT_MARKER' is missing"
        return 0
    fi

    # Teardown must follow a live container: `docker stop` also "succeeds" for
    # a container that already exited, so re-check liveness right before stop.
    if [[ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null || echo false)" != "true" ]]; then
        exit_code=$(docker inspect -f '{{.State.ExitCode}}' "$name" 2>/dev/null || echo unknown)
        fail_case "$name" "$cache_vol" "$image" "$variant" \
            "container not running before SIGTERM (exit code: $exit_code)"
        return 0
    fi

    # Teardown: SIGTERM with a bounded grace period, then verify the exit code.
    if ! timeout $((STOP_TIMEOUT_SECS + 10)) docker stop -t "$STOP_TIMEOUT_SECS" "$name" >/dev/null; then
        fail_case "$name" "$cache_vol" "$image" "$variant" \
            "docker stop -t $STOP_TIMEOUT_SECS did not finish"
        return 0
    fi

    exit_code=$(docker inspect -f '{{.State.ExitCode}}' "$name" 2>/dev/null || echo unknown)
    if [[ "$exit_code" == "0" || "$exit_code" == "143" ]]; then
        log "GUARD $image $variant: PASS readiness observed, clean SIGTERM exit ($exit_code)"
        log "  bridge:  $(grep -F "$ATTEMPT_MARKER" "$LAST_LOGS" | tail -n1 | sed -E $'s/\x1b\\[[0-9;]*m//g')"
        log "  started: $(grep -F "$READY_MARKER" "$LAST_LOGS" | tail -n1 | sed -E $'s/\x1b\\[[0-9;]*m//g')"
    else
        fail_case "$name" "$cache_vol" "$image" "$variant" \
            "exit code $exit_code after SIGTERM (expected 0 or 143)"
        return 0
    fi
    release_case "$name" "$cache_vol"
}

main() {
    if [[ ! -f "$FAKE_BRIDGE_BIN" ]]; then
        log "ERROR: fake bridge fixture not found at $FAKE_BRIDGE_BIN" >&2
        log "Build it with: cargo build -p camel-fake-bridge --release --target x86_64-unknown-linux-musl" >&2
        exit 1
    fi
    FAKE_BRIDGE_BIN=$(realpath "$FAKE_BRIDGE_BIN")

    ensure_image "$PRODUCTION_IMAGE" production "$CAMEL_MUSL_BIN"
    ensure_image "$GNU_IMAGE" gnu "$CAMEL_GNU_BIN"

    WORK=$(mktemp -d "${TMPDIR:-/tmp}/camel-bridge-guard.XXXXXX")
    mkdir -p "$WORK/fixtures" "$WORK/empty-tmp"
    # /tmp shadow: an empty host directory mounted read-only makes /tmp
    # nonexistent-or-unusable in every image (the gnu distroless rootfs ships
    # a writable /tmp). Without it a pre-fix binary could pass the gnu cases
    # by writing TLS material to the image's /tmp.
    # Bind-mounting a single file needs the file itself; a symlink resolves on
    # the host and Docker would create a directory for a missing target.
    cp "$FAKE_BRIDGE_BIN" "$WORK/fixtures/fake-bridge"
    chmod +x "$WORK/fixtures/fake-bridge"
    prepare_inputs

    # Default user: writable rootfs; HOME supplies the attempt-journal root and
    # the cache-dir TLS rung when /tmp is unusable (the read-only empty-dir
    # shadow over /tmp makes it unusable in all four cases, including gnu).
    run_case "$PRODUCTION_IMAGE" default \
        -e HOME=/
    run_case "$GNU_IMAGE" default \
        -e HOME=/

    # Read-only rootfs as an unprivileged user: the mounted volume is the only
    # writable location, reached through XDG_CACHE_HOME (TLS material) and HOME
    # (attempt journal).
    run_case "$PRODUCTION_IMAGE" read-only-65532 \
        --read-only --user 65532 \
        -e HOME=/cache -e XDG_CACHE_HOME=/cache
    run_case "$GNU_IMAGE" read-only-65532 \
        --read-only --user 65532 \
        -e HOME=/cache -e XDG_CACHE_HOME=/cache

    exit "$EXIT_CODE"
}

main "$@"
