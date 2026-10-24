# Design: 389-jms-selector

## Context

The selector is parsed (`config.rs:432`, `messageSelector` | `message_selector`, empty -> None)
into `JmsEndpointConfig.message_selector` and dropped at `consumer.rs:428-434`
(`TODO(JMS-010)`). The wire contract (`SubscribeRequest`) has only `destination` +
`subscription_id`; the Java consumer uses `session.createConsumer(dest)` (`JmsConsumer.java:320`).
The capability mechanism this design reuses was introduced by mission 383 (rc-c4fbo M1, commit
dff018c7e): `HealthResponse.capabilities` (repeated string), Java advertises via
`JmsBridgeService.healthResponse` (`.addCapabilities(CAP_BROKER_TLS_V1)`), Rust gates via a
diagnostics constant + ensure fn applied at two sites (startup gate that parks the slot before
first Subscribe; runtime gate on every observed Health response).

## Decisions

### D1 — Wire plumbing (additive proto only)

- `SubscribeRequest` gains `string message_selector = 3;`. Fields 1-2 unchanged. Both proto
  copies must remain byte-identical; `diff` check is part of the task acceptance.
- Rust: `consumer.rs` builds the request with `message_selector:
  endpoint_config.message_selector.clone().unwrap_or_default()` at the single construction site
  (`consumer.rs:546`). Existing struct literals in tests gain the new field (`..Default::default()`
  or explicit empty `String` (prost maps proto3 `string` to `String`, not `Option<String>`)).
- Producer path excluded: `SendRequest` untouched, no producer capability gate.

### D2 — Capability `message-selector-v1`, fail closed

Reuse the 383 channel; no second version channel.

- Java advertises `CAP_MESSAGE_SELECTOR_V1 = "message-selector-v1"` next to `CAP_BROKER_TLS_V1`
  in `JmsBridgeService.healthResponse`, unconditionally (a bridge that can honour selectors is
  one whose `JmsConsumer` uses the selector overload; the capability describes the bridge build).
- Rust diagnostics (mirroring `MISSING_BROKER_TLS_CAPABILITY`):
  `MISSING_MESSAGE_SELECTOR_CAPABILITY: &str = "lacks message-selector-v1; use a jms bridge >=
  0.11.0"` and `ensure_selector_capability(&capabilities, selector_configured) -> Result<(), ...>`
  that errors only when `selector_configured` and the capability list lacks the string. No
  selector configured -> gate passes trivially (request unchanged, old bridges keep working).
- Diagnostic contract (assert all three in tests): the park/error text MUST contain (1) the
  missing capability name `message-selector-v1`, (2) the required minimum bridge version
  `0.11.0`, and (3) a bridge version reference — the pinned `crate::BRIDGE_VERSION` the
  component was built for, same as the TLS gate text. Recorded decision: `HealthResponse`
  carries NO remote version field on the wire (fields: healthy, broker_connected, message,
  status, age_ms, capabilities, failure_class), so an "observed remote version" cannot be
  reported without a further proto change, which is out of scope for this mission; the pinned
  version is the only version identifier available and matches the existing TLS precedent.
- Required bridge version, recorded: **0.11.0** — the first jms bridge release advertising
  `message-selector-v1` (master bumps jms after the park; this change carries the bridge-side
  advertisement). The error text names the capability (TLS precedent: capability-first); docs
  name 0.11.0.
