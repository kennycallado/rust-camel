# Design: cxf-legacy-sha1

## Approach

Two coupled mechanisms, both generic and per-profile (bd rc-cln4q; e_opus root cause binding):

**1. Provider pinning (always-on).** WSS4J enables secure validation unconditionally; acceptance
depends on which XML-signature provider is active. The bridge only registers Santuario
`ApacheXMLDSig` at runtime (`WSSConfig.init()`), which GraalVM native cannot do — the shipped
native binary falls back to the JDK provider whose `jdk.xml.dsig.secureValidationPolicy` forbids
`#sha1`/`#rsa-sha1`. Fix: install the signature processor with an explicit
`new SignatureProcessor(new XMLDSigRI())` on the profile's `WSSConfig`:
- Consumer path (`WssSecurityProcessor`): build the engine via the existing `engineFactory`
  seam with the profile's `WSSConfig` carrying the SAME shared wrapper component as the producer
  path (the wrapper itself injects the suite into the `RequestData`; exact WSS4J 4.0.1 API
  verified by the worker — `WSSecurityEngine.setWssConfig` + `processSecurityHeader(doc, reqData)`
  confirmed present).
- Producer path (`SecurityProfile.createInInterceptor`): pass the same wrapper through
  CXF's `wss4j.processor.map` property (`PROCESSOR_MAP`, key `WSConstants.SIGNATURE`).

Pinning is unconditional: JVM and native become deterministic. No `Security.addProvider` call
(native-safe). `jdk.xml.dsig.secureValidationPolicy` is NOT touched globally.

**2. Per-profile inbound allow-list, off by default.** New profile settings (replacement
semantics — the list IS the allow-list):
- `CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` (`cxf.security.allowed.signature.algorithms`;
  Rust option `allowed_signature_algorithms`), default: rsa-sha256/384/512 URIs.
- `CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS` (`cxf.security.allowed.digest.algorithms`;
  Rust option `allowed_digest_algorithms`), default: sha256/384/512 URIs.
