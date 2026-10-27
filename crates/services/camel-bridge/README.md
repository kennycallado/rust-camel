# camel-bridge

> Bridge process lifecycle management for rust-camel (spawn, health, download, mTLS)

Bridge process lifecycle management for [rust-camel](https://github.com/kennycallado/rust-camel).

Internal crate — not intended for direct use. Provides process management, binary download, and mTLS gRPC connectivity for bridge-based components (e.g. `camel-component-jms`).

## Overview

- **Process management** — Spawn an external bridge binary, read ephemeral SSL port from stdout JSON `{"status":"ready","port":N}`, and manage its lifecycle (group SIGTERM, bounded wait, group SIGKILL).
- **Parent-death supervision** — A watchdog process group dies with the host, including launcher descendants. See [Process supervision and platform guarantees](#process-supervision-and-platform-guarantees).
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

## Process supervision and platform guarantees

On Unix the child runs in a watchdog-guarded process group. The watchdog is
armed before the bridge is spawned, and the bridge joins its group at exec.
Any parent death — normal exit, panic, `SIGTERM`, `SIGINT`, or `SIGKILL` —
closes the liveness pipe. The watchdog sees EOF and `SIGKILL`s its own group.
The bridge, launcher descendants, and the watchdog die together.

With the embedder hook installed, the watchdog is the host binary re-executed
in a hidden role (`argv[1]` `__camel-bridge-watchdog`, ack fd 9, ack byte
`0x01`). It refuses to arm unless it leads its own process group (exit 74),
ignores `SIGTERM`, and `SIGKILL`s group 0 on stdin EOF. Linux sources the
executable from literal `/proc/self/exe`; other Unix targets use the path
captured at startup by `std::env::current_exe()`. POSIX `sh` remains only as
the no-hook fallback. No hook plus no shell fails closed and names both
remedies.

| Platform | Guarantee |
|----------|-----------|
| Linux | Full group supervision, via self-exec or the `sh` fallback. Verified in CI. |
| macOS | Full group supervision by design, via self-exec or the `sh` fallback. The evidence suite has not been run. |
| Windows | No process groups and no POSIX shell. Explicit-path teardown only: `stop()` and `Drop` kill the child directly. An unclean parent death can leak the bridge and its descendants. |

`stop()` is group-scoped: `SIGTERM` to the group, a bounded non-reaping wait,
unconditional group `SIGKILL`, group-id invalidation, then reaping of the
bridge and the watchdog. `Drop` is a synchronous fallback, not a graceful
shutdown; it does not wait or reap.

## Embedder contract (watchdog hook)

Call `camel_bridge::watchdog::maybe_run()` as the **first statement of a
synchronous `main`**, before any code consumes argv. On Unix the call is a
no-op unless the process was re-executed in the watchdog role. On non-Unix it
is a no-op.

```rust,ignore
fn main() {
    camel_bridge::watchdog::maybe_run(); // before argv parsing / artifact self-detection
    // ... build the async runtime and run the application
}
```

In `camel-cli` the call runs before `self_detect_artifact` and is
unconditional in every flavor. The xsd bridge (`camel-component-validator`) is
an unconditional dependency, so `camel-bridge` is in every flavor's closure,
slim included. A compiled artifact would otherwise consume the watchdog argv.

## TLS directory resolution

Ephemeral certificate files are written under the first usable base directory:

1. `$TMPDIR`, when set, non-empty, and usable.
2. The system temporary directory, when usable.
3. `{cache_dir}/{crate version}/tls`, created on demand.

Each candidate is probed by performing the real temporary-directory creation.
Directories this crate creates get mode `0700`; pre-existing components keep
their mode. If every candidate fails, the error names `XDG_CACHE_HOME` and
`$TMPDIR` as remedies.

## Container requirements

With the hook installed, bridge routes start in shell-less images such as
`production` (`scratch`) and `gnu` (distroless). No shell and no usable `/tmp`
are required. The operator must provide one writable location for TLS
material:

- a writable cache directory (default `{cache_dir}`), or
- a writable `$TMPDIR`.

A read-only root filesystem with a non-root user works when a writable volume
is mounted and `XDG_CACHE_HOME` points at it. The CI container guard exercises
both the default and the read-only variants.

## Bridge TLS

The bridge subprocess (Java/Quarkus native image) runs in unified gRPC mode with
`ssl.client-auth=REQUIRED`. Build-time TLS properties are in each bridge's
`application.yml`; runtime cert paths are passed via 4 env vars:

| Variable | Description |
|----------|-------------|
| `QUARKUS_HTTP_SSL_PORT` | SSL port mode: `0` (supervisor handoff — JVM binds an OS-assigned port and announces the actual port) or a fixed port for standalone runs |
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
