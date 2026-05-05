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
# We are inside the Mandrel Docker container (quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21)
# GRADLE_USER_HOME is set by xtask to /project/.gradle-docker-cache

echo "Building Quarkus native image (Mandrel)..."
echo "  Version: ${VERSION}"
echo "  Gradle home: ${GRADLE_USER_HOME:-<not set>}"
echo ""

# Invoke Gradle via the wrapper jar directly (avoids JAVA_HOME lookup issues
# when bash is used as --entrypoint in the Mandrel container).
java -cp gradle/wrapper/gradle-wrapper.jar org.gradle.wrapper.GradleWrapperMain \
    build -Dquarkus.package.jar.enabled=false -Dquarkus.native.enabled=true -Pversion="${VERSION}" --no-daemon

# Locate the native runner (resilient to Quarkus naming changes)
RUNNER=$(find build -maxdepth 1 -name '*-runner' -not -name '*.jar' -type f 2>/dev/null | head -1)

if [[ -z "$RUNNER" ]]; then
    echo "ERROR: Native runner not found in build/. Native build may have failed." >&2
    exit 1
fi

echo "Native runner: $RUNNER"

# Copy to canonical path — use install(1) instead of cp so that if the
# destination binary is currently executing (ETXTBSY), install writes to a
# fresh inode and atomically renames it into place rather than overwriting
# the live file.
mkdir -p build/native
install -m 0777 "$RUNNER" build/native/jms-bridge
echo "Binary: build/native/jms-bridge"

# Package release tarball
DIST_NAME="jms-bridge-${VERSION}-linux-x86_64"
BUILD_DIR="build/release"
mkdir -p "${BUILD_DIR}/${DIST_NAME}/bin"
install -m 0777 build/native/jms-bridge "${BUILD_DIR}/${DIST_NAME}/bin/jms-bridge"

tar -czf "${BUILD_DIR}/${DIST_NAME}.tar.gz" -C "${BUILD_DIR}" "${DIST_NAME}"
SHA256=$(sha256sum "${BUILD_DIR}/${DIST_NAME}.tar.gz" | cut -d' ' -f1)
echo "${SHA256}  ${DIST_NAME}.tar.gz" > "${BUILD_DIR}/${DIST_NAME}.tar.gz.sha256"

echo ""
echo "Tarball: ${BUILD_DIR}/${DIST_NAME}.tar.gz"
echo "SHA256:  ${SHA256}"
