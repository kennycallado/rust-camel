# Design: cxf-soap-headers

## Approach

The design below is the verified mission-375 design (bd `rc-imlee`). The order's rules
R1–R7 are implemented as stated; this document records the resulting seams.

**R1 — Extraction point.** `WssSecurityProcessor.processInbound` extracts the
allow-listed children of the SOAP `Header` from the post-WSS DOM, before
`domToString`. The nodes are then exactly the nodes WSS4J verified. A header is
`signed = true` only when a verified `SIGN` result carries a data reference whose
protected element is the exported post-WSS DOM node itself (`isSameNode`). A
reference with no protected element does not set the flag, even when a `wsu:Id`
matches: node identity is the only evidence that the signature covered the exact
exported element, so the contract fails closed. References from `ENCR` results never
set the flag, and a signed request that does not cover the header yields
`signed = false`; verification failures keep their existing fail-closed behavior and
are never downgraded to unsigned dispatch. A
profile whose inbound verification is unavailable is not processed by
`processInbound`; `SoapEndpointPublisher` extracts from the parsed request document
and passes no signature results, so every header is `signed = false`. `wsse:Security`
is never exported: the allow-list rejects that namespace at startup and the exporter
also skips reserved namespaces defensively.

**R2 — Allow-list.** Per-profile `security.exposed_soap_headers`, a list of QNames in
`{namespace}local` form, at most 16 entries. The partner's canonical QNames are
`{http://tempuri.org/commonSchema}requestHeaderType` and
`{http://tempuri.org/resources}RequestHeaderElement`. The grammar is fixed and
ASCII-only so the comma-joined env carrier round-trips losslessly: the namespace is a
non-empty URI that contains no `{`, `}`, `,`, or whitespace, and the local name
matches `[A-Za-z_][A-Za-z0-9._-]*`. Startup rejects malformed entries, reserved
namespaces (WSSE 1.0/1.1, WSU, DS, XENC, SOAP 1.1/1.2), duplicate QNames, and any two
local names that collide under ASCII case-insensitive comparison — the exchange key is
`CxfSoapHeader.<localName>` and `Message::header_ic` matches case-insensitively, so
`Tenant` and `tenant` would be ambiguous. An element that is not listed is ignored
silently.

**R3 — Wire.** A new `SoapHeader` message (`ns`, `local_name`, `xml`, `signed`) and
`repeated SoapHeader soap_headers = 8` on `ConsumerRequest`, added identically to
`bridges/cxf/src/main/proto/cxf_bridge.proto` and
`crates/components/camel-cxf/proto/cxf_bridge.proto`. The field is additive, so old
and new builds stay wire compatible. The `headers` map is never used for SOAP values:
gRPC metadata keys are lowercased and `Message::header_ic` matches case-insensitively,
so a client could otherwise spoof `cxfsoapheader.organisation`.

**R4 — Rust mapping.** In `camel-cxf/src/consumer.rs`, `build_exchange` sets
`CxfSoapHeader.<localName>` (serialized element XML) and
`CxfSoapHeaderSigned.<localName>` (`"true"`/`"false"`) AFTER the HTTP-header loop. The
HTTP-header loop skips any key whose lowercase form starts with `cxfsoapheader` and any
key equal to `cxfrequestid`, `cxfoperation`, `cxfsoapaction`, or `cxfsecurityprofile`.
Bridge-owned values therefore always win (`rc-e6ddw`).

**R5 — Limits, fail closed.** Extraction runs before dispatch. A duplicate
allow-listed QName in one request; a serialized header larger than 16,384 UTF-8 bytes;
or a total larger than 65,536 UTF-8 bytes raises `SoapHeaderExportException`, which
the publisher renders as a Sender fault. The limits are inclusive: exactly 16,384 and
65,536 bytes are accepted. The measured value is the byte length of the UTF-8 encoding
of the serialized element, never the Java `String` length. The exception is not a
`WSSecurityException`, so a profile's `fault_status` override cannot rewrite it.
Messages name only QNames, counts, and byte sizes — never header values.

**R6 — No value logging.** Java logs only the profile name and, on a rejected request,
the QName or size. Rust writes header values into the exchange only; no new log line
carries a value. `user` is PII.

**R7 — Docs and schema.** `docs/src/components/ws-soap.md` documents the allow-list,
both exchange header families, the caps, and that an unsigned header must not be
trusted for authorization unless TLS covers the hop. The text describes capabilities
and never names a bridge version. Regeneration runs `cargo xtask schema` (not only
`--check`) and the resulting tree is committed; if evidence shows that no generated
schema covers `CxfSecurityFields`, that evidence is recorded in the final report
rather than waived in advance.

