# cli-compile Specification

## Purpose
TBD - created by archiving change cli-compile. Update Purpose after archive.
## Requirements
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

### Requirement: Preserve runtime interpolation

The compiler SHALL capture document text before `${env:}` interpolation and the artifact SHALL resolve environment expressions from its deployment environment through the existing discovery path.

#### Scenario: Build and run with different environments

- **GIVEN** a document containing `${env:NAME}` and a compile environment value
- **WHEN** the operator compiles the document and runs the artifact with a different `NAME`
- **THEN** the artifact uses the deployment value and the compile value is absent from the embedded payload and manifest

### Requirement: Reject unsupported compile-time assets

The compiler SHALL embed the R2 asset matrix — certificates, private keys, and client-CA files in TLS and listener document contexts; TLS endpoint URI parameters (`tlsCert`/`tlsKey` on HTTP and WS endpoints; `serverCertPath`/`serverKeyPath`/`clientCaPath` on gRPC server endpoints and `caCertPath`/`clientCertPath`/`clientKeyPath` on gRPC client endpoints); the `protoFile` URI parameter on gRPC endpoints; `xslt` fields and `xslt:` URI operands; `xsd` fields and `validator:` URI operands; `sql:file:` URI operands; `static_dir` trees — and SHALL reject everything else fail-closed: `wasm:` URI operands, file-valued secret fields, dynamic `${env:}` placeholders and absolute paths inside asset fields and file-valued URI parameters, compile-time `CAMEL_*` configuration overrides — the signing input `CAMEL_COMPILE_SIGNING_KEY` under `--sign` excepted, whose stray presence without `--sign` still rejects — embedded `Camel.toml` `[beans.<name>]` `plugin` entries, embedded `Camel.toml` `[security.permissions.<name>]` WASM-provider `path` declarations, and asset references that escape the selected root or go missing. TLS-class references — document `tls` blocks, the `tlsCert`/`tlsKey` URI parameters, and the gRPC `*Path` family — SHALL resolve embedded-only: each reference resolves to an embedded virtual-store entry or compilation fails, and NO host-filesystem fallback exists at compile time or runtime. The gRPC `protoFile` parameter follows the same embedded-only resolution: its bytes come from the store through the confined per-boot materialization, and the sealed artifact never reads the proto from the deployment directory. The runtime gRPC URI parse accepts an absolute `protoFile` value only when its canonicalized path stays inside the OS temp directory that hosts the per-boot materialization directory; every other absolute path and every `..` traversal keeps failing closed. The document `wasm` field is a security-policy registry name, not an embedded asset, and SHALL compile as ordinary document data; there is no `sql` document field and no `plugin` document field. Certificate, key, and CA fields outside TLS or listener contexts remain ordinary document data. Runtime endpoint URI paths, deployment-time `${env:}` values, and deploy-side network/file I/O remain permitted. The `wasm:` URI-operand, `[beans]` plugin, and `[security.permissions]` WASM-provider-path rejections are R2-only deferrals recorded for their roadmap owner, not permanent rejections.

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

#### Scenario: gRPC protoFile compiles into the store

- **GIVEN** a route with `from: grpc://127.0.0.1:50051/helloworld.Greeter/SayHello?protoFile=protos/helloworld.proto&transport=plaintext` and the proto file present under the selected root
- **WHEN** the operator invokes compilation
- **THEN** the proto embeds as a typed asset entry with the non-secret `proto file` class (no `--embed-secrets` opt-in required), the `protoFile` parameter receives a Uri-context substitution-table entry, and compilation exits 0

#### Scenario: Runtime parse confines absolute protoFile to the OS temp directory

- **GIVEN** a gRPC endpoint URI whose `protoFile` value is an absolute path
- **WHEN** the runtime parses the URI
- **THEN** a path canonically confined under the OS temp directory parses, while paths outside it and paths carrying `..` traversal fail closed naming the proto path

#### Scenario: Absolute protoFile fails closed

- **GIVEN** a gRPC endpoint URI with `protoFile=/etc/service/helloworld.proto`
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

### Requirement: Verify trailer integrity and version

The artifact reader SHALL distinguish an absent trailer from a malformed trailer, enforce footer bounds, reject unsupported format versions, and verify the mandatory BLAKE3 checksum before runtime boot.

#### Scenario: Corrupted artifact fails closed

