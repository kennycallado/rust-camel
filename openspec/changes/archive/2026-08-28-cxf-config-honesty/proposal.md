# Proposal: cxf-config-honesty

## Why

Two latent defects in `bridges/cxf` betray operator expectations, found by
the pre-release oracle audit (docs/temp-bridges-findings.md, e_glm review):

1. **Dead security config (rc-0zE).** The Rust component
   (`camel-cxf/src/config.rs`) exposes `signature_algorithm`,
   `signature_digest_algorithm`, `signature_c14n_algorithm`, and
   `signature_parts` options. The sidecar receives them as
   `CXF_PROFILE_<NAME>_SIGNATURE_{ALGORITHM,DIGEST_ALGORITHM,C14N_ALGORITHM,PARTS}`
   env vars, `SecurityProfileStore` parses them, `SecurityProfile` stores
   them — and BOTH signing paths ignore them:
   `SecurityProfile.createOutInterceptor()` (producer Dispatch path) never
   reads them, and `WssSecurityProcessor.processOutbound()` (consumer
   signed-response path) takes the same `SecurityProfile` but hardcodes
   WSS4J default algorithms and Body+Timestamp coverage. A route author
   who pins `rsa-sha384` silently gets defaults on both paths.

2. **Dispatch cache request-context race (rc-bp5c).**
   `CxfClientManager.getDispatch` mutates the SHARED cached Dispatch's
   request context per call (`soapaction.use`/`soapaction.uri`), omits
   `operation` from the cache key, and `CxfBridgeService` additionally
   writes per-request `connectionTimeout`/`receiveTimeout` into the same
   shared context after cache lookup. Concurrent invokes with different
   operations or timeouts cross-contaminate: the wrong SOAPAction reaches
   the server (or is signed over), and timeouts race.

Both live in the cxf bridge sidecar (Java) plus one small Rust-side
validation (`camel-cxf` consumer endpoint construction); the Rust option
surface already exists and needs no API change.

## What Changes

- **Signature knobs applied on BOTH signing paths:**
  - `SIGNATURE_ALGORITHM` / `SIGNATURE_DIGEST_ALGORITHM` /
    `SIGNATURE_C14N_ALGORITHM` → WSS4J properties in
    `createOutInterceptor()` (producer) AND `WSSecSignature` setters in
    `WssSecurityProcessor.processOutbound()` (consumer responses).
  - `SIGNATURE_PARTS` → producer path only (WSS4J `SIG_PARTS`). On the
    consumer path the hardcoded Body+Timestamp coverage is the rc-gevh
    replay-defense invariant and MUST NOT be narrowed by config. The
    consumer reaches profiles per-request (no static registration exists
    on the Java side), so enforcement lives in Rust:
    `CxfEndpoint::create_consumer` fails endpoint construction when the
    consumer's profile sets `signature_parts`, naming the conflict; the
    Java `WssSecurityProcessor` constructor additionally refuses a
    PARTS-configured profile at runtime (defense-in-depth for direct
    sidecar use).
- **Startup validation (fail loud, ADR-0033 family):**
  - Any signature knob set without an out-signing-capable keystore →
    profile construction fails.
  - Any signature knob set while out-actions lack `Signature` → fails.
  - `SIGNATURE_PARTS` grammar (strict, WSS4J canonical order):
    `;`-separated segments; each is either a bare non-empty localName
    (no braces) or `{modifier}{namespace}localName` with modifier empty
    or exactly `Element`/`Content`, namespace possibly empty, localName
    non-empty.
  - Algorithm knobs must be absolute URIs (any scheme; WSS4J remains the
    semantic authority for support — first-invoke failure there is loud,
    documented).
- **Race fix:** typed `DispatchKey` record (wsdl, address, service, port,
  profile, operation, normalized timeout) replaces `#`-concatenation
  (fragment-unsafe); ALL per-request context writes (soapaction pair +
  both timeouts) move into `createDispatch`; `CxfBridgeService` performs
  no post-cache context mutation.
- Operator docs: cxf README signature section (knobs take effect on both
  paths, PARTS consumer prohibition, timeout now cache-keyed).

Excluded (existing bds): producer WSS Timestamp asymmetry (rc-u97s — the
consumer Timestamp action remains as-is), unbounded response allocation
(rc-wlgy), https truststore (NTH).

## Acceptance criteria

- Consumer signed responses honor `SIGNATURE_ALGORITHM` / `DIGEST` /
  `C14N` when set (asserted behaviorally on `SignatureMethod` /
  `DigestMethod` / `CanonicalizationMethod` of emitted signatures);
  producer requests carry the four knobs on the literal WSS4J property
  keys (property-level verification — emitted-signature behavior behind
  those keys is WSS4J's documented contract, per design's
  semantic-authority decision).
- Producer `SIGNATURE_PARTS` value lands on the literal WSS4J
  `signatureParts` property key verbatim (property-level; the
  reference-narrowing behavior behind that key is WSS4J's documented
  contract).
- PARTS-configured profile + Rust consumer endpoint creation →
  construction fails naming `SIGNATURE_PARTS`; the Java consumer path
  refuses a PARTS-configured profile at runtime if ever reached directly.
- Knob without `Signature` action, knob without keystore, malformed
  PARTS segment (empty localName, or braced modifier other than
  empty/`Element`/`Content`), non-absolute-URI algorithm → construction
  abort naming the env var.
- Two concurrent invokes on the same endpoint tuple differing in
  operation each observe their own SOAPAction; differing in timeout each
  get their own Dispatch (key includes normalized timeout); no context
  mutation after publish.
- Profiles setting no knobs behave byte-identically to pre-change
  builds; existing cxf suite green; `spotlessCheck` green.

## Risk budget

- Behind existing config surface — no new public Rust API, no IPC change.
- Cache cardinality grows by distinct (operation, timeout) pairs per
  endpoint tuple — producer routes enumerate both at config time
  (control-plane-bounded); documented.
- Consumer-path coverage stays pinned (Body+Timestamp) — no replay-
  defense regression is in scope or acceptable.
- Algorithm pass-through keeps WSS4J as the crypto authority; we validate
  syntax only.
