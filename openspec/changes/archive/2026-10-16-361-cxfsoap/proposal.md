# Proposal: 361-cxfsoap

## Why

The 112 Baleares/CyL teams need the CXF/SOAP surface for production. Six
verified gaps (KB `cxfsoap-implementation-research-2026-10-11`; bd rc-stql8,
rc-yoqxn, rc-9pnym, rc-xqwda, rc-2r2i8, rc-2vn9q) block it.

- **A (rc-stql8):** the producer dispatch sets no TLSClientParameters; profile
  keystore/truststore feed only WSS4J Merlin (`SecurityProfile.java:374-382`),
  so no client certificate reaches the wire.
- **B (rc-yoqxn):** the bridge defaults to SOAP 1.1 and manually wraps the
  payload before a `Mode.PAYLOAD` dispatch, risking a nested Envelope; the mock
  discards the request body (`cxf_test.rs:97-121`).
- **C (rc-9pnym):** version is detected after WSS; security failures return a
  hardcoded 400, route faults 200, and faults use an undeclared `soap:` prefix.
- **D (rc-xqwda):** verified signer identity is discarded
  (`WssSecurityProcessor.java:145-160`), no proto field carries it, and Body
  coverage is not required.
- **E (rc-2r2i8):** the listener rejects `https`, binds plain HTTP, and has no
  pool-level TLS identity.
- **F (rc-2vn9q):** schemas travel as one blob with no systemId, so
  `xs:import`/`xs:include` cannot resolve; external access is off and stays off.

## What Changes

- **A.** Nested `[profiles.security.tls]` keys plus `keystore_type`/
  `truststore_type` (`JKS`|`PKCS12`, default `PKCS12`); path+password paired,
  truststore password required; wrong password fails Java startup before ready;
  absent → JVM defaults; truststore-only supported.
- **B.** Wire-assert first; version from the WSDL binding; `soap-version` header
  ignored; a pre-wrapped full Envelope is rejected before any network call.
- **C.** Detect version before WSS (`application/soap+xml`→1.2; `text/xml`/
  absent→1.1); Sender/Receiver matrix; `security.fault_status` integer 400–599,
  override **security (WSS) faults only**, validated at startup.
- **D.** `optional SignerIdentity signer = 7` with string fields `subject=1`,
  `issuer=2`, `serial=3`, `fingerprint_sha256=4` (no DER); RFC 2253 DNs,
  lowercase-hex serial, 64-lowercase-hex SHA-256(DER); `uint64 timeout_ms=7`
  (0 = existing default, >2^31−1 rejected). Metadata via `exchange.set_property`
  under `camel.cxf.signer.*` ONLY — never `store_principal_properties`/
  `Principal`/`camel.auth.*`. Body coverage required only when inbound actions
  include `Signature`; at most 8 `SIGN` results; distinct Body fingerprints
  rejected.
- **E.** Pool `soap_tls_cert_path`/`key_path`/`client_ca_path`/`pkcs12_path`/
  `pkcs12_password` (five `CXF_SOAP_TLS_*`); cert+key XOR pkcs12+password, CA
  required, `http` rejected; one listener identity; `BridgeProcess` parses
  `soap-ready` and `ready` in either order.
- **F.** Discovery from the root file via reachable imports only (depth ≤ 32;
  symlinks and traversal rejected); supplied-only `schema_aliases`, configured
  through a percent-encoded JSON `schemaAliases` URI query parameter parsed at
  endpoint creation (malformed, duplicate, or over-128 fails before the bridge
  starts); a single file with no imports is a one-entry set.

Excluded: Camel parity, one-way SOAP, MTOM, WSDL port auto-selection, and any
bridge IPC mTLS change (ADR-0036).

## Acceptance criteria

- Each bd issue has a regression test red on the pre-fix tree.
- Both `cxf_bridge.proto` copies are byte-identical (signer field + `timeout_ms`
  reconciliation); the three `xml_bridge.proto` copies stay identical.
- Fault matrix holds; `fault_status` overrides security faults only, 400–599,
  isolated per profile.
- No `camel.auth.*` write; signer properties only; malformed/oversized
  descriptor rejected; multi-signer and no-Body rejected.
- One listener identity, fail-closed on incomplete TLS, actual port consumed in
  either ready order. Lints, tests, native builds green.

## Risk budget

Acceptable: intended breaking rejection of timestamp/header-only signatures and
full-Envelope payloads; additive proto field; explicit `timeout_ms`
reconciliation. Out of bounds: weakening XXE posture, forging authentication via
`camel.auth.*`, silently defaulting the version, or collapsing listener/schema
subsystems. CXF/XML pins are independent of the JMS tag; final tags
pending master.
