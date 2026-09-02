# Bridges

Language-specific bridge processes for Apache Camel components that require non-Rust runtimes. Each bridge communicates with the Rust runtime via mTLS gRPC (protobuf over HTTPS with mutual certificate authentication on localhost).

## Available Bridges

| Bridge         | Runtime        | Components                                 |
| -------------- | -------------- | ------------------------------------------ |
| [`cxf/`](cxf/) | Java / Quarkus | `camel-cxf`                                |
| [`jms/`](jms/) | Java / Quarkus | `camel-jms`                                |
| [`xml/`](xml/) | Java / Quarkus | `camel-xslt`, `camel-validator` (XSD mode) |

---

## Environment Variables

Each bridge reads its configuration from environment variables at startup. Malformed values fail loud before the bridge accepts traffic (ADR-0033). Per-bridge semantics: [`cxf/`](cxf/README.md), [`jms/`](jms/README.md).

### cxf bridge

| Variable             | Default             | Description                                                                                    |
| -------------------- | ------------------- | ---------------------------------------------------------------------------------------------- |
| `CXF_MAX_BODY_BYTES` | `16777216` (16 MiB) | Body cap, both directions: listener request bodies (oversized get HTTP 413) and producer response bodies (oversized fail the route with gRPC `RESOURCE_EXHAUSTED`). Ceiling 17 MiB, below the 18 MiB Rust gRPC decode limit. |
| `CXF_MAX_DISPATCHES` | `64` | Bounds the producer's Dispatch cache (LRU eviction, entries closed on evict/shutdown). Ceiling 1024. Malformed values abort startup. |

The cxf listener accepts `http://` consumer addresses only; TLS listener support is not yet available.

### jms bridge

| Variable                        | Default             | Description                                                                                     |
| ------------------------------- | ------------------- | ----------------------------------------------------------------------------------------------- |
| `JMS_MAX_BODY_BYTES`            | `16777216` (16 MiB) | Message body cap. Ceiling 19 MiB, below the 20 MiB Rust IPC decode limit.                        |
| `BRIDGE_BROKER_URL`             | `tcp://localhost:61616` | Broker connection URL.                                                                       |
| `BRIDGE_BROKER_TYPE`            | `activemq`          | Broker adapter type. Valid values: `activemq`, `artemis`.                                        |
| `BRIDGE_BROKER_KEYSTORE_PATH`   | _(none)_            | PKCS12 keystore for secure broker schemes (`ssl://`, `wss://`).                                  |
| `BRIDGE_BROKER_TRUSTSTORE_PATH` | _(none)_            | PKCS12 truststore for secure broker schemes.                                                     |
| `BRIDGE_BROKER_KEYSTORE_PASSWORD` | _(none)_          | Keystore password. Required for secure broker schemes.                                           |

---

## Running Tests

The bridges require a Java 21 toolchain with GraalVM CE for native compilation. In environments where Java is not installed (e.g. NixOS with Rust-only toolchain), use the official Quarkus builder image via Docker.

### Prerequisites

- Docker (or Podman with `alias docker=podman`)
- Image: `quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25`
  (tag MUST match the toolchain CI builds with — `java-version` in
  `.github/workflows/*-bridge-release.yml`; keep both in sync)

Pull once:

```bash
docker pull quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25
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
  quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 \
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
  quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-25 \
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
