## ADDED Requirements

### Requirement: Pin signing keys through a deployment truststore

The runtime SHALL accept a deployment-owned truststore supplied by the
sanctioned artifact argument `--truststore <path>` or the
`CAMEL_TRUSTSTORE` environment variable; the argument SHALL take
precedence when both are present. The truststore SHALL be a single
UTF-8 text file; each non-empty, non-comment line SHALL pin exactly one
verifying key as `blake3:` + 64 lowercase hex characters — the manifest
`key_fingerprint` form — optionally followed by whitespace and a
decimal unsigned 64-bit freshness floor. Blank lines and lines starting
with `#` SHALL be ignored. A truststore that cannot be read or is not
UTF-8 SHALL exit 2 naming the path; a malformed line or a duplicate pin
SHALL exit 2 naming the path and the line (step `truststore-parse`). An
empty truststore — zero bytes or only blank and comment lines — SHALL
be valid and pin nothing, so every signed artifact fails its pin check.
When a truststore is supplied and the artifact
carries a present envelope, the manifest fingerprint SHALL be a member
of the pin set; otherwise the artifact SHALL exit 2 with a
`truststore-pin` diagnostic naming the fingerprint. When a truststore
is supplied and the manifest carries a signing block (schema 4 or 5)
with an absent envelope, the artifact SHALL exit 2 with a
`truststore-pin` diagnostic naming the manifest fingerprint: deleting
the envelope must not convert a signed artifact into an unsigned one
under pin policy. The truststore SHALL
hold public fingerprints and floors only: no key material SHALL enter
the truststore or any log. The truststore SHALL NOT apply to unsigned
artifacts: an unsigned artifact under a supplied truststore SHALL behave
exactly as with no truststore. With no truststore supplied, boot and
`--verify` SHALL behave exactly as the R4 self-contained chain. The
compile-side clean-environment guard SHALL treat `CAMEL_TRUSTSTORE` as
benign: the variable is a verify-side input, is never read at compile
time, and its presence SHALL NOT reject a compile.

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

- **GIVEN** an unsigned artifact (no envelope) with a truststore supplied
- **WHEN** the artifact starts
- **THEN** it boots unchanged, because the truststore does not apply to unsigned artifacts

#### Scenario: Truststore argument surface rules

- **GIVEN** a valid artifact
- **WHEN** the operator runs `./app --truststore` with a missing value, or `--truststore <path> --manifest`,
  `--truststore <path> --help`, or `--truststore <path> --version`
- **THEN** it exits 2 with the argument-surface diagnostic, because `--truststore` is a modifier, not an exclusive mode

#### Scenario: Environment truststore supplies the store

- **GIVEN** a signed artifact and `CAMEL_TRUSTSTORE` pointing at a truststore pinning its key
- **WHEN** the artifact starts without `--truststore`
- **THEN** the pin check applies; and when both the argument and the variable are present, the argument's path wins

### Requirement: Reject rolled-back artifacts by freshness floor

A signed compile SHALL emit manifest schema 5: the schema-4 signing block
plus a mandatory `freshness` marker encoded as an unsigned 64-bit
unix-seconds integer inside the signing block. The marker SHALL live in
the manifest bytes, which the envelope signature covers, so an attacker
cannot change it without breaking the signature. Readers SHALL accept
manifest schemas 2, 3, 4, and 5; a schema-5 manifest without a valid
freshness marker SHALL be rejected. When a truststore pins the
artifact's key, the truststore SHALL record the highest previously
accepted marker as that key's floor: a marker below the floor SHALL
exit 2 with a `freshness-rollback` diagnostic; a marker at or above the
floor SHALL pass. The first sight of a pinned key with no recorded
floor SHALL pass and record the marker as the floor. Rollback detection
SHALL be bounded by the marker's one-second resolution: artifacts
compiled in the same second as the floor marker are not orderable by
the marker. A schema-4 signed artifact pinned in the truststore SHALL
pass the pin check with no freshness check while no floor is recorded
for its key; once a floor is recorded, a schema-4 artifact of that key
SHALL exit 2 with a `freshness-rollback` diagnostic — the first
schema-5 boot of a key is the migration boundary, and it does not
re-open. Both boot and `--verify` SHALL apply the rollback decision on
the floors read at decision time. Only boot SHALL record floors, inside
one lock-serialized critical section: an exclusive advisory lock on
`<truststore>.lock`, a re-read of the floor under the lock, the
decision, and on accept a write of max(current floor, marker) via
atomic replace; a concurrent boot SHALL NOT accept on a stale floor or
lower a recorded floor. `--verify` SHALL be a dry run: it reads floors
without locking and never writes the truststore. A boot inside the
freshness critical section that cannot acquire the lock, or that must
record a floor and cannot write the truststore, SHALL exit 2 with a
`truststore-update` diagnostic. Manifest schema 5
SHALL pair with store schema 2 exactly as schemas 3 and 4 do.

