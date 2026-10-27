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

`BridgeProcess::start` spawns the binary and passes `QUARKUS_HTTP_SSL_PORT=0`: the Quarkus JVM binds an OS-assigned port itself and announces the ACTUAL bound port (read from the Quarkus runtime registry) on stdout as one line of JSON: `{"status":"ready","port":N}`. The crate reads that line, then connects a mTLS tonic channel to `https://127.0.0.1:{port}`. Port selection is JVM-authoritative — the port is held by the JVM from the first moment it exists, so the bind-read-drop race class (ADR-0070, bd rc-s7dyw) cannot occur.

The TLS material is generated before spawn. The child receives the cert paths through four env vars. Build-time TLS properties live in each bridge's `application.yml`. Runtime cert paths and the SSL port are the only runtime surface.

| Env var | Purpose |
|---------|---------|
| `QUARKUS_HTTP_SSL_PORT` | SSL port mode: `0` (JVM binds OS-assigned, announces actual) or fixed port |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_CERT` | Server cert PEM path |
| `QUARKUS_TLS_BRIDGE_KEY_STORE_PEM_0_KEY` | Server key PEM path |
| `QUARKUS_TLS_BRIDGE_TRUST_STORE_PEM_CERTS` | CA cert PEM path (for client auth) |

The connect step retries the TLS handshake up to ten times. Quarkus `PortAnnouncer` fires on `StartupEvent` and can precede full SSL listener readiness by a few hundred milliseconds in native images. The first attempt often fails. The retry loop absorbs that.

## Containers and the TLS directory

Ephemeral certificate files need a writable base directory. The crate resolves it in order:

1. `$TMPDIR`, when set, non-empty, and usable.
2. The system temporary directory, when usable.
3. `{cache_dir}/{crate version}/tls`, created on demand.

Each candidate is probed by performing the real temporary-directory creation, so there is no separate check-then-use window. Directories this crate creates get mode `0700`; pre-existing components keep their mode. If every candidate fails, the error names `XDG_CACHE_HOME` and `$TMPDIR` as remedies.

With the watchdog hook installed, bridge routes start in shell-less images such as `production` (`scratch`) and `gnu` (distroless), and do not need a usable `/tmp`. The operator must provide one writable location for TLS material: a writable cache directory or a writable `$TMPDIR`. A read-only root filesystem with a non-root user works when a writable volume is mounted and `XDG_CACHE_HOME` points at it. The CI container guard exercises the default and the read-only variants.

## Process lifecycle

`BridgeProcess::start_and_connect` performs the full bootstrap in one call. It validates the config, generates the TLS material, spawns the child, reads the ready line, then connects the channel. A bounded stdout drain runs in the background for the rest of the process lifetime.

`BridgeProcess::stop` cancels the drain task, sends `SIGTERM`, waits five seconds, then sends `SIGKILL`. The `Drop` impl cancels the drain and best-effort kills the child. Drop cannot wait. Long shutdowns need an explicit `stop` call.

Stdout drain is bounded. Single lines cap at 64 KiB. Logging rate-limits to 100 lines per second with a drop summary. The drain never stops reading. An undrained OS pipe fills and blocks the child. Bounded size prevents memory growth. Rate-limited logging prevents log flooding. Both are required for stable long-running bridges.

Child stderr is owned by the parent by default: it is piped and drained with the same bounded, rate-limited pattern, and the drain starts before readiness parsing. The child never inherits the parent's stderr. The [`CAMEL_BRIDGE_LOG_STDERR`](#environment-overrides) override keeps redirecting stderr to per-bridge files with unchanged semantics.

## Child process lifetime

On Linux and macOS the bridge child cannot outlive the parent binary. Normal exit, panic, `SIGTERM`, `SIGINT`, and `SIGKILL` all kill the whole bridge process group: the bridge, any launcher descendants it created, and the watchdog. The watchdog supervises a process group over a parent-liveness pipe. The parent creates the pipe and arms the watchdog before the bridge exists, and the bridge joins the watchdog's process group at exec. Any parent death closes the pipe write end, the watchdog sees EOF, and it `SIGKILL`s its own group. The watchdog ignores `SIGTERM` so a graceful teardown cannot disarm it.

With the embedder hook installed, the watchdog is the host binary re-executed in a hidden role: `argv[1]` is `__camel-bridge-watchdog`, `argv[2]` is ack fd 9, and the role writes one `0x01` acknowledgement byte. It refuses to arm unless it leads its own process group (exit 74), ignores `SIGTERM`, and `SIGKILL`s group 0 after stdin EOF. Linux execs literal `/proc/self/exe`; other Unix targets use the path captured at startup by `std::env::current_exe()`. POSIX `sh` remains only as the no-hook fallback, and no hook plus no shell fails closed naming both remedies. The guarantee holds by design on both platforms and is verified on Linux; the macOS evidence suite has not been run.

The hook contract is strict. `camel_bridge::watchdog::maybe_run()` must be the first statement of a synchronous `main`, before any code consumes argv. In `camel-cli` it runs before `self_detect_artifact` and is unconditional in every flavor, slim included, because the xsd bridge (`camel-component-validator`) is an unconditional dependency. A compiled artifact would otherwise eat the watchdog argv.

```rust,ignore
fn main() {
    camel_bridge::watchdog::maybe_run();
    // build the async runtime and run the application
}
```

Because the bridge runs outside the terminal's foreground process group, terminal `SIGINT`/`SIGHUP` reach the parent, not the bridge — the parent's cleanup handles them. Bridge `stdin` and `stdout` must stay piped (never a tty): a background process that reads or writes the terminal gets `SIGTTIN`/`SIGTTOU`.

Teardown is group-scoped. The awaited `stop()` performs the graceful bounded sequence: `SIGTERM` to the group, a bounded non-reaping wait, unconditional group `SIGKILL`, invalidation of the stored group id, then reaping of the bridge and watchdog. A successful `stop()` return implies both children are reaped. `Drop` is a synchronous fallback, not a graceful shutdown: it immediately `SIGKILL`s the group and closes the parent-liveness pipe without waiting, so it neither gives the children a graceful teardown window nor guarantees they are reaped. Do not rely on `Drop` for graceful shutdown or reaping. Group signals are never sent after invalidation. This guarantee is the fix for the leaked-process evidence in bd `rc-r8od1`.

Windows has no process groups and no POSIX shell, so supervision there is explicit-path teardown only. `stop()` and `Drop` kill the child directly; there is no group watchdog. If the parent dies uncleanly, the bridge and any descendants it spawned can survive. Treat that as a known leak rather than a supervised guarantee.

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

## XML bridge: supported encodings in native

The native XML bridge image registers all standard JDK charsets. The opt-in is the build-time property `quarkus.native.add-all-charsets`, declared as `export QUARKUS_NATIVE_ADD_ALL_CHARSETS="true"` in `bridges/xml/native-build-args.env`. That file is the single source of truth; Quarkus then emits `-H:+AddAllCharsets` itself. A JVM bridge runs on the full JDK runtime, which carries every standard JDK charset regardless; the registry opt-in matters only for the native image. This guarantee is native-specific.

The probe observed:

- XML declarations naming Xerces-mapped IANA encodings parse correctly, in the probed forms: `ISO-8859-15`, plus the aliases `LATIN-9` and `ISO_8859-15` (both alias spellings are exercised by the `charset_alias_latin9_decl_resolves` probe).
- `xsl:output @encoding` serializes correctly for the probed charset: the ISO-8859-15 round trip with the euro sign is byte-exact (single byte `0xA4`, never the UTF-8 sequence or an entity). Because the opt-in registers every standard JDK charset with the JDK's own alias resolution, output encodings beyond the probed set are expected to work by the same mechanism — that is a property of the opt-in, not a separately measured result.
- An encoding name outside the JDK registry surfaces as a `TRANSFORM_ERROR:` bridge error on both the input and output side.

The opt-in also ships CJK and EBCDIC decoders. Measured image growth is +5.6 MiB (95028304 → 100861008 bytes). Startup is unchanged (median 60 ms → 60 ms). Do not read this section as support for arbitrary or unregistered charset names.

**Reference**: [camel-bridge crate](https://github.com/kennycallado/rust-camel/blob/main/crates/services/camel-bridge/CONTEXT.md). See also [ADR-0007](../adr/0007-route-supervised-consumer-failure.md), [ADR-0012](../adr/0012-log-level-convention-handler-contract-boundaries.md), and [ADR-0051](../adr/0051-credential-redaction-at-diagnostic-boundaries.md).