- **GIVEN** an artifact with a recognizable trailer whose payload, manifest, footer, or checksum was truncated or modified
- **WHEN** the artifact starts
- **THEN** it exits with a clear integrity diagnostic and does not partially boot

#### Scenario: Trailer-free executable falls through

- **GIVEN** the normal Camel executable has no recognizable trailer magic
- **WHEN** the executable starts with a normal CLI command
- **THEN** it invokes the existing Clap command path with unchanged behavior

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

- **GIVEN** a marked artifact whose index declares a store schema the executable does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot

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

### Requirement: Resolve and confine compile-time sources

The compiler SHALL normalize source names to UTF-8 relative paths using `/`, anchor resolution to the explicitly selected `Camel.toml` root (or the primary document directory when no configuration root is required), and reject absolute paths, empty or `.` components, `..` traversal, non-UTF-8 names, symlink escapes, missing sources, and references outside the root. Asset references — document fields, TLS URI parameters, endpoint-URI operands, and `Camel.toml` declarations — SHALL obey the same confinement as route sources. Static directories SHALL expand to individual regular-file entries through a deterministic sorted walk; a symlink encountered inside a static tree SHALL fail compilation with a named confinement diagnostic rather than being ignored, and non-regular files other than symlinks are skipped. Two references resolving to one canonical target SHALL embed one shared asset entry rather than failing, and every alias spelling SHALL receive a substitution-table entry mapping to that single entry. Route-source duplicate canonical targets keep failing closed. For job entry documents, configuration-declared `routes` patterns are not route sources: they add no source-plan entries, and the duplicate rule applies within the job document's own declared route-file family — an overlap between a configuration pattern and a job document's declared route files is not a duplicate-source rejection (the `camel job` exactly-one-source parity). Resolution SHALL be deterministic and SHALL not depend on ambient current-directory discovery.

#### Scenario: Path escape fails before output

- **GIVEN** a route source pattern, include, or asset reference that resolves outside the selected root, including through a symlink
- **WHEN** compilation runs
- **THEN** it exits 2 with a confinement diagnostic and does not create a usable artifact

#### Scenario: Symlink inside a static tree fails closed

- **GIVEN** a `static_dir` tree containing an entry that is a symbolic link
- **WHEN** compilation runs
- **THEN** it exits 2 naming the symlink path and does not silently skip or follow it

#### Scenario: Overlapping sources fail closed

- **GIVEN** two route-file patterns within one route-source family that resolve the same canonical source, or two names that normalize to the same logical path
- **WHEN** compilation runs
- **THEN** it exits 2 naming the duplicate and does not silently embed two copies; for job entry documents a configuration `routes` pattern is outside the job's route-source family and never triggers this rejection

#### Scenario: Shared asset targets deduplicate through the substitution table

- **GIVEN** two TLS blocks referencing the same client-CA file through different relative spellings
- **WHEN** compilation runs
- **THEN** the store contains one asset entry for the canonical target and the index substitution table maps both alias spellings to that single entry

#### Scenario: Static directory expansion is deterministic

- **GIVEN** a `static_dir` tree whose filesystem enumeration order varies
- **WHEN** compilation runs twice
- **THEN** both runs embed the same regular-file asset entries in the same canonical order and contain no entries for non-regular files

#### Scenario: Deterministic ordering is stable

- **GIVEN** the same source tree presented with filesystem enumeration in different orders
- **WHEN** compilation runs twice
- **THEN** the store index, source plan, embedded bytes, and artifact digest are identical

#### Scenario: Store offsets and schema are validated

- **GIVEN** a marked artifact with an unknown `store_schema`, an out-of-bounds or overlapping content range, a missing source-plan target, noncanonical index ordering, unreferenced content, or a substitution entry naming a nonexistent document or asset
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a format diagnostic and does not reinterpret the bytes as a different store schema

### Requirement: Embed and serve deploy-time assets

