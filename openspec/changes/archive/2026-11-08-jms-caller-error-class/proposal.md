# Proposal: jms-caller-error-class

## Why

The producer wraps **every** bridge send failure in the transport prefix
(`crates/components/camel-jms/src/producer.rs:201-205`:

```rust
CamelError::ProcessorError(format!("{BRIDGE_TRANSPORT_ERROR_PREFIX}send error: {s}"))
```

), and `is_bridge_transport_error` (`component.rs:2266-2275`) classifies any
`ProcessorError` starting with `BRIDGE_TRANSPORT_ERROR_PREFIX` (`"JMS gRPC "`)
as transport. Its caller (`component.rs:2151-2160`) then emits
`warn!("JMS send transport error; refreshing channel")` and runs
`refresh_slot_channel()` — a process lock plus channel rebuild. Since
rc-ooczk, an illegal forwarded property is rejected by the bridge with gRPC
`INVALID_ARGUMENT` — a caller fault — yet it still triggers the transport warn
and a channel refresh: misleading ops signal and avoidable churn on a user
mistake (bd rc-qpkok, P3).

## What Changes

`producer.rs` classifies the send failure by gRPC status **before** wrapping,
via a single mapping helper. No consumer change, no contract change.

### Status→class mapping (established from the bridge source)

`JmsBridgeService.send()` (`bridges/jms/src/main/java/org/rustcamel/jms/JmsBridgeService.java:79-107`)
returns exactly two statuses:

| gRPC status | Bridge source | Class | New wrapping |
| --- | --- | --- | --- |
| `INVALID_ARGUMENT` | `catch (InvalidPropertyException e)` → `Status.INVALID_ARGUMENT.withDescription(e.getMessage())` (lines 90-99); `InvalidPropertyException` message is `"Invalid or rejected JMS property names: " + propertyNames` — names only, never values | **Caller fault** | `CamelError::ProcessorError("JMS send rejected by the bridge: …")` — no transport prefix, property names kept, values absent |
| `INTERNAL` | `catch (Exception e)` → `Status.INTERNAL.withDescription(e.getMessage())` (lines 100-106); broker/JMS trouble | **Transport-class fault** | unchanged, byte-identical `{BRIDGE_TRANSPORT_ERROR_PREFIX}send error: {s}` |
| transport-level failures surfaced by tonic itself (`UNAVAILABLE`, `DEADLINE_EXCEEDED`, `UNKNOWN`, `CANCELLED`, connection reset) | client-side gRPC transport | **Transport-class fault** | unchanged, byte-identical |

`INVALID_ARGUMENT` is the only caller-validation status the bridge uses on
`send()` — `FAILED_PRECONDITION` and `ALREADY_EXISTS` are subscribe-path only
(lines 121, 175), `UNAVAILABLE` is the shutdown race on subscribe registration
(line 166). Nothing else in `send()` is caller-attributable, so the caller set
is exactly `{INVALID_ARGUMENT}`.

### Bytes-identical transport path claim

The transport branch of the mapping emits the identical format string
`{BRIDGE_TRANSPORT_ERROR_PREFIX}send error: {s}` for every non-`INVALID_ARGUMENT`
status. The consumer's subscribe wrapping (`consumer.rs:649,905`) is **not
touched**: the producer helper is send-specific ("send rejected" wording), so
reusing it on subscribe would be a semantic change, not a zero-change reuse.

### Affected crates

- `camel-component-jms` (`producer.rs` + its test module) — only crate touched.

## Acceptance criteria

- `caller_error_is_not_transport_classified`: the producer error for an
  `INVALID_ARGUMENT` send is not `is_bridge_transport_error`, uses the
  rejection wording, and names the offending property (no value).
- `transport_error_keeps_prefix_and_classification`: simulated transport
  failures (`UNAVAILABLE`, `INTERNAL`, `DEADLINE_EXCEEDED`, `UNKNOWN`) keep
  the prefix and classify as transport.
- `cargo test -p camel-component-jms` passes; fmt, clippy `-D warnings`,
  `cargo xtask lint-unwrap`, `cargo xtask lint-log-levels` pass.
- Integration leg (broker + bridge harness, Docker) not run in this mission:
  unit-level proof is `!is_bridge_transport_error(err)` — the exact predicate
  `component.rs:2151` checks before warn + `refresh_slot_channel()` — so the
  caller fault cannot reach the transport reaction. The rc-ooczk harness
  (`crates/camel-test/tests/support/jms_invalid_property_cases.rs`) pins the
  bridge half in CI.

## Risk budget

Trivial mapping + tests change (mission-pre-blessed TRIVIAL). Out of bounds:
retry-policy change, new gRPC fields, bridge rebuild, consumer semantic
change, silencing the transport warn for real transport failures.

## References

- bd: rc-qpkok (discovered-from rc-ooczk review)
- CONTEXT-MAP.md: JMS bridge error-classification language unchanged; this
  change aligns producer error classification with it.
