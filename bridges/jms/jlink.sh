#!/usr/bin/env bash
set -euo pipefail

VERSION=${1:-"0.1.0"}
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
[ "$ARCH" = "arm64" ] && ARCH="aarch64"

BUILD_DIR="build/release"
DIST_NAME="jms-bridge-${VERSION}-${OS}-${ARCH}"

echo "Building fat JAR..."
gradle quarkusBuild -Dquarkus.package.type=uber-jar

echo "Creating minimal JRE with jlink..."
JAVA_HOME_ACTUAL=$(java -XshowSettings:properties -version 2>&1 | grep "java.home" | awk '{print $3}')
"$JAVA_HOME_ACTUAL/bin/jlink" \
  --add-modules java.base,java.net.http,java.naming,java.sql,java.management \
  --output "${BUILD_DIR}/${DIST_NAME}/jre" \
  --strip-debug \
  --no-man-pages \
  --no-header-files \
  --compress=2

echo "Packaging..."
mkdir -p "${BUILD_DIR}/${DIST_NAME}/lib"
cp build/*-runner.jar "${BUILD_DIR}/${DIST_NAME}/lib/jms-bridge.jar"

mkdir -p "${BUILD_DIR}/${DIST_NAME}/bin"
cat > "${BUILD_DIR}/${DIST_NAME}/bin/jms-bridge" << 'EOF'
#!/bin/sh
DIR="$(cd "$(dirname "$0")/.." && pwd)"
exec "$DIR/jre/bin/java" -jar "$DIR/lib/jms-bridge.jar" "$@"
EOF
chmod +x "${BUILD_DIR}/${DIST_NAME}/bin/jms-bridge"

mkdir -p "${BUILD_DIR}"
tar -czf "${BUILD_DIR}/${DIST_NAME}.tar.gz" -C "${BUILD_DIR}" "${DIST_NAME}"
SHA256=$(sha256sum "${BUILD_DIR}/${DIST_NAME}.tar.gz" | cut -d' ' -f1)
echo "${SHA256}  ${DIST_NAME}.tar.gz" > "${BUILD_DIR}/${DIST_NAME}.tar.gz.sha256"
echo "Created: ${BUILD_DIR}/${DIST_NAME}.tar.gz (sha256: ${SHA256})"
