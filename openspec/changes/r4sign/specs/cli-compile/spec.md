## ADDED Requirements

### Requirement: Sign artifacts with a detached outer envelope

The compiler SHALL offer `--sign`, taking a 32-byte ed25519 seed file supplied by `--signing-key <path>` or the namespaced `CAMEL_COMPILE_SIGNING_KEY` environment variable (argument takes precedence). The key SHALL be used only as a signing input: no key material SHALL enter the artifact, envelope, manifest, output, or log, and embedded content SHALL stay unchanged by signing. `--require-signature` SHALL require `--sign`. A signed compile SHALL emit a detached envelope at `<artifact>.sig` with fixed framing `CAMELTR1`-family style: leading `CAMELSG1` magic, little-endian `u16` envelope version, `u8` algorithm, zero `u8` flags, the 32-byte ed25519 public key, the 64-byte signature, a BLAKE3 checksum over a domain-separated encoding of the envelope fields, and terminal `CAMELSG1` magic. The signature SHALL be Ed25519ph (RFC 8032 prehash, null context) over the complete final artifact bytes — executable copy and trailer — so sign and verify stream with flat memory, and envelope bytes SHALL be deterministic for the same key and artifact bytes. The signed artifact's manifest SHALL use schema 4 and record exactly the algorithm name, the BLAKE3 fingerprint of the public key (`blake3:` + hex), and the required bit. Envelope-write failure SHALL remove the artifact and fail compilation. At boot, an artifact with a present envelope SHALL verify before boot: envelope parse, algorithm match, fingerprint binding to the manifest signing block, and signature over the final artifact bytes; any failure SHALL exit 2 with a named diagnostic. An absent envelope SHALL exit 2 when the manifest required bit is set and SHALL boot unchanged otherwise. A present envelope with a schema-3-or-earlier manifest SHALL exit 2 as an unpaired envelope. `--verify` SHALL run the same chain without booting, SHALL stay exclusive with every other artifact flag, SHALL exit 0 naming the algorithm and fingerprint on success, and SHALL exit 2 with a named diagnostic otherwise.

#### Scenario: Signing emits a verified envelope round trip

- **GIVEN** a route compiled with `--sign` and a valid `--signing-key` seed file
- **WHEN** the artifact starts, and separately runs `./app --verify`
- **THEN** the artifact boots with the valid envelope beside it, and `--verify` exits 0 naming algorithm `ed25519ph` and the key fingerprint recorded in the manifest

#### Scenario: Signature covers the final artifact bytes

- **GIVEN** a signed artifact with one byte flipped anywhere in the file, whether in the executable body or the trailer span
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
- **THEN** the signing block lists the algorithm name, key fingerprint, and required bit, and no key material appears in any output or log

#### Scenario: Envelope framing is deterministic

- **GIVEN** the same 32-byte seed and byte-identical artifact input
- **WHEN** the envelope is encoded twice
- **THEN** the `.sig` bytes are identical across runs

#### Scenario: Signing-key input is validated

- **GIVEN** a compile with `--sign` but no key source, or `--signing-key` without `--sign`, or a stray `CAMEL_COMPILE_SIGNING_KEY` without `--sign`, or a key file whose size is not exactly 32 bytes, or `--require-signature` without `--sign`
- **WHEN** the operator compiles
- **THEN** compilation exits 2 with a diagnostic naming the broken rule, and no artifact or `.sig` file is left behind

#### Scenario: --verify stays exclusive

- **GIVEN** a valid signed artifact
- **WHEN** the operator runs `./app --verify --manifest`
- **THEN** it exits 2 with a duplicate-exclusive diagnostic without booting or verifying

## MODIFIED Requirements

### Requirement: Compile a native single-document artifact

