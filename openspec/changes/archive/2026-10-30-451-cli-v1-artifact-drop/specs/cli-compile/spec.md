## MODIFIED Requirements

### Requirement: Compile a native single-document artifact

The CLI SHALL provide `camel compile <document> -o <artifact>` that resolves one logical route or job entry point and appends `CAMELTR1 || content || index || manifest || footer` to a copy of the current executable, preserving executable permissions. The 76-byte v2 footer contains `CAMELTR1`, little-endian `u16` version 2, `u8` kind (`1=route`, `2=job`), zero `u8` flags, little-endian `u64` content/index/manifest lengths, BLAKE3, and terminal `CAMELTR1`. The checksum SHALL cover ASCII `rust-camel-trailer-v2`, one zero byte, the encoded version/kind/three lengths, content, index, and manifest, excluding magic and flags. Normalization SHALL accept valid UTF-8, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and reject invalid UTF-8; normalization SHALL apply to document entries only, and asset entries SHALL embed verbatim. The content SHALL contain normalized pre-interpolation route, job, `Camel.toml`, include, and selected-profile entries plus verbatim deploy-time asset entries. The index SHALL be canonical UTF-8 JSON with independent `store_schema: 2`, typed entries, offsets, lengths, one logical entry point, configuration references, an ordered source plan, and the compile-time substitution table recording each (document, declared string) pair's exact byte span(s) in the normalized document or config entry together with its asset logical path. The manifest SHALL use independent `manifest_schema: 3` for unsigned artifacts and `manifest_schema: 4` for artifacts signed under the detached-envelope requirement, and `store_schema: 2` SHALL pair with either manifest schema: The reader SHALL accept exactly trailer framing version 2 with `store_schema: 2` and `manifest_schema` 3 (unsigned), 4 (signed), or 5 (signed freshness); readers SHALL reject a store-2 index with a manifest other than 3, 4, or 5, a manifest-3, manifest-4, or manifest-5 body with a non-2 store, and a manifest-4 or manifest-5 body without a signing block. The compiler SHALL resolve route files, includes, profiles, supported job route sources, and deploy-time asset references at compile time using the explicitly supplied `--config <Camel.toml>`, repeated `--profile <name>`, and `--embed-secrets` options; without `--config`, it SHALL embed no configuration and select no profile, and it SHALL never discover ambient configuration. TLS-class asset references SHALL resolve embedded-only, with no host-filesystem fallback at compile time or runtime (sealed bd `rc-p823t`); each TLS-class reference resolves to an embedded virtual-store entry or compilation fails, leaving the reference syntax free for R5 `host:`/`env:` prefixes. The store SHALL preserve logical relative paths and the ordered source plan. Aggregate embedded bytes — normalized document bytes plus verbatim asset bytes — SHALL remain limited to the compile-time cap selected with `--max-payload-bytes <bytes>`, a positive value whose default SHALL be 16 MiB; exceeding the cap SHALL fail compilation with a diagnostic naming the total and the cap before any output is created. Embedding a secret-family asset — exactly the private-key class: the `key` document field and the `tlsKey`, `serverKeyPath`, and `clientKeyPath` TLS URI parameters — SHALL require explicit opt-in: compilation SHALL fail closed with a diagnostic naming the field and the secret class unless the operator passes `--embed-secrets`. An artifact compiled with the opt-in that contains a secret-family entry SHALL be written with file mode 0700; artifacts without embedded secret-family entries SHALL be written with mode 0755.

<!-- openspec-scenario-ops
dropped: Single-document v1 remains readable | v1 read path removed pre-1.0; a marked v1 artifact is a tombstone naming version 1 and the recompile remedy (ADR-0090 "A removal carries a tombstone", shedding-plan row 10 bd rc-v6wiz, bd rc-n6dof)
-->

#### Scenario: Compile a supported document

- **GIVEN** a valid route or job document and the native Linux target
- **WHEN** the operator runs `camel compile <document> -o <artifact>`
- **THEN** the command exits 0 and the artifact boots as the compiled document

#### Scenario: Trailer framing is deterministic

