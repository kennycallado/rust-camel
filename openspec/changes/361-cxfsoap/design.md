# Design: 361-cxfsoap

## Approach

Delta specs carry the contracts and bounds; this states the binding choices.

**A — producer transport TLS.** Nested `[profiles.security.tls]` (`keystore_*`,
`truststore_*`; type `JKS`|`PKCS12`, default `PKCS12`) applied in
`CxfClientManager.createDispatch` as `TLSClientParameters`; absent block → JVM
defaults; path/password paired; wrong password fails pre-ready; WSS stores independent;
hostname verification kept.

**B — envelope fidelity.** Wire-assert first. Remove `wrapInEnvelope` from
`CxfBridgeService.invoke`; version from `dispatch.getBinding().getBindingID()`;
ignore `soap-version`; reject a pre-wrapped `Envelope` pre-network.

**C — fault status.** Detect version before WSS (`application/soap+xml`→1.2;
`text/xml`/absent→1.1 when the root is unreadable); Sender/Receiver matrix per
spec; `security.fault_status` 400–599 overrides security (WSS) faults only;
version-matched faults. `hasConsumerObserver` is checked before WSS/body work;
no consumer stream yields a non-fault HTTP 503 (`Retry-After`, plaintext) outside
the fault matrix; late stream loss yields the same 503.

**D — signer identity.** `ConsumerRequest` gains `optional SignerIdentity signer
= 7` (four string fields); sync both `cxf_bridge.proto` copies and reconcile
`uint64 timeout_ms=7` (0 = default, >2^31−1 rejected). No DER crosses the proto.
`processInbound` returns a descriptor, requires Body coverage when `Signature` is
required (breaking), examines ≤8 `SIGN` results, rejects distinct Body
fingerprints, and lets non-Body signatures yield no signer. Rust validates
subject/issuer ≤4096 UTF-8 bytes, serial ≤128 lowerhex, fingerprint 64-lowerhex,
then writes only `camel.cxf.signer.*` exchange properties; it SHALL NOT call
`store_principal_properties`, construct a `Principal`, or write `camel.auth.*`. A
descriptor failure is a non-WSS `Receiver` fault (`soap:Server` → HTTP 500); the
override does not apply.

**E — listener TLS.** Pool-level five `soap_tls_*` keys beside `bind_address`,
forwarded as `CXF_SOAP_TLS_*` via `bridge_config_with_env_trace`; one Vert.x
listener identity; cert+key XOR pkcs12+password; CA required; `http` with TLS
rejected; validated at Rust load and Java pre-bind. Emit
`{"status":"soap-ready","port":<N>}` from the Quarkus startup path before
readiness; `BridgeProcess` stores the optional SOAP port, bounded by
`bridge_start_timeout_ms`, without a consumer RPC.

**F — schema set.** Extend `RegisterSchemaRequest` with `map<string, bytes>
resources`, `map<string, string> aliases`, and root systemId, syncing three
`xml_bridge.proto` copies; the alias map is authoritative for the aliased
schema's base, with a redundant alias-URI bytes entry. Discovery starts at `schema_path` (virtual root
= parent), follows only reachable `xs:import`/`xs:include` using `quick-xml`,
rejects escapes, caches each canonical file once, expands once per
`(canonical target, normalized virtual path)` pair, so each virtual base
expands its own refs and every reachable key ships. Caps (≤256 resources, ≤8
MiB, ≤128 aliases, ≤128 resource alias-URI entries, depth ≤32) count all
transmitted keys/bytes and fail closed on symlink growth. Rust discovery reads
within the root; the Java resolver fetches nothing. The root source and each
returned `LSInput` set `systemId` to the normalized virtual path and leave
`baseURI` unset; Xerces expands it against the process anchor, and the resolver
validates the expanded base, strips it to a virtual key, and resolves
base-relative. Normalization runs first: an
in-root `..` resolves, an escaping `..` is rejected; only unsupplied URLs are
refused. `schema_aliases` is supplied only through a percent-encoded JSON
`schemaAliases` URI query parameter parsed by `from_uri` at endpoint creation;
malformed, duplicated, or over-128 values fail closed pre-startup.

## Affected crates

- `crates/components/camel-cxf` (config, pool, producer, consumer, proto) and
  `camel-validator` (config, xsd_bridge, resolver, proto).
- `crates/services/camel-bridge`: dual-line parsing; pool and `CxfProfileEnvVars`
  env vars.
- `bridges/cxf` and `bridges/xml` (Java) plus `crates/camel-test` wire/mTLS/schema
  tests.

## Architecture boundaries

All six are data-plane adapters; no control-plane change. Proto data is
untrusted and bounded (ADR-0032); bridge IPC mTLS (ADR-0036) is unchanged.

## Phases

### Phase 1: Producer transport + envelope fidelity (A + B)
- **Goal:** producer mTLS; single-envelope wire shape.
- **Dependencies:** none; no proto change.
- **Deliverable:** `[profiles.security.tls]`; ws-soap and CONTEXT docs.
- **Exit-criteria:** wire capture RED first, then binding-derived version, PAYLOAD
  unwrap, `soap-version` ignored, pre-wrapped Envelope rejected; then TLS
  conduit/mTLS with bad CA/SAN.

### Phase 2: Consumer production surface (C + D + E)
- **Goal:** fault status/version, signer metadata, mutual-TLS listener.
- **Dependencies:** Phase 1 conventions; owns the proto bump.
- **Deliverable:** byte-identical `cxf_bridge.proto` copies + synced callers;
  `security.fault_status`; `camel.cxf.signer.*` only; pool `bind_address` + five
  `soap_tls_*`; `BridgeProcess` optional SOAP port + `expect_soap_ready`;
  soap-ready announcer; docs.
- **Exit-criteria:** matrix and security-only override pass; descriptor bounds
  enforced; no `camel.auth.*` write; mutual-TLS succeeds, incomplete forms fail
  pre-bind; dual ready either order; pre-stream POST 503 with `Retry-After`.

### Phase 3: Schema set (F)
- **Goal:** `xs:import`/`xs:include` via an in-memory virtual set; clean builds.
- **Dependencies:** none; independent of A–E.
- **Deliverable:** `RegisterSchemaRequest.resources` + `aliases` + root systemId;
  three proto copies + callers synced; `schema --check`, `lint-non-exhaustive`,
  clean bridge builds; English docs.
- **Exit-criteria:** reachable imports resolve, unreferenced excluded,
  depth/symlink/traversal/unsupplied-alias rejected, changed-child rotates the
  key, restart replay re-registers, builds green.

## Alternatives considered

- **`soap_version` key as primary; WSS-store reuse for transport TLS;
  `store_principal_properties` for the signer** — rejected (binding ID is
  authoritative; cross-profile leakage; it forges `camel.auth.*`).

## Open decisions

- **Bridge releases/pins:** cxf 0.8.0, xml 0.8.0 (master/e_opus ruling,
  2026-10-12). Task 3.7 updates Gradle defaults and the CXF, validator, and XSLT
  Rust pins together at release time. Tags and pushes are owner-only.
- **Blessing hash:** conductors use `scripts/bless-hash.sh`, never concatenation.
