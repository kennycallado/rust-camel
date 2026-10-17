# cxf-signer-identity Specification

## Purpose
Signer identity carried from the CXF bridge. The consumer request carries an
optional bounded signer descriptor, Rust validates and exposes it through
dedicated exchange properties, and signer extraction runs only for
Signature-required profiles.

## Requirements
### Requirement: Signer identity is exchanged as bounded string fields only

`ConsumerRequest` SHALL carry an OPTIONAL `SignerIdentity signer = 7`.
`SignerIdentity` SHALL have exactly these string fields: `subject = 1`,
`issuer = 2`, `serial = 3`, `fingerprint_sha256 = 4`. No DER bytes SHALL cross
the proto. `subject` and `issuer` SHALL be RFC 2253 distinguished names.
`serial` SHALL be the certificate serial as unsigned lowercase hex only.
`fingerprint_sha256` SHALL be exactly 64 lowercase hex characters of the
SHA-256 digest of the DER certificate. Java SHALL enforce a 64 KiB cap on the
encoded certificate it hashes; a certificate whose DER encoding exceeds 64 KiB
SHALL be rejected, not truncated. When no signature is present the field SHALL
be absent, meaning unknown. The two `cxf_bridge.proto` copies SHALL be
byte-identical.

#### Scenario: Signed request carries the four string fields

- **GIVEN** an inbound SOAP request with a valid Body signature
- **WHEN** inbound WS-Security processing succeeds
- **THEN** the consumer request carries `signer` with RFC 2253 subject/issuer,
  lowercase-hex serial, and a 64-lowercase-hex-char SHA-256 fingerprint

#### Scenario: Oversized certificate is rejected, not truncated

- **GIVEN** a signer certificate whose DER encoding exceeds 64 KiB
- **WHEN** the descriptor is exported
- **THEN** it is rejected as a security failure and no truncated digest is used

#### Scenario: Unsigned request yields unknown signer

- **GIVEN** an inbound SOAP request with no signature
- **WHEN** the request is dispatched
- **THEN** the `signer` field is absent and no signer property is set

#### Scenario: Mirrored proto copies are byte-identical

- **GIVEN** the two `cxf_bridge.proto` copies
- **WHEN** `SignerIdentity` and `ConsumerRequest.signer` are added
- **THEN** the two files are byte-identical

### Requirement: An invalid signer descriptor is a non-WSS Receiver fault and is not dispatched

Rust SHALL validate the received descriptor: `subject` and `issuer` each at
most 4096 UTF-8 bytes, `serial` at most 128 bytes and lowercase hex only, and
`fingerprint_sha256` exactly 64 lowercase hex characters. A descriptor failing
any bound SHALL cause the consumer request to fail as a `Receiver` fault (not a
WSS fault; `security.fault_status` does not apply), SHALL NOT dispatch the
request to the route, and SHALL set no signer properties. The failure SHALL be
emitted to the bridge as a `soap:Server` response code mapping to HTTP 500; the
end-to-end HTTP 500 assertion is a native integration concern.

#### Scenario: Oversized subject is not dispatched

- **GIVEN** a descriptor whose `subject` exceeds 4096 UTF-8 bytes
- **WHEN** Rust validates it
- **THEN** the request fails as a non-WSS Receiver fault, is not dispatched,
  and no signer property is set

#### Scenario: Malformed or non-lowercase fingerprint is not dispatched

- **GIVEN** a descriptor whose `fingerprint_sha256` is not exactly 64 lowercase
  hex characters
- **WHEN** Rust validates it
- **THEN** the request fails as a non-WSS Receiver fault, is not dispatched,
  and no signer property is set

#### Scenario: Non-lowercase serial is not dispatched

- **GIVEN** a descriptor whose `serial` contains non-lowercase-hex characters
- **WHEN** Rust validates it
- **THEN** the request fails as a non-WSS Receiver fault, is not dispatched,
  and no signer property is set

### Requirement: Signer metadata is written to dedicated exchange properties only