- **GIVEN** the same normalized store content, index, manifest, and artifact kind
- **WHEN** the trailer codec encodes the v2 artifact
- **THEN** the appended `CAMELTR1 || content || index || manifest || footer` byte sequence, including the 76-byte footer fields and checksum, is identical across runs

#### Scenario: Marked trailer corruption fails closed

- **GIVEN** an artifact whose final 8 bytes are `CAMELTR1` but whose preceding v2 footer fields, bounds, or checksum are invalid
- **WHEN** the artifact starts
- **THEN** it exits 2 with a format diagnostic and never boots or falls back to the ordinary CLI path

#### Scenario: Unmarked truncation follows normal fallback

- **GIVEN** truncation removes the terminal trailer magic so the executable has no recognizable trailer
- **WHEN** the executable starts
- **THEN** it follows the normal CLI path because trailer presence cannot be distinguished from an ordinary executable

#### Scenario: Compile a route with ordered route files

- **GIVEN** one route entry whose declared route-file patterns resolve to multiple files under its selected `Camel.toml` root
- **WHEN** the operator runs `camel compile <document> -o <artifact>` on the native Linux target
- **THEN** the artifact contains one logical entry, every resolved normalized document, canonical logical paths, and a source plan that preserves pattern order and sorts each pattern's matches deterministically

#### Scenario: Configuration is embedded with the document set

- **GIVEN** a route whose selected `Camel.toml` uses ordered includes and profiles
- **WHEN** the operator compiles the route
- **THEN** the artifact contains typed configuration, include, and profile entries plus index references sufficient to build `CamelConfig` in memory without ambient files

#### Scenario: Compile preserves document boundaries

- **GIVEN** two route documents with distinct logical relative paths and source provenance
- **WHEN** the compiler builds the virtual store
- **THEN** the index identifies each document separately, runtime can request each by logical path, and only the substitution table's recorded byte spans are rewritten — the document text is otherwise untouched

#### Scenario: Reject unsupported target compilation

- **GIVEN** a requested target different from the native Linux target
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 with a native-Linux-only diagnostic and does not create an artifact

#### Scenario: Aggregate cap follows the compile-time knob

- **GIVEN** a document set with assets whose embedded aggregate exceeds 16 MiB but not 32 MiB
- **WHEN** the operator compiles with `--max-payload-bytes 33554432`
- **THEN** compilation succeeds, and the same input compiled without the option exits 2 naming the aggregate total and the 16 MiB default cap

#### Scenario: Secret-class artifact is written 0700 under opt-in

- **GIVEN** a route whose TLS block embeds a private-key asset
- **WHEN** the operator compiles it with `--embed-secrets`
- **THEN** compilation exits 0, the artifact file mode is 0700, and a route without a private-key asset compiles to mode 0755 with or without the flag

#### Scenario: Secret embedding without opt-in fails closed

- **GIVEN** a route whose TLS block references a private-key asset
- **WHEN** the operator compiles it without `--embed-secrets`
- **THEN** compilation exits 2 with a diagnostic naming the field and the secret class, and no artifact is written

#### Scenario: Schema pairing is enforced

- **GIVEN** an artifact whose declared store and manifest schemas are not a current pair
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with the first failing validation's own diagnostic: a manifest body whose schema is not 3, 4, or 5 — including the removed schemas 1 and 2 — fails the manifest-schema tombstone naming the schema and the recompile remedy; a store index whose schema is not 2, beside a current manifest, fails the store-schema tombstone naming the store schema and the remedy; a manifest-4 or manifest-5 body without its signing block fails the manifest-field rules; the explicit pairing check remains as defense in depth behind those validators

#### Scenario: Unsupported artifact version is a tombstone

- **GIVEN** a marked artifact carrying the removed version-1 trailer framing or any other unsupported trailer version
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a payload-free tombstone error naming the declared artifact version and the remedy to recompile from source with this version (`camel compile`), never booting, never adapting to a one-entry store, and never falling through to the ordinary CLI path

#### Scenario: Legacy store-1/manifest-2 pairing is a tombstone