The artifact SHALL carry deploy-time assets as typed `asset` store entries under the `assets/` logical namespace with `store_schema: 2`, embedding asset bytes verbatim and recording each entry's asset class. The index SHALL carry the compile-time substitution table recording each (document, declared string) pair's exact byte span(s) in the normalized document or config entry together with its asset logical path; runtime SHALL replace only those recorded spans — applied last-offset-first so earlier offsets and line numbers stay stable — and SHALL NOT perform declared-string text search, canonicalize, re-resolve, or re-read declared asset strings from the filesystem. Values substituted into a URI SHALL be percent-encoded so they round-trip consumer decoding (the `validator:` URI path is percent-decoded before use); when a materialization path contains URI-reserved characters or whitespace that cannot be encoded safely, the runtime SHALL fail before boot with a named diagnostic. The artifact runtime SHALL populate an in-memory, read-only asset registry from the decoded asset entries before boot, verifying each BLAKE3 digest against the manifest and failing closed on mismatch, duplicate, or missing entry. Static-file entries SHALL be served from the registry; static files are the only memory-served class in R2. Consumers that read files by path and are named in the design as legacy consumers (TLS PEM loading from document fields and TLS URI parameters, `xslt:` stylesheets, `validator:` schemas, `sql:file:` operands) SHALL receive a per-boot confined materialization: only referenced assets, written into a directory created under `std::env::temp_dir()` with a random per-boot name at mode 0700, each file written once with `create_new` semantics at mode 0600 so an existing entry or symlink is never followed, ancestor confinement re-checked per the bd `rc-0ks57` discipline, referenced through substituted paths resolved before discovery, and removed at shutdown AND on boot failure by a guard owned by the artifact runtime. An abrupt `SIGKILL`, a `process::exit` call, or the in-tree force-exit on a second stop signal bypasses the guard and may leave residue in the system temp directory; this caveat SHALL be documented. The artifact SHALL perform no bulk extraction, and the registry SHALL stay empty in normal `camel run` operation. Runtime SHALL NOT load assets that the store does not contain. TLS-class references — document `tls` blocks, the `tlsCert`/`tlsKey` URI parameters, and the gRPC `*Path` family — SHALL resolve exclusively through the embedded store and its confined materialization, with NO host-filesystem fallback: the runtime SHALL NOT probe the host filesystem for any TLS path, and a host path that was not embedded SHALL NOT be consulted.

#### Scenario: Asset round trip through the registry

- **GIVEN** a compiled artifact embedding a static-file tree
- **WHEN** the artifact boots
- **THEN** the registry exposes the asset bytes by logical path, the digest matches the manifest, and no temporary file is created — static files are the memory-served class

#### Scenario: Legacy consumer materialization is confined

- **GIVEN** a compiled artifact whose TLS listener references embedded certificate and key entries
- **WHEN** the artifact boots and the listener spawns
- **THEN** the referenced assets exist only inside the confined per-boot directory under the system temp directory, the substituted paths resolve inside it, files are mode 0600 and the directory mode 0700, no asset is written outside it, and shutdown removes the directory

#### Scenario: Materialization is removed on boot failure

- **GIVEN** a compiled artifact whose route boot fails after the confined materialization has run
- **WHEN** the artifact exits with a boot failure
- **THEN** the per-boot directory and its files are removed by the runtime guard and no materialized file outlives the failed boot

#### Scenario: Post-compile asset decoy is ignored

- **GIVEN** a compiled artifact and a new file placed at a path one of its embedded asset references originally named
- **WHEN** the artifact starts
- **THEN** runtime resolves declared strings only through the substitution table without canonicalizing them, serves only the embedded entry, and never reads the new file

#### Scenario: TLS resolution never touches the host filesystem

- **GIVEN** a compiled artifact whose TLS assets were embedded, deployed on a host where the originally declared certificate and key paths do not exist
- **WHEN** the artifact boots and the TLS listener spawns
- **THEN** TLS paths resolve only through the embedded store and the confined materialization, the absent host paths are never probed, and the listener boots successfully

#### Scenario: Asset digest mismatch fails boot

- **GIVEN** a marked artifact whose asset content bytes no longer match the recorded BLAKE3 digest
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot and never populates the registry

#### Scenario: Normal run never populates the registry

- **GIVEN** a plain `camel run` invocation without a compiled artifact
- **WHEN** routes boot
- **THEN** the asset registry stays empty and asset embedding plays no part in discovery

#### Scenario: Registry tests are process-isolated

- **GIVEN** the registry test suite runs in its own test process with the process-global registry
- **WHEN** a test populates the registry and asserts through `is_populated()`
- **THEN** no other test binary observes the population, and no test resets global state

### Requirement: Compile and boot multi-entry job artifacts