**Java structure.** A new `SoapHeaderExporter` class owns extraction, the QName and
namespace policy helpers shared with `SecurityProfile`, the size caps, and the
`signed` computation. `SecurityProfile` stores and validates the immutable
`List<QName>`. `SecurityProfileStore` parses
`CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS`. `VerifiedInboundMessage` gains a
`List<cxf_bridge.SoapHeader> soapHeaders` component. `SoapEndpointPublisher` adds the
field to the outgoing `ConsumerRequest` and maps the new exception to the Sender
branch of the fault matrix.

**Configuration flow.** `CxfSecurityFields.exposed_soap_headers: Vec<String>` →
`CxfProfileEnvVars` → `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS` (comma-joined, emitted
only when non-empty) → `SecurityProfileStore` → `SecurityProfile.Builder`. Rust
validates the same grammar, cap, reserved namespaces, and duplicate rules at config
load; the Java bridge remains the authoritative fail-closed validator, following the
`allowed_signature_algorithms` precedent.

## Affected crates

- `crates/components/camel-cxf`: `CxfSecurityFields` field + validation, proto copy,
  consumer header mapping.
- `crates/services/camel-bridge`: `CxfProfileEnvVars` field and env emission.
- `crates/camel-test`: one new signed-request fixture leg in `cxf_test/wss_e2e.rs`.
- `bridges/cxf` (Java): `SoapHeaderExporter`, `SecurityProfile`,
  `SecurityProfileStore`, `WssSecurityProcessor`, `SoapEndpointPublisher`, proto.
- `docs`, `schemas`: `ws-soap.md`; schema regeneration and drift evidence.

## Architecture boundaries

- Data/control plane: the allow-list is operator configuration (trusted, ADR-0032);
  header values are exchange data (untrusted) and cross into the route only as message
  headers. No control-plane action reads them.
- ADR-0033 fail-closed startup validation: malformed, reserved, oversized, or
  ambiguous allow-lists reject the profile at startup; there is no permissive default.
- Components boundary: `camel-cxf` owns the configuration surface and the exchange
  mapping; the Java bridge owns XML/WSS processing; `camel-bridge` only transports
  configuration and never sees request data.
- ADR-0051: no secret or PII value enters a log, an error message, or `Debug`.
- ADR-0084: proto compilation stays hermetic; both proto copies carry the same
  additive field.

## Phases

### Phase 1: Bridge exports allow-listed SOAP headers

- **Goal:** add the additive wire field, extract and bound the allow-listed headers from
  the post-WSS DOM, validate the per-profile allow-list at startup, dispatch the field,
  and generate the signed header-bearing Rust fixture.
- **Dependencies:** none; this phase establishes the contract Phase 2 consumes.
- **Externally-visible types/interfaces:** `cxf_bridge.SoapHeader`,
  `ConsumerRequest.soap_headers = 8`, `SecurityProfile.exposedSoapHeaders()`,
  `CXF_PROFILE_<NAME>_EXPOSED_SOAP_HEADERS`.
- **Deliverable:** Java classes and tests, the generated fixture
  `crates/camel-test/tests/fixtures/cxf-wss-signed-request-with-header.xml`.
- **Exit-criteria:** T1–T7 and T10 green; `./gradlew --no-daemon spotlessCheck test`
  green; both proto copies byte-identical.

### Phase 2: Rust exchange mapping, spoofing fix, e2e, docs

- **Goal:** transport the allow-list, map exported headers into the exchange, drop
  spoofed `Cxf*` HTTP keys (`rc-e6ddw`), add the e2e leg, and document the capability.
- **Dependencies:** Phase 1 (proto field and signed fixture).
- **Externally-visible types/interfaces:** `CxfSecurityFields.exposed_soap_headers`,
  `CxfSoapHeader.<localName>`, `CxfSoapHeaderSigned.<localName>`.
- **Deliverable:** Rust config/mapping code and tests, `camel-test` e2e leg, updated
  `docs/src/components/ws-soap.md`, schema drift evidence.
- **Exit-criteria:** T8 green locally; T9 green where the native bridge binary is
  available, otherwise explicitly deferred to CI; affected-crate Rust gates, docs and
  schema gates green.

## Alternatives considered

- Reusing the `headers` map for SOAP headers — rejected: lowercase gRPC metadata makes
  spoofing indistinguishable from bridge output.
- Exporting arbitrary headers — rejected: unbounded, and exposes security material.
- Parsing headers into typed fields — rejected: the route can XPath the serialized
  element; a typed model would need one schema per partner.
- Failing the request on a duplicate/oversized *unlisted* header — rejected: an
  unlisted header is ignored by contract, so it cannot create ambiguity or cost beyond
  the request body cap.
- Keying the exchange header by QName — rejected by the verified design; instead the
  allow-list rejects duplicate local names.