The CLI SHALL provide `camel compile <document> -o <artifact>` that resolves one logical route or job entry point and appends `CAMELTR1 || content || index || manifest || footer` to a copy of the current executable, preserving executable permissions. The 76-byte v2 footer contains `CAMELTR1`, little-endian `u16` version 2, `u8` kind (`1=route`, `2=job`), zero `u8` flags, little-endian `u64` content/index/manifest lengths, BLAKE3, and terminal `CAMELTR1`. The checksum SHALL cover ASCII `rust-camel-trailer-v2`, one zero byte, the encoded version/kind/three lengths, content, index, and manifest, excluding magic and flags. Normalization SHALL accept valid UTF-8, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and reject invalid UTF-8; normalization SHALL apply to document entries only, and asset entries SHALL embed verbatim. The content SHALL contain normalized pre-interpolation route, job, `Camel.toml`, include, and selected-profile entries plus verbatim deploy-time asset entries. The index SHALL be canonical UTF-8 JSON with independent `store_schema: 2`, typed entries, offsets, lengths, one logical entry point, configuration references, an ordered source plan, and the compile-time substitution table recording each (document, declared string) pair's exact byte span(s) in the normalized document or config entry together with its asset logical path. The manifest SHALL use independent `manifest_schema: 3` for unsigned artifacts and `manifest_schema: 4` for artifacts signed under the detached-envelope requirement, and `store_schema: 2` SHALL pair with either manifest schema: readers SHALL reject a store-2 index with a manifest other than 3 or 4, a manifest-3 or manifest-4 body with a non-2 store, and a manifest-4 body without a signing block. The compiler SHALL resolve route files, includes, profiles, supported job route sources, and deploy-time asset references at compile time using the explicitly supplied `--config <Camel.toml>`, repeated `--profile <name>`, and `--embed-secrets` options; without `--config`, it SHALL embed no configuration and select no profile, and it SHALL never discover ambient configuration. TLS-class asset references SHALL resolve embedded-only, with no host-filesystem fallback at compile time or runtime (sealed bd `rc-p823t`); each TLS-class reference resolves to an embedded virtual-store entry or compilation fails, leaving the reference syntax free for R5 `host:`/`env:` prefixes. The store SHALL preserve logical relative paths and the ordered source plan. Aggregate embedded bytes — normalized document bytes plus verbatim asset bytes — SHALL remain limited to the compile-time cap selected with `--max-payload-bytes <bytes>`, a positive value whose default SHALL be 16 MiB; exceeding the cap SHALL fail compilation with a diagnostic naming the total and the cap before any output is created. Embedding a secret-family asset — exactly the private-key class: the `key` document field and the `tlsKey`, `serverKeyPath`, and `clientKeyPath` TLS URI parameters — SHALL require explicit opt-in: compilation SHALL fail closed with a diagnostic naming the field and the secret class unless the operator passes `--embed-secrets`. An artifact compiled with the opt-in that contains a secret-family entry SHALL be written with file mode 0700; artifacts without embedded secret-family entries SHALL be written with mode 0755.

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

#### Scenario: Single-document v1 remains readable

- **GIVEN** an existing valid version-1 single-document artifact
- **WHEN** a version-2-capable executable starts it
- **THEN** the reader exposes it as a one-entry virtual store and preserves existing runtime behavior

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

- **GIVEN** an artifact whose index declares `store_schema: 2` but whose manifest body declares a schema other than 3 or 4
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a schema-pairing diagnostic, and the symmetric case (manifest 3 or 4 with a non-2 store, or a manifest-4 body without a signing block) fails the same way

### Requirement: Reject unsupported compile-time assets