Rust SHALL expose the signer descriptor **only** through Exchange properties
via `exchange.set_property` under the dedicated keys
`camel.cxf.signer.subject`, `camel.cxf.signer.issuer`,
`camel.cxf.signer.serial`, and `camel.cxf.signer.fingerprint_sha256`. The
component SHALL NOT call `store_principal_properties`, SHALL NOT construct or
write a `Principal`, and SHALL NOT write ANY `camel.auth.*` property; the
present implementation makes `principal_from_exchange` trust those keys, so
writing them would forge an authenticated principal. No `claims`, `roles`, or
`scopes` data SHALL be set. The metadata is intended signer identity
information, not authentication. A client-supplied message header with a
`camel.cxf.signer.*` name SHALL remain an ordinary message header and SHALL NOT
become an exchange property.

#### Scenario: Signer is stored as dedicated properties

- **GIVEN** a dispatched consumer request with a valid signer descriptor
- **WHEN** the Exchange is assembled
- **THEN** the four `camel.cxf.signer.*` properties are set and no `camel.auth.*`
  property is written

#### Scenario: No Principal is constructed

- **GIVEN** any signer descriptor
- **WHEN** the Exchange is assembled
- **THEN** `principal_from_exchange` reports no authenticated principal

#### Scenario: Spoofed signer header stays a header

- **GIVEN** a request that supplies a `camel.cxf.signer.subject` message header
- **WHEN** the Exchange is assembled
- **THEN** the value remains a message header and no signer property is set from
  it

### Requirement: Signer extraction is scoped to profiles requiring a Signature

The bridge SHALL inspect signatures and require Body coverage only when the
profile's inbound security actions include `Signature`. When they do not, an
unsigned request, or a request whose signatures do not cover Body, SHALL be
allowed to dispatch and proceed through normal WS-Security processing with no
signer set. When `Signature` is required, a request with no Body-covering
signature SHALL be rejected, and at most one distinct verified Body signer SHALL
be accepted.

Within a `Signature`-required profile, inbound processing SHALL examine at most
8 `SIGN` results. Among results that cover Body content, distinct certificate
fingerprints SHALL be considered distinct signers; more than one distinct
fingerprint SHALL reject the request as ambiguous. The same certificate
appearing in multiple results SHALL be accepted as one signer. Ancillary
signatures not covering Body MAY be validated for integrity but SHALL NOT yield
a signer.

#### Scenario: Profile without Signature action allows an unsigned request

- **GIVEN** a profile whose inbound actions exclude `Signature`
- **WHEN** an unsigned request arrives
- **THEN** it dispatches through normal WS-Security processing with no signer
  set

#### Scenario: Signature-required profile rejects no Body coverage

- **GIVEN** a profile whose inbound actions include `Signature` and a request
  signed only over the timestamp or only over a header
- **WHEN** inbound processing runs
- **THEN** the request fails with a security error and is not dispatched

#### Scenario: Same certificate in multiple results is one signer

- **GIVEN** a `Signature`-required profile and multiple `SIGN` results whose
  certificates share one fingerprint and cover Body
- **WHEN** inbound processing runs
- **THEN** the request is accepted with that single signer

#### Scenario: Distinct Body signers are rejected

- **GIVEN** a `Signature`-required profile and Body-covering `SIGN` results with
  two distinct fingerprints
- **WHEN** inbound processing runs
- **THEN** the request is rejected as ambiguous

#### Scenario: Non-Body signature cannot yield a signer

- **GIVEN** a signature covering only a header plus a Body-covering signature
- **WHEN** inbound processing runs
- **THEN** only the Body-covering certificate yields the signer

### Requirement: Body signature coverage is a breaking change for Signature-required profiles

Requiring Body coverage under a `Signature`-required profile SHALL be an
intended breaking change: timestamp-only and header-only signers that previously
passed such profiles SHALL now be rejected. Profiles that do not require
`Signature` SHALL be unaffected.

#### Scenario: Previously passing timestamp-only client is rejected

- **GIVEN** a `Signature`-required profile and a client signing only the
  timestamp
- **WHEN** it calls the listener
- **THEN** the request is rejected as a security failure