Comma-separated absolute URIs, aligned with existing knob grammar. **The explicit outbound
signing knobs (`signature_algorithm`/`signature_digest_algorithm` set to legacy URIs) are NOT
the opt-in** (partner-update design question, settled 2026-10-19): the signing knobs keep their
outbound-only semantics; inbound acceptance is gated solely by the new lists. A startup WARN
fires — on profiles with inbound verification material (truststore or keystore configured) — when a signing knob
names a SHA-1 URI the inbound allow-list excludes, pointing the operator at the new knobs (the
exact camel-112 confusion; word the message so an intentionally asymmetric profile — sign
SHA-1 outbound, accept only SHA-256 inbound — is recognisable, not flagged as misconfiguration).
Enforced by ONE shared component on
both inbound paths: a small `Processor` wrapper that (a) delegates to
`SignatureProcessor(new XMLDSigRI())` (the pin) and (b) installs the profile's `AlgorithmSuite`
(built from the lists via `addSignatureMethod`/`addDigestAlgorithm`, every other algorithm-set
axis left empty = unchecked) into the `RequestData` when absent. ALL key-length and
derived-key-length bounds are set to NON-REJECTING values (min 0 / max `Integer.MAX_VALUE`):
WSS4J's `EncryptedKeyProcessor`, `ReferenceListProcessor`, `EncryptedDataProcessor` and
`SignatureProcessor` read the same suite, so leaving the default bounds (RSA 1024–4096, EC
160–512, symmetric 128–256) would make decryption and signer-key checks appear or disappear
depending on security-header element order and silently reject, e.g., 8192-bit partner keys.
The suite therefore constrains ONLY signature-method and digest URIs. Consumer path: installed on
the engine's `WSSConfig` via `setProcessor(WSConstants.SIGNATURE, …)` through the existing
`engineFactory` seam. Producer path: the same wrapper passed through CXF's `wss4j.processor.map`
(`PROCESSOR_MAP`) property for the pin; the SUITE needs a separate install hook there because
WSS4J's `WSHandler.decodeAlgorithmSuite` mechanically seeds an EMPTY (permissive) suite on the
CXF path when none is preset — implemented as a minimal `ProfileInInterceptor extends
WSS4JInInterceptor` overriding `setAlgorithmSuites` (the same hook CXF's own
`PolicyBasedWSS4JInInterceptor` overrides) to install the SAME `inboundAlgorithmSuite()`
instance after super. One allow-list source ⇒ no drift between paths; the empty-seed discovery
was made and bytecode-verified during task 1.5 (r_glm review 2026-10-19).
**MUST-VERIFY resolved empirically 2026-10-19** (probe, evidence `/home/shared/tmp/370/`):
the pinned engine verifies rsa-sha1+sha1 under WSS4J secureValidation=true with the Santuario
provider ABSENT (native-equivalent) — Santuario's own secure validation does NOT forbid SHA-1;
**the pin alone is sufficient**, no secure-validation-relaxing wrapper needed. The
provider-absent + default-engine probe reproduces the partner's exact error string; suite
scoping (SHA-1 not listed ⇒ reject) confirmed. **SHA-1 rejected by default** (matches today's
native behaviour; JVM tightens to match — see breaking-change note). **Exempt** from the
dead-config signing-precondition rule — a verify-only profile may set it — but NOT unconditional:
an explicitly set but empty or blank-only list fails construction (WSS4J's
`AlgorithmSuiteValidator` skips checks on empty sets, so an empty list would degenerate to
allow-all), as does an `ALLOWED_*` list on a profile with NO inbound verification material —
neither truststore nor keystore (`canVerifyInbound()` false ⇒ both inbound paths skip
WS-Security processing entirely ⇒ the knob is silently inert). Note the in-actions surface
cannot express this: every buildable profile has `Signature` or `Encrypt` in its in-actions
(`SUPPORTED_INBOUND_ACTIONS` + `validateInboundActions`; unset defaults to `Signature`), and
`Encrypt`-only consumers still verify signatures present in the header (the consumer path
processes every signature in the security header regardless of required in-actions), so they
MAY opt in. Malformed values
(non-absolute URI) fail profile construction naming
the env var. Startup `LOG.warning` naming the profile whenever any effective list contains a
SHA-1 URI (detected set: `xmldsig#rsa-sha1`, `xmldsig#dsa-sha1`, `xmldsig#hmac-sha1`,
`xmldsig-more#ecdsa-sha1`, `xmldsig#sha1`), stating that inbound SHA-1 is accepted for that
profile (ADR-0085). Outbound defaults unchanged (rsa-sha1+sha1 WSS4J default stays; asserted by
`WssSecurityProcessorIntegrationTest` L611-613).

**Breaking-change note (declare in changelog + README + ADR-0085).** The default allow-list
makes inbound verification reject — on BOTH JVM and native — messages signed with the SHA-1
family (rsa-sha1, dsa-sha1, ecdsa-sha1, hmac-sha1, sha1 digest), and additionally tightens
today's JVM behaviour by rejecting everything outside the six modern URIs (e.g.
ecdsa-sha256/384/512, RSA-PSS variants, HMAC signatures, sha224 digests). This includes consumer
profiles with `Encrypt`-only in-actions that receive signed messages: the consumer path verifies
every signature in the header regardless of the required in-actions, so those profiles also hit
the default list and must opt in to keep accepting legacy signers. Migration: operators
of legacy partners opt in via the two new knobs.

