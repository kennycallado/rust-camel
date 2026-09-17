# cli-compile Specification

## Purpose
TBD - created by archiving change cli-compile. Update Purpose after archive.
## Requirements
### Requirement: Compile a native single-document artifact

The CLI SHALL provide `camel compile <document> -o <artifact>` that copies the current executable and appends a versioned, fixed EOF trailer containing normalized document text and operational manifest. Normalization SHALL accept valid UTF-8, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and enforce a 16 MiB encoded-byte limit. The manifest SHALL be canonical UTF-8 JSON with lexicographically ordered keys and source-ordered arrays; logical source name SHALL be input path relative to compile working directory. The exact 68-byte footer SHALL be `CAMELTR1` magic, little-endian `u16` version 1, `u8` kind (`1=route`, `2=job`), zero reserved `u8`, little-endian `u64` payload length, little-endian `u64` manifest length, 32-byte BLAKE3, and terminal `CAMELTR1` magic. The checksum SHALL cover ASCII bytes `rust-camel-trailer-v1`, one `0x00` byte, then little-endian encoded version, kind, payload length, manifest length, payload, and manifest; it SHALL exclude both magic fields and reserved byte.

#### Scenario: Compile a supported document

- **GIVEN** a supported single route or job document and a writable output path
- **WHEN** the operator runs `camel compile <document> -o <artifact>` on the native Linux target
- **THEN** the command writes an executable artifact containing the document payload, manifest, footer metadata, and mandatory BLAKE3 integrity value

#### Scenario: Trailer framing is deterministic

- **GIVEN** the same normalized document, manifest, and artifact kind
- **WHEN** the trailer codec encodes them
- **THEN** it emits the specified little-endian fields and checksum domain bytes, and the decoder rejects length overflow or kind/manifest mismatch

#### Scenario: Marked trailer corruption fails closed

- **GIVEN** an artifact whose final 8 bytes are `CAMELTR1` but whose preceding footer fields, bounds, or checksum are invalid
- **WHEN** the artifact starts
- **THEN** it exits with an integrity or format diagnostic and does not fall through to normal CLI parsing

#### Scenario: Unmarked truncation follows normal fallback

- **GIVEN** truncation removes the terminal trailer magic so the executable has no recognizable trailer
- **WHEN** the executable starts
- **THEN** it follows the normal CLI path because trailer presence cannot be distinguished from an ordinary executable

#### Scenario: Reject unsupported target compilation

- **GIVEN** a requested target different from the host target or a cross-compilation option
- **WHEN** the operator invokes compilation
- **THEN** the command exits 2 with a diagnostic stating that v1 supports native Linux only

### Requirement: Preserve runtime interpolation

The compiler SHALL capture document text before `${env:}` interpolation and the artifact SHALL resolve environment expressions from its deployment environment through the existing discovery path.

#### Scenario: Build and run with different environments

- **GIVEN** a document containing `${env:NAME}` and a compile environment value
- **WHEN** the operator compiles the document and runs the artifact with a different `NAME`
- **THEN** the artifact uses the deployment value and the compile value is absent from the embedded payload and manifest

### Requirement: Reject unsupported compile-time assets

The compiler SHALL fail closed when the document requires assets that v1 cannot embed. It SHALL reject route-source fields (`routeFiles`, `routeFilesFromRoot`, globs, includes, external paths), configuration fields (`Camel.toml`, profiles, `CAMEL_*` compile overrides), and asset-bearing endpoint fields (certificates, private keys, CA files, WASM or plugin files, XSLT/XSD, SQL files, static directories, literal secret files, and dynamic placeholders in those fields). Runtime endpoint URI paths, runtime `${env:}` values, and deploy-side network/file I/O remain permitted.

#### Scenario: Unsupported asset names the reason

- **GIVEN** a document referencing an unsupported compile-time asset
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact and names the rejected asset class

#### Scenario: Job configuration is self-contained

- **GIVEN** a job document that depends on external config, profiles, includes, or a route source outside the document
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 before output creation and names the dependency

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

The artifact SHALL feed the indexed documents through the existing parse, runtime interpolation, lowering, boot, and start paths using an in-memory virtual store. Runtime SHALL build configuration from embedded `Camel.toml`, include, and selected-profile entries. Embedded job configuration SHALL pass through the same job boot projection as argv jobs before context configuration; embedded route configuration SHALL boot unchanged. Runtime SHALL perform no source-tree reads, glob expansion, ambient `Camel.toml` or profile loading, canonicalization, temporary extraction, watch, or hot reload. Source diagnostics SHALL use `compiled://<logical-path>` identities. `${env:NAME}` expressions SHALL resolve from the deployment environment, not the compiler environment. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job reports SHALL use the existing job outcome schema. Boot and report-write failures SHALL exit 2; route pipeline failures SHALL exit 1; completed routes SHALL exit 0.

#### Scenario: Multi-document artifact runs without its source tree

- **GIVEN** a valid compiled artifact whose source files, `Camel.toml`, and working directory are unavailable
- **WHEN** the artifact starts with required deployment environment values
- **THEN** all indexed route documents boot and run from memory without file discovery, extraction, or temporary writes

#### Scenario: Runtime cannot discover an unembedded file

- **GIVEN** a compiled artifact and a new route file placed beside it after compilation
- **WHEN** the artifact starts
- **THEN** the new file is ignored because runtime consumes only indexed store entries

#### Scenario: Unknown store schema fails closed

- **GIVEN** a marked artifact whose index declares a store schema the executable does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before configuration or route boot

#### Scenario: Read-only deployment

