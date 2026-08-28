## ADDED Requirements

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
a diagnostic naming the offending environment variable when: any knob is
set while out-actions lack `Signature`; any knob is set without a
signing keystore; `SIGNATURE_PARTS` violates the strict grammar
(WSS4J canonical order: `;`-separated segments, each a bare non-empty
localName or `{modifier}{namespace}localName` with modifier empty or
exactly `Element`/`Content` and non-empty localName); or an algorithm
knob is not an absolute URI. A profile that sets none of these knobs
SHALL behave identically to pre-change builds.

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
- **WHEN** any `SIGNATURE_*` knob is set at construction
- **THEN** construction fails naming the knob's environment variable

#### Scenario: malformed knob values abort construction

- **GIVEN** a `SIGNATURE_PARTS` segment with an empty localName or a braced modifier other than empty/`Element`/`Content`, an algorithm knob that is not an absolute URI, or any knob set without a signing keystore
- **WHEN** the profile is constructed
- **THEN** construction fails naming the offending environment variable

#### Scenario: unset knobs preserve defaults

- **GIVEN** a profile with out-actions `Signature` and no `SIGNATURE_*` knobs set
- **WHEN** either path signs a message
- **THEN** WSS4J default algorithms and (consumer) Body+Timestamp coverage apply, byte-identical to pre-change builds
