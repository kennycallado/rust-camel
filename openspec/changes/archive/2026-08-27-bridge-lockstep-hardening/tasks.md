# Tasks: bridge-lockstep-hardening

<!--
  Container test harness (no JDK on host). Canonical command for any
  gradle invocation inside a bridge directory:

  docker run --rm --user=0:0 --network=host \
    --volume=<repo>/bridges/<b>:/project:z --workdir=/project \
    --env=GRADLE_USER_HOME=/project/.gradle-docker-cache \
    --tmpfs=/tmp:rw,exec \
    --entrypoint bash \
    quay.io/quarkus/ubi9-quarkus-graalvmce-builder-image:jdk-21 \
    -c 'HOST_UID=$(stat -c "%u" /project); \
        java -cp gradle/wrapper/gradle-wrapper.jar \
        org.gradle.wrapper.GradleWrapperMain <GRADLE-ARGS> --no-daemon; \
        ST=${PIPESTATUS[0]}; \
        chown -R ${HOST_UID}:${HOST_UID} /project/build \
        /project/.gradle-docker-cache 2>/dev/null; exit $ST'

  (`<repo>` = /home/kenny/dev/rust-camel/.worktrees/bridge-lockstep-hardening)
  The `gradlew` shell script is broken in-container; ALWAYS invoke the
  wrapper jar directly. Below, "CT(<b>) <args>" means run this harness with
  GRADLE-ARGS=<args> in bridge <b>.

  Rust test/build commands run in the worktree root (never the main
  checkout).
-->

## Phase 1: cxf bridge hardening

### Task 1.1: Sidecar scheme gate — only `http://` binds

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherTest.java` (modified)

**Steps:**
1. In `SoapEndpointPublisher`, extract a `static void validateAddressScheme(URI address)` method that throws `IllegalStateException` with message starting `CXF_ADDRESS scheme not supported:` naming the offending scheme and stating `TLS listener support is not yet available; use http://` when scheme != `http`.
2. Call `validateAddressScheme` at the top of the publisher's start/bind path, before `new HttpServerOptions()` construction, so no socket binds on rejection.

**Tests:** (executable spec)
- `httpsAddressFailsStartup`: setup — publisher built with address `https://0.0.0.0:9000/soap`; action — invoke its start method; assert — `IllegalStateException` thrown, message contains `scheme not supported` and `https`; command — CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest.httpsAddressFailsStartup"`; expected — fails before step 1-2 (scheme accepted), passes after.
- `httpAddressStillBinds`: setup — publisher with `http://127.0.0.1:0/soap`; action — start + stop; assert — starts without exception and announces port; command — CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest.httpAddressStillBinds"`; expected — test absent before (vacuous pass); behavior unchanged; passes after.

**Acceptance:**
- CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherTest"` exits 0.
- `git -C /home/kenny/dev/rust-camel/.worktrees/bridge-lockstep-hardening diff` shows no plaintext bind reachable with a non-`http` address.

- [x] 1.1

### Task 1.2: Rust config layer rejects non-`http` consumer addresses

**Files:**
- `crates/components/camel-cxf/src/config.rs` (modified)
- `crates/components/camel-cxf/src/config.rs` test module (modified)

**Steps:**
1. Add `pub(crate) fn validate_consumer_address(address: &str) -> Result<(), CamelError>` (error variant per existing config error type) that parses the URI scheme and rejects any scheme other than `http` (lowercased), error message containing `TLS listener support is not yet available; use http://`.
2. Call it from `CxfPoolConfig::validate` (`crates/components/camel-cxf/src/config.rs:183`) on the `bind_address` field (config.rs:129-135 — the field forwarded as `CXF_ADDRESS` at pool.rs:269-275), so rejection surfaces at route-build time.
3. Producer addresses are NOT gated in this task: `CxfEndpointConfig.address` (the `cxf://` URI target, component.rs:107-112) must remain untouched — gating it would break legitimate `cxf://` producer endpoints.

