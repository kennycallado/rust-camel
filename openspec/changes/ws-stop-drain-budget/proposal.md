# Proposal: ws-stop-drain-budget

## Why

`WsConsumer::stop` waits for in-flight exchanges with a hardcoded 5 s bound
(`WS_STOP_SETTLE_TIMEOUT`, `crates/components/camel-ws/src/lib.rs`), but the
graceful-shutdown drain budget promised to operators is the configured
`drain_timeout_ms` (default 10 s in `camel-config`). An exchange that completes
in the (5 s, 10 s] window loses its echo today: the ws settle wait gives up,
teardown kills the connection, and the client never sees the response. The
CLI scenario "WebSocket consumer drains an in-flight exchange" promises the
configured budget (bd rc-evw3z, from rc-z332y; the camel-ws CONTEXT.md
lifecycle bullet names the coupling).

## What Changes

- Propagate the configured drain budget through the graceful stop chain
  (`RuntimeCommand::StopRoute` → controller actor forwarding layers →
  `stop_route_internal`) into a per-consumer stop-budget slot, following the
  existing `ConsumerContext` install-at-start convention used by the
  in-flight gauge (drainclaim).
- `WsConsumer::stop` reads the slot; on graceful shutdown the settle bound
  equals the configured `drain_timeout_ms`. The reload path writes an
  explicit `Some(DEFAULT_SHUTDOWN_TIMEOUT)` so reload behavior is unchanged;
  suspend leaves the slot unset for its consumer incarnation.
- The hardcoded 5 s constant becomes a named, documented fallback
  (`WS_STOP_SETTLE_TIMEOUT_FALLBACK`) used only when no budget was
  propagated; the doc comment no longer describes a known gap.
- Deterministic tests (tokio paused time) proving the bound follows config
  downward (500 ms) and upward (10 s, exchange completing after the old 5 s
  bound), plus a real-IO echo test with a small configured budget.

Explicitly excluded: reload/suspend bound changes, bridge/Java changes,
`drain_timeout_ms` semantics/defaults changes, new config knobs, ADR-0043
rework, `Consumer` trait signature changes.

## Acceptance criteria

- Graceful stop settle bound equals the configured `drain_timeout_ms`
  (default 10 s); exchanges completing in (5 s, 10 s] keep their echo
  (proved by paused-time tests in both directions).
- Reload and suspend stop paths keep the existing 5 s bound; their code
  paths' constants and behavior are unchanged.
- Fail-open expiry behavior (warn + proceed) and warn fields are intact.
- `cargo test -p camel-ws --lib` green; fmt + clippy (`-p camel-ws -p
  camel-core --all-targets -D warnings`) green; doc gate green.

## Risk budget

- The budget travels through several forwarding layers (runtime command →
  actor → controller → registry); every construction/match site must be
  updated — mechanical but wide. Acceptable: in-repo, compiler-enforced.
- Equal-deadline shape (settle bound == consumer-join bound) matches
  today's 5 s/5 s shape; no new race class is introduced.
- Out of bounds: changing any other component's stop behavior; touching
  `stop_timeout`'s ignored-argument promise (separate knob `timeout_ms`).

Affected crates: `camel-component-api`, `camel-api`, `camel-core`,
`camel-ws`, `camel-cli` (construction sites only).

bd: rc-evw3z