- **GIVEN** a marked v2-framed artifact whose index declares the removed `store_schema: 1` beside a `manifest_schema: 2` body
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a tombstone naming the removed manifest schema 2 and the remedy to recompile from source with this version, because manifest validation precedes store decoding; a removed `store_schema: 1` beside a current manifest fails its own store-schema tombstone the same way
### Requirement: Verify trailer integrity and version

The artifact reader SHALL distinguish an absent trailer from a malformed trailer, enforce footer bounds, reject unsupported format versions, and verify the mandatory BLAKE3 checksum before runtime boot. A rejected unsupported version — including the removed version-1 framing — SHALL surface a payload-free tombstone error naming the declared version and the remedy to recompile from source with this version; the artifact SHALL NOT be treated as a trailer-free executable and SHALL NOT fall back to ordinary CLI behavior.

#### Scenario: Corrupted artifact fails closed

- **GIVEN** an artifact with a recognizable trailer whose payload, manifest, footer, or checksum was truncated or modified
- **WHEN** the artifact starts
- **THEN** it exits with a clear integrity diagnostic and does not partially boot

#### Scenario: Trailer-free executable falls through

- **GIVEN** the normal Camel executable has no recognizable trailer magic
- **WHEN** the executable starts with a normal CLI command
- **THEN** it invokes the existing Clap command path with unchanged behavior

#### Scenario: Unsupported version fails closed with a tombstone

- **GIVEN** a marked artifact whose footer declares a format version the reader does not support, including a genuine version-1 trailer
- **WHEN** the artifact starts
- **THEN** it exits 2 with a tombstone naming the declared version and the recompile remedy, and the same rejection results whether the whole image or only the bounded probe tail is decoded
### Requirement: Run embedded documents without extraction

The artifact SHALL feed the indexed documents through the existing parse, runtime interpolation, lowering, boot, and start paths using an in-memory virtual store. Runtime SHALL build configuration from embedded `Camel.toml`, include, and selected-profile entries. Embedded job configuration SHALL pass through the same job boot projection as argv jobs before context configuration; embedded route configuration SHALL boot unchanged. Runtime SHALL perform no source-tree reads, glob expansion, ambient `Camel.toml` or profile loading, canonicalization of declared asset strings, temporary extraction, watch, or hot reload. The only permitted filesystem writes are an explicitly requested report and the confined per-boot materialization of asset entries consumed by documented legacy path-reading consumers. Source diagnostics SHALL use `compiled://<logical-path>` identities. `${env:NAME}` expressions SHALL resolve from the deployment environment, not the compiler environment. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job reports SHALL use the existing job outcome schema. Boot and report-write failures SHALL exit 2; route pipeline failures SHALL exit 1; completed routes SHALL exit 0. The read-only deployment contract is class-dependent: an artifact whose assets are all memory-served SHALL write nothing under any filesystem; an artifact containing a disk-written class SHALL require a writable system temp directory (`TMPDIR`) and SHALL fail before boot when none is available.

#### Scenario: Multi-document artifact runs without its source tree

- **GIVEN** a valid compiled artifact whose source files, `Camel.toml`, and working directory are unavailable
- **WHEN** the artifact starts with required deployment environment values
- **THEN** all indexed route documents boot and run from memory without file discovery, extraction, or temporary writes beyond the documented materialization carve-out

#### Scenario: Runtime cannot discover an unembedded file

- **GIVEN** a compiled artifact and a new route file placed beside it after compilation
- **WHEN** the artifact starts
- **THEN** the new file is ignored because runtime consumes only indexed store entries

#### Scenario: Unknown store schema fails closed

- **GIVEN** a marked artifact whose index declares a store schema other than the supported `store_schema: 2` — including the removed schema 1 — beside an otherwise current (schema 3, 4, or 5) manifest
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot with an error naming the unsupported store schema and the remedy to recompile from source with this version

#### Scenario: Read-only deployment

- **GIVEN** a valid artifact whose assets are all static files (memory-served), running with a read-only root filesystem and no writable `TMPDIR`
- **WHEN** it boots and receives deployment environment values
- **THEN** it runs without materializing any entry and writes nothing other than an explicitly requested report

