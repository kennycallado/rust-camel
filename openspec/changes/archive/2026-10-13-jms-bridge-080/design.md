# Design: jms-bridge-080

## Approach

**Settlement.** `JmsConsumer` opens `CLIENT_ACKNOWLEDGE` and holds one unacknowledged delivery per session, carrying a reservation token and generation. Rust submits with `send_and_wait`; `Ok` sends `COMMIT`, and `Err` (including `ChannelClosed`) sends `ROLLBACK` or drops the stream, never `COMMIT`. Settlement linearizes at its terminal decision: cancel before prevents ack, after and a lost commit stand with no retry or speculative ack. A conversion failure recovers before the next receive.

**Lazy ownership.** Journal ownership is per destination, acquired at first subscribe and released when the last subscription ends (cancel, disconnect, stop, shutdown). Producers and topics never touch it. Readiness validates only the directory; a busy lock waits `JMS_ATTEMPT_JOURNAL_LOCK_WAIT_MS` then returns `FAILED_PRECONDITION` naming `JMS_ATTEMPT_JOURNAL_DIR`.

**Scope.** The journal domain is (directory, broker key, destination key) with one live owner; sequential owners inherit counts through lock handoff; N is the number of distinct journal domains consuming the queue within one retention window. Rust derives the broker key and owns the default directory (`$XDG_STATE_HOME` else `$HOME/.local/state`), passing both via env on launch and restart; unset env defers to a `FAILED_PRECONDITION` at queue subscribe. Topics are non-durable.

**Journal.** One process-wide service with framed records, torn-tail replay keeping counts, and a fsynced atomic snapshot with parent-dir fsync. Compaction runs at open and before any ceiling decision, retiring entries untouched past `JMS_ATTEMPT_JOURNAL_RETENTION_MS`; out-of-range config is a startup permanent failure, and the cap is not absolute forever.

**Park.** A startup directory failure signals `permanent-failure` with a reserved exit code, mapped to `Parked`, never `Restarting`; `Parked` is terminal for every waiter. Later permanent failures fail only the affected route with `FAILED_PRECONDITION`, slot `Ready`. Any task exit other than a requested cancellation cancels siblings and returns `Err` to supervision.

**Poison.** Six reservations; the seventh receipt is reject-then-log with the message ID and count only. No dead-letter forwarding. The cap is terminal only before a provider or broker native limit, which the docs name as precedence; redelivery is not promised to the source queue.

**Headers.** Separator-normalized deny-before-allowlist covers reserved JMS names, `jms` and `camel` prefixes, and credential substrings reusing `BROKER_SENSITIVE_KEY_SUBSTRINGS` plus `apikey`, `authorization`, `bearer`, `principal`.

**Native.** Register `ActiveMQConnectionFactory` public methods for invocation in a namespaced native-image resource. Quarkus generates the flat `META-INF/native-image/reflect-config.json` and shadows the source resource at that path. Move only the factory entry to `META-INF/native-image/org.rustcamel/jms-bridge/reflect-config.json`. Do not activate unrelated legacy entries. Verify the namespaced resource in the generated runner JAR before the live proof.

`JmsClientFactory` keeps an explicit `jms.connectionIDPrefix` base plus a per-process token. Pinned ActiveMQ 5.19.10 has no connection-prefix getter. Read that explicit option with `URISupport.parseParameters` after the unchanged factory constructor. This dispatcher handles plain and composite URIs. Use the available `getClientIDPrefix()` for the client prefix. Preserve non-empty bases, including whitespace.

**Native observer.** `ConnectionView.getClientId()` delegates to the broker connection identity accessor. For brokers 5.18.3 and 5.19.10, that accessor prefers `ConnectionInfo.clientId` when set. This attribute does not prove `connectionIDPrefix` application. Keep the existing `client_ids()` helper and its uniqueness tests unchanged.

Use a dedicated consumer-free queue for the native sentinel proof. Start two native processes, require connected health, and send one message per process. Browse both stored messages through the broker Queue MBean. ActiveMQ builds each `JMSMessageID` from its producer identity, which contains the transport connection identifier. Recover that connection identifier from the broker-returned message ID, not from the send response or factory configuration. Both identifiers must contain the connection-prefix sentinel and different process UUID tokens. The test must not set a message ID or add a client-prefix sentinel as a substitute.

