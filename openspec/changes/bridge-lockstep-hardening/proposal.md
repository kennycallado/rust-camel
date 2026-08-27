# Proposal: bridge-lockstep-hardening

## Why

All three Java/Quarkus sidecar bridges (xml, cxf, jms) sit at `v0.5.0`
(2026-07-23). The just-merged `sidecar-xml-hardening` change plus a
pre-release expert review pass (e_gpt on jms, e_glm on xml/cxf; evidence in
`docs/temp-bridges-findings.md`) surfaced blocking defects that must land
before a lockstep `0.6.0` tag:

- **jms Critical**: Artemis `ssl://` URIs are silently downgraded to
  plaintext — scheme parsed but discarded, no `SSL_ENABLED_PROP_NAME`, no
  hostname verification (`JmsClientFactory.java:186-230`). Credentials cross
  the wire unencrypted under a secure-looking config.
- **cxf Critical**: the SOAP consumer endpoint cannot do TLS; `https://` in
  `CXF_ADDRESS` is silently discarded and the listener serves plaintext on
  `0.0.0.0` (`SoapEndpointPublisher.java:24,59-69`).
- **cxf Important**: unbounded request-body aggregation enables single-request
  OOM of the native image (`SoapEndpointPublisher.java:98-100`).
- **cxf Important**: CXF 4.1.1 ships five 2026 CVEs (XXE + attachment DoS);
  fix is 4.1.8+.
- **jms Critical**: ActiveMQ Classic 5.18.3 carries CVE-2025-27533
  (OpenWire memory exhaustion) and 2026 TLS/path CVEs (fix 5.19.x).
- **jms Important**: broker-controlled `BytesMessage.getBodyLength()` is
  allocated without limit (`JmsConsumer.java:152-162`).
- **Transversal**: Quarkus 3.20.0 is EOL and resolves Netty 4.1.118 with
  HTTP/2 CVEs (fix ≥ 4.1.137) in all three bridges.
- **Tracked follow-ups due now**: rc-gevh (cxf WSS active-attacker replay
  bypass via unsigned `wsu:Timestamp` rewrite) and rc-1dq7 (spotless
  violation in CxfBridgeService.java).

## What Changes

In scope (all three bridges converge on a releasable `0.6.0`):

1. jms: activate SSL for `ssl://`/`wss://` Artemis URIs (or fail-loud where
   TLS cannot be honored) — never silent downgrade.
2. jms: consumer body limit (configurable cap on `BytesMessage` reads).
3. jms: dependency bumps — ActiveMQ Classic 5.19.10, Log4j API 2.26.1.
4. cxf: reject `https://` consumer addresses fail-loud at startup (TLS
   listener support deferred — Rust side validates scheme too).
5. cxf: request-body size cap on the SOAP listener.
6. cxf: CXF 4.1.1 → 4.1.8+.
7. cxf: rc-gevh fix — sign emitted `wsu:Timestamp` via `WSEncryptionPart` and
   require Timestamp in `enforceRequiredActions`; rc-1dq7 spotless fix.
8. Alignment: `max-inbound-message-size: 16MB` in cxf + jms
   `application.yml` (parity with xml).
9. Quarkus bump (all three bridges) to a release resolving Netty ≥ 4.1.137.
10. jms: first Java test suite (TLS scheme handling, oversized messages,
    unsupported message types, both broker clients).

Out of scope (filed as bd, deferred): rc-41h3, rc-kzti, rc-5e4l, rc-bp5c,
rc-0xze, rc-u97s, rc-q5be, rc-aq7f, rc-3yq6. No lockstep tag mechanics here
(tagging is the human's release action).

## Acceptance criteria

- No bridge silently downgrades a secure scheme to plaintext — secure
  schemes either get TLS or fail startup loudly.
- Every externally-sized allocation on jms consumer and cxf listener paths
  is capped by a configurable limit; oversized input is rejected, not OOM.
- Byte-identical WSS replay is rejected AND rewriting/unsigned timestamps
  cannot mint fresh cache keys (rc-gevh oracle test).
- Gradle deps: CXF ≥ 4.1.8, ActiveMQ ≥ 5.19.10, Quarkus non-EOL with
  Netty ≥ 4.1.137 — verified in resolved dependency output of all three
  bridges.
- `gradle build` + test suites green in the three bridges (container test
  pattern, no JDK on host).
- Spotless clean (`rc-1dq7` fixed).

## Risk budget

Acceptable: dependency-bump churn (native-image re-verification is the cost,
bundled deliberately); small observable behavior changes (fail-loud on
previously-silent downgrades, replay rejection) — these are the minor-bump
justification. Out of bounds: wire-format changes to the gRPC IPC contract;
changes to Rust-side public API beyond scheme validation; new features beyond
the listed hardening.