**Tests:**
- `consumer_address_https_rejected`: setup — `CxfPoolConfig` with `bind_address = "https://0.0.0.0:9000/soap"`; action — run `CxfPoolConfig::validate`; assert — `Err` containing `TLS listener support is not yet available`; command — `cargo test -p camel-component-cxf consumer_address_https_rejected`; expected — fails before steps 1-2, passes after.
- `consumer_address_http_accepted`: setup — `CxfPoolConfig` with `bind_address = "http://localhost:9000/soap"`; action — `validate`; assert — `Ok(())`; command — `cargo test -p camel-component-cxf consumer_address_http_accepted`; expected — before-state: compile fail (symbol absent); passes after.

**Acceptance:**
- `cargo test -p camel-component-cxf` exits 0 in the worktree.
- `cargo clippy -p camel-component-cxf -- -D warnings` exits 0.
- `cargo fmt --check` clean on touched files.

- [x] 1.2

### Task 1.3: Listener request-body cap (`CXF_MAX_BODY_BYTES`)

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/SoapEndpointPublisher.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/SoapEndpointPublisherBodyCapTest.java` (new)

**Steps:**
1. Add `static long maxBodyBytes()` reading env `CXF_MAX_BODY_BYTES` (default `16777216`), `NumberFormatException` → `IllegalStateException` naming the env var (fail-loud, ADR-0033).
2. Upfront gate: in the request handler, before `req.bodyHandler`, reject with HTTP 413 + short body `request body exceeds CXF_MAX_BODY_BYTES` when a present `Content-Length` header parses to > cap.
3. Mid-stream gate: replace the unbounded `bodyHandler` aggregation with a bounded accumulator — an explicit `Handler<Buffer>` appended into a `Buffer` while a running byte counter stays ≤ cap; when exceeded, invoke `req.response().setStatusCode(413).end(...)` and `req.connection().close()`, skipping the downstream handler. Pause/resume the request stream if needed for backpressure correctness.

**Tests:**
- `declaredOversizedContentLengthRejectedUpfront`: setup — published endpoint with `CXF_MAX_BODY_BYTES=1024`; action — POST with `Content-Length: 4096` header; assert — HTTP 413, response body contains `CXF_MAX_BODY_BYTES`, downstream handler NOT invoked (flag), connection not fully read; command — CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherBodyCapTest.declaredOversizedContentLengthRejectedUpfront"`; expected — fails before, passes after.
- `lyingContentLengthRejectedMidStream`: setup — cap 1024, POST declaring `Content-Length: 10` but writing 4096 bytes; assert — HTTP 413, downstream handler NOT invoked; command — CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherBodyCapTest.lyingContentLengthRejectedMidStream"`; expected — fails before, passes after.
- `underCapBodyPasses`: setup — cap 1024, valid small SOAP POST; assert — normal 200 processing path (existing behavior preserved); command — CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherBodyCapTest.underCapBodyPasses"`; expected — test absent before (vacuous pass); behavior unchanged by the fix; passes after.

**Acceptance:**
- CT(cxf) `test --tests "org.rustcamel.cxf.SoapEndpointPublisherBodyCapTest"` exits 0.
- No unbounded `bodyHandler` remains on the listener path (`grep -n "bodyHandler" SoapEndpointPublisher.java` shows only the bounded accumulator or none).

- [x] 1.3

### Task 1.4: rc-gevh — signed Timestamp + required on inbound (MODIFIED WSS requirement)

**Files:**
- `bridges/cxf/src/main/java/org/rustcamel/cxf/WssSecurityProcessor.java` (modified)
- `bridges/cxf/src/test/java/org/rustcamel/cxf/WssSecurityProcessorIntegrationTest.java` (modified)

