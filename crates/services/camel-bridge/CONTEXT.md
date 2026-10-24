# camel-bridge

Cross-language bridge service that spawns and manages subprocesses (JMS, XML, CXF bridges).
Communication uses mutual TLS (mTLS) with ephemeral rcgen-generated certificates.

## Language

**BridgeSpec**:
Static descriptor for one bridge binary. It defines release, cache, binary-override,
and stderr-log metadata.
_Avoid_: bridge configuration, process configuration

**BridgeProcessConfig**:
Inputs for one subprocess start. Its `env_vars` field is the final environment sent
to the child and can contain raw credential values.
_Avoid_: safe diagnostic view, redacted configuration

**BridgeProcess**:
Owner of one child process, its mTLS material, announced gRPC port, cancellation
token, and stdout-drain task.
_Avoid_: bridge consumer, bridge supervisor

**Redacted<T>**:
Credential wrapper whose `Debug` implementation emits `[REDACTED]`. It protects only
values that remain inside the wrapper.
_Avoid_: encrypted value, zeroized value

**BridgeReconnectHandler**:
Callback that restores component-owned state after a replacement bridge starts and
connects. It does not own restart detection or process replacement.
_Avoid_: reconnect loop, bridge supervisor

## Architecture boundary

This crate provides bridge primitives: binary acquisition, subprocess lifecycle,
mTLS channel creation, health waits, and the reconnect callback contract. Component
crates own reconnect orchestration. They detect failure, replace the process, connect
the new channel, and call `BridgeReconnectHandler::on_reconnect` when they must restore
stateful resources.

## Child process lifetime

`BridgeProcess` supervises its child through a supervisor-first POSIX-shell
watchdog over a parent-liveness pipe on Linux and macOS. The watchdog is armed
before the bridge is spawned, and the bridge joins the watchdog's process group
at exec. When the parent dies for any reason — normal exit, panic, `SIGTERM`,
`SIGINT`, or `SIGKILL` — the pipe write end closes, the watchdog sees EOF, and it
`SIGKILL`s its own group. The bridge, launcher descendants, and the watchdog all
die together. The mechanism uses only POSIX shell builtins, `setpgid`, `trap`,
and `waitid`, with no `/proc` dependency. This guarantee is the fix for bd
`rc-r8od1`.

The watchdog is SIGTERM-immune (`trap "" TERM`), so a graceful stop cannot
disarm the parent-death backstop. Teardown signals process groups only, never
bare PIDs. The awaited `stop()` performs the graceful bounded sequence:
`SIGTERM` to the group, a bounded non-reaping wait, unconditional group
`SIGKILL`, invalidation of the stored group id, then reaping of both the bridge
and the watchdog. A successful `stop()` return implies both children are reaped.
`Drop` is a synchronous fallback, not a graceful shutdown: it immediately
group-`SIGKILL`s and closes the liveness pipe without waiting, so it neither
reaps the children nor gives them a graceful teardown window. Do not rely on
`Drop` for graceful shutdown or reaping.

Stderr is owned by default: it is piped and drained with the bounded pattern
(64 KiB line cap, rate-limited forwarding, overflow dropped with a summary), and
the drain starts immediately after spawn so stderr can never stall readiness or
pin the parent's stderr. `CAMEL_BRIDGE_LOG_STDERR` keeps its existing
file-redirect behavior and failure semantics. The drains run on a
process-lifetime dedicated tokio runtime (`DRAIN_RUNTIME`), NOT on the caller's
runtime — do not "fix" this back to caller-scoped spawning: dropping the
caller's runtime would close the pipe readers and `SIGPIPE` a live bridge.

The guarantee holds by design on Linux and macOS and is verified on Linux; the
macOS evidence suite has not been run.

Windows has no process groups and no POSIX shell. Supervision there is
explicit-path teardown only: `stop()` and `Drop` kill the child directly, with no
group watchdog. An unclean parent death can leave the bridge and its descendants
running; that is a known leak, not a supervised guarantee.

## Credential redaction posture

ADR-0051 applies to every diagnostic representation of bridge configuration.
`BridgeProcessConfig` now uses a manual `Debug` implementation that redacts
`broker_url` and `env_vars` values. The `Redacted<T>` password field remains
protected via its own `Debug` impl. A sentinel regression test verifies that
credential values do not appear in `Debug` output.

## ADR-0012 log-policy annotations

Bridge subprocess output and startup are outside a Route handler contract. The
`outside-contract` rows use `warn!` or `debug!` and need no replacement signal.
Startup contract failures use `error!` as bootstrap failures.

| Site | Annotation value | Level | Reason |
|------|------------------|-------|--------|
| `drain_stdout`: oversized line | `outside-contract` | `warn!` | The bounded drain truncates a child stdout line. |
| `drain_stdout`: child line | `outside-contract` | `debug!` | The bounded drain records child output. |
| `drain_stdout`: drop summary | `outside-contract` | `debug!` | The rate limiter summarizes dropped child lines. |
| `BridgeProcess::start`: malformed ready message | `system-broken` | `error!` | The child violated its startup protocol. |
| `BridgeProcess::start`: stdout closed before ready | `system-broken` | `error!` | The child violated its startup protocol. |
| `BridgeProcess::start`: startup timeout | `system-broken` | `error!` | The bridge failed during bootstrap. |

## Authority

- ADR-0007: consumers and Routes own supervision; this support crate does not
- ADR-0012: handler-contract log ownership and annotation values
- ADR-0051: credential redaction at diagnostic boundaries