A job entry document whose file-form route source (`routeFiles` or `routeFilesFromRoot`) resolves to one or more route documents SHALL compile into a v2 virtual-store artifact embedding the job document, every resolved route document, and the selected configuration chain; the artifact SHALL boot all indexed route entries through the existing job outcome lifecycle with no source tree, no extraction, and no filesystem route discovery. The job source plan SHALL contain exactly the job document and its own file-form expansions: configuration-declared `routes` patterns SHALL NOT add entries to a job artifact's source plan, mirroring the `camel job` exactly-one-source rule, and overlapping configuration patterns SHALL NOT fail a job compile that `camel job` accepts. A file-form job route source that resolves zero route documents SHALL fail compilation with a diagnostic naming the document and the resolved-zero rule, before any output byte is created. Route-entry consumption order SHALL be the source-plan order — declared pattern order, each pattern's matches sorted by normalized logical path — and identical input sets SHALL produce byte-identical artifacts. A job artifact boot in which any embedded route entry fails parse or validation SHALL exit 2 with a diagnostic naming that entry's `compiled://<logical-path>` identity before any route starts; no partial boot SHALL occur. The operational manifest SHALL list every route entry of a multi-entry job artifact with its canonical logical path, kind, byte length, and digest, alongside the job and configuration entries.

#### Scenario: Multi-entry job artifact boots every entry

- **GIVEN** a compiled job artifact embedding a job document and three or more route documents forming a direct-endpoint chain across files, deployed without its source tree or configuration
- **WHEN** the artifact starts with `--report <path>`
- **THEN** it exits 0, every embedded route entry participates in the boot, the one-shot send traverses the chain across all files, and the report records outcome `Completed` with the composed reply body and the `compiled://<job-path>` document identity

#### Scenario: Entry order follows the source plan

- **GIVEN** a job document declaring route-file patterns in non-sorted order where one pattern matches multiple files
- **WHEN** the document is compiled
- **THEN** the source plan preserves declared pattern order with each pattern's matches sorted by normalized logical path, the store packs entries in canonical path order, and two compiles of the identical input set produce byte-identical artifacts

#### Scenario: Failing entry is named at boot with no partial boot

- **GIVEN** a compiled job artifact in which one embedded route document among several is structurally invalid, which compilation deliberately allows
- **WHEN** the artifact starts
- **THEN** it exits 2 with a diagnostic naming the invalid entry's `compiled://<logical-path>` identity before any route starts, and no job report or outcome is produced

#### Scenario: Manifest lists every route entry

- **GIVEN** a compiled multi-entry job artifact embedding a job document and several route documents
- **WHEN** the artifact runs `--manifest`
- **THEN** it exits 0 without booting and lists every route entry with canonical logical path, `route` kind, byte length, and digest, alongside the `job` entry and configuration entries, under `artifact_kind: "job"`

#### Scenario: Configuration route patterns do not widen the job plan

- **GIVEN** a job document declaring `routeFiles` entries that a configuration `routes` pattern in the selected `Camel.toml` also matches, a document set `camel job` accepts and boots
- **WHEN** the operator compiles the job document with that configuration
- **THEN** compilation succeeds, and the source plan contains exactly the job document and its own declared route-file expansions — the configuration pattern adds no entry and the overlap is not a duplicate-source rejection

#### Scenario: Zero-route-entry job source rejected at compile

- **GIVEN** a job document whose file-form route source patterns resolve zero route documents, a document `camel job` rejects as a job-safety violation
- **WHEN** the operator compiles the job document
- **THEN** compilation fails with exit 2, a diagnostic names the document and the resolved-zero rule, and no artifact is created

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

### Requirement: Long-running route-server artifacts

A compiled route artifact SHALL run like `camel run` in a deployment
posture: after successful boot it SHALL keep running — with every
listener the embedded documents and embedded configuration declare
bound and serving — until the first stop signal. The SIGINT and SIGTERM
streams SHALL be armed before boot so a signal arriving during boot is
buffered and consumed by the shutdown wait rather than default-killing
the process. The first SIGINT or SIGTERM SHALL start a graceful
shutdown: in-flight listener work is given the configured drain budget
to complete, then listeners close, and the process exits 0, writing the
completed route report when `--report` was given; work still in flight
when the budget expires is dropped. A second SIGINT or SIGTERM received
while teardown is still running SHALL force-exit with code 1 without
waiting for teardown — the same first-graceful/second-force SIGNAL
contract `camel run` and `camel job` implement (outcome codes differ by
kind: a signal-stopped route exits 0; a job interruption keeps the job
taxonomy codes). A signal burst of identical signals MAY coalesce
before delivery; the force-exit hatch MUST accept either signal.
Compiled job artifacts SHALL keep the bounded exit-after-completion
behavior and SHALL NOT serve until a signal. For a signed artifact,
envelope verification SHALL finish before any listener binds: a failed
or missing required envelope exits 2 with no port ever bound.

