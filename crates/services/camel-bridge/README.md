# camel-bridge

> Bridge process lifecycle management for rust-camel (spawn, health, download, mTLS)

Bridge process lifecycle management for [rust-camel](https://github.com/kennycallado/rust-camel).

Internal crate — not intended for direct use. Provides process management, binary download, and mTLS gRPC connectivity for bridge-based components (e.g. `camel-component-jms`).

## Overview

- **Process management** — Spawn an external bridge binary, read ephemeral SSL port from stdout JSON `{"status":"ready","port":N}`, and manage its lifecycle (SIGTERM + wait, then SIGKILL fallback).
- **Binary download** — Fetch bridge binaries from GitHub releases, verify SHA256 checksums, and cache locally.
- **mTLS cert generation** — Generate ephemeral CA + server + client certs via rcgen on each bridge spawn. Certs are written to a 0700 TempDir and cleaned up on drop.
- **Health checking** — Poll a gRPC health endpoint until ready (100 ms interval, configurable timeout).
- **Channel connectivity** — Create a tonic `Channel` to `https://127.0.0.1:{port}` with mutual TLS.

## Modules

| Module | Description |
|--------|-------------|
| `process` | `BridgeProcess` — spawn, read ready message, graceful stop |
| `tls` | `BridgeTlsMaterial` — ephemeral rcgen CA+server+client cert generation |
| `channel` | `connect_channel()` — create mTLS tonic Channel (crate-private) |
| `reconnect` | `BridgeReconnectHandler` trait — re-seed stateful resources after bridge restart |
| `download` | `ensure_binary()` / `default_cache_dir()` — download + verify + cache bridge binary |
| `health` | `wait_for_health()` — poll gRPC health check with timeout |

## Bridge TLS

The bridge subprocess (Java/Quarkus native image) runs in unified gRPC mode with
`ssl.client-auth=REQUIRED`. Build-time TLS properties are in each bridge's
`application.yml`; runtime cert paths are passed via 4 env vars:

| Variable | Description |
|----------|-------------|
| `QUARKUS_HTTP_SSL_PORT` | SSL port the bridge listens on |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT` | Server cert PEM path |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY` | Server key PEM path |
| `QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS` | CA cert PEM path (for client auth) |

Placeholder certs in `bridges/*/src/main/resources/tls/` enable SSL at Quarkus
native build time. A fail-closed guard in PortAnnouncer rejects startup if cert
paths resolve to `placeholder-*`.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CAMEL_JMS_BRIDGE_BINARY_PATH` | Use a local bridge binary instead of downloading |
| `CAMEL_JMS_BRIDGE_RELEASE_URL` | Override release download URL (must be `https://github.com/**`) |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-bridge = "*"
```
