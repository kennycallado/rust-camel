## MODIFIED Requirements

### Requirement: cxf signature knobs are applied or rejected

The cxf bridge sidecar SHALL apply the parsed signature configuration
(`SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`,
`SIGNATURE_C14N_ALGORITHM`) on BOTH signing paths — the producer Dispatch
out-interceptor and the consumer `processOutbound` signed-response path —
whenever the profile's out-actions include `Signature`.
`SIGNATURE_PARTS` SHALL be applied on the producer path only: consumer
endpoint construction SHALL fail when the consumer's profile sets
`SIGNATURE_PARTS` (enforced at Rust `create_consumer`), and the Java
consumer path SHALL refuse a PARTS-configured profile at runtime,
because consumer coverage (Body plus Timestamp) is the fixed
replay-defense invariant. Profile construction SHALL fail with
a diagnostic naming the offending environment variable when: any of
`SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`,
`SIGNATURE_C14N_ALGORITHM`, or `SIGNATURE_PARTS` is set while out-actions
lack `Signature`; any of those knobs is set without a signing keystore;
`SIGNATURE_PARTS` violates the strict grammar (WSS4J canonical order:
`;`-separated segments, each a bare non-empty localName or
`{modifier}{namespace}localName` with modifier empty or exactly
`Element`/`Content` and non-empty localName); or an algorithm knob is not
an absolute URI. A profile that sets none of these knobs SHALL behave
identically to pre-change builds.

The `ALLOWED_SIGNATURE_ALGORITHMS` and `ALLOWED_DIGEST_ALGORITHMS`
settings (`cxf.security.allowed.signature.algorithms` /
`cxf.security.allowed.digest.algorithms`, sidecar env
`CXF_PROFILE_<NAME>_ALLOWED_SIGNATURE_ALGORITHMS` /
`CXF_PROFILE_<NAME>_ALLOWED_DIGEST_ALGORITHMS`) are INBOUND verification
knobs with replacement semantics: when set, the comma-separated list of
absolute algorithm URIs IS the inbound allow-list for that axis
(signature method URIs, digest method URIs respectively). They are EXEMPT
from the signing preconditions above — a profile MAY set them without a
signing keystore and without a `Signature` out-action — but they are NOT
unconditional: profile construction SHALL fail naming the offending
environment variable when an `ALLOWED_*` value is set and is not a
non-empty, non-blank, comma-separated list of absolute URIs (an empty or
blank-only list MUST NOT degenerate to allow-all), or when `ALLOWED_*` is
set on a profile with NO inbound verification material (neither
truststore nor keystore configured: `canVerifyInbound()` is false, both
inbound paths skip WS-Security processing entirely, and the knob would be
silently inert). When both `ALLOWED_*` knobs are unset, the default
allow-list applies: signature rsa-sha256/rsa-sha384/rsa-sha512, digest
sha256/sha384/sha512 — SHA-1 signature and digest URIs are therefore
REJECTED on inbound verification by default, on JVM and native alike, and
so is everything outside those six URIs (the ecdsa, rsa-pss, hmac and
sha224 families included); this tightening of today's JVM behaviour is a
declared breaking change and MUST be stated in the changelog, README and
ADR, including its effect on consumer profiles with `Encrypt`-only
in-actions that receive signed messages (the consumer path verifies every
signature in the header regardless of the required in-actions). Inbound
verification SHALL reject any message whose signature method
or digest method is not on the profile's effective allow-list for that
axis, on BOTH inbound paths: the consumer `processInbound` path and the
producer response-verification in-interceptor. The allow-list is enforced
by ONE shared allow-list source with per-path installation: a signature
`Processor` wrapper pins the Santuario implementation explicitly (`new
SignatureProcessor(new XMLDSigRI())`, no runtime `Security` provider
registration) and installs the profile's `AlgorithmSuite` — built solely
from the two lists, with every key-length and derived-key-length bound
set to a
non-rejecting value (minimum 0, maximum `Integer.MAX_VALUE`) so the suite
constrains ONLY the signature-method and digest axes and never narrows
decryption, key-wrap or key-length behaviour relative to a suite-less
run, independent of security-header element order — into the `RequestData`
on the consumer engine path; on the CXF producer path WSS4J
mechanically seeds an empty (permissive) suite, so the SAME profile
suite from the SAME single source is installed via the in-interceptor's
`setAlgorithmSuites` hook — so acceptance is
deterministic in JVM and native builds and identical across both paths.
Setting the outbound signing knobs (`SIGNATURE_ALGORITHM`,
`SIGNATURE_DIGEST_ALGORITHM`) to legacy SHA-1 URIs does NOT opt any
profile into inbound SHA-1 acceptance. Startup SHALL log a WARN naming
the profile whenever the effective allow-list contains a SHA-1 signature
or digest URI — detected among `xmldsig#rsa-sha1`, `xmldsig#dsa-sha1`,
`xmldsig#hmac-sha1`, `xmldsig-more#ecdsa-sha1`, `xmldsig#sha1` — stating
that inbound SHA-1 acceptance is enabled for that profile. Startup SHALL
additionally log a WARN, on profiles with inbound verification material
(a truststore or keystore configured), when a signing knob names a SHA-1 URI that the profile's
effective inbound allow-list excludes, directing the operator to the
`ALLOWED_*` knobs. Outbound signing defaults are unchanged by
these knobs.
#### Scenario: algorithm lands on the producer out-interceptor

