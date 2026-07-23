#!/usr/bin/env bash
set -euo pipefail

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

# --- Musl toolchain setup for static linking ---
MUSL_PREFIX="/tmp/musl-toolchain"
ZLIB_VERSION="1.3.1"
MUSL_TOOLCHAIN_SHA256="eb1db6f0f3c2bdbdbfb993d7ef7e2eeef82ac1259f6a6e1757c33a97dbcef3ad"
MUSL_TOOLCHAIN_URL="https://github.com/kennycallado/rust-camel/releases/download/musl-toolchain-v11.2.1/x86_64-linux-musl-native.tgz"
MUSL_TOOLCHAIN_FALLBACK_URL="https://more.musl.cc/11.2.1/x86_64-linux-musl/x86_64-linux-musl-native.tgz"
ZLIB_URL="https://github.com/madler/zlib/releases/download/v${ZLIB_VERSION}/zlib-${ZLIB_VERSION}.tar.gz"

echo "Setting up musl toolchain for static native-image build..."

if [[ ! -x "${MUSL_PREFIX}/bin/x86_64-linux-musl-gcc" ]]; then
    echo "  Downloading musl toolchain..."
    mkdir -p "${MUSL_PREFIX}"
    ARCHIVE="/tmp/musl-toolchain.tgz"

    download_with_verify() {
        local url="$1"
        echo "  Trying ${url}..."
        curl -sSL --retry 3 --max-time 120 --retry-delay 5 -o "${ARCHIVE}" "${url}"
        echo "  Verifying SHA256..."
        echo "${MUSL_TOOLCHAIN_SHA256}  ${ARCHIVE}" | sha256sum -c - || return 1
    }

    if ! download_with_verify "${MUSL_TOOLCHAIN_URL}"; then
        echo "  Primary download failed, trying fallback..."
        download_with_verify "${MUSL_TOOLCHAIN_FALLBACK_URL}" || {
            echo "ERROR: Both musl toolchain downloads failed" >&2
            exit 1
        }
    fi

    tar -xz -C "${MUSL_PREFIX}" --strip-components=1 < "${ARCHIVE}"
    rm -f "${ARCHIVE}"
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
    curl -sSL --retry 3 --max-time 120 --retry-delay 5 "${ZLIB_URL}" \
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