- Gate sites, same two as TLS, with dynamic requirement registration and a bounded fresh
  Health check at attach:
  1. **Requirement registration**: `tls_configured` is static broker config captured once;
     the selector requirement is per-endpoint and can attach late. Slot state gains
     `selector_required: Arc<AtomicBool>`, set by the consumer-attach path before any gate
     evaluation; the startup gate and runtime monitor READ the shared flag per evaluation
     (no closure capture), so late attach stays covered.
  2. **Startup**: gate runs on the startup Health evidence BEFORE the first Subscribe is sent.
     A selector-configured consumer against a bridge whose startup Health response lacks
     `message-selector-v1` parks the slot permanently with an error naming the capability,
     the pinned bridge version, and the required version. Zero Subscribe calls — tested.
  3. **Runtime**: every observed Health response re-checks (armed by the shared flag). A
     bridge replaced by an older build (capability loss) parks permanently and stops
     resubscription, matching TLS task-8.1 semantics — including for consumers that attached
     after monitor start.
  4. **Consumer-attach**: `BridgeSlot` stores no capability evidence and runtime responses
     retain none, so a selector-configured consumer attaching to an already-running slot
     issues ONE bounded fresh Health RPC (existing health client + timeout) and gates on that
     response before its first Subscribe. Missing capability, or RPC error/timeout, parks
     that consumer permanently (three-fragment error, or a "could not verify message-selector
     support" message naming capability + required version). No capability cache is added;
     fresh evidence by construction makes replacement invalidation moot at attach time.
- Health-status semantics (pre-flight trap): capability evidence is the completed Health RPC
  itself, which per ADR-0086 proves bridge liveness regardless of broker fields — capabilities
  describe the bridge build, so `STALE`/`UNKNOWN`/`SKIPPED`/`UNSPECIFIED` broker status neither
  bypasses nor strengthens the gate. A Health RPC that never completes is handled by the existing
  bounded startup-failure policy; no selector-specific path. Restart invalidation follows the
  TLS gate: capability evidence is re-collected per gate window from the current bridge instance;
  cached evidence from a previous daemon instance does not authorize a replacement bridge.

### D3 — Selector validation (no parser)

- Selector syntax is provider-specific; the broker owns it. Rust validates only shape:
  a configured value that is empty or whitespace-only after trim is rejected at URI validation
  with a clear error (fail fast, not silent). Existing behavior `message_selector=` -> `None`
  is preserved (documented: absent option and empty option both mean "no selector").
- Decode boundary (recorded decision): the selector is percent-DECODED at parse time and the
  decoded string is stored in `JmsEndpointConfig.message_selector` — the current parser stores
  the raw encoded value (`parse_message_selector` pins `"priority%20%3E%205"`), which would
  reach the broker un-decoded and filter on the wrong expression. The existing test assertion
  is deliberately updated to the decoded value; this is a behavior fix within this mission's
  scope (the option never worked end to end, so no compatibility surface exists).
- Invalid selector syntax surfaces as a broker/bridge error at subscribe time
  (`InvalidSelectorException` on the JMS side); it is reported through the existing permanent /
  retryable failure classification, not reinterpreted. Executable plumbing evidence: the Java
  consumer test asserts a thrown InvalidSelectorException reaches the existing failure path
  with no local parsing; end-to-end semantics stay with the live-broker suite.

### D4 — Durables and topic path

- Durables: no durable subscription surface exists today (rc-c4fbo M2/M3 non-goal). The
  selector+durable combination cannot be expressed, so there is nothing to reject; when M2/M3
  lands, `createDurableConsumer(topic, name, selector, noLocal)` takes the selector. Tracked by
  rc-c4fbo (covered-by-open-bd).
- Topic: a non-durable topic consumer uses the same `createConsumer(dest, selector)` overload;
  the JMS selector applies at the broker identically for topics. No rejection.
- Multicast/EIP fan-out happens in Rust after delivery; broker-side filtering is upstream of it.
  No interaction, no new config.

### D5 — Unknown-parameter policy (rc-d8svj) — adjacent touch only

`messageSelector` is already parsed by the component URI parser. If the strict unknown-parameter
walk (rc-d8svj) does not recognize `messageSelector`, this change adds ONLY that recognition (the
`metadata.rs` `#[uri_param(name = "messageSelector")]` registration is the recognition surface).
No other policy change. Task includes a verification test.

### D6 — Docs

- `crates/components/camel-jms/README.md` endpoint option table: `messageSelector` row — applied
  at the broker, provider-specific syntax, requires jms bridge >= 0.11.0
  (`message-selector-v1`), unsupported-bridge error shape (startup park naming the capability).
- `docs/src/components/jms.md` guide: short subsection, same facts + non-matching messages are
  withheld by the broker (route sees only matches).
- `bridges/jms/README.md`: capability list mentions `message-selector-v1`.
- Docs state explicitly: JVM unit tests prove plumbing only; selector semantics need a real
  broker (CI-owned container test).

## Affected crates/modules

- `crates/components/camel-jms` (proto copy, config validation, consumer, diagnostics,
  component gate wiring, metadata/docs)
- `bridges/jms` (proto copy, JmsBridgeService capability + wiring, JmsConsumer overload, tests)
- `camel-tests` (CI-owned live-broker selector test)
- `docs/src/components/jms.md`

## Test strategy

- Rust unit (in-file mocks, no broker): selector reaches SubscribeRequest; no-selector request
  field is `None`/empty and request otherwise unchanged; startup fail-closed (selector + empty
  capabilities -> permanent park, zero Subscribe); no-selector + old bridge -> subscribes;
  runtime capability loss -> permanent error; blank selector -> URI validation error.
- Java unit: selector overload used when configured; one-arg otherwise; capability advertised in
  health response; selector threaded RPC -> consumer.
- CI-owned container test (marked infra/ignored locally): queue + topic — matching message
  delivered, non-matching withheld.