The compiler SHALL embed the R2 asset matrix — certificates, private keys, and client-CA files in TLS and listener document contexts; TLS endpoint URI parameters (`tlsCert`/`tlsKey` on HTTP and WS endpoints; `serverCertPath`/`serverKeyPath`/`clientCaPath` on gRPC server endpoints and `caCertPath`/`clientCertPath`/`clientKeyPath` on gRPC client endpoints); `xslt` fields and `xslt:` URI operands; `xsd` fields and `validator:` URI operands; `sql:file:` URI operands; `static_dir` trees — and SHALL reject everything else fail-closed: `wasm:` URI operands, file-valued secret fields, dynamic `${env:}` placeholders and absolute paths inside asset fields and TLS URI parameters, compile-time `CAMEL_*` configuration overrides — the signing input `CAMEL_COMPILE_SIGNING_KEY` under `--sign` excepted, whose stray presence without `--sign` still rejects — embedded `Camel.toml` `[beans.<name>]` `plugin` entries, embedded `Camel.toml` `[security.permissions.<name>]` WASM-provider `path` declarations, and asset references that escape the selected root or go missing. TLS-class references — document `tls` blocks, the `tlsCert`/`tlsKey` URI parameters, and the gRPC `*Path` family — SHALL resolve embedded-only: each reference resolves to an embedded virtual-store entry or compilation fails, and NO host-filesystem fallback exists at compile time or runtime. The document `wasm` field is a security-policy registry name, not an embedded asset, and SHALL compile as ordinary document data; there is no `sql` document field and no `plugin` document field. Certificate, key, and CA fields outside TLS or listener contexts remain ordinary document data. Runtime endpoint URI paths, deployment-time `${env:}` values, and deploy-side network/file I/O remain permitted. The `wasm:` URI-operand, `[beans]` plugin, and `[security.permissions]` WASM-provider-path rejections are R2-only deferrals recorded for their roadmap owner, not permanent rejections.

#### Scenario: Unsupported asset names the reason

- **GIVEN** a document containing a file-valued secret field or a stray compile-time `CAMEL_*` variable — any `CAMEL_*` name other than `CAMEL_COMPILE_SIGNING_KEY` supplied under `--sign`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact and names the rejected asset class

#### Scenario: Beans plugin declaration fails closed

- **GIVEN** a selected `Camel.toml` declaring a `[beans.<name>]` entry with a `plugin` field
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the bean and explaining that cwd-relative plugin loading cannot be embedded, and no artifact is written

#### Scenario: Security-policy WASM provider path fails closed

- **GIVEN** a selected `Camel.toml` declaring `[security.permissions.<name>]` with a WASM-provider `path`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the policy and explaining that the path-taking policy constructor cannot consume embedded bytes in R2, and no artifact is written

#### Scenario: Dynamic asset placeholder is rejected

- **GIVEN** a TLS context field or TLS URI parameter whose certificate path is a dynamic `${env:}` placeholder
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the field and explaining that an unknowable asset path cannot be embedded

#### Scenario: Absolute TLS URI parameter fails closed

- **GIVEN** an HTTP endpoint URI with `tlsCert=/etc/pki/cert.pem`
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 naming the parameter and requiring a root-relative compile-known path

#### Scenario: Embedded job sources are allowed

- **GIVEN** a job document with route sources and configuration selected through explicit compile inputs
- **WHEN** the operator invokes compilation
- **THEN** the route sources and configuration are embedded in the virtual store and the job does not require external source files at runtime

#### Scenario: Job configuration is self-contained

- **GIVEN** a job document that depends on external config, profiles, includes, or a route source outside the document
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 before output creation and names the dependency

#### Scenario: TLS assets compile into the store

- **GIVEN** a route declaring certificate, private-key, and client-CA files under a `tls` block, with the files present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** the files embed as typed asset entries and compilation exits 0

#### Scenario: TLS URI parameters compile into the store

- **GIVEN** a route with `https://…?tlsCert=certs/cert.pem&tlsKey=certs/key.pem` and a gRPC endpoint carrying `transport=tls&serverCertPath=…&serverKeyPath=…`, all files present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** every referenced file embeds as a typed asset entry, the URI parameters receive substitution-table entries, and compilation exits 0

#### Scenario: Registry-name wasm field compiles unchanged

- **GIVEN** a document whose security policy declares `wasm` with a registry-name value
- **WHEN** the operator invokes compilation
- **THEN** the field compiles as ordinary document data and no asset entry is created for it

#### Scenario: Wasm URI operand fails closed

