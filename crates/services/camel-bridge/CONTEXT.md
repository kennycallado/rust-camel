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