#### Scenario: Signed compile emits the freshness marker

- **GIVEN** a route compiled with `--sign` and a valid signing key
- **WHEN** the operator runs `./app --manifest`
- **THEN** the manifest reports schema 5 and the signing block carries algorithm name, key fingerprint, required bit,
  and a unix-seconds freshness marker, with no key material in any output

#### Scenario: Rollback below the floor fails closed

- **GIVEN** a pinned key whose truststore floor records marker F, and a signed artifact from the same key whose marker
  is below F
- **WHEN** the artifact starts with the truststore supplied
- **THEN** it exits 2 with a `freshness-rollback` diagnostic and does not boot

#### Scenario: First sight records the floor

- **GIVEN** a pinned key with no recorded floor and a signed artifact with marker M
- **WHEN** the artifact boots with the truststore supplied
- **THEN** it boots and the truststore records the floor M for that key

#### Scenario: The floor never decreases

- **GIVEN** a truststore whose floor for the pinned key is F and a booting artifact with marker M where M is at or above
  F
- **WHEN** the artifact boots
- **THEN** the recorded floor becomes max(F, M) via an atomic replace, and a later boot of an artifact with a marker
  below that floor exits 2

#### Scenario: Concurrent boots serialize on the truststore lock

- **GIVEN** two pinned artifacts of the same key with markers 100 and 200 booting concurrently against the same
  truststore
- **WHEN** both boots complete
- **THEN** the recorded floor is 200 and the boot at marker 100 did not pass on a stale floor: the freshness decision
  and the floor write share one advisory-lock critical section on `<truststore>.lock`

#### Scenario: Verify is a dry run

- **GIVEN** a signed artifact with marker M above the recorded floor
- **WHEN** the operator runs `./app --verify --truststore <path>`
- **THEN** it exits 0 and the truststore file is byte-identical afterwards

#### Scenario: Unwritable truststore fails closed

- **GIVEN** a boot that must record a floor and a truststore whose lock file cannot be created or whose file cannot be
  written
- **WHEN** the artifact starts
- **THEN** it exits 2 with a `truststore-update` diagnostic and does not boot, rather than degrading to no freshness
  enforcement

#### Scenario: Schema-4 artifacts pass only before a floor exists

- **GIVEN** a pre-keypin schema-4 signed artifact whose key is pinned in the truststore
- **WHEN** the artifact starts with no floor recorded for that key
- **THEN** the pin check passes, no freshness check applies, and the artifact boots
- **WHEN** a schema-5 artifact of the same key boots and records a floor, and the schema-4 artifact starts again
- **THEN** it exits 2 with a `freshness-rollback` diagnostic and does not boot

#### Scenario: Freshness without a truststore does not apply

- **GIVEN** a schema-5 signed artifact verified with no truststore supplied
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** the R4 chain alone decides the verdict, and no floor is read or recorded

## MODIFIED Requirements

### Requirement: Sign artifacts with a detached outer envelope