- **GIVEN** a document whose endpoint URI carries a `wasm:` operand naming a module file
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact, naming the operand and explaining that `wasm:` module embedding is a recorded R2 deferral

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept only `--report <path>`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface `--verify`, which SHALL stay exclusive of every other artifact flag. Duplicate exclusive flags, missing report values, positional arguments, and other arguments SHALL exit 2. The operational manifest SHALL contain a separate `manifest_schema` field, an `embedded_files` list with canonical logical paths, entry kinds, asset classes, a secret-material `class` per asset entry (`"public"` | `"secret"` — documents are implicitly public; `secret` is exactly the private-key family), byte lengths, and BLAKE3 content digests, a `total_embedded_bytes` aggregate over all content entries, and a top-level `artifact_kind` (`"job"` | `"server"` — the trailer route kind maps to `server`, job to `job`). A schema-4 manifest SHALL carry a signing block with exactly the algorithm name, the key fingerprint, and a required bit, and no signing block SHALL appear in a schema-3 manifest; no key material SHALL appear in any manifest, output, or log. Entries for secret-class assets SHALL expose only their class, byte length, and BLAKE3 digest: their logical path SHALL be withheld from the manifest body and `--manifest` output, and no key material SHALL appear in any output or log. The `artifact_kind` field is the sealed R3 tripwire (bd `rc-p823t`): no long-lived manifest entries ship before the R5 TLS decision exists, and any artifact that can outlive a short bounded run triggers that decision; the optional compile-side warning for server-type consumers in job artifacts is NOT taken in R2. Manifest schema values SHALL be validated independently from trailer version; readers SHALL accept manifest schemas 2, 3, and 4 and reject others, and SHALL enforce the pairing rule that manifest schemas 3 and 4 require store schema 2 and store schema 2 requires manifest schema 3 or 4. `--manifest` SHALL print this metadata without booting. The manifest SHALL not contain compile-time environment values and SHALL list required environment variables without defaults. Listener declarations SHALL report the artifact kind's effective runtime listeners: job artifacts SHALL omit listeners the job boot projection suppresses, and route artifacts SHALL list all configuration-declared listeners.

#### Scenario: Manifest inspection

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the command exits 0 without booting and prints manifest schema, artifact kind, logical embedded-file metadata, components, required environment names, and listener declarations

#### Scenario: Manifest reports asset digests and total size

- **GIVEN** a valid artifact embedding documents and assets
- **WHEN** the operator runs `./app --manifest`
- **THEN** each asset entry lists its logical path, asset class, secret-material `class`, byte length, and BLAKE3 digest, and the output includes `total_embedded_bytes` equal to the sum of all content-entry lengths

#### Scenario: Manifest records artifact kind

- **GIVEN** one compiled job artifact and one compiled route artifact
- **WHEN** the operator runs `./app --manifest` on each
- **THEN** `artifact_kind` is `job` for the job artifact and `server` for the route artifact, without booting

#### Scenario: Secret manifest entries expose digest and length only

- **GIVEN** an artifact compiled with `--embed-secrets` embedding a private key
- **WHEN** the operator runs `./app --manifest`
- **THEN** the secret entry lists class `secret`, its byte length, and its BLAKE3 digest, the logical path is withheld, and no key material appears anywhere in the output

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
- **THEN** output includes runtime version, artifact kind, embedded files, required environment names, and listener declarations without booting

#### Scenario: Job artifact manifest omits suppressed listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list is empty of the suppressed health and Prometheus endpoints, matching the job boot projection's effective runtime

#### Scenario: Route artifact manifest keeps config-declared listeners

- **GIVEN** a compiled route artifact whose embedded configuration enables health and Prometheus listeners
- **WHEN** the operator runs `./app --manifest`
- **THEN** the listener declarations list contains both endpoints, because route artifacts bind them at runtime

### Requirement: Record the artifact decision

The project SHALL document the format and trust boundary in ADR 0075, the R4 detached signing envelope in ADR 0083, and define `compiled artifact`, `EOF trailer`, `self-detect`, `operational manifest`, `signature envelope`, and `key fingerprint` in the canonical context documentation with citations to the ADRs.

#### Scenario: Documentation remains aligned

