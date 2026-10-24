# Proposal: 389-jms-selector

## Why

`messageSelector` / `message_selector` is parsed into `JmsEndpointConfig` and then dropped at
`crates/components/camel-jms/src/consumer.rs` (`TODO(JMS-010)`): it is cloned and discarded, the
gRPC `SubscribeRequest` has no selector field, and the Java consumer calls the one-argument
`session.createConsumer(dest)`. A route declaring a selector therefore receives EVERY message on
the destination. Silent over-delivery is worse than a hard failure: the route looks correct and
mis-processes traffic. Found during the rc-c4fbo investigation (e_opus memo), classified P1.
Bd: rc-d1cz9.

## What Changes

- Additive proto change: `string message_selector = 3;` on `SubscribeRequest`, both proto copies
  byte-identical (`bridges/jms/src/main/proto/jms_bridge.proto`,
  `crates/components/camel-jms/proto/jms_bridge.proto`). `SendRequest` untouched — producer path
  excluded.
- Rust: consumer passes the configured selector in the subscribe request; blank-after-trim
  selector values are rejected at URI validation; no selector-expression parsing (broker-owned).
- Fail closed via the existing health-capability channel (pattern of rc-c4fbo M1, commit
  dff018c7e): new capability `message-selector-v1`; when a selector is configured and the bridge
  does not advertise the capability, the slot fails at startup before any Subscribe, and a
  runtime capability loss parks permanently. Error names the capability and the required bridge
  version (0.11.0). An old bridge would otherwise ignore the unknown field and silently deliver
  everything.
- Java: `JmsConsumer` creates the consumer with the selector overload when a selector is present;
  `JmsBridgeService` advertises the capability and threads the selector from the RPC to the
  consumer.
- Docs: endpoint option table + JMS guide state broker-side filtering, required bridge version,
  and the unsupported-capability error shape.
- Validation/scope decisions recorded in design.md (durables, topic path, strict-walk policy).

## Acceptance criteria

- Rust test proves the selector reaches `SubscribeRequest`; the no-selector request is unchanged.
- Java test proves the selector overload is used when configured; one-arg call otherwise.
- Fail-closed: selector configured + bridge without `message-selector-v1` -> startup error, zero
  Subscribe calls, no silent start.
- Live-broker semantics test (matching delivered, non-matching withheld) added to the CI-owned
  container suite, marked accordingly; JVM unit tests documented as plumbing-only.
- Full gate registry per AGENTS.md; `cargo test -p camel-component-jms`; camel-bridge tests; Java
  bridge module tests.

## Risk budget

- Acceptable: proto struct-literal churn in Rust tests (additive field forces updates).
- Out of bounds: SQL-92 selector parser, durable subscriptions (rc-c4fbo M2/M3), version bump,
  producer-path changes, a second capability/version channel.
