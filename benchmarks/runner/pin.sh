#!/usr/bin/env bash
# Build the canonical benchmark runner image and record its digest.
#
# Usage: bash benchmarks/runner/pin.sh [--report]
#
# --report prints the pin record (tool versions + digests baked into
# the image) and exits 0 without building.
#
# Builds runner/Dockerfile with `docker build --iidfile`, resolves the
# FULL image digest, and writes it to benchmarks/runner/DIGEST as
# `sha256:<64hex>` + newline.
#
# Digest resolution (verified against a local-build docker daemon):
# - RepoDigests is EMPTY for purely local builds — it is only
#   populated by a registry pull/push. Prefer RepoDigests[0] when
#   non-empty (a push-qualified reference), else fall back to the
#   image config digest (`docker image inspect --format '{{.Id}}'`,
#   which IS `sha256:<hex>` for local builds).
# - A config digest is the identity docker itself reports for the
#   local image; once the image is pushed, re-run this script so a
#   registry (manifest) digest replaces it in DIGEST.
#
# The script FAILS (exit 1) when the resolved reference is a mutable
# tag (e.g. the floating newest tag) or malformed — only
# `sha256:<64hex>` may be recorded. Canonical runs consume DIGEST;
# tags are convenience labels only.
set -euo pipefail

# =====================================================================
# Pin record — single source of truth for the tool versions baked
# into the runner image. The Dockerfile ARG defaults mirror these
# literals (so a bare `docker build` still works); this script feeds
# them via --build-arg and prints them via --report. The drift guard
# below fails on any divergence between the two.
# =====================================================================
NODE_VERSION="22.14.0"
# sha256 of node-v${NODE_VERSION}-linux-x64.tar.gz (nodejs.org/dist).
NODE_SHA256="9d942932535988091034dc94cc5f42b6dc8784d6366df3a36c4c9ccb3996f0c2"
# docker static CLI (official tgz, download.docker.com) for
# BENCH_NATIVE_MODE=docker — drives host-socket native builds.
DOCKER_CLI_VERSION="28.5.2"
# sha256 of docker-${DOCKER_CLI_VERSION}.tgz (computed from the
# official artifact; no .sha256 is published alongside).
DOCKER_CLI_SHA256="ea90cfd12e1eeb12aa1c971741adb8bd4ed88e2a574eaac13f5029a1dbc6300d"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_TAG="${IMAGE_TAG:-benchmark-runner:v1}"
DIGEST_FILE="$SCRIPT_DIR/DIGEST"

REPORT_ONLY=false
while [[ $# -gt 0 ]]; do
    case "$1" in
        --report) REPORT_ONLY=true ;;
        *) echo "error: unknown flag '$1' (usage: bash benchmarks/runner/pin.sh [--report])" >&2; exit 2 ;;
    esac
    shift
done

if [[ "$REPORT_ONLY" == "true" ]]; then
    echo "NODE_VERSION=$NODE_VERSION"
    echo "NODE_SHA256=$NODE_SHA256"
    echo "DOCKER_CLI_VERSION=$DOCKER_CLI_VERSION"
    echo "DOCKER_CLI_SHA256=$DOCKER_CLI_SHA256"
    exit 0
fi

# Drift guard: the Dockerfile ARG defaults must equal the pin.sh
# literals, or a bare `docker build` (no --build-arg) would produce a
# different image than the pinned build. Fail closed on a mismatch OR
# a missing ARG line.
for pin in NODE_VERSION NODE_SHA256 DOCKER_CLI_VERSION DOCKER_CLI_SHA256; do
    dockerfile_default="$(grep -E "^ARG ${pin}=" "$SCRIPT_DIR/Dockerfile" | head -1 | cut -d= -f2- || true)"
    if [[ "$dockerfile_default" != "${!pin}" ]]; then
        echo "error: Dockerfile ARG ${pin} default '${dockerfile_default}' drifted from pin.sh literal '${!pin}' (update one or the other)" >&2
        exit 1
    fi
done

iidfile="$(mktemp)"
trap 'rm -f "$iidfile"' EXIT

docker build --iidfile "$iidfile" -t "$IMAGE_TAG" \
    --build-arg "NODE_VERSION=$NODE_VERSION" \
    --build-arg "NODE_SHA256=$NODE_SHA256" \
    --build-arg "DOCKER_CLI_VERSION=$DOCKER_CLI_VERSION" \
    --build-arg "DOCKER_CLI_SHA256=$DOCKER_CLI_SHA256" \
    -f "$SCRIPT_DIR/Dockerfile" "$SCRIPT_DIR"

image_id="$(tr -d '[:space:]' < "$iidfile")"
if [[ ! "$image_id" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "error: iidfile did not contain a sha256 image ID: '$image_id'" >&2
    exit 1
fi

# Prefer a push-qualified RepoDigest; empty for local builds.
repo_digest=""
if repo_digest="$(docker image inspect --format '{{index .RepoDigests 0}}' "$image_id" 2>/dev/null)" \
   && [[ -n "$repo_digest" ]]; then
    digest="${repo_digest##*@}"
else
    digest="$(docker image inspect --format '{{.Id}}' "$image_id")"
fi

if [[ ! "$digest" =~ ^sha256:[0-9a-f]{64}$ ]]; then
    echo "error: resolved reference is not an immutable sha256 digest: '$digest'" >&2
    echo "       (mutable tags and malformed references are never recorded)" >&2
    exit 1
fi

printf '%s\n' "$digest" > "$DIGEST_FILE"
echo "wrote $DIGEST_FILE: $digest"
