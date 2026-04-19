# Bridges

Language-specific bridge processes for Apache Camel components that require non-Rust runtimes. Each bridge communicates with the Rust runtime via gRPC (protobuf over Unix socket or TCP).

## Available Bridges

| Bridge | Runtime | Components |
|--------|---------|------------|
| [`jms/`](jms/) | Java / Quarkus | `camel-jms` |
| [`xml/`](xml/) | Java / Quarkus (GraalVM native) | `camel-xslt`, `camel-validator` (XSD mode) |

---

## Running Tests

The bridges require a Java 21 toolchain with GraalVM/Mandrel for native compilation. In environments where Java is not installed (e.g. NixOS with Rust-only toolchain), use the official Quarkus builder image via Docker.

### Prerequisites

- Docker (or Podman with `alias docker=podman`)
- Image: `quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21`

Pull once:

```bash
docker pull quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21
```

### xml bridge tests

```bash
BRIDGE_DIR="$(pwd)/bridges/xml"

docker run --rm \
  --user root \
  --volume="$BRIDGE_DIR:/project:z" \
  --workdir=/project \
  --env=GRADLE_USER_HOME=/tmp/gradle-home \
  --env=HOME=/tmp \
  --env=APP_HOME= \
  --entrypoint bash \
  quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21 \
  -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache 2>&1"
```

> **Note:** `--user root` is required so that the container can clean up the `build/` directory it creates (owned by root inside the container). Without it, subsequent runs fail because the host user cannot remove those files.

### jms bridge tests

```bash
BRIDGE_DIR="$(pwd)/bridges/jms"

docker run --rm \
  --user root \
  --volume="$BRIDGE_DIR:/project:z" \
  --workdir=/project \
  --env=GRADLE_USER_HOME=/tmp/gradle-home \
  --env=HOME=/tmp \
  --entrypoint bash \
  quay.io/quarkus/ubi9-quarkus-mandrel-builder-image:jdk-21 \
  -c "rm -rf /project/build && ./gradlew test --no-daemon --project-cache-dir /tmp/gradle-project-cache 2>&1"
```

---

## Native Build

Each bridge ships a `build-native.sh` script that compiles a self-contained native binary (no JVM required at runtime).

```bash
# From the project root (uses xtask)
cargo xtask build-xml-bridge

# Or directly inside the bridge directory
cd bridges/xml
./build-native.sh
```

The resulting binary is placed at `bridges/xml/build/camel-xml-bridge`.

---

## Architecture

```
Rust component (camel-xslt, camel-validator, …)
        │
        │  gRPC (Unix socket / TCP)
        ▼
   xml bridge (native binary)
        │
        ├── Xerces-J   → XSD validation
        └── Saxon-HE   → XSLT 2.0/3.0 transformation
```

The bridge is started on-demand by the Rust component via `camel-bridge` process manager and communicates via the `xml_bridge.proto` service definition in `bridges/xml/src/main/proto/`.
