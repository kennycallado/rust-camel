# Proposal: jms-bridge-hygiene

## Why

Three small JMS-bridge defects block a clean bridges 0.6.1 (all P3, one
security-adjacent, discovered during blessed reviews of earlier changes):

1. **`failover:` URLs degrade to plaintext localhost** (`rc-2yiq`).
   `JmsClientFactory.transportConfig` (`JmsClientFactory.java:212-230`) reads
   the URI host and scheme. For `failover:(ssl://host:61616)` the outer
   scheme is `failover` and `getHost()` is null — the method silently
   substitutes `localhost:61616` with no TLS: a URL that asks for a secure
   failover cluster produces an insecure local plaintext connection. The
   current behavior is javadoc'd as intentional; the Rust config allowlist
   (`config.rs:469`) even advertises `failover://` as accepted. ADR-0033
   (fail-closed configuration) says this must abort instead.
2. **TextMessage cap counts UTF-16 code units, not bytes** (`rc-5r45`).
   The body-cap guard compares `text.length()` (UTF-16 units) against
   `JMS_MAX_BODY_BYTES` (bytes) (`JmsConsumer.java:256-258`). Up to ~16.7 M
   CJK chars (~3 B/char) pass the gate then encode to ~48 MiB UTF-8 — the
   Rust decode limit (20 MiB) rejects it with worse diagnostics, breaking
   the README's "a message accepted by the bridge is always decodable on
   the Rust side" contract.
3. **`shutdown()` can double-destroy consumers** (`rc-lupv`, filed during
   jms-message-fidelity's phase-2 review). `@PreDestroy shutdown()` stops +
   destroys every consumer and clears `activeConsumers` without touching
   per-stream `finished` flags (`JmsBridgeService.java:175-182`). A late
   stream-termination path can still win its CAS and call
   `consumerFactory.destroy(consumer)` a second time (CDI-undefined).

## What Changes

- `transportConfig` gains a fail-loud unknown-scheme branch: `tcp`/`ws`
  proceed insecure, `ssl`/`wss` proceed secure (material checks unchanged),
  anything else — including `failover` — throws `IllegalStateException` with
  remediation text. A missing or blank host also fails loud (no silent
  `localhost`). The Classic (ActiveMQ) path is untouched: it hands URLs to
  `ActiveMQConnectionFactory`, which supports `failover:` natively. The
  Rust allowlist becomes broker-type-aware: `failover://` valid for
  `activemq` broker entries, rejected for `artemis` with the migration
  hint (config + tests + doc).
- The TextMessage cap becomes UTF-8-byte-accurate with zero extra encoding:
  materialize `ByteString.copyFromUtf8(text)` once, gate on `size()`.
  README's decodability claim gains the precise caveat wording.
- Teardown destroy becomes exactly-once under all interleavings:
  `cleanupSubscription` destroys only when its owner-checked
  `remove(subId, consumer)` succeeded; `shutdown()` iterates with the same
  owner-check (`entrySet` remove(k,v) gates each destroy).

Single phase, one bridge, one goal: hygiene for bridges 0.6.1.

## Acceptance criteria

- `failover:(ssl://...)` and `failover://...` abort Artemis transport setup
  with a remediation-naming exception; `tcp/ws/ssl/wss` behave exactly as
  before (TLS material checks unchanged); Rust config rejects `failover://`
  URLs for `artemis` entries at `validate()` with a clear error — `activemq`
  (Classic) entries remain accepted.
- A TextMessage whose UTF-8 encoding exceeds the cap is rejected with the
  byte-accurate diagnostic; an ASCII text at exactly the cap passes; README
  documents the byte-accurate semantics.
- No interleaving of `shutdown()` × `cleanupSubscription()` destroys any
  consumer twice (owner-checked destroy both sides).
- Java suites + `cargo test -p camel-component-jms --lib` green; full gate
  battery green in the worktree.

## Risk budget

- All fixes fail-loud or tighten accounting; no new happy-path behavior.
- Rejecting `failover://` for `artemis` entries is a breaking config change
  for Artemis users who wrote it — but every such user is on the
  plaintext-localhost bug today; 0.6.1 release notes must carry the
  migration line. Classic (`activemq`) entries keep `failover://` support.
- Owner-checked destroy could, if wrong, leak a consumer (never destroy).
  Shutdown drains in a loop with an admission flag closing the
  subscribe-races-shutdown window; tests pin both directions
  (destroy-once, no leak).
- README states the TextMessage cap bounds the FORWARDED body (UTF-8
  bytes), not peak sidecar allocation (the string is materialized first).
