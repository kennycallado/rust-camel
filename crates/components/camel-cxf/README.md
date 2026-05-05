# camel-component-cxf

SOAP/Web Services component for [rust-camel](https://github.com/kennycallado/rust-camel) using Apache CXF via a Java bridge process. Supports **PAYLOAD mode** (raw SOAP body XML) for both producing (calling SOAP services) and consuming (hosting SOAP endpoints).

## URI Format

```
cxf://http://host:port/path?wsdl=path/to/service.wsdl&service={namespace}ServiceName&port={namespace}PortName&operation=operationName&profile=my_profile
```

## URI Query Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `wsdl` | _(from profile)_ | Path to WSDL file (optional if set in profile) |
| `service` | _(from profile)_ | Service name with namespace in `{ns}Name` format |
| `port` | _(from profile)_ | Port name with namespace in `{ns}Name` format |
| `operation` | _(none)_ | SOAP operation name (can also be set via header `CamelCxfOperation`) |
| `profile` | _(required)_ | Security profile name (defined in `Camel.toml`) |

> `address` is the URI path portion after `cxf://` — for producers this is the target endpoint URL, for consumers this is the bind address.

## Overview

Bridge-based architecture: the Rust component communicates with a Quarkus/GraalVM native Java bridge via gRPC. The bridge uses Apache CXF to handle SOAP protocol details (envelope construction, WS-Security, WSDL parsing).

**Multi-tenant**: a single bridge process serves multiple security profiles (e.g., one per 112-community), each with its own certificates and WS-Security configuration.

## Camel.toml Configuration

```toml
[components.cxf]
version = "0.8.0"

[[components.cxf.profiles]]
name = "baleares"
wsdl_path = "wsdl/hello.wsdl"
service_name = "{http://example.com/hello}HelloService"
port_name = "{http://example.com/hello}HelloPort"

[[components.cxf.profiles]]
name = "madrid"
wsdl_path = "wsdl/hello.wsdl"
service_name = "{http://example.com/hello}HelloService"
port_name = "{http://example.com/hello}HelloPort"
# Optional: address override
# address = "http://localhost:8080/service"
# Optional: WS-Security
# [components.cxf.profiles.security]
# keystore_path = "/path/to/keystore.jks"
# keystore_password = "changeit"
# truststore_path = "/path/to/truststore.jks"
# truststore_password = "changeit"
```

Multiple profiles are supported — add as many `[[components.cxf.profiles]]` entries as needed. All profiles share a single bridge process. Profile names must match `[a-z0-9_]+` (lowercase, digits, underscore).

## Quick Start

### Producer Route (call a SOAP service)

```yaml
routes:
  - id: "cxf-producer-hello"
    from:
      uri: "timer:hello-tick?period=10000"
    steps:
      - set-body:
          constant: "<sayHello><name>World</name></sayHello>"
      - to: "cxf://http://localhost:8080/hello?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort&operation=sayHello&profile=baleares"
      - log: "SOAP response: ${body}"
```

### Consumer Route (host a SOAP endpoint)

```yaml
routes:
  - id: "cxf-consumer-hello"
    from:
      uri: "cxf://http://0.0.0.0:9090/cxf/baleares?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort&profile=baleares"
    steps:
      - log: "Received SOAP request: ${body}"
      - set-body:
          simple: "<sayHelloResponse><message>Hello from rust-camel!</message></sayHelloResponse>"
```

## How It Works

The component manages a Java bridge process (GraalVM native-image binary) internally:

1. On first use, downloads the bridge binary from GitHub releases (SHA256-verified, cached at `~/.cache/rust-camel/cxf-bridge/`).
2. Spawns **one bridge process** shared across all profiles.
3. Reads the ephemeral gRPC port from stdout (`{"status":"ready","port":N}`).
4. Passes profile configuration via environment variables (`CXF_PROFILES=list`, `CXF_PROFILE_<NAME>_*`).
5. Polls the health endpoint until ready, then routes messages over gRPC.

**Producer flow:** Exchange body (raw XML) → gRPC call with `security_profile` → Java CXF bridge resolves profile → SOAP HTTP call with profile's WS-Security → response XML back to exchange body.

**Consumer flow:** Vert.x HTTP server receives SOAP requests at `/cxf/<profile>/...` → extracts profile from URL path → gRPC stream with `security_profile` → Rust route processes → response XML → bridge signs with profile's certificates → HTTP response.

No Java runtime is required on the host — the bridge is a native binary.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CAMEL_CXF_BRIDGE_BINARY_PATH` | **Development override** — absolute path to a local `cxf-bridge` binary; skips download entirely |
| `CAMEL_CXF_BRIDGE_RELEASE_URL` | Override release download URL (must be `https://github.com/**`) |

## Development Setup

### Prerequisites

- Docker (daemon running)
- **NixOS:** `programs.nix-ld.enable = true` must be set in your NixOS configuration
- **Linux:** glibc >= 2.34 (Ubuntu 22.04+, Debian 12+, RHEL 9+)

### Building the CXF Bridge

The bridge binary is built using Docker (no Java required on your host):

```bash
# One-time build (~5-8 min first time, faster on subsequent runs)
cargo xtask build-cxf-bridge

# The binary is auto-detected. No env vars needed:
cargo run -p cxf-example
```

### Override

If you need to use a custom binary:

```bash
export CAMEL_CXF_BRIDGE_BINARY_PATH=/path/to/your/cxf-bridge
cargo run -p cxf-example
```

## Features

- SOAP 1.1 / 1.2 support
- WS-Security (UsernameToken, X.509 certificates)
- **Multi-tenant profiles** — single bridge serves multiple communities
- WSDL-first development
- PAYLOAD mode (raw XML body)
- Auto-managed bridge lifecycle
- Health monitoring and auto-restart
- Concurrent request support
- Profile-aware dispatch cache

## Known Limitations

- PAYLOAD mode only (no MESSAGE mode)
- Requires GraalVM native-image or Docker for bridge binary
- Hot-reload of profiles not supported (restart required)

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-cxf = "*"
```
