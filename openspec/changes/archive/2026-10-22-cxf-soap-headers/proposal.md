# Proposal: cxf-soap-headers

## Why

Partner camel-112 sends an application header next to `wsse:Security`:

```xml
<requestHeaderType xmlns="http://tempuri.org/commonSchema">
  <user>…</user><messageId>…</messageId><date>…</date><organisation>28071</organisation>
</requestHeaderType>
```

They sign every message with one system certificate, so `camel.cxf.signer.*` cannot
distinguish tenants. The header can, and some query operations carry no sender in the
SOAP Body. Today the CXF consumer exports only the Body and the HTTP headers: every
`soap:Header` child except `wsse:Security` is dropped, so the route cannot read the
partner's tenant identity.

The same export path also exposes a pre-existing spoofing bug (bd `rc-e6ddw`): the
consumer copies client HTTP headers verbatim into the exchange after setting
`CxfRequestId` / `CxfOperation` / `CxfSoapAction` / `CxfSecurityProfile`. gRPC metadata
keys are lowercased and matched case-insensitively, so a client can overwrite these
bridge-owned values. The same trick would let a client spoof any future
`CxfSoapHeader.*` value.

## What Changes

- Per-profile allow-list of SOAP header QNames (`security.exposed_soap_headers`, at most
  16 entries). Only listed elements are exported; unlisted elements are ignored
  silently.
- Export each listed element as serialized XML plus namespace, local name, and a
  `signed` flag computed from the verified signature references.
- Additive wire field `repeated SoapHeader soap_headers = 8` on `ConsumerRequest`, in
  both proto copies. The existing `headers` map is never reused for SOAP headers.
- Rust maps exported headers to `CxfSoapHeader.<localName>` and
  `CxfSoapHeaderSigned.<localName>` after the HTTP-header loop, and drops HTTP keys
  whose lowercase form starts with `cxfsoapheader` or equals a bridge-owned `Cxf*`
  name.
- Fail-closed validation: reserved WSSE/WSSE11/WSU/DS/XENC/SOAP-ENV namespaces,
  malformed QNames, more than 16 entries, and duplicate QNames or duplicate local
  names are rejected at startup. Duplicate request QNames and the 16 KiB per-header /
  64 KiB total caps are Sender faults raised before dispatch.
- Docs: `docs/src/components/ws-soap.md` describes the allow-list, the exchange header
  names, the caps, and the trust limit of an unsigned header. Delta on
  `cxf-consumer-dispatch`.

Explicitly excluded: exporting unlisted headers, typed header parsing (the route uses
XPath on the serialized XML), signature-coverage changes, and producer/outbound SOAP
header support.

Affected crates: `crates/components/camel-cxf`, `crates/services/camel-bridge`,
`crates/camel-test` (e2e fixture leg), `bridges/cxf` (Java), `docs`.

Bd: `rc-imlee` (+ `rc-e6ddw`).

## Acceptance criteria

- T1–T10 green; `./gradlew --no-daemon spotlessCheck`; full Java suite green.
- `cargo test -p camel-component-cxf -p camel-bridge` green under containment;
  `cargo build --workspace` green. The `camel-test` cxf integration battery runs
  where the native bridge binary is available; when it is not, T9 is deferred to CI
  and the reason is recorded (AGENTS gate policy for infrastructure tests).
- `cargo test -p camel-cli --test feature_profiles` green with no golden change (no
  dependency closure change).
- `cargo fmt --check --all`; affected-crate clippy `-D warnings`.
- `openspec validate cxf-soap-headers --type change` clean.
- `cargo xtask schema` regeneration is verified (no drift); evidence that no generated
  schema covers `CxfSecurityFields` is recorded if that remains true.
- No bridge version bump, no release, no version string added to docs.

## Risk budget

Accepted: a route may treat an exported header as trustworthy when the profile has no
WS-Security; the `signed` flag is exported so the route can refuse it, and the docs
state that an unsigned header must not authorize anything unless TLS covers the hop.
Accepted: the exchange key uses the local name only, so the allow-list rejects two
QNames that share a local name. Out of bounds: exporting `wsse:Security` or any
reserved-namespace element, trusting the `headers` map for SOAP values, unbounded
header bytes, and silently overwriting bridge-owned `Cxf*` values.
