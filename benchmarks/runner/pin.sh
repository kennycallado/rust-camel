#!/usr/bin/env bash
# Build the canonical benchmark runner image and record its digest.
#
# Usage: bash benchmarks/runner/pin.sh
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE_TAG="${IMAGE_TAG:-benchmark-runner:v1}"
DIGEST_FILE="$SCRIPT_DIR/DIGEST"

iidfile="$(mktemp)"
trap 'rm -f "$iidfile"' EXIT

docker build --iidfile "$iidfile" -t "$IMAGE_TAG" -f "$SCRIPT_DIR/Dockerfile" "$SCRIPT_DIR"

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
