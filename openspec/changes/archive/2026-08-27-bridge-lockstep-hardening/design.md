# Design: bridge-lockstep-hardening

## Approach

Five hardening threads converge on one pre-release window. The unifying
principle is the project's established fail-closed posture (ADR-0033,
ADR-0036): a security-relevant configuration error aborts startup loudly —
it is never silently downgraded.

1. **jms Artemis TLS** (`JmsClientFactory`): map the URI scheme honestly.
   `ssl://`/`wss://` set
   `transportConfig SSL_ENABLED_PROP_NAME=true` with broker-facing material from
   a NEW sidecar env contract — `BRIDGE_BROKER_KEYSTORE_PATH`,
   `BRIDGE_BROKER_TRUSTSTORE_PATH` (PKCS12, operator-provided, distinct from
   the IPC mTLS PEM pair), `BRIDGE_BROKER_KEYSTORE_PASSWORD`; schemes with no
   TLS semantics (`tcp://`, `nio://`) stay plaintext; `failover:` outer
   scheme gets no SSL props (Rust allowlist admits only the `failover://`
   prefix — inner-URI mapping is unreachable and dropped). A secure scheme
   with any of the three envs missing or pointing at a placeholder path, or
   with `BRIDGE_BROKER_TYPE` other than `artemis` (the Classic path does
   not implement this contract) → `IllegalStateException` at startup
   (placeholder fail-closed guard pattern, ADR-0036). Hostname verification stays ON (Artemis default) —
   no opt-out in this window.
2. **jms body limit** (`JmsConsumer`): configurable cap
   (`JMS_MAX_BODY_BYTES`, default 16 MiB — parity with xml's
   `max-inbound-message-size`). Reads bounded by
   `min(bodyLength, cap)`; overflow → JMS exception path (bridged error,
   warn-logged per handler-contract boundary, ADR-0012), never unbounded
   allocation. Configurable caps follow the per-format config-channel
   pattern of ADR-0038. Consumed bodies are bounded by the Rust client's
   tonic decode limit (4 MiB default, not the sidecar's request-side
   `max-inbound-message-size`); the `camel-jms` bridge client SHALL set its
   decode limit above the cap (envelope headroom) so a near-cap body is
   deliverable end-to-end; Phase 2's near-cap test exercises the real IPC
   boundary.
3. **cxf listener** (`SoapEndpointPublisher`): (a) scheme gate — only
   `http://` binds; `https://` (or any other scheme) → startup abort with
   actionable message (TLS listener support is deferred, tracked); Rust side
   (`camel-cxf` config) validates the scheme before spawn so the failure
   surfaces at route-build time. (b) request-body cap: reject
   `Content-Length > cap` upfront + running-counter mid-stream rejection
   when a liar header streams past the cap (`CXF_MAX_BODY_BYTES`, default
   16 MiB).
4. **rc-gevh** (`WssSecurityProcessor`): the emitted `wsu:Timestamp` becomes
   a signed part (`WSEncryptionPart` for Timestamp in the signature action)
   and `enforceRequiredActions` requires a Timestamp on inbound signed
   messages. Oracle test: captured valid request → timestamp rewritten
   (fresh Created/expires) → replay MUST be rejected as tampered, proving
   rewrite cannot mint fresh cache keys. rc-1dq7: run spotless apply on
   `CxfBridgeService.java` (formatting only).
5. **Dependency bumps**: CXF 4.1.1→4.1.8+, ActiveMQ 5.18.3→5.19.10, Log4j
   API→2.26.1, Quarkus 3.20.0→newest release whose resolved Netty is
   ≥4.1.137 (verified via `gradle dependencyInsight`/resolved-output, all
   three bridges in lockstep — same version). `max-inbound-message-size:
   16MB` added to cxf + jms `application.yml` (parity with xml).

Test strategy follows the container pattern (no host JDK): dockerized
`gradle test` per bridge; observing/oracle tests where the failure mode is
"silently ignored" (TLS scheme, timestamp rewrite).

## Phases

- **Phase 1 — cxf bridge**: scheme gate, body cap, CXF bump, rc-gevh,
  rc-1dq7, inbound-size alignment, tests.
- **Phase 2 — jms bridge**: Artemis TLS, body limit, ActiveMQ/Log4j bumps,
  first test suite (scheme handling, oversized bodies, unsupported message
  types, both broker clients), inbound-size alignment.
- **Phase 3 — transversal**: Quarkus/Netty bump on all three, full
  three-bridge verification (build + tests), cross-bridge parity check.

Phase-exit: each phase leaves its bridge green in container tests; Phase 3
exits with the three bridges releasable for lockstep `0.6.0` tagging.

## Affected crates

- `camel-component-jms` (Rust): scheme-only validation of broker URI (the
  existing allowlist is retained unchanged — secure schemes pass through;
  unknown schemes rejected at route-build time). The TLS-material fail-loud
  guarantee lives entirely in the sidecar (Java) with the
  `BRIDGE_BROKER_*` env contract — no new public Rust options
  (risk-budget compliant); bridge gRPC client sets its tonic decode limit
  above the consumer body cap (consumer path first).
- `camel-component-cxf` (Rust): `CXF_ADDRESS` scheme validation at config
  layer (`http://` only in this window) before bridge spawn.
- `bridges/jms`, `bridges/cxf` (Java): all hardening items above.
- `bridges/xml` (Java): Quarkus/Netty bump only (Phase 3).

## Architecture boundaries

Components (camel-jms, camel-cxf config validation) and the Java sidecar
services (bridge internals, outside the crate taxonomy). No Runtime, DSL,
Languages, or Functions changes. The gRPC IPC contract is untouched
(ADR-0036 posture preserved); bridge-internal config surface grows by two
env caps. References: ADR-0033 (fail-closed startup validation), ADR-0036
(bridge IPC mTLS + placeholder fail-closed guard), ADR-0038 (configurable
DoS caps), ADR-0012 (handler-contract boundary for bridged consumer
errors).

## Risks

- Quarkus bump churn: native-image reflection configs may need additions;
  contained by running full three-bridge verification in Phase 3.
- ActiveMQ 5.19 client wire-compat with 5.18 brokers is a documented
  supported matrix (client ≥ broker); container tests cover both client
  constructors.
- Fail-loud on `https://` is an observable break for misconfigured
  deployments — intended (they were silently insecure).