## Affected files

- Rust: `crates/components/camel-jms/src/{consumer,producer,headers,config,component,lib}.rs`, `crates/services/camel-bridge/src/process.rs`, `proto`.
- Java: `bridges/jms/.../{JmsConsumer,JmsProducer,JmsBridgeService,JmsClientFactory,BridgeConfig}.java`, new `JournalService.java`, `HeaderPolicy.java`, `reflect-config.json`, `build.gradle.kts`.
- Tests: `tests/jms_test.rs`, `tests/support/jms_bridge_procs.rs`, a crash-probe binary, Java tests.
- Docs: `docs/src/components/jms.md`, `crates/components/camel-jms/CONTEXT.md`, `bridges/jms/README.md`, `docs/adr/0067-jms-message-type-forwarding-policy.md`.

## Architecture boundaries

The change stays in the Components context; submission uses `ConsumerContext`, so the data-plane boundary is unchanged. Outcome handling follows ADR-0024; header filtering ADR-0032/ADR-0051; fail-closed validation ADR-0033.

## Phases

Phases are park boundaries. Phase 3 is not release-ready: at that park permanent failures still surface as transient reconnects, and phase 4 makes them route-only.

| # | Phase | Goal | Deps | Public surface | Deliverable | Machine exit |
|---|---|---|---|---|---|---|
| 1 | Header filtering | separator-normalized deny-before-allowlist, empty default | none | `jmsHeaderAllowlist` | `headers.rs`, `producer.rs`, `config.rs`, Java `HeaderPolicy.java`, tests | empty default drops user headers; `api-key`/`API_KEY` and `camel`/`xcamel`/`jms` denied when listed; business names pass; parity holds |
| 2 | JournalService standalone | durable journal engine, unwired; ACK stays `AUTO_ACKNOWLEDGE` | none | `JournalService` API and env names | `JournalService.java`, unit tests | tests prove framing, torn-tail replay, never-replaced lock, lazy acquire/release, bounded busy wait, atomic snapshot plus dir fsync, retention retirement, no live eviction |
| 3 | Settlement end-to-end and wiring | switch ack coherently (Java and Rust) | 1, 2 | `Complete` RPC, `delivery_token`, `subscription_generation`, `JMS_ATTEMPT_JOURNAL_DIR`, `JMS_BRIDGE_BROKER_KEY`, cap env | proto, Java owner thread and journal wiring, Rust `send_and_wait`, `component.rs`/`config.rs`/`process.rs`, `support/jms_bridge_procs.rs` temp dir plus key | mock tests prove COMMIT once, no COMMIT on `Err` including `ChannelClosed`, no completion on cancel, recover-before-receive, acquire/release, six-reservation cap; coexist regression green |
| 4 | Permanent-failure propagation | park startup failure; route-only otherwise; surface tasks | 3 | `BridgeState::Parked`, `FAILED_PRECONDITION`, `permanent-failure` line | `component.rs`, `consumer.rs`, `camel-bridge/process.rs`, `producer.rs`, Java signalling | startup failure parks and never restarts; destination failure is route-only with slot `Ready`; any task exit cancels siblings and returns Err; a `Parked` waiter returns Err bounded; no restart hot-loop |
| 5 | Broker crash proof | prove crash recovery and record disposition | 4 | none | `tests/jms_test.rs`, `support/jms_bridge_procs.rs`, crash probe | Classic and Artemis tests pass; survivor acquires the journal and observes the count; message unchanged, no COMMIT; disposition recorded, mismatch reported; orphan reaped |
| 6 | Native, 0.8.0, docs | apply `jms.*` options, prefix precedence, version, docs | 1 to 5 | `BRIDGE_VERSION="0.8.0"` | namespaced factory reflection resource, `JmsClientFactory.java`, `lib.rs`, `build.gradle.kts`, docs | runner JAR retains the invocation registration; native builds; broker-browsed message identities prove two distinct sentinel-bearing connection IDs and process tokens; strict validate and lint-ignore pass |

## Alternatives considered

- A lifetime lock before readiness: rejected; it breaks concurrent, producer-only, and orphaned bridges.
- Dead-letter forwarding: rejected; its non-atomic send/ack window risks loss.
