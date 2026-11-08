#!/usr/bin/env bash
set -euo pipefail

# --- Musl toolchain setup for static linking ---
# The extracted toolchain lives in /tmp (fast, execable tmpfs on CI runners),
# but the 89 MB source tarball is cached under GRADLE_USER_HOME, which the CI
# workflow persists via actions/cache. After the first successful run the
# tarball is verified from cache and the network download never happens again.
MUSL_PREFIX="/tmp/musl-toolchain"
ZLIB_VERSION="1.3.1"
MUSL_TOOLCHAIN_SHA256="eb1db6f0f3c2bdbdbfb993d7ef7e2eeef82ac1259f6a6e1757c33a97dbcef3ad"
MUSL_TOOLCHAIN_URL="https://github.com/kennycallado/rust-camel/releases/download/musl-toolchain-v11.2.1/x86_64-linux-musl-native.tgz"
# Test-only override for the offline battery (scripts/tests/
# musl-fetch-retry-test.sh). Default unchanged; the SHA256 check below
# still fails a wrong payload, which the battery relies on.
MUSL_TOOLCHAIN_URL="${MUSL_TOOLCHAIN_URL_OVERRIDE:-${MUSL_TOOLCHAIN_URL}}"
ZLIB_URL="https://github.com/madler/zlib/releases/download/v${ZLIB_VERSION}/zlib-${ZLIB_VERSION}.tar.gz"
# Persist the verified tarball in the CI-cached Gradle home so re-runs skip the
# download entirely. Falls back to /tmp when GRADLE_USER_HOME is unset.
MUSL_CACHE_DIR="${GRADLE_USER_HOME:-/tmp}/musl-toolchain-cache"
ARCHIVE="${MUSL_CACHE_DIR}/x86_64-linux-musl-native.tgz"

# Verify a candidate tarball against the pinned SHA256. Returns non-zero on a
# missing file or hash mismatch so callers can decide whether to (re)download.
verify_archive() {
    [[ -f "${ARCHIVE}" ]] || return 1
    echo "${MUSL_TOOLCHAIN_SHA256}  ${ARCHIVE}" | sha256sum -c - >/dev/null 2>&1
}

