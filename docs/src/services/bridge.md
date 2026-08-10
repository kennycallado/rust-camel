# Bridge

The `camel-bridge` crate spawns and supervises external Java bridge binaries for components that need JVM-only protocols. JMS, XML, and CXF bridges all use it. Communication is gRPC with mutual TLS. Ephemeral rcgen-generated certificates are issued per spawn and never persisted.

The crate is internal. Components depend on it. Application code does not.

## Architecture

The bridge pipeline has four layers:

1. **BridgeSpec** is a static descriptor for one bridge binary. It pins the name, cache subdir, release tag prefix, and stderr log template. Constants live in `spec.rs` and are `&'static str`. Three specs ship today: `JMS_BRIDGE`, `XML_BRIDGE`, `CXF_BRIDGE`.
2. **BridgeProcess** owns one child process, its mTLS material, the announced gRPC port, the cancellation token, and the stdout-drain task. The process is the unit of lifecycle.
3. **BridgeTlsMaterial** generates a fresh CA plus server and client certs on every spawn. PEM files go to a 0700 `TempDir` on Unix, 0600 on the key file. The directory is cleaned up when the material drops.
4. **BridgeReconnectHandler** is a trait components implement to re-seed stateful resources after a bridge restart. The bridge crate owns neither restart detection nor process replacement. Components do.

Per [ADR-0007](../adr/0007-route-supervised-consumer-failure.md), consumers and Routes own supervision. The bridge crate is a primitive. It does not restart children on its own.

## gRPC channel

`BridgeProcess::start` spawns the binary, binds `:0` to let the OS pick a free port, then passes the port to the bridge through `QUARKUS_HTTP_SSL_PORT`. The bridge announces its chosen SSL port on stdout as one line of JSON: `{"status":"ready","port":N}`. The crate reads that line, then connects a mTLS tonic channel to `https://127.0.0.1:{port}`.

The TLS material is generated before spawn. The child receives the cert paths through four env vars. Build-time TLS properties live in each bridge's `application.yml`. Runtime cert paths and the SSL port are the only runtime surface.

| Env var | Purpose |
|---------|---------|
| `QUARKUS_HTTP_SSL_PORT` | SSL port the bridge binds |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT` | Server cert PEM path |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY` | Server key PEM path |
| `QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS` | CA cert PEM path (for client auth) |

The connect step retries the TLS handshake up to ten times. Quarkus `PortAnnouncer` fires on `StartupEvent` and can precede full SSL listener readiness by a few hundred milliseconds in native images. The first attempt often fails. The retry loop absorbs that.

## Process lifecycle

`BridgeProcess::start_and_connect` performs the full bootstrap in one call. It validates the config, generates the TLS material, spawns the child, reads the ready line, then connects the channel. A bounded stdout drain runs in the background for the rest of the process lifetime.

`BridgeProcess::stop` cancels the drain task, sends `SIGTERM`, waits five seconds, then sends `SIGKILL`. The `Drop` impl cancels the drain and best-effort kills the child. Drop cannot wait. Long shutdowns need an explicit `stop` call.

Stdout drain is bounded. Single lines cap at 64 KiB. Logging rate-limits to 100 lines per second with a drop summary. The drain never stops reading. An undrained OS pipe fills and blocks the child. Bounded size prevents memory growth. Rate-limited logging prevents log flooding. Both are required for stable long-running bridges.

## Binary acquisition

`ensure_binary` resolves the bridge binary in four steps:

1. `CAMEL_JMS_BRIDGE_BINARY_PATH` (or the equivalent var for the spec) overrides everything. Use this for local development.
2. A workspace-root build at `{workspace}/bridges/{name}/build/native/{name}`. Picked up automatically when `cargo xtask build-{name}-bridge` has run.
3. A previously downloaded and verified copy in the cache dir, default `~/.cache/rust-camel/{name}/`.
4. A download from GitHub Releases with SHA256 verification.

The release URL must point at `https://github.com/...`. The crate rejects HTTP, non-GitHub hosts, and `github.com.evil.com` lookalikes. Tarball extraction rejects absolute paths and `..` components. Path traversal during unpack is a hard error.

## Reconnect contract

Components that hold stateful resources inside a bridge (compiled XSDs, compiled XSLT stylesheets, open JMS sessions) implement `BridgeReconnectHandler`. The component's reconnect loop detects failure, replaces the process, connects the new channel, then calls `on_reconnect(&channel)`.

```rust,ignore
use camel_bridge::reconnect::BridgeReconnectHandler;

#[derive(Debug)]
struct XmlStylesheetCache {
    // compiled XSLT handles
}

impl BridgeReconnectHandler for XmlStylesheetCache {
    fn on_reconnect(
        &self,
        channel: &tonic::transport::Channel,
    ) -> Result<(), camel_bridge::process::BridgeError> {
        // Re-seed state from the new bridge. Spawn async work; do not block.
        Ok(())
    }
}
```

The contract has two rules. `on_reconnect` must not block synchronously. Spawn a Tokio task for async work. Returning `Err` is advisory. The reconnect loop logs the error and treats the bridge as live. Individual resource re-seeds may be retried lazily.

## Credential redaction

Password fields use a `Redacted<T>` wrapper. Its `Debug` and `Display` implementations emit `[REDACTED]`. The wrapper protects only values that stay inside it. A `BridgeProcessConfig` derived `Debug` shows `[REDACTED]` for the password field, but its `env_vars` vec is a separate `Vec<(String, String)>` used for process injection. That vec legitimately contains the raw password for the child. ADR-0051 applies. The full config must not be formatted or logged until the invariant violation tracked in bd `rc-4tbt` is resolved. A fix needs a sentinel regression test against the complete configuration, not just one `Redacted<T>` field.

## Configuration in Camel.toml

Bridge-based components declare their brokers in `Camel.toml`. The bridge pool starts one process per broker. JMS uses this shape:

```toml
[default.components.jms]
default_broker = "main"

[default.components.jms.brokers.main]
broker_url  = "tcp://localhost:61616"
broker_type = "activemq"   # "activemq" | "artemis" | "generic"
username    = "admin"      # optional
password    = "admin"      # optional
```

The bridge downloads on first use and caches at `~/.cache/rust-camel/jms-bridge/`. Set `CAMEL_JMS_BRIDGE_BINARY_PATH` to a local build for development. The pool admits at most `max_bridges` (default 8) bridges concurrently.

## Environment overrides

| Variable | Effect |
|----------|--------|
| `CAMEL_JMS_BRIDGE_BINARY_PATH` | Use a local JMS bridge binary, skip download |
| `CAMEL_XML_BRIDGE_BINARY_PATH` | Use a local XML bridge binary |
| `CAMEL_CXF_BRIDGE_BINARY_PATH` | Use a local CXF bridge binary |
| `CAMEL_JMS_BRIDGE_RELEASE_URL` | Override release download URL (must be `https://github.com/**`) |
| `CAMEL_XML_BRIDGE_RELEASE_URL` | Same override for the XML bridge |
| `CAMEL_CXF_BRIDGE_RELEASE_URL` | Same override for the CXF bridge |
| `CAMEL_BRIDGE_LOG_STDERR` | Directory path; bridge stderr is redirected per-bridge log files. Empty value uses `/tmp`. |

**Reference**: [camel-bridge crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-bridge/CONTEXT.md). See also [ADR-0007](../adr/0007-route-supervised-consumer-failure.md), [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md), and [ADR-0051](../adr/0051-credential-redaction-at-diagnostic-boundaries.md).