#### Scenario: Route artifact serves its declared listener until SIGTERM

- **GIVEN** a compiled route artifact whose document declares a REST
  listener on a free local port
- **WHEN** the artifact starts and an HTTP request is sent to the
  listener after boot completes
- **THEN** the listener answers with the route's response while the
  process keeps running, and the artifact's `--manifest` output lists
  the listener

#### Scenario: gRPC consumer serves from a sealed artifact until SIGTERM

- **GIVEN** a compiled route artifact whose document declares a
  `grpc://` consumer route carrying
  `?protoFile=…&transport=plaintext`, deployed into a fresh
  source-free directory with the proto resolved from the embedded
  store
- **WHEN** the artifact boots and a client completes the HTTP/2
  connection preface against the declared port after the consumer
  reports it is serving
- **THEN** the server answers with its SETTINGS frame, the process
  keeps serving (a liveness probe observes it still running), the first
  SIGTERM drains gracefully with exit 0, the report file contains
  `{"kind":"route","status":"completed","error":null}`, and the
  artifact's `--manifest` output reports `artifact_kind` `server` with
  `grpc` among the components

#### Scenario: WebSocket consumer drains an in-flight exchange

- **GIVEN** a serving compiled route artifact started with
  `--report <path>` whose `ws://` consumer route holds a connected
  WebSocket exchange inside a delay step that completes within the
  configured drain budget
- **WHEN** SIGTERM arrives while the exchange is in flight
- **THEN** the in-flight WebSocket response still completes inside the
  drain budget, the process exits 0, and the report file contains
  `{"kind":"route","status":"completed","error":null}`

#### Scenario: First signal drains gracefully and exits 0

- **GIVEN** a serving compiled route artifact started with
  `--report <path>` and a listener request in flight whose route
  completes within the configured drain budget
- **WHEN** SIGTERM arrives while the request is in flight
- **THEN** the in-flight request completes inside the drain budget,
  listeners close, the process exits 0, and the report file contains
  `{"kind":"route","status":"completed","error":null}`

#### Scenario: Second signal force-exits during teardown

- **GIVEN** a booting compiled route artifact receives SIGINT followed
  by SIGTERM, both buffered before the shutdown wait begins
- **WHEN** boot finishes and the buffered signals are consumed — the
  first starting graceful teardown, the second already queued for the
  force-exit guard
- **THEN** the process exits with code 1 without waiting for teardown
  to complete

#### Scenario: Route artifact signal during boot is buffered

- **GIVEN** a compiled route artifact is loading configuration or
  booting components
- **WHEN** SIGTERM arrives mid-boot
- **THEN** the signal is buffered, boot reaches the shutdown wait, and
  the process shuts down gracefully with exit 0 instead of dying to the
  default disposition

#### Scenario: Deployment-equivalence with camel run

- **GIVEN** the same listener-bearing route document run once via
  `camel run --routes <doc> --no-watch` and once as a compiled artifact
- **WHEN** the listener is queried and SIGTERM is sent in both runs
- **THEN** both runs answer the query before the signal and exit 0
  after it — identical serve, drain, and exit behavior

#### Scenario: Job artifacts stay bounded

- **GIVEN** a compiled job artifact whose embedded routes include a
  direct consumer
- **WHEN** the artifact runs
- **THEN** it completes its bounded send and exits with the job outcome
  taxonomy codes, never serving until a signal

#### Scenario: Envelope verification precedes listener binding

- **GIVEN** a compiled route artifact whose manifest marks the detached
  signature envelope required and whose envelope bytes were corrupted
  after signing, leaving the artifact trailer itself valid
- **WHEN** the artifact starts
- **THEN** it exits 2 naming the failed signature-verification step and
  no declared listener port is ever bound

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

