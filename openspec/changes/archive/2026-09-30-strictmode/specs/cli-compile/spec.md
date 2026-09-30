# cli-compile delta

## MODIFIED Requirements

### Requirement: Pin signing keys through a deployment truststore

The runtime SHALL accept a deployment-owned truststore supplied by the
sanctioned artifact argument `--truststore <path>` or the
`CAMEL_TRUSTSTORE` environment variable; the argument SHALL take
precedence when both are present. The truststore SHALL be a single
UTF-8 text file; each non-empty, non-comment line SHALL pin exactly one
verifying key as `blake3:` + 64 lowercase hex characters — the manifest
`key_fingerprint` form — optionally followed by whitespace and a
decimal unsigned 64-bit freshness floor, or SHALL carry exactly the
bare directive token `strict`, whose semantics the requirement
"Reject unsigned artifacts under a strict truststore" governs. Blank
lines and lines starting with `#` SHALL be ignored. A truststore that
cannot be read or is not UTF-8 SHALL exit 2 naming the path; a
malformed line or a duplicate pin SHALL exit 2 naming the path and the
line (step `truststore-parse`). An empty truststore — zero bytes or
only blank and comment lines — SHALL be valid and pin nothing, so
every signed artifact fails its pin check. When a truststore is
supplied and the artifact carries a present envelope, the manifest
fingerprint SHALL be a member of the pin set; otherwise the artifact
SHALL exit 2 with a `truststore-pin` diagnostic naming the fingerprint.
When a truststore is supplied and the manifest carries a signing block
(schema 4 or 5) with an absent envelope, the artifact SHALL exit 2
with a `truststore-pin` diagnostic naming the manifest fingerprint:
deleting the envelope must not convert a signed artifact into an
unsigned one under pin policy. The truststore SHALL hold public
fingerprints, floors, and the strict directive only: no key material
SHALL enter the truststore or any log. The truststore SHALL NOT apply
to unsigned artifacts except to read the strict directive: an unsigned
artifact under a supplied well-formed truststore that does not carry
the strict directive SHALL behave exactly as with no truststore, and
the strict-directive exception is governed by the requirement "Reject
unsigned artifacts under a strict truststore". With no truststore
supplied, boot and `--verify` SHALL behave exactly as the R4
self-contained chain. The compile-side clean-environment guard SHALL
treat `CAMEL_TRUSTSTORE` as benign: the variable is a verify-side
input, is never read at compile time, and its presence SHALL NOT
reject a compile.

#### Scenario: Pinned key verifies under a truststore

- **GIVEN** a signed artifact and a truststore file pinning its manifest key fingerprint
- **WHEN** the artifact starts, and separately runs `./app --verify --truststore <path>`
- **THEN** the artifact boots after the pin check passes, and `--verify` exits 0

#### Scenario: Unpinned key fails closed

- **GIVEN** a signed artifact whose manifest fingerprint is not a member of the supplied truststore's pin set
- **WHEN** the artifact starts, or runs `./app --verify` with the truststore supplied
- **THEN** it exits 2 with a `truststore-pin` diagnostic naming the fingerprint and does not boot

#### Scenario: Malformed truststore fails closed

- **GIVEN** a truststore file that is unreadable or not UTF-8
- **WHEN** the artifact starts with the truststore supplied
- **THEN** it exits 2 with a `truststore-parse` diagnostic naming the path, and does not boot

#### Scenario: Malformed truststore line names the line

- **GIVEN** a readable UTF-8 truststore containing a malformed pin line or a duplicate pin
- **WHEN** the artifact starts with the truststore supplied
- **THEN** it exits 2 with a `truststore-parse` diagnostic naming the path and the offending line number, and does not
  boot

#### Scenario: Empty truststore pins nothing

- **GIVEN** a truststore file that is zero bytes or contains only comments and blank lines
- **WHEN** a signed artifact starts with it supplied
- **THEN** it exits 2 with a `truststore-pin` diagnostic, because no key is pinned

#### Scenario: Stripped envelope fails closed under a truststore

- **GIVEN** an artifact compiled with `--sign` (required bit not set) whose `.sig` file is removed, and a supplied
  truststore
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** both exit 2 with a `truststore-pin` diagnostic naming the manifest fingerprint, and the artifact does not
  boot

#### Scenario: No truststore leaves the chain unchanged

- **GIVEN** a signed artifact verified with no `--truststore` argument and no `CAMEL_TRUSTSTORE` variable
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** boot and verify behave exactly as the R4 chain with no pin step and no freshness step

#### Scenario: Unsigned artifacts ignore the truststore

- **GIVEN** an unsigned artifact (no envelope) with a well-formed truststore supplied that does not carry the strict
  directive
- **WHEN** the artifact starts
- **THEN** it boots unchanged, because a non-strict truststore does not apply to unsigned artifacts

#### Scenario: Truststore argument surface rules

- **GIVEN** a valid artifact
- **WHEN** the operator runs `./app --truststore` with a missing value, or `--truststore <path> --manifest`,
  `--truststore <path> --help`, or `--truststore <path> --version`
- **THEN** it exits 2 with the argument-surface diagnostic, because `--truststore` is a modifier, not an exclusive mode

#### Scenario: Environment truststore supplies the store

- **GIVEN** a signed artifact and `CAMEL_TRUSTSTORE` pointing at a truststore pinning its key
- **WHEN** the artifact starts without `--truststore`
- **THEN** the pin check applies; and when both the argument and the variable are present, the argument's path wins