**Steps:**
1. Outbound (`processOutbound`): when the profile declares the Timestamp action, make the signature cover Body + Timestamp explicitly: `WSSecSignature.setParts(List.of(new WSEncryptionPart("Body", WSConstants.URI_SOAP11_ENV, "Content"), new WSEncryptionPart("Timestamp", WSConstants.WSU_NS, "")))` (use the envelope-URI constant matching the test fixtures' envelope version). There is no existing parts list to append to — WSS4J defaults to signing the Body; `setParts([Timestamp])` alone would UN-sign the Body and break the preserved tamper tests.
2. Inbound (`processInbound`): in the `RequestData` setup, require the Timestamp element and its signature coverage — set `sigRequiredElements` (or `sigRequiredParts` with the same `WSEncryptionPart`) so a message whose Timestamp is absent, stripped, or outside the signature fails `WSSecurityException`.
3. Extend `enforceRequiredActions` (or its equivalent check site) to require the Timestamp action on inbound when the profile declares it.

**Tests:**
- `timestampRewriteCannotMintFreshCacheKey`: setup — processor with signing configured, actions `Timestamp Signature`; produce envelope via `processOutbound`; action A — `processInbound` original bytes (succeeds); action B — POST/process the SAME bytes with the `wsu:Timestamp` element's `Created`/`Expires` rewritten to fresh values (signature now broken); action C — re-process the ORIGINAL bytes again; assert — A succeeds; B throws `WSSecurityException` (signature validation failure — rewritten unsigned timestamp is NOT processed as fresh); C throws `WSSecurityException` (replay-cache hit — cache key unchanged); command — CT(cxf) `test --tests "org.rustcamel.cxf.WssSecurityProcessorIntegrationTest.timestampRewriteCannotMintFreshCacheKey"`; expected — fails before step 1-2 (B succeeds as fresh), passes after.
- Existing tests `replayed fresh signed message rejected` (processor-level) and endpoint-level replay (both in `WssSecurityProcessorIntegrationTest` and related classes) MUST keep passing unmodified — they are the preserved scenarios of the MODIFIED requirement.
- Fidelity note: this oracle is processor-level; the spec's rewrite scenario is endpoint-level (POSTed) — the existing endpoint replay test covers the POST path, so the combination satisfies the scenario.

**Acceptance:**
- CT(cxf) `test --tests "org.rustcamel.cxf.WssSecurityProcessorIntegrationTest"` exits 0 (new + preserved scenarios).
- `grep -n "WSEncryptionPart" WssSecurityProcessor.java` shows the Timestamp part on the outbound signature path.

- [x] 1.4

### Task 1.5: CXF 4.1.8 bump, rc-1dq7 spotless, inbound-size alignment

**Files:**
- `bridges/cxf/build.gradle.kts` (modified)
- `bridges/cxf/src/main/java/org/rustcamel/cxf/CxfBridgeService.java` (modified — formatting only)
- `bridges/cxf/src/main/resources/application.yml` (modified)

**Steps:**
1. `build.gradle.kts`: `cxfVersion = "4.1.1"` → `"4.1.8"` (the `cxf-rt-frontend-*`/`cxf-rt-ws-security` pins derive from this property — bump the property, keep the artifact list unchanged).
2. Run spotless apply on the cxf bridge (CT(cxf) `spotlessApply`) so `CxfBridgeService.java` passes `spotlessCheck` (rc-1dq7).
3. `application.yml`: add `max-inbound-message-size: 16777216` matching xml's setting (parity anchor `bridges/xml/src/main/resources/application.yml:5`).

**Tests:**
- `dependencyFloorCxf`: command — CT(cxf) `dependencyInsight --dependency org.apache.cxf:cxf-core --configuration runtimeClasspath`; assert — output resolves `4.1.8` (or higher); expected — passes after step 1.
- Spotless: command — CT(cxf) `spotlessCheck`; assert — exit 0; expected — fails before step 2, passes after.

**Acceptance:**
- CT(cxf) `build -x native-tests` or `test` full suite exits 0 (regression gate after the bump).
- CT(cxf) `spotlessCheck` exits 0.
- `grep -n "max-inbound-message-size" bridges/cxf/src/main/resources/application.yml` → `16777216`.

- [x] 1.5

## Phase 2: jms bridge hardening

### Task 2.1: Artemis TLS — `ssl://`/`wss://` activate SSL, fail-loud without material

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsClientFactory.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsClientFactoryTlsTest.java` (new)

**Steps:**
1. Add `static Map<String, Object> transportConfig(URI brokerUri)` reading the NEW broker-facing env contract directly: `BRIDGE_BROKER_KEYSTORE_PATH`, `BRIDGE_BROKER_TRUSTSTORE_PATH` (PKCS12 paths, operator-provided — distinct from the IPC mTLS PEM pair), `BRIDGE_BROKER_KEYSTORE_PASSWORD`. No resolution helper exists today; these envs are the contract (design.md thread 1).
2. Scheme mapping: `ssl` or `wss` scheme → first verify all three envs are present and point at existing, non-placeholder files (placeholder marker mirroring the IPC guard's), else `IllegalStateException` naming the scheme and the missing env; then put `TransportConstants.SSL_ENABLED_PROP_NAME=true`, `SSL_KEYSTORE_PATH_PROP_NAME`, `SSL_TRUSTSTORE_PATH_PROP_NAME`, `SSL_KEYSTORE_PASSWORD_PROP_NAME` from the envs. Schemes `tcp`/`nio`/`ws` and outer `failover:` scheme → no SSL properties (Rust allowlist admits only the `failover://` prefix, so inner-URI mapping is unreachable and deliberately dropped). BROKER-TYPE GUARD: a secure scheme with `BRIDGE_BROKER_TYPE` != `artemis` (e.g. the default `activemq` Classic path, which passes the URL to `ActiveMQConnectionFactory` outside this contract) → `IllegalStateException` naming both the scheme and the broker type — the guard must fire for every broker type, not just Artemis.
3. No locator is built on the fail-loud path.

**Tests:**
- `sslSchemeEnablesSslTransport`: setup — URI `ssl://broker:61617`, the three `BRIDGE_BROKER_*` envs set to existing temp PKCS12 files (created via the test with a self-signed store) ; action — `transportConfig(...)`; assert — map has `SSL_ENABLED_PROP_NAME=true` and both store paths; command — CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest.sslSchemeEnablesSslTransport"`; expected — compile fail before (method absent); passes after.
- `secureSchemeWithoutMaterialFailsStartup`: setup — URI `ssl://broker:61617`, `BRIDGE_BROKER_KEYSTORE_PATH` unset; action — build client; assert — `IllegalStateException` naming `ssl` and `BRIDGE_BROKER_KEYSTORE_PATH`; command — CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest.secureSchemeWithoutMaterialFailsStartup"`; expected — compile fail before (method absent); passes after.
- `sslSchemeWithWrongBrokerTypeFailsLoud`: setup — URI `ssl://broker:61617`, all three `BRIDGE_BROKER_*` envs set to existing PKCS12 files, `BRIDGE_BROKER_TYPE=activemq`; action — build client; assert — `IllegalStateException` naming `ssl` and the broker type `activemq`; command — CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest.sslSchemeWithWrongBrokerTypeFailsLoud"`; expected — compile fail before (method absent); passes after.
- `secureSchemeWithPlaceholderMaterialFailsStartup`: setup — URI `ssl://broker:61617`, `BRIDGE_BROKER_TYPE=artemis`, the three `BRIDGE_BROKER_*` envs set to paths containing the `placeholder-` marker (mirroring `PortAnnouncer.java:18`); action — build client; assert — `IllegalStateException` naming `ssl` and the placeholder material; command — CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest.secureSchemeWithPlaceholderMaterialFailsStartup"`; expected — compile fail before (method absent); passes after.
- `tcpSchemeStaysPlaintext`: setup — URI `tcp://broker:61616`; action — `transportConfig(...)`; assert — no `SSL_ENABLED_PROP_NAME` key present; command — CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest.tcpSchemeStaysPlaintext"`; expected — before-state: compile fail (method absent); passes after.

**Acceptance:**
- CT(jms) `test --tests "org.rustcamel.jms.JmsClientFactoryTlsTest"` exits 0.
- No code path discards a parsed scheme silently (the `scheme parsed but discarded` pattern is gone).

- [x] 2.1

### Task 2.2: Rust camel-jms scheme validation at config layer

**Files:**
- `crates/components/camel-jms/src/config.rs` (modified)
- `crates/components/camel-jms/src/config.rs` test module (modified)

**Steps:**
1. The existing allowlist (`config.rs:469`: `["tcp://","ssl://","failover://","ws://","wss://"]`) already rejects unknown schemes and passes secure schemes through — that behavior is RETAINED unchanged. No TLS-material requirement on the Rust side: the fail-loud material guarantee lives in the sidecar (Task 2.1, `BRIDGE_BROKER_*` contract). No new public options (proposal risk budget).
2. Add regression tests documenting the pass-through contract (extend the existing config test module; the JMS-016 enumeration test is the anchor).

**Tests:**
- `secure_scheme_passes_through_without_material`: setup — broker URL `ssl://broker:61617`, no TLS options anywhere; action — config validation; assert — `Ok(())` (sidecar will fail-loud if material is missing — not Rust's job); command — `cargo test -p camel-component-jms secure_scheme_passes_through_without_material`; expected — before-state: compile fail (test absent); passes after.
- `unknown_scheme_rejected`: setup — broker URL `stomp://broker:61613`; action — config validation; assert — `Err` from the allowlist; command — `cargo test -p camel-component-jms unknown_scheme_rejected`; expected — test absent before (vacuous pass); documents existing allowlist behavior; passes after.

**Acceptance:**
- `cargo test -p camel-component-jms` exits 0.
- `cargo clippy -p camel-component-jms -- -D warnings` exits 0; `cargo fmt --check` clean.
- `git diff` on `crates/components/camel-jms/src/config.rs` shows test additions only — no production change.

- [x] 2.2

### Task 2.3: JMS consumer body cap (`JMS_MAX_BODY_BYTES`)

**Files:**
- `bridges/jms/src/main/java/org/rustcamel/jms/JmsConsumer.java` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerBodyCapTest.java` (new)

**Steps:**
1. Add `static long maxBodyBytes()` reading env `JMS_MAX_BODY_BYTES` (default `16777216`), malformed → `IllegalStateException` naming the env var; values > `19 * 1024 * 1024` also `IllegalStateException` — a cap above 19 MiB inverts the decode-limit ordering (bodies would pass the Java cap and decode-fail at the 20 MiB Rust IPC limit, design thread 2).
2. In the `BytesMessage` branch, replace `new byte[(int) bm.getBodyLength()]` with a bounded read: `long len = bm.getBodyLength(); if (len > maxBodyBytes()) { route to bridged-error } else { byte[] buf = new byte[(int) len]; bm.readBytes(buf); }` — bridged-error path follows the existing error-forwarding shape (warn-level log naming the cap and the actual length, exchange carries error outcome; handler-contract boundary ADR-0012).

**Tests:**
- `oversizedBytesMessageRejectedWithoutFullAllocation`: setup — consumer with `JMS_MAX_BODY_BYTES=1024`, mocked `BytesMessage` whose `getBodyLength()` returns 4096 and whose `readBytes` sets a flag if called; action — consume the message; assert — exchange carries the error outcome, `readBytes` never called, a `warn` log naming `JMS_MAX_BODY_BYTES` and `4096`; command — CT(jms) `test --tests "org.rustcamel.jms.JmsConsumerBodyCapTest.oversizedBytesMessageRejectedWithoutFullAllocation"`; expected — fails before, passes after.
- `underCapBytesMessageDelivered`: setup — cap 1024, mocked `BytesMessage` length 512; action — consume; assert — body forwarded intact; command — CT(jms) `test --tests "org.rustcamel.jms.JmsConsumerBodyCapTest.underCapBytesMessageDelivered"`; expected — test absent before (vacuous pass); behavior unchanged by the fix; passes after.

**Acceptance:**
- CT(jms) `test --tests "org.rustcamel.jms.JmsConsumerBodyCapTest"` exits 0.
- `grep -n "getBodyLength" JmsConsumer.java` shows no direct unbounded `new byte[(int)` allocation.

- [x] 2.3

### Task 2.4: camel-jms tonic decode limit above the body cap

**Files:**
- `crates/components/camel-jms/src/consumer.rs` (modified — `BridgeServiceClient` construction at ~line 208, the ONLY path decoding bridge→Rust `JmsMessage` bodies)
- `crates/components/camel-jms/src/component.rs` (modified — remaining `BridgeServiceClient::new` sites at ~401, ~574, ~700 + shared helper)
- `crates/components/camel-jms/src/bridge_client_test.rs` or test module (new)

**Steps:**
1. Define `pub(crate) const BRIDGE_MAX_DECODING_MESSAGE_SIZE: usize = 20 * 1024 * 1024;` (20 MiB — headroom above the 16 MiB body cap for the protobuf envelope) and `pub(crate) fn bridge_decode_limit() -> usize { BRIDGE_MAX_DECODING_MESSAGE_SIZE }` in component.rs.
2. The critical site is `consumer.rs:208` — the subscribe client that decodes streamed `JmsMessage` bodies. Set `.max_decoding_message_size(bridge_decode_limit())` on its channel/client (tonic 0.14: `Channel::builder(...).max_decoding_message_size(n)` before connect).
3. Apply the same limit at the remaining client sites (component.rs ~401/~574/~700) via the helper for uniformity — their RPCs decode small responses today but the uniform limit prevents future traps.

**Tests:**
- `near_cap_body_decodes_end_to_end`: setup — an in-process tonic mock gRPC server implementing the bridge `Subscribe` stream that emits ONE `JmsMessage` with a ~15 MiB body (body < 16 MiB cap, encoded message > 4 MiB tonic default); test seam: reuse the channel-construction helper against the mock (the established pattern is `crates/components/camel-cxf/tests/support/mock_bridge.rs`), state in the result which seam you used; action — consume the stream; assert — the message is delivered intact (with the limit: succeeds; the SAME test with the limit removed/4 MiB would fail decode — demonstrate by asserting delivery succeeds, which only happens with the limit); command — `cargo test -p camel-component-jms near_cap_body_decodes_end_to_end`; expected — fails before (decode error at 4 MiB default), passes after.
- `bridge_decode_limit_above_cap`: assert `bridge_decode_limit() > 16 * 1024 * 1024`; command — `cargo test -p camel-component-jms bridge_decode_limit_above_cap`; expected — before-state: compile fail (helper absent); passes after.

**Acceptance:**
- `cargo test -p camel-component-jms` exits 0.
- `grep -rn "max_decoding_message_size\|bridge_decode_limit" crates/components/camel-jms/src/` shows the consumer.rs subscribe site + all component.rs sites.
- `cargo clippy -p camel-component-jms -- -D warnings` exits 0; `cargo fmt --check` clean.

- [x] 2.4

### Task 2.5: ActiveMQ 5.19.10 + Log4j bumps, jms inbound-size alignment

**Files:**
- `bridges/jms/build.gradle.kts` (modified)
- `bridges/jms/src/main/resources/application.yml` (modified)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsClientFactoryTlsTest.java` (modified — adds `bothBrokerFactoriesConstruct`)
- `bridges/jms/src/test/java/org/rustcamel/jms/JmsConsumerMessageTypesTest.java` (new)

**Steps:**
1. `build.gradle.kts`: ActiveMQ Classic client `5.18.3` → `5.19.10`; Log4j API → `2.26.1` (both in the dependencies block).
2. `application.yml`: add `max-inbound-message-size: 16777216` (parity with xml).

**Tests:**
- `dependencyFloorJms`: command — CT(jms) `dependencyInsight --dependency org.apache.activemq:activemq-client --configuration runtimeClasspath`; assert — resolves `5.19.10`; expected — passes after step 1.
- `bothBrokerFactoriesConstruct` (new test in `JmsClientFactoryTlsTest.java` or a sibling): setup — factory invocation for BOTH broker types (`BRIDGE_BROKER_TYPE=artemis` with `tcp://` URI, and the ActiveMQ Classic path with `tcp://` URI); action — construct the client factory objects; assert — both construct without error (post-bump smoke: 5.19.10 client API still compatible); command — CT(jms) `test --tests "org.rustcamel.jms.*"`; expected — passes after.
- `unsupportedMessageTypeForwardedEmptyAndAcked` (new test): setup — mocked `ObjectMessage` (no `getObject()` call — deserialization stays absent, rc-41h3 policy deferred); action — consume; assert — message acknowledged, exchange forwarded with empty body (documents current policy); command — CT(jms) `test --tests "org.rustcamel.jms.*"`; expected — passes after.
- Whole-suite regression — CT(jms) `test` (Phase 2 exit gate).

**Acceptance:**
- CT(jms) `test` exits 0 (full suite including the three new tests above — Phase 2 exit gate).
- CT(jms) `spotlessCheck` exits 0.
- `grep -n "max-inbound-message-size" bridges/jms/src/main/resources/application.yml` → `16777216`.

- [x] 2.5

## Phase 3: transversal Quarkus/Netty bump + lockstep verification

### Task 3.1: Quarkus bump on all three bridges (Netty ≥ 4.1.137)

**Files:**
- `bridges/xml/build.gradle.kts` (modified)
- `bridges/cxf/build.gradle.kts` (modified)
- `bridges/jms/build.gradle.kts` (modified)

**Steps:**
1. Determine the newest stable Quarkus release whose resolved Netty is ≥ 4.1.137 (probe with `dependencyInsight --dependency io.netty:netty-common` via CT on one bridge before committing the version; quarkus 3.20.0 is EOL — pick a supported 3.x).
2. Bump BOTH version anchors in all three `build.gradle.kts` (lockstep, identical version): the plugin `id("io.quarkus") version "3.20.0"` AND the `val quarkusVersion = "3.20.0"` (cxf:16, jms:13, xml:13) feeding `enforcedPlatform` — bumping only the plugin leaves Netty on the old platform.
3. Fix any compilation breaks from Quarkus API drift (expected small; if native reflection config additions are needed, extend the existing reflection config files — do not disable checks).

**Tests:**
- `nettyFloorAllBridges`: command — CT(xml), CT(cxf), CT(jms) each `dependencyInsight --dependency io.netty:netty-common --configuration runtimeClasspath`; assert — all three resolve ≥ `4.1.137`; expected — fails before, passes after.

**Acceptance:**
- All three CT dependency checks resolve Netty ≥ 4.1.137 with the SAME quarkus version string in the three `build.gradle.kts`.
- CT(xml/cxf/jms) `compileJava` exits 0 for each bridge.

- [x] 3.1

### Task 3.2: Three-bridge lockstep verification (phase-exit + dependency floor proof)

**Files:**
- no production files; verification-only task

**Steps:**
1. Run the full Gradle test suite for each bridge: CT(xml) `test`, CT(cxf) `test`, CT(jms) `test` — record pass counts.
2. Dependency floor proof in one output per bridge: CXF ≥ 4.1.8 (cxf), ActiveMQ ≥ 5.19.10 (jms), Netty ≥ 4.1.137 (all three).
3. Parity check: `max-inbound-message-size` present in all three `application.yml` with coupling comments — xml at 16777216; cxf at 17825792 (16 MiB listener cap + 1 MiB headroom); jms at 20971520 (19 MiB cap ceiling + 1 MiB headroom; both inter-phase review findings: the limit must exceed each bridge body cap + protobuf envelope overhead).
4. Rust side regression: `cargo test -p camel-component-cxf -p camel-component-jms` + `cargo clippy -p camel-component-cxf -p camel-component-jms -- -D warnings` in the worktree.

**Tests:**
- (Verification task — the commands above ARE the test spec; expected — all green.)

**Acceptance:**
- CT(xml/cxf/jms) `test` all exit 0 (record suite sizes in the task result).
- All dependency-floor greps/insights satisfy the floor.
- Rust tests + clippy exit 0; `cargo fmt --check` clean.
- Phase-3 exit criteria of design.md met: three bridges releasable for lockstep 0.6.0 tagging.

- [x] 3.2