**Partner message shape (camel-112, updates #1/#2).** Inbound must accept THREE signature
references (BST + Timestamp + Body), exclusive c14n, `BST_DIRECT_REFERENCE` KeyInfo. The
consumer coverage checks are required-contains (Timestamp covered when the profile's in-actions
include `Timestamp`; Body covered always; extra references do not fail; one signature with N
refs is one SIGN result under the 8-result budget) — verified by code reading 2026-10-19; the
partner profile sets in-actions `Signature` only, so no Timestamp-coverage requirement applies.
`generateLegacySha1Fixture` mimics the wse-php shape (3 references). The counterpart's verifier
is plain `openssl_verify` (PHP) — our OUTBOUND rsa-sha1+sha1 output must stay plain-XMLDSig
verifiable (it is; no outbound change). Outbound coverage configurability (partner update #2
item 10): ALREADY configurable on the producer path via `SIGNATURE_PARTS`
(`cxf.security.signature.parts`), e.g. Body-only is expressible today with
`actionsOut="Signature"` + `SIGNATURE_PARTS=Body`; default stays Body+Timestamp when the
Timestamp action is active. NO new scope in this mission — the finding goes to the park report.

**Test strategy**: mission-mandated JVM tests (`inboundRejectsRsaSha1ByDefault`,
`inboundAcceptsRsaSha1WhenOptedIn`, `inboundModernUnchanged`,
`inboundSha1WithoutRegisteredProvider` — removes `ApacheXMLDSig`, restore in `finally`, simulates
native; in-interceptor `wss4j.processor.map` test; WARN + parse tests), extended by the
bless-fix set: empty/blank allow-list rejected; `ALLOWED_*` on a profile with neither
truststore nor keystore rejected; Encrypt-only consumer with allow-list accepts an
rsa-sha1-signed + encrypted message (consumer path only — CXF producer
in-interceptor Encrypt-only behaviour is unspecified, out of scope); `ecdsa-sha256` rejected by default and accepted when
allow-listed (declares the tightening beyond SHA-1; needs an EC-signed test certificate in the
test PKI — add to `TestKeystoreHelper`, do not reuse the RSA alias); signed+encrypted inbound
under a default profile behaves as before in BOTH header orders (sign-then-encrypt AND
encrypt-then-sign — proves the non-rejecting key-length bounds remove the order dependence);
the reject/accept pair runs on BOTH inbound paths (consumer
`processInbound` AND producer in-interceptor via `PROCESSOR_MAP`). Un-work-around the fixture:
real `generateLegacySha1Fixture` (rsa-sha1 + sha1 on ALL THREE references + exclusive c14n +
BST_DIRECT_REFERENCE + three references BST/Timestamp/Body, mimicking the wse-php shape),
truthful comments replace the false "secure validation forbids RSA-SHA1" ones. Blast radius:
non-legacy inbound test fixtures move to rsa-sha256 (`createProcessor` L721). LEG C in
`crates/camel-test/tests/cxf_test/wss_e2e.rs` (actual path; opted-in → 200, default → WSS fault).
Published-artifact repro (0.8.2 download path, `download.rs:253`): record 500 + exact partner
log line before (the error string is the assertion), record fix behaviour after (JVM).
**Native fix verification gates bd closure**: rc-cln4q stays open until a CI run or the
post-release smoke run shows LEG C green on the native binary (opted-in → 200, default → fault).

**Evidence harness**: scratch scripts under `/home/shared/tmp/370/` (never committed); the
durable artifact is the post-release smoke job: new workflow `.github/workflows/` job that
downloads the published bridge artifact and runs an e2e subset against it (workflow_dispatch +
after-release trigger; documented in the workflow header). Release verification, not a PR gate.

**Java toolchain**: nix-provided JDK 21 (`nix shell nixpkgs#jdk21_headless -c ./gradlew ...`).
**Containment**: every test battery in `systemd-run --user --scope` (mission order env caps),
`TMPDIR=/home/shared/tmp`, scratch `/home/shared/tmp/370/`.

## Affected crates

- `bridges/cxf` (Java sidecar): `SecurityProfile` (parse lists, validate, WARN,
  in-interceptor `PROCESSOR_MAP`), `WssSecurityProcessor` (pinned engine + suite on
  `RequestData`), tests (~63 inbound calls re-based to rsa-sha256), fixture generator.
- `camel-cxf`: config options → `CxfProfileEnvVars` → env vars; new-rule validation only
  (non-empty URI list + keystore-or-truststore present) — Java remains the authoritative
  signing-precondition validator (the Rust side has no existing signature-knob precondition
  list; the only Rust-side signature enforcement today is `create_consumer`'s SIGNATURE_PARTS
  rejection); schema (`cargo xtask schema`).
- `camel-test`: `wss_e2e.rs` LEG C + comment rewrite (L126).
- `xtask`: schema regeneration only.
- Docs: `bridges/cxf/README.md`, `docs/src/components/ws-soap.md`, new `docs/adr/0085-*`.

## Architecture boundaries

Control-plane-only change: configuration parsing, sidecar process env, and Java interceptor
wiring. Data plane (pipeline/EIP semantics) untouched. Follows ADR-0033 (security defaults fail
closed — SHA-1 reject-by-default preserves it) and ADR-0078 (accepted residual limitations — the
opt-in is a NEW accepted risk, recorded in new ADR-0085, not silently added to 0078's list).

## Phases

### Phase 1: Java bridge core — deterministic provider + allow-list
- **Goal:** SHA-1 inbound verifiable when opted in, rejected by default, on JVM and native.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** two new profile env vars; pinned engine behaviour.
- **Deliverable:** Java implementation + all mission-mandated JVM tests + blast-radius fixture
  re-base + `generateLegacySha1Fixture` + Gradle suite green.
- **Exit-criteria:** the five behavioural tests pass; full Java test battery passes in scope.

### Phase 2: Rust plumbing
- **Goal:** Rust config surface for the two new options.
- **Dependencies:** Phase 1 (env var contract).
- **Externally-visible types/interfaces:** `allowed_signature_algorithms`,
  `allowed_digest_algorithms` component options; schema fields; env vars emitted.
- **Deliverable:** camel-cxf parse/emit tests + regenerated schema.
- **Exit-criteria:** unit tests green; `cargo xtask schema --check` passes; exemption tested.

### Phase 3: Integration evidence — e2e, repro, smoke job
- **Goal:** prove behaviour end-to-end and against the published artifact.
- **Dependencies:** Phases 1-2.
- **Externally-visible types/interfaces:** LEG C e2e case; smoke workflow.
- **Deliverable:** LEG C in `wss_e2e.rs`; 0.8.2 repro evidence (500 + log line) recorded;
  post-release smoke workflow.
- **Exit-criteria:** LEG C runs against a locally built JVM bridge (native deferred to CI);
  repro evidence in park report.

### Phase 4: ADR + docs
- **Goal:** durable decision record + truthful docs.
- **Dependencies:** design settled (Phases 1-3 shape).
- **Deliverable:** `docs/adr/0085-cxf-legacy-sha1-inbound-opt-in.md`; README L31 + legacy
  example + flag rows + BSP R5404 note; `docs/src/components/ws-soap.md`.
- **Exit-criteria:** docs name the flag, defaults, WARN, and R5404; ADR references 0033/0078.

## Alternatives considered

- **Global `secure_validation=false` / policy-file edit:** rejected — weakens every profile
  (e_opus, mission order binding).
- **Additive SHA-1 flag (`allow_legacy_sha1=true`):** rejected — allow-list replaces the default
  and generalizes to non-SHA-1 needs (partner asked for `signature_allowed_algorithms` shape).
- **Explicit legacy signing knob as the opt-in** (treating `signature_algorithm=rsa-sha1` as
  inbound consent): rejected — conflates the outbound signing surface with inbound acceptance,
  cannot express asymmetric partners (sign rsa-sha256, accept rsa-sha1), and widens inbound
  silently when an operator changes an outbound knob; the accepted risk must be visible at the
  surface it affects (fail-closed, ADR-0033).
- **JVM-only fix (rely on runtime provider registration):** rejected — native is the shipped
  path; pinning is the only deterministic mechanism.
- **Addendum to ADR-0078:** rejected — 0078 enumerates residual limitations of shipped features;
  this is a deliberate new opt-in mechanism deserving its own ADR (cross-referenced with 0078).