- **GIVEN** a profile with keystore, out-actions `Signature`, and `SIGNATURE_ALGORITHM` set to the rsa-sha384 URI
- **WHEN** the producer out-interceptor is created
- **THEN** its WSS4J configuration carries the rsa-sha384 URI verbatim under the literal `signatureAlgorithm` property key (the signature bytes this key produces are WSS4J's documented contract — the consumer path, where this repo's code calls `WSSecSignature` directly, is the behavioral twin of this scenario)

#### Scenario: algorithm takes effect on signed consumer responses

- **GIVEN** a consumer profile with keystore, out-actions including `Signature` and `Timestamp`, and `SIGNATURE_DIGEST_ALGORITHM` set to the sha-384 digest URI
- **WHEN** `processOutbound` signs the response
- **THEN** the emitted signature's `DigestMethod` is sha-384 and its coverage still includes Body and Timestamp

#### Scenario: parts land on the producer out-interceptor

- **GIVEN** a producer profile with signing keystore, out-actions `Signature`, and `SIGNATURE_PARTS` naming only a header element
- **WHEN** the producer out-interceptor is created
- **THEN** its WSS4J configuration carries the `SIGNATURE_PARTS` value verbatim under the literal `signatureParts` property key (the reference-narrowing behavior behind that key is WSS4J's documented contract)

#### Scenario: parts-configured profile cannot serve a consumer endpoint

- **GIVEN** a consumer endpoint whose selected profile sets `SIGNATURE_PARTS`
- **WHEN** the Rust `create_consumer` constructs the endpoint
- **THEN** construction fails naming `SIGNATURE_PARTS` and the Body-plus-Timestamp replay invariant, and the Java consumer path would also refuse the profile at runtime

#### Scenario: knob without matching action aborts construction

- **GIVEN** a profile whose out-actions omit `Signature`
- **WHEN** any of `SIGNATURE_ALGORITHM`, `SIGNATURE_DIGEST_ALGORITHM`, `SIGNATURE_C14N_ALGORITHM`, or `SIGNATURE_PARTS` is set at construction
- **THEN** construction fails naming the knob's environment variable

#### Scenario: malformed knob values abort construction

- **GIVEN** a `SIGNATURE_PARTS` segment with an empty localName or a braced modifier other than empty/`Element`/`Content`, an algorithm knob that is not an absolute URI, or any of those knobs set without a signing keystore
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable

#### Scenario: unset knobs preserve defaults

- **GIVEN** a profile with out-actions `Signature` and no `SIGNATURE_*` knobs set
- **WHEN** either path signs a message
- **THEN** WSS4J default algorithms and (consumer) Body+Timestamp coverage apply, byte-identical to pre-change builds

#### Scenario: inbound legacy sha1 rejected by default on both inbound paths

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the consumer `processInbound` path, and the same message is verified through the producer response-verification in-interceptor
- **THEN** verification fails on BOTH paths with an algorithm-forbidden error

#### Scenario: inbound legacy sha1 accepted when allow-listed on both inbound paths

- **GIVEN** a profile with a truststore, in-actions `Signature`, and `ALLOWED_SIGNATURE_ALGORITHMS`/`ALLOWED_DIGEST_ALGORITHMS` including the rsa-sha1 and sha1 URIs
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the consumer `processInbound` path, and the same message is verified through the producer response-verification in-interceptor
- **THEN** verification succeeds on BOTH paths

#### Scenario: inbound verification is provider-deterministic

- **GIVEN** a profile allow-listing the rsa-sha1 and sha1 URIs, with the `ApacheXMLDSig` security provider absent from `java.security.Security` (the GraalVM native condition)
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the inbound path
- **THEN** verification succeeds (the pinned processor makes provider availability independent of runtime registration)

#### Scenario: inbound modern algorithms unchanged

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha256 and sha256 digest is verified on the inbound path
- **THEN** verification succeeds exactly as before this change

#### Scenario: three-reference partner shape verifies

- **GIVEN** an opted-in profile with a truststore and in-actions `Signature`
- **WHEN** a message whose single signature carries three references — BinarySecurityToken, Timestamp and SOAP Body — with exclusive c14n, sha1 digest on ALL three references, and a SecurityTokenReference KeyInfo is verified on the inbound path
- **THEN** verification succeeds (coverage checks are required-contains: extra references neither fail nor narrow the signer extraction)

#### Scenario: non-sha1 algorithm outside the default list rejected then accepted when allow-listed

- **GIVEN** a profile with a truststore, in-actions `Signature`, and no `ALLOWED_*` settings
- **WHEN** a message signed with ecdsa-sha256 and sha256 digest is verified on the inbound path
- **THEN** verification fails with an algorithm-forbidden error; when the profile additionally allow-lists the ecdsa-sha256 URI, the same message verifies

#### Scenario: signed and encrypted inbound is unaffected by the default suite

- **GIVEN** a profile with truststore and keystore, in-actions `Signature Encrypt`, and no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha256 and encrypted with an AES-128/rsa-oaep combination is verified and decrypted on the inbound path — in BOTH security-header orders (signature before EncryptedKey and EncryptedKey before signature)
- **THEN** verification and decryption succeed exactly as before this change in BOTH orders (the suite constrains only the signature-method and digest axes; all key-length and derived-key-length bounds are non-rejecting, so no check appears or disappears based on header element order)

#### Scenario: encrypt-only consumer with allow-list accepts signed and encrypted legacy message

- **GIVEN** a profile with truststore and keystore, in-actions `Encrypt` only, and `ALLOWED_SIGNATURE_ALGORITHMS`/`ALLOWED_DIGEST_ALGORITHMS` including the rsa-sha1 and sha1 URIs
- **WHEN** a message signed with rsa-sha1 and sha1 digest and then encrypted is verified and decrypted on the consumer path
- **THEN** verification and decryption succeed (the consumer path verifies every signature in the header regardless of the required in-actions, so Encrypt-only consumers can opt in)

#### Scenario: allowed-list flags exempt from signing preconditions

- **GIVEN** a verify-only profile with a truststore, in-actions `Signature`, no signing keystore, and out-actions lacking `Signature`
- **WHEN** `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` is set at construction
- **THEN** construction succeeds (the exemption is what lets a verify-only consumer opt in)

#### Scenario: allowed-list without inbound verification material aborts construction

- **GIVEN** a profile with neither truststore nor keystore configured (inbound WS-Security processing is skipped entirely)
- **WHEN** `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` is set at construction
- **THEN** construction fails naming the offending environment variable

#### Scenario: empty or blank allowed-list aborts construction

- **GIVEN** an `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` value that is empty, whitespace-only, or only separators
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable (an empty effective list would bypass the suite checks entirely and accept everything)

#### Scenario: malformed allowed-list values abort construction

- **GIVEN** an `ALLOWED_SIGNATURE_ALGORITHMS` or `ALLOWED_DIGEST_ALGORITHMS` value containing an entry that is not an absolute URI
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable

#### Scenario: legacy signing knob does not opt in to inbound sha1

- **GIVEN** a profile with a signing keystore, out-actions `Signature`, a truststore, in-actions `Signature`, and `SIGNATURE_ALGORITHM`/`SIGNATURE_DIGEST_ALGORITHM` set to the rsa-sha1 and sha1 URIs, with no `ALLOWED_*` settings
- **WHEN** a message signed with rsa-sha1 and sha1 digest is verified on the inbound path
- **THEN** verification fails with an algorithm-forbidden error, and a startup WARN has pointed the operator at the `ALLOWED_*` knobs

#### Scenario: startup warn names profiles allowing sha1

- **GIVEN** a profile whose effective allow-list contains a SHA-1 signature or digest URI via an explicit `ALLOWED_*` list
- **WHEN** the bridge starts
- **THEN** a WARN-level log names the profile and the SHA-1 algorithms it allows
