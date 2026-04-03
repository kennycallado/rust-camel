# camel-bridge

Bridge process lifecycle management for [rust-camel](https://github.com/kennycallado/rust-camel).

Internal crate — not intended for direct use. Provides process management, binary download, and gRPC connectivity for bridge-based components (e.g. `camel-component-jms`).

## Overview

- **Process management** — Spawn an external bridge binary, read ephemeral port from stdout JSON `{"status":"ready","port":N}`, and manage its lifecycle (SIGTERM + wait, then SIGKILL fallback).
- **Binary download** — Fetch bridge binaries from GitHub releases, verify SHA256 checksums, and cache locally.
- **Health checking** — Poll a gRPC health endpoint until ready (100 ms interval, configurable timeout).
- **Channel connectivity** — Create a tonic `Channel` to `127.0.0.1:{port}`.

## Modules

| Module | Description |
|--------|-------------|
| `process` | `BridgeProcess` — spawn, read ready message, graceful stop |
| `download` | `ensure_binary()` / `default_cache_dir()` — download + verify + cache bridge binary |
| `health` | `wait_for_health()` — poll gRPC health check with timeout |
| `channel` | `connect_channel()` — create tonic Channel to localhost |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `CAMEL_JMS_BRIDGE_BINARY_PATH` | Use a local bridge binary instead of downloading |
| `CAMEL_JMS_BRIDGE_RELEASE_URL` | Override release download URL (must be `https://github.com/**`) |

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-bridge = "0.5"
```
