# Design: cxf-bridge-hardening

## Context

The CXF sidecar ships in bridges 0.6.1. Five reviewed defects remain
open; four are the fail-loud / allocation-bound classes the 0.6.0
sweep (rc-ep2f) closed on other paths, one is supply-chain hygiene.
All fixes are sidecar-local (Java) — no Rust changes, no wire-format
changes, no new gRPC surface.

## Goals / Non-Goals

- Goals: kill the silent-no-op action-token class; bound both
  allocation directions (response bytes, Dispatch entries); make
  request-body extraction namespace-aware; scope the Shibboleth repo
  to the groups it must serve.
- Non-Goals: extending materialization beyond Timestamp-inbound (no
  UsernameToken, SignatureConfirmation, SAML — feature bds); changing
  the gRPC contract; Rust-side validation.

## Decisions

### D1 — Whitelist the materialized token set; Timestamp joins inbound

`validateInboundActions` accepts {Signature, Encrypt, Timestamp} with
the same composition rule the outbound validator already enforces:
Timestamp alone is rejected (a bare timestamp is not tamper-evident);
`Timestamp Signature` is the supported replay-mitigation shape and is
already live through `WssSecurityProcessor.processInbound`
(enforceRequiredActions + `verifyTimestampSignatureCoverage`,
integration-tested). `validateOutboundActions` accepts {Signature,
Encrypt, Timestamp} — unchanged composition, new unknown-token
rejection.

The inbound interceptor (`createInInterceptor`) additionally
materializes WSS4J timestamp validation when Timestamp is configured,
so the producer's response verification does not partially ignore the
token while `processInbound` enforces it. The worker verifies the
exact WSS4J 4.x action wiring (action-string composition with
SIGNATURE/ENCRYPT) against the existing interceptor code.

Unknown tokens fail profile build with `IllegalArgumentException`
naming the token, the supported set, and the raw actions string —
same error shape as the existing material checks. Blank stays
raw-exempt.

Rejected alternative: extending materialization for UsernameToken et
al — feature work with crypto-material decisions, out of scope for a
pre-release hygiene change. Operators who need those tokens file a
feature bd.

Consequence: configs using tokens outside the sets (currently silent
no-ops — no security enforced) now fail at profile build. Intended;
release notes carry the line.

### D2 — Response cap reuses the consumer contract

`CXF_MAX_BODY_BYTES` already names a byte budget (default 16 MiB,
ceiling 17 MiB, ADR-0033 fail-loud parse at startup — in
`SoapEndpointPublisher`). The producer direction gets the same budget:
one knob, both directions, one README paragraph. A separate
`CXF_MAX_RESPONSE_BYTES` was rejected — two knobs for one heap and
one trust posture invite drift.

Mechanics: `toXmlString` wraps the ByteArrayOutputStream in a counting
stream that throws when the serialized byte count exceeds the cap.
The service catches it and answers `Status.RESOURCE_EXHAUSTED` with a
description naming `CXF_MAX_BODY_BYTES` **and the observed size that
exceeded it**. The cap is read through the same fail-loud parse the
publisher uses (one shared parse site, not two copies).

Allocation honesty: the DOM behind the Source is already in heap when
serialization runs (CXF parsed it). The cap bounds the sidecar's
forwarded aggregation and ByteString copy, and turns an
unbounded-heap remote into a bounded, diagnosable failure — the same
honesty level the README states for the JMS cap.

### D3 — Namespace-aware extraction via the existing helper

`SoapEnvelopeHelper.extractBody(Document)` is the canonical extractor
(already used by the client path). The publisher parses the capped
request bytes with the hardened DocumentBuilder the helper uses and
calls it. The string-scan method is deleted — it has no other
callers.

Behavior deltas (intended, spec'd):
- No `Body` element → empty payload forwarded (today: the whole
  envelope is mis-forwarded).
- Malformed XML → 400 fail-loud (today: lenient splice forwards
  garbage).
- Prefixed bodies (`soapenv:Body`) and SOAP 1.1/1.2 namespaces
  resolve by local name; the scan's `":body"`/`"<body"` fallbacks
  die.

### D4 — Bounded LRU Dispatch cache, deterministic close

`CXF_MAX_DISPATCHES` (default 64, ceiling 1024, ADR-0033 fail-loud
parse — positive count, `0` rejects). The cache becomes a
synchronized access-order `LinkedHashMap`:

- **Lookup** (hit path) and **cold creation** are serialized under one
  lock; the SOAP invoke itself stays outside the lock. Today's
  `ConcurrentHashMap.computeIfAbsent` gives atomic per-key creation —
  the synchronized map preserves that atomicity (no duplicate
  concurrent creation, no last-write-wins) at the cost of
  serializing cold starts. Hot-path cost is a brief monitor
  acquisition; invokes (seconds) never hold it.
- **Eviction**: insertion past the cap evicts the LRU entry.
- **Close**: CXF 4.1.8's `DispatchImpl` exposes a public `close()`
  and implements `java.io.Closeable` (jakarta `Dispatch` itself is
  not AutoCloseable). Evicted entries and all entries at
  `@PreDestroy` are closed via `close()`, best-effort per entry
  (close failure logs and continues). Deterministic — not "closed or
  documented."
- **Logging**: eviction logs at FINE with the evicted key's wsdl
  basename + address + port — never the operation string (untrusted
  data).

Not chosen: ConcurrentHashMap + queue (two structures to keep
consistent; loses per-key atomicity or needs extra locking), Caffeine
(new dependency for a 60-line cache).

### D5 — Scope the Shibboleth repo, do not remove it

Removal is not viable: CXF 4.1.8 pulls WSS4J 4.0.1 → OpenSAML 5.1.6,
whose artifacts 404 on Maven Central; the Shibboleth nexus is the
only source. The supply-chain fix that ships: Gradle
`exclusiveContent` on the repo — it resolves **only**
`org.opensaml.*` and `net.shibboleth.*` groups; every other group
resolves from Central regardless of what the nexus hosts. Proven with
`./gradlew --refresh-dependencies` (forced re-resolution from the
declared filters). rc-aq7f's close-reason records the pivot: repo
retained, access-scoped.

## Risks / Trade-offs

- Reject-unknown (D1) breaks silently-insecure configs at startup —
  intended, release-noted.
- Shared cap env (D2): directions cannot be budgeted separately —
  accepted (one heap).
- LRU under attack degenerates to create+evict thrash — bounded
  memory is the security property; thrash cost is documented.
- Cold-creation lock (D4) serializes concurrent first-invokes per
  process — startup thundering herd on N distinct keys becomes N
  sequential creations; acceptable for a bridge.
- Extraction tightening (D3) may 400 payloads that previously
  mis-forwarded — those were already broken.

## Migration Notes

Release notes (bridges 0.6.1): unknown action tokens fail profile
build (supported sets listed; Timestamp requires Signature both
directions); producer responses beyond `CXF_MAX_BODY_BYTES` fail
`RESOURCE_EXHAUSTED`; malformed inbound envelopes 400; Dispatch cache
bounded at 64 (`CXF_MAX_DISPATCHES`); Shibboleth repo scoped to
OpenSAML groups only.

## Open Questions

None — the expert-bless round resolved the two forks (inbound
Timestamp is live → whitelisted with composition; repo removal is
impossible → scoped instead).