#### Scenario: Disk-written class boots on read-only root with writable TMPDIR

- **GIVEN** a valid artifact containing a disk-written class (for example an embedded TLS certificate), running with a read-only root filesystem and a writable `TMPDIR`
- **WHEN** it boots
- **THEN** the confined per-boot materialization succeeds inside `TMPDIR`, the listener spawns against the substituted paths, and nothing is written outside `TMPDIR` and an explicitly requested report

#### Scenario: Disk-written class without writable temp fails before boot

- **GIVEN** a valid artifact containing a disk-written class, running with no writable system temp directory
- **WHEN** it starts
- **THEN** it exits 2 before route or configuration boot with a diagnostic naming the class and the missing writable temp location

#### Scenario: Embedded job artifact binds no diagnostic listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables `[observability.prometheus]` and `[observability.health]` and declares `[runtime_journal]`
- **WHEN** the artifact starts
- **THEN** it projects the configuration through the job boot policy, opens no journal, binds no Prometheus or health listener, and exits with the normal job outcome codes
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
decision; the optional compile-side warning for server-type consumers in job artifacts is NOT taken in R2. Manifest schema values SHALL be validated independently from trailer version; readers SHALL accept manifest schemas 3, 4, and 5 and reject others — including the removed schemas 1 and 2 and a missing `manifest_schema` field — with a tombstone naming the schema and the remedy to recompile from source with this version, and SHALL enforce the pairing rule that manifest schemas 3, 4, and 5 require store schema 2 and store schema 2 requires manifest schema 3, 4, or 5. `--manifest` SHALL print this metadata without booting. The manifest
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

- **GIVEN** a marked artifact with a manifest schema that the reader does not support — including the removed schemas 1 and 2 or a missing `manifest_schema` field
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a payload-free tombstone naming the rejected schema — or, for a missing field, the missing `manifest_schema` field — and the remedy to recompile from source with this version, and does not reinterpret trailer version as manifest schema

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

### Requirement: Reject rolled-back artifacts by freshness floor

A signed compile SHALL emit manifest schema 5: the schema-4 signing block
plus a mandatory `freshness` marker encoded as an unsigned 64-bit
unix-seconds integer inside the signing block. The marker SHALL live in
the manifest bytes, which the envelope signature covers, so an attacker
cannot change it without breaking the signature. Readers SHALL accept
manifest schemas 3, 4, and 5; a schema-5 manifest without a valid
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

### Requirement: Reject unsigned artifacts under a strict truststore

A truststore line whose whitespace-separated token set is exactly the
single token `strict` (leading/trailing whitespace tolerated as for
pin lines; lowercase, case-sensitive; no pin, no floor, no second
token) SHALL set the store's strict directive. Repeated `strict`
lines SHALL be idempotent. A `strict` line with any second token
SHALL be a malformed line (step `truststore-parse`), and `strict`
SHALL NOT be accepted as a pin fingerprint. When a supplied
truststore carries the strict directive and the manifest carries no
signing block (schema 3 — the only unsigned schema the reader
accepts; the removed schemas 1 and 2 fail their own tombstones before
trust policy runs), every trust-policy surface SHALL exit 2 with a `strict-unsigned` diagnostic naming the
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

#### Scenario: Strict applies to every accepted manifest without a signing block

- **GIVEN** an artifact whose manifest is a current schema-3 body (no signing block) and a strict truststore
  supplied
- **WHEN** the artifact starts
- **THEN** the strict decision keys on the absence of the signing block, not on the schema number, and it exits 2 with
  a `strict-unsigned` diagnostic

#### Scenario: Removed schema under a strict store fails its tombstone

- **GIVEN** an artifact whose manifest declares the removed `manifest_schema: 2` and a strict truststore supplied
- **WHEN** the artifact starts
- **THEN** it exits 2 with the manifest-schema tombstone naming schema 2 and the recompile remedy — the tombstone
  precedes trust policy, so the `strict-unsigned` diagnostic never fires for it

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