# Download the musl toolchain tarball with exponential backoff inside a
# total time budget (MUSL_FETCH_BUDGET_SECONDS, default 600 s; the schedule
# is 5/10/20/40/60 s capped at 60 s, overridable via MUSL_FETCH_DELAYS for
# the offline battery). curl's fixed `--retry 5 --retry-delay 5` gave a
# ~25 s window and lost a Full Tests run to a transient 504 storm on the
# release asset (rc-1c47h). Per-attempt curl semantics are unchanged:
# -f fails on any HTTP >=400 instead of writing an error page to disk,
# -C - resumes partial transfers across attempts; --retry-all-errors is
# kept for parity. On exhaustion: non-zero exit naming the URL, the
# attempt count and the elapsed budget — no silent skip.
fetch_musl_toolchain() {
    local archive="$1"
    local url="${MUSL_TOOLCHAIN_URL_OVERRIDE:-${MUSL_TOOLCHAIN_URL}}"
    local budget="${MUSL_FETCH_BUDGET_SECONDS:-600}"
    local -a delays
    read -r -a delays <<< "${MUSL_FETCH_DELAYS:-5 10 20 40 60}"
    local start=$SECONDS
    local attempt=0
    local elapsed remaining delay

    while true; do
        elapsed=$((SECONDS - start))
        remaining=$((budget - elapsed))
        if (( remaining <= 0 )); then
            echo "ERROR: musl toolchain download failed from ${url}" >&2
            echo "  Gave up after ${attempt} attempt(s): ${elapsed}s elapsed against a ${budget}s budget." >&2
            rm -f "${archive}"
            return 1
        fi
        attempt=$((attempt + 1))
        echo "  Attempt ${attempt}: GET ${url} (budget left: ${remaining}s)"
        if curl -fSL --retry-all-errors --connect-timeout 30 \
            --max-time "${remaining}" -C - -o "${archive}" "${url}"; then
            return 0
        fi
        if (( attempt <= ${#delays[@]} )); then
            delay=${delays[$((attempt - 1))]}
        else
            delay=${delays[${#delays[@]} - 1]}
        fi
        if (( delay > remaining )); then
            delay=$remaining
        fi
        echo "  Attempt ${attempt} failed; retrying in ${delay}s..."
        sleep "${delay}"
    done
}

# Ensure the toolchain tarball at $ARCHIVE exists and matches the pinned
# SHA256: cache hit is used as-is; otherwise fetch (with backoff) and
# verify. Original call-site logic, unchanged apart from the extraction.
ensure_musl_archive() {
    if verify_archive; then
        echo "  Using cached musl toolchain tarball (SHA256 verified)."
        return 0
    fi
    # A stale/corrupt cached tarball must not be trusted or extracted.
    rm -f "${ARCHIVE}"
    echo "  Downloading musl toolchain..."
    fetch_musl_toolchain "${ARCHIVE}" || return 1
    echo "  Verifying SHA256..."
    if ! verify_archive; then
        echo "ERROR: musl toolchain SHA256 mismatch after download." >&2
        echo "  Expected: ${MUSL_TOOLCHAIN_SHA256}" >&2
        echo "  Actual:   $(sha256sum "${ARCHIVE}" | cut -d' ' -f1)" >&2
        echo "  The pinned release asset may have been replaced. Do NOT" >&2
        echo "  update the pin without verifying the new artifact." >&2
        rm -f "${ARCHIVE}"
        return 1
    fi
    return 0
}

# --- Test hook -------------------------------------------------------------
# `--print-musl-fetch` prints the production musl-download functions and
# exits 0, before the in-container guard: the offline battery
# (scripts/tests/musl-fetch-retry-test.sh) sources this output on the
# host so it exercises exactly the logic CI runs instead of a drifting
# copy. A guarded main would not work — this script's top-level flow
# (arg parsing, container guard, traps) is meant to run, not to be
# sourced.
if [[ "${1:-}" == "--print-musl-fetch" ]]; then
    declare -f verify_archive fetch_musl_toolchain ensure_musl_archive
    exit 0
fi

IN_CONTAINER=false
VERSION="dev"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --in-container)
            IN_CONTAINER=true
            shift
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

if [[ "$IN_CONTAINER" == "false" ]]; then
    echo "ERROR: Direct invocation is not supported." >&2
    echo "Use 'cargo xtask build-jms-bridge' to build the JMS bridge." >&2
    exit 1
fi

# Validate version string to prevent path traversal or malformed filenames
if [[ ! "$VERSION" =~ ^(dev|[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9._]+)?)$ ]]; then
    echo "ERROR: Invalid version string: '$VERSION'" >&2
    exit 1
fi

# --- In-container execution ---
# We are inside the GraalVM CE Docker container
# GRADLE_USER_HOME is set by xtask to /project/.gradle-docker-cache

# Fix ownership of cache/build dirs on exit (container runs as root,
# but host may have different uid). chown first, chmod fallback.
HOST_UID="$(stat -c '%u' /project 2>/dev/null || echo 0)"
HOST_GID="$(stat -c '%g' /project 2>/dev/null || echo 0)"
cleanup_permissions() {
    chown -R "${HOST_UID}:${HOST_GID}" /project/build /project/.gradle-docker-cache 2>/dev/null \
        || chmod -R a+rwX /project/build /project/.gradle-docker-cache 2>/dev/null || true
}
trap cleanup_permissions EXIT

echo "Setting up musl toolchain for static native-image build..."

if [[ ! -x "${MUSL_PREFIX}/bin/x86_64-linux-musl-gcc" ]]; then
    mkdir -p "${MUSL_PREFIX}" "${MUSL_CACHE_DIR}"
    ensure_musl_archive || exit 1
    tar -xz -C "${MUSL_PREFIX}" --strip-components=1 < "${ARCHIVE}"
fi

# Expose ONLY x86_64-linux-musl-gcc to PATH via a minimal wrapper directory.
# This lets GraalVM find it for --libc=musl static linking without polluting
# PATH with musl binutils (ld, as, ar, etc.) that shadow system tools and
# break GraalVM's probe compilation (PosixDirectives, JNIHeaderDirectives…).
# The cross-compiler has its own sysroot so it finds its binutils and libz.a
# without LIBRARY_PATH.
mkdir -p /tmp/musl-bin
ln -sf "${MUSL_PREFIX}/bin/x86_64-linux-musl-gcc" /tmp/musl-bin/x86_64-linux-musl-gcc
# native-image --libc=musl looks for 'musl-gcc' (no arch prefix) in PATH.
ln -sf "${MUSL_PREFIX}/bin/x86_64-linux-musl-gcc" /tmp/musl-bin/musl-gcc
export PATH="/tmp/musl-bin:${PATH}"

# native-image selects x86_64-linux-musl-gcc on its own from --libc=musl; no
# CC override is needed (an earlier CC= export here was a proven no-op).

# Build static zlib against musl if not already built
if [[ ! -f "${MUSL_PREFIX}/lib/libz.a" ]]; then
    echo "  Building static zlib ${ZLIB_VERSION} against musl..."
    ZLIB_SRC="/tmp/zlib-${ZLIB_VERSION}"
    curl -fSL --retry 5 --retry-all-errors --retry-delay 5 \
        --connect-timeout 30 --max-time 300 "${ZLIB_URL}" \
        | tar -xz -C /tmp
    cd "${ZLIB_SRC}"
    CC=x86_64-linux-musl-gcc ./configure --static --prefix="${MUSL_PREFIX}"
    make -j"$(nproc)" install
    cd /project
    rm -rf "${ZLIB_SRC}"
fi

echo "  Musl toolchain ready: $(${MUSL_PREFIX}/bin/x86_64-linux-musl-gcc --version | head -1)"

echo "  Musl toolchain ready: $(x86_64-linux-musl-gcc --version | head -1)"
echo ""

echo "Building Quarkus native image (GraalVM CE + musl)..."
echo "  Version: ${VERSION}"
echo "  Gradle home: ${GRADLE_USER_HOME:-<not set>}"
echo "  Static:  yes (musl)"
echo ""

# Invoke Gradle via the wrapper jar directly (avoids JAVA_HOME lookup issues
# when bash is used as --entrypoint in the GraalVM CE container).
#
# Native-image args come from TWO parts:
#   1. Platform-neutral args from native-build-args.env (the single source of
#      truth shared with the windows/macos xtask path).
#   2. Linux-only musl/static args, appended here so application.yml + the
#      neutral file both stay platform-neutral.
#
# -no-pie is REQUIRED: the musl toolchain (more.musl.cc 11.2.1) is built with
# --enable-default-pie/--enable-static-pie, so musl-gcc defaults to -static-pie.
# GraalVM's .svm_heap carries absolute relocations that are incompatible with
# -pie, so the link fails with "read-only segment has dynamic relocations".
# Without --no-fallback native-image then silently emits a glibc-dynamic
# fallback image (the mislabeled-static bug). -H:NativeLinkerOption=-no-pie
# forces a non-PIE static link; --no-fallback turns any musl link failure into
# a hard error instead of a silent glibc fallback.
#
# NOTE: the env var is the SOLE source of native-image args. application.yml must
# NOT set quarkus.native.additional-build-args, or SmallRye prioritizes the
# indexed YAML form and silently drops these comma-form args.
# shellcheck source=./native-build-args.env
source "$(dirname "$0")/native-build-args.env"
export QUARKUS_NATIVE_ADDITIONAL_BUILD_ARGS="${NATIVE_BUILD_ARGS_NEUTRAL},--no-fallback,--static,--libc=musl,-H:NativeLinkerOption=-no-pie"
java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain \
    build -Dquarkus.package.jar.enabled=false -Dquarkus.native.enabled=true \
    -Pversion="${VERSION}" --no-daemon

# Locate the native runner (resilient to Quarkus naming changes)
RUNNER=$(find build -maxdepth 1 -name '*-runner' -not -name '*.jar' -type f 2>/dev/null | head -1)

if [[ -z "$RUNNER" ]]; then
    echo "ERROR: Native runner not found in build/. Native build may have failed." >&2
    exit 1
fi

echo "Native runner: $RUNNER"

# Verify static linking
if readelf -l "$RUNNER" 2>/dev/null | grep -q "Requesting program interpreter"; then
    echo "WARNING: Binary has dynamic interpreter (not fully static)" >&2
else
    echo "Verified: binary is statically linked (no interpreter segment)"
fi

# Copy to canonical path — use install(1) instead of cp so that if the
# destination binary is currently executing (ETXTBSY), install writes to a
# fresh inode and atomically renames it into place rather than overwriting
# the live file.
mkdir -p build/native
install -m 0777 "$RUNNER" build/native/jms-bridge
echo "Binary: build/native/jms-bridge"

# Package release tarball
DIST_NAME="jms-bridge-${VERSION}-linux-musl-x86_64"
BUILD_DIR="build/release"
mkdir -p "${BUILD_DIR}/${DIST_NAME}/bin"
install -m 0777 build/native/jms-bridge "${BUILD_DIR}/${DIST_NAME}/bin/jms-bridge"

tar -czf "${BUILD_DIR}/${DIST_NAME}.tar.gz" -C "${BUILD_DIR}" "${DIST_NAME}"
SHA256=$(sha256sum "${BUILD_DIR}/${DIST_NAME}.tar.gz" | cut -d' ' -f1)
echo "${SHA256}  ${DIST_NAME}.tar.gz" > "${BUILD_DIR}/${DIST_NAME}.tar.gz.sha256"

echo ""
echo "Tarball: ${BUILD_DIR}/${DIST_NAME}.tar.gz"
echo "SHA256:  ${SHA256}"