The compiler SHALL offer `--sign`, taking a 32-byte ed25519 seed file supplied by `--signing-key <path>` or the
namespaced `CAMEL_COMPILE_SIGNING_KEY` environment variable (argument takes precedence). The key SHALL be used only as a
signing input: no key material SHALL enter the artifact, envelope, manifest, output, or log, and embedded content SHALL
stay unchanged by signing. `--require-signature` SHALL require `--sign`. A signed compile SHALL emit a detached envelope
at `<artifact>.sig` with fixed framing `CAMELTR1`-family style: leading `CAMELSG1` magic, little-endian `u16` envelope
version, `u8` algorithm, zero `u8` flags, the 32-byte ed25519 public key, the 64-byte signature, a BLAKE3 checksum over
a domain-separated encoding of the envelope fields, and terminal `CAMELSG1` magic. The signature SHALL be Ed25519ph (RFC
8032 prehash, null context) over the complete final artifact bytes — executable copy and trailer — so sign and verify
stream with flat memory, and envelope bytes SHALL be deterministic for the same key and artifact bytes. The signed
artifact's manifest SHALL use schema 5 and record exactly the algorithm name, the BLAKE3 fingerprint of the public key
(`blake3:` + hex), the required bit, and the mandatory unix-seconds freshness marker; unsigned compiles SHALL stay
schema 3 and byte-identical. Envelope-write failure SHALL remove the artifact and fail compilation. At boot, an artifact
with a present envelope SHALL verify before boot: envelope parse, algorithm match, fingerprint binding to the manifest
signing block, and signature over the final artifact bytes; any failure SHALL exit 2 with a named diagnostic. An absent
envelope SHALL exit 2 when the manifest required bit is set and SHALL boot unchanged otherwise. A present envelope with
a schema-3-or-earlier manifest SHALL exit 2 as an unpaired envelope. `--verify` SHALL run the same chain without
booting, SHALL stay exclusive with every other artifact mode flag (the `--truststore` modifier MAY pair with it), SHALL
exit 0 naming the algorithm and fingerprint on
success, and SHALL exit 2 with a named diagnostic otherwise. When a truststore is supplied, the chain SHALL continue
with the pin
check and, for a schema-5 manifest, the freshness check, per the
truststore and freshness requirements; those requirements also govern
signed manifests with an absent envelope when a truststore is supplied.

#### Scenario: Signing emits a verified envelope round trip

- **GIVEN** a route compiled with `--sign` and a valid `--signing-key` seed file
- **WHEN** the artifact starts, and separately runs `./app --verify`
- **THEN** the artifact boots with the valid envelope beside it, and `--verify` exits 0 naming algorithm `ed25519ph` and
  the key fingerprint recorded in the manifest

#### Scenario: Signature covers the final artifact bytes

- **GIVEN** a signed artifact with one byte flipped anywhere in the file, whether in the executable body or the trailer
  span
- **WHEN** the artifact starts, or runs `./app --verify`
- **THEN** it exits 2 with an integrity or signature diagnostic and does not boot

#### Scenario: Wrong key fails closed

- **GIVEN** a signed artifact whose envelope is re-created under a different key while the artifact bytes stay untouched
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a fingerprint-mismatch diagnostic

#### Scenario: Tampered envelope fails closed

- **GIVEN** a signed artifact whose `.sig` envelope bytes are corrupted in structure or checksum
- **WHEN** the artifact starts
- **THEN** it exits 2 with an envelope diagnostic distinguishable from a signature failure

#### Scenario: Required signature without envelope fails closed

- **GIVEN** an artifact compiled with `--sign --require-signature` whose `.sig` file is removed
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a diagnostic naming the missing required signature

#### Scenario: Unsigned artifact with stray envelope fails closed

- **GIVEN** an unsigned artifact (manifest schema 3) with any `.sig` file beside it
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with an unpaired-envelope diagnostic

#### Scenario: Unsigned artifacts still run

- **GIVEN** an artifact compiled without `--sign` and no `.sig` file beside it
- **WHEN** the artifact starts
- **THEN** it boots unchanged with no signature verification step, preserving v1 compatibility

#### Scenario: Manifest records fingerprint only

- **GIVEN** a signed artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the signing block lists the algorithm name, key fingerprint, required bit, and freshness marker, and no key
  material appears in any output or log

#### Scenario: Envelope framing is deterministic

- **GIVEN** the same 32-byte seed and byte-identical artifact input
- **WHEN** the envelope is encoded twice
- **THEN** the `.sig` bytes are identical across runs

#### Scenario: Signing-key input is validated

- **GIVEN** a compile with `--sign` but no key source, or `--signing-key` without `--sign`, or a stray
  `CAMEL_COMPILE_SIGNING_KEY` without `--sign`, or a key file whose size is not exactly 32 bytes, or
  `--require-signature` without `--sign`
- **WHEN** the operator compiles
- **THEN** compilation exits 2 with a diagnostic naming the broken rule, and no artifact or `.sig` file is left behind

#### Scenario: --verify stays exclusive