- **GIVEN** a valid multi-document artifact running with a read-only root filesystem
- **WHEN** it boots and receives deployment environment values
- **THEN** it runs without materializing documents or writing files other than an explicitly requested report

#### Scenario: Embedded job artifact binds no diagnostic listeners

- **GIVEN** a compiled job artifact whose embedded configuration enables `[observability.prometheus]` and `[observability.health]` and declares `[runtime_journal]`
- **WHEN** the artifact starts
- **THEN** it projects the configuration through the job boot policy, opens no journal, binds no Prometheus or health listener, and exits with the normal job outcome codes

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept only `--report <path>`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface. Duplicate exclusive flags, missing report values, positional arguments, and other arguments SHALL exit 2. The operational manifest SHALL contain a separate `manifest_schema` field and an `embedded_files` list with canonical logical paths, document kinds, byte lengths, and content digests. Manifest schema values SHALL be validated independently from trailer version. `--manifest` SHALL print this metadata without booting. The manifest SHALL not contain compile-time environment values and SHALL list required environment variables without defaults. Listener declarations SHALL report the artifact kind's effective runtime listeners: job artifacts SHALL omit listeners the job boot projection suppresses, and route artifacts SHALL list all configuration-declared listeners.

#### Scenario: Manifest inspection

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the command exits 0 without booting and prints manifest schema, artifact kind, logical embedded-file metadata, components, required environment names, and listener declarations

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

The project SHALL document the format and trust boundary in ADR 0075 and define `compiled artifact`, `EOF trailer`, `self-detect`, and `operational manifest` in the canonical context documentation with citations to the ADR.

#### Scenario: Documentation remains aligned

- **GIVEN** the implementation and capability delta are reviewed
- **WHEN** context and documentation lint runs
- **THEN** ADR 0075, `CONTEXT-MAP.md`, and `camel-cli/CONTEXT.md` describe the same native Linux preview scope and no unsupported performance claim

### Requirement: Permanent v1 non-goals

The R1 artifact SHALL preserve the sealed deployment-unit wall: no watch or hot reload, runtime file discovery or globbing, ambient `Camel.toml`, compile-time `CAMEL_*` overrides, wider artifact-runtime arguments, compression, signing, cross-target compilation, or R2 deploy-time asset embedding. Compile-time source selection may use explicit `--config` and `--profile` options, but runtime accepts only `--report`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface. R1 SHALL keep one logical entry point even though its store contains multiple documents. R3 may extend entry-point cardinality using this store without changing R1 runtime semantics.

#### Scenario: Permanent non-goals remain outside the artifact contract

- **GIVEN** an operator or roadmap proposal requests watch/hot-reload, runtime file discovery/globbing, ambient `Camel.toml`, a command argument beyond `--report`/`--help`/`--version`/`--manifest` other than the sanctioned R4 signature-verification surface, or a compile-time `CAMEL_*` configuration override
- **WHEN** the proposal is evaluated against the v1 compiled-artifact contract
- **THEN** the capability is rejected as a permanent non-goal rather than added to the artifact surface

#### Scenario: Embedded configuration does not become ambient configuration

- **GIVEN** a compiled artifact is deployed without its source tree or an ambient `Camel.toml`
- **WHEN** the artifact starts
- **THEN** it loads no external configuration, resolves only permitted deployment-time `${env:NAME}` expressions from the embedded document, and performs no runtime discovery, globbing, watch, or hot-reload behavior

#### Scenario: Artifact arguments stay narrow

- **GIVEN** a valid compiled artifact
- **WHEN** the operator supplies an argument other than `--report`, `--help`, `--version`, `--manifest`, or the sanctioned R4 signature-verification surface
- **THEN** the artifact rejects the argument with exit 2 and does not expand its command surface

#### Scenario: MUST-NOT capabilities remain rejected

- **GIVEN** a proposal to add ambient configuration, runtime discovery, compile-time overrides, watch, a wider artifact argument surface, or asset embedding to R1
- **WHEN** the proposal is evaluated against the sealed-artifact contract
- **THEN** it is rejected as outside this change and recorded for its roadmap owner rather than added to the virtual-store implementation

### Requirement: Resolve and confine compile-time sources

The compiler SHALL normalize source names to UTF-8 relative paths using `/`, anchor resolution to the explicitly selected `Camel.toml` root (or the primary document directory when no configuration root is required), and reject absolute paths, empty or `.` components, `..` traversal, non-UTF-8 names, symlink escapes, duplicate canonical targets, missing sources, and references outside the root. Resolution SHALL be deterministic and SHALL not depend on ambient current-directory discovery. The compiler SHALL reject unsupported asset-bearing fields rather than treating them as virtual documents.

#### Scenario: Path escape fails before output

- **GIVEN** a route source pattern or include that resolves outside the selected root, including through a symlink
- **WHEN** compilation runs
- **THEN** it exits 2 with a confinement diagnostic and does not create a usable artifact

#### Scenario: Overlapping sources fail closed

- **GIVEN** two route-file patterns resolve the same canonical source or two names normalize to the same logical path
- **WHEN** compilation runs
- **THEN** it exits 2 naming the duplicate and does not silently embed two copies

#### Scenario: Deterministic ordering is stable

- **GIVEN** the same source tree presented with filesystem enumeration in different orders
- **WHEN** compilation runs twice
- **THEN** the store index, source plan, and embedded bytes are identical

#### Scenario: Store offsets and schema are validated

- **GIVEN** a marked artifact with an unknown `store_schema`, an out-of-bounds or overlapping content range, a missing source-plan target, noncanonical index ordering, or unreferenced content
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot with a format diagnostic and does not reinterpret the bytes as a different store schema