- **GIVEN** the implementation and capability delta are reviewed
- **WHEN** context and documentation lint runs
- **THEN** ADR 0075, ADR 0083, `CONTEXT-MAP.md`, and `camel-cli/CONTEXT.md` describe the same native Linux preview scope, the same signing envelope contract, and no unsupported performance claim

### Requirement: Permanent v1 non-goals

The artifact SHALL preserve the sealed deployment-unit wall: no watch or hot reload, runtime file discovery or globbing, ambient `Camel.toml`, compile-time `CAMEL_*` configuration overrides, wider artifact-runtime arguments, or component-crate source edits for byte seams in R2. Compile-time source and asset selection may use explicit `--config`, `--profile`, `--max-payload-bytes`, and `--embed-secrets` options, and compile-time signing may use explicit `--sign`, `--signing-key`, and `--require-signature` options; the namespaced `CAMEL_COMPILE_SIGNING_KEY` environment variable SHALL supply the signing-key path only under `--sign` — it is a signing input, not a configuration override, and its stray presence without `--sign` stays rejected — but runtime accepts only `--report`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface `--verify`. Compression (roadmap R6) and cross-target compilation (roadmap R7) remain deferred roadmap milestones owned by their epic, not permanent non-goals; signing (roadmap R4) is delivered as the detached-envelope requirement, not a non-goal. Per-consumer byte-seam work in component crates is a recorded additive follow-up. R2 SHALL keep one logical entry point. Deploy-time asset embedding is an R2 capability, not a non-goal; asset classes outside the R2 matrix — including `[beans]` plugin files, `[security.permissions]` WASM-provider paths, and file-valued or literal secret files — stay rejected in R2 as recorded deferrals to their roadmap owner, revisit-able by later roadmap work rather than permanently sealed.

#### Scenario: Permanent non-goals remain outside the artifact contract

- **GIVEN** an operator or roadmap proposal requests watch/hot-reload, runtime file discovery/globbing, ambient `Camel.toml`, a command argument beyond `--report`/`--help`/`--version`/`--manifest` other than the R4 `--verify` surface, or a compile-time `CAMEL_*` configuration override
- **WHEN** the proposal is evaluated against the compiled-artifact contract
- **THEN** the capability is rejected as a permanent non-goal rather than added to the artifact surface

#### Scenario: MUST-NOT capabilities remain rejected

- **GIVEN** a proposal to add ambient configuration, runtime discovery, compile-time overrides, watch, a wider artifact argument surface, or asset embedding to R1
- **WHEN** the proposal is evaluated against the sealed-artifact contract
- **THEN** it is rejected as outside this change and recorded for its roadmap owner rather than added to the virtual-store implementation

#### Scenario: Roadmap milestones are not permanent non-goals

- **GIVEN** a proposal for compression or cross-target compilation
- **WHEN** the proposal is evaluated against the compiled-artifact contract
- **THEN** it is routed to its roadmap milestone owner (R6 and R7 respectively) with the current contract unchanged, rather than being sealed as permanently impossible

#### Scenario: Embedded configuration does not become ambient configuration

- **GIVEN** a compiled artifact is deployed without its source tree or an ambient `Camel.toml`
- **WHEN** the artifact starts
- **THEN** it loads no external configuration, resolves only permitted deployment-time `${env:NAME}` expressions from the embedded document, and performs no runtime discovery, globbing, watch, or hot-reload behavior

#### Scenario: Artifact arguments stay narrow

- **GIVEN** a valid compiled artifact
- **WHEN** the operator supplies an argument other than `--report`, `--help`, `--version`, `--manifest`, or the R4 `--verify` surface
- **THEN** the artifact rejects the argument with exit 2 and does not expand its command surface

#### Scenario: Out-of-matrix assets stay rejected for R2

- **GIVEN** a proposal to embed plugin files, WASM security-policy modules, secret files, or an asset class outside the R2 matrix
- **WHEN** the proposal is evaluated against the compiled-artifact contract
- **THEN** it is rejected for R2 and recorded as a deferral for its roadmap owner with rationale, rather than added to the store or declared permanently impossible