- **GIVEN** a valid signed artifact
- **WHEN** the operator runs `./app --verify --manifest`
- **THEN** it exits 2 with a duplicate-exclusive diagnostic without booting or verifying

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept only `--report <path>`, `--help`, `--version`, `--manifest`, and `--verify` as exclusive
modes, plus the sanctioned modifier `--truststore <path>` which pairs with a boot or with `--verify` and SHALL be
rejected alongside `--help`, `--version`, or `--manifest`; `--verify` SHALL stay exclusive of every other artifact mode
flag. Duplicate exclusive flags, missing report or truststore values, positional arguments, and other arguments SHALL
exit 2. The operational manifest SHALL contain a separate `manifest_schema` field, an `embedded_files` list with
canonical logical paths, entry kinds, asset classes, a secret-material `class` per asset entry (`"public"` | `"secret"`
— documents are implicitly public; `secret` is exactly the private-key family), byte lengths, and BLAKE3 content
digests, a `total_embedded_bytes` aggregate over all content entries, and a top-level `artifact_kind` (`"job"` |
`"server"` — the trailer route kind maps to `server`, job to `job`). A schema-4 manifest SHALL carry a signing block
with exactly the algorithm name, the key fingerprint, and the required bit; a schema-5 manifest SHALL carry that signing
block plus the freshness marker; no signing block SHALL appear in a schema-3 manifest; no key material SHALL appear in
any manifest, output, or log. Entries for secret-class assets SHALL expose only their class, byte length, and BLAKE3
digest: their logical path SHALL be withheld from the manifest body and `--manifest` output, and no key material SHALL
appear in any output or log. The `artifact_kind` field is the sealed R3 tripwire (bd `rc-p823t`): no long-lived manifest
entries ship before the R5 TLS decision exists, and any artifact that can outlive a short bounded run triggers that
decision; the optional compile-side warning for server-type consumers in job artifacts is NOT taken in R2. Manifest
schema values SHALL be validated independently from trailer version; readers SHALL accept manifest schemas 2, 3, 4, and
5 and reject others, and SHALL enforce the pairing rule that manifest schemas 3, 4, and 5 require store schema 2 and
store schema 2 requires manifest schema 3, 4, or 5. `--manifest` SHALL print this metadata without booting. The manifest
SHALL not contain compile-time environment values and SHALL list required environment variables without defaults.
Listener declarations SHALL report the artifact kind's effective runtime listeners: job artifacts SHALL omit listeners
the job boot projection suppresses, and route artifacts SHALL list all configuration-declared listeners.

#### Scenario: Manifest inspection

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the command exits 0 without booting and prints manifest schema, artifact kind, logical embedded-file
  metadata, components, required environment names, and listener declarations

#### Scenario: Manifest reports asset digests and total size

- **GIVEN** a valid artifact embedding documents and assets
- **WHEN** the operator runs `./app --manifest`
- **THEN** each asset entry lists its logical path, asset class, secret-material `class`, byte length, and BLAKE3
  digest, and the output includes `total_embedded_bytes` equal to the sum of all content-entry lengths

#### Scenario: Manifest records artifact kind

- **GIVEN** one compiled job artifact and one compiled route artifact
- **WHEN** the operator runs `./app --manifest` on each
- **THEN** `artifact_kind` is `job` for the job artifact and `server` for the route artifact, without booting

#### Scenario: Secret manifest entries expose digest and length only

- **GIVEN** an artifact compiled with `--embed-secrets` embedding a private key
- **WHEN** the operator runs `./app --manifest`
- **THEN** the secret entry lists class `secret`, its byte length, and its BLAKE3 digest, the logical path is withheld,
  and no key material appears anywhere in the output

#### Scenario: Unknown manifest schema fails closed

- **GIVEN** a marked artifact with a manifest schema that the reader does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot and does not reinterpret trailer version as manifest schema

#### Scenario: Unknown artifact argument

- **GIVEN** a valid artifact
- **WHEN** the operator supplies an unknown, positional, duplicate-exclusive, or incomplete report argument
- **THEN** it exits 2, names the rejected argument, and does not boot

#### Scenario: Manifest includes operational version

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** output includes runtime version, artifact kind, embedded files, required environment names, and listener
  declarations without booting

#### Scenario: Job artifact manifest omits suppressed listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list is empty of the suppressed health and Prometheus endpoints, matching the job
  boot projection's effective runtime

#### Scenario: Route artifact manifest keeps config-declared listeners

- **GIVEN** a compiled route artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list contains both endpoints, because route artifacts bind them at runtime