## ADDED Requirements

### Requirement: Reject unsigned artifacts under a strict truststore

A truststore line whose whitespace-separated token set is exactly the
single token `strict` (leading/trailing whitespace tolerated as for
pin lines; lowercase, case-sensitive; no pin, no floor, no second
token) SHALL set the store's strict directive. Repeated `strict`
lines SHALL be idempotent. A `strict` line with any second token
SHALL be a malformed line (step `truststore-parse`), and `strict`
SHALL NOT be accepted as a pin fingerprint. When a supplied
truststore carries the strict directive and the manifest carries no
signing block (schema 1, 2, or 3 — any manifest without a signing
block), every trust-policy surface SHALL exit 2 with a `strict-unsigned` diagnostic naming the
artifact and the truststore path, and the artifact SHALL NOT boot:
boot and `--verify` under either store source (the `--truststore`
argument or the `CAMEL_TRUSTSTORE` variable), and the boot-side
dispatch of `--manifest`, `--help`, and `--version` under the
`CAMEL_TRUSTSTORE` variable — the `--truststore` argument stays
argument-surface-rejected for those modes. The strict directive SHALL
require a signing block and change nothing else: pin and freshness
decisions for signed artifacts SHALL stay exactly as under the same
store without the directive. The strict directive SHALL NOT change
the diagnosis of a signed manifest whose envelope is absent: that
case keeps the `truststore-pin` strip-rule diagnostic. To decide
strictness, the runtime SHALL parse the supplied store for unsigned
artifacts too: a supplied store that cannot be read or parsed SHALL
fail closed for an unsigned artifact with the `truststore-parse`
diagnostic, never silently boot past an unreadable strict policy.
With no truststore supplied, the strict directive cannot apply and
unsigned artifacts boot exactly as the R4 chain. Unsigned compiles
SHALL stay schema 3 and byte-identical; the strict directive is
verify-side policy only and the compile side SHALL NOT read the
truststore.

#### Scenario: Strict truststore rejects an unsigned artifact at boot

- **GIVEN** an unsigned artifact (manifest schema 3, no envelope) and a supplied truststore whose lines are a valid pin
  and the bare directive `strict`
- **WHEN** the artifact starts
- **THEN** it exits 2 with a `strict-unsigned` diagnostic naming the artifact and the truststore path, and does not boot

#### Scenario: Strict truststore rejects an unsigned artifact under verify

- **GIVEN** the same unsigned artifact and strict truststore
- **WHEN** the operator runs `./app --verify --truststore <path>`
- **THEN** it exits 2 with a `strict-unsigned` diagnostic and prints no algorithm or fingerprint line

#### Scenario: Strict directive is idempotent and isolated

- **GIVEN** a truststore containing two `strict` lines, a comment, a blank line, and one valid pin
- **WHEN** the store is parsed
- **THEN** it parses valid with the strict directive set and exactly one pin entry

#### Scenario: Whitespace around the directive is tolerated as for pins

- **GIVEN** a truststore containing the line `␣strict␣` (leading and trailing whitespace)
- **WHEN** the store is parsed
- **THEN** it parses valid with the strict directive set, because the line tokenizes to exactly one `strict` token

#### Scenario: Strict with a second token stays malformed

- **GIVEN** a truststore containing the line `strict 42`
- **WHEN** the store is parsed
- **THEN** it exits 2 with a `truststore-parse` diagnostic naming the path and the line

#### Scenario: Malformed store fails closed for unsigned artifacts

- **GIVEN** an unsigned artifact and a supplied truststore that cannot be read or is not UTF-8
- **WHEN** the artifact starts
- **THEN** it exits 2 with a `truststore-parse` diagnostic naming the path, and does not boot — an unreadable strict
  policy is never silently skipped

#### Scenario: Strict applies to every manifest without a signing block

- **GIVEN** an artifact whose manifest carries no signing block, whatever its schema number, and a strict truststore
  supplied
- **WHEN** the artifact starts
- **THEN** the strict decision keys on the absence of the signing block, not on the schema number, and it exits 2 with
  a `strict-unsigned` diagnostic

#### Scenario: Strip rule keeps precedence under a strict store

- **GIVEN** an artifact compiled with `--sign` whose `.sig` file is removed, and a supplied strict truststore pinning
  its key
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** it exits 2 with the `truststore-pin` strip-rule diagnostic naming the manifest fingerprint, not the
  `strict-unsigned` diagnostic

#### Scenario: Strict store leaves signed pinned artifacts bootable

- **GIVEN** a signed schema-5 artifact whose key is pinned with a satisfied freshness floor, and the strict truststore
  carrying that pin
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** the pin and freshness steps decide exactly as under the same store without the directive: strict mode adds
  the unsigned rejection and nothing else

#### Scenario: Environment strict store governs the boot-side dispatch modes

- **GIVEN** an unsigned artifact (no signing block) and `CAMEL_TRUSTSTORE` pointing at a strict truststore
- **WHEN** the operator runs `./app --manifest`, or `./app --help`, or `./app --version`
- **THEN** the strict rejection fires exactly as at boot: exit 2 with a `strict-unsigned` diagnostic; and running
  `./app --truststore <path> --manifest` instead exits 2 with the argument-surface diagnostic, because the
  `--truststore` modifier does not pair with those modes
