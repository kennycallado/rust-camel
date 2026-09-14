## MODIFIED Requirements

### Requirement: Compile a native single-document artifact

The CLI SHALL provide `camel compile <document> -o <artifact>` that resolves one logical route or job entry point and appends `CAMELTR1 || content || index || manifest || footer` to a copy of the current executable, preserving executable permissions. The 76-byte v2 footer contains `CAMELTR1`, little-endian `u16` version 2, `u8` kind (`1=route`, `2=job`), zero `u8` flags, little-endian `u64` content/index/manifest lengths, BLAKE3, and terminal `CAMELTR1`. The checksum SHALL cover ASCII `rust-camel-trailer-v2`, one zero byte, the encoded version/kind/three lengths, content, index, and manifest, excluding magic and flags. Normalization SHALL accept valid UTF-8, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and reject invalid UTF-8. The content SHALL contain normalized pre-interpolation route, job, `Camel.toml`, include, and selected-profile entries. The index SHALL be canonical UTF-8 JSON with independent `store_schema: 1`, typed entries, offsets, lengths, one logical entry point, configuration references, and ordered source plan. The manifest SHALL use independent `manifest_schema: 2`. The compiler SHALL resolve route files, includes, profiles, and supported job route sources at compile time using the explicitly supplied `--config <Camel.toml>` and repeated `--profile <name>` options; without `--config`, it SHALL embed no configuration and select no profile, and it SHALL never discover ambient configuration. The store SHALL preserve logical relative paths and the ordered source plan. Aggregate normalized embedded bytes SHALL remain limited to 16 MiB. Trailer framing SHALL retain the `CAMELTR1` family marker; v2 readers SHALL support v1 as a one-entry store, while v1 readers may reject v2 as unsupported. Trailer version SHALL NOT define store or manifest schema.

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
- **THEN** the index identifies each document separately and runtime can request each by logical path without concatenating or rewriting document text

#### Scenario: Single-document v1 remains readable

- **GIVEN** an existing valid version-1 single-document artifact
- **WHEN** a version-2-capable executable starts it
- **THEN** the reader exposes it as a one-entry virtual store and preserves existing runtime behavior

#### Scenario: Cross-target compilation remains rejected

- **GIVEN** a requested target different from the native Linux target
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 with a native-Linux-only diagnostic and does not create an artifact

## MODIFIED Requirements

### Requirement: Reject unsupported compile-time assets

The compiler SHALL reject unsupported asset-bearing endpoint fields and compile-time `CAMEL_*` overrides, while permitting route files, includes, `Camel.toml`, selected profiles, and job route sources only when explicitly resolved and embedded by the virtual store. It SHALL reject certificates, private keys, CA files, WASM or plugin files, XSLT/XSD, SQL files, static directories, literal secret files, and dynamic placeholders in those asset fields. Runtime endpoint URI paths, deployment-time `${env:}` values, and deploy-side network/file I/O remain permitted.

#### Scenario: Unsupported asset names the reason

- **GIVEN** a document referencing an unsupported asset-bearing endpoint field
- **WHEN** the operator invokes compilation
- **THEN** compilation exits 2 without writing a usable artifact and names the rejected asset class

#### Scenario: Embedded job sources are allowed

- **GIVEN** a job document with route sources and configuration selected through explicit compile inputs
- **WHEN** the operator invokes compilation
- **THEN** the route sources and configuration are embedded in the virtual store and the job does not require external source files at runtime

## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: Run embedded documents without extraction

The artifact SHALL feed the indexed documents through the existing parse, runtime interpolation, lowering, boot, and start paths using an in-memory virtual store. Runtime SHALL build configuration from embedded `Camel.toml`, include, and selected-profile entries. Runtime SHALL perform no source-tree reads, glob expansion, ambient `Camel.toml` or profile loading, canonicalization, temporary extraction, watch, or hot reload. Source diagnostics SHALL use `compiled://<logical-path>` identities. `${env:NAME}` expressions SHALL resolve from the deployment environment, not the compiler environment. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job reports SHALL use the existing job outcome schema. Boot and report-write failures SHALL exit 2; route pipeline failures SHALL exit 1; completed routes SHALL exit 0.

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

#### Scenario: Read-only deployment remains supported

- **GIVEN** a valid multi-document artifact running with a read-only root filesystem
- **WHEN** it boots and receives deployment environment values
- **THEN** it runs without materializing documents or writing files other than an explicitly requested report

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept only `--report <path>`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface. Duplicate exclusive flags, missing report values, positional arguments, and other arguments SHALL exit 2. The operational manifest SHALL contain a separate `manifest_schema` field and an `embedded_files` list with canonical logical paths, document kinds, byte lengths, and content digests. Manifest schema values SHALL be validated independently from trailer version. `--manifest` SHALL print this metadata without booting. The manifest SHALL not contain compile-time environment values and SHALL list required environment variables without defaults.

#### Scenario: Manifest inspection exposes store metadata

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the command exits 0 without booting and prints manifest schema, artifact kind, logical embedded-file metadata, components, required environment names, and listener declarations

#### Scenario: Unknown manifest schema fails closed

- **GIVEN** a marked artifact with a manifest schema that the reader does not support
- **WHEN** the artifact starts
- **THEN** it exits 2 before boot and does not reinterpret trailer version as manifest schema

#### Scenario: Artifact argument contract remains narrow

- **GIVEN** a valid artifact
- **WHEN** the operator supplies an unknown, positional, duplicate-exclusive, or incomplete report argument
- **THEN** it exits 2, names the rejected argument, and does not boot

#### Scenario: Manifest includes operational version

- **GIVEN** a valid multi-document artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** output includes runtime version, artifact kind, embedded files, required environment names, and listener declarations without booting

### Requirement: Permanent v1 non-goals

The R1 artifact SHALL preserve the sealed deployment-unit wall: no watch or hot reload, runtime file discovery or globbing, ambient `Camel.toml`, compile-time `CAMEL_*` overrides, wider artifact-runtime arguments, compression, signing, cross-target compilation, or R2 deploy-time asset embedding. Compile-time source selection may use explicit `--config` and `--profile` options, but runtime accepts only `--report`, `--help`, `--version`, and `--manifest` plus the sanctioned R4 signature-verification surface. R1 SHALL keep one logical entry point even though its store contains multiple documents. R3 may extend entry-point cardinality using this store without changing R1 runtime semantics.

#### Scenario: MUST-NOT capabilities remain rejected

- **GIVEN** a proposal to add ambient configuration, runtime discovery, compile-time overrides, watch, a wider artifact argument surface, or asset embedding to R1
- **WHEN** the proposal is evaluated against the sealed-artifact contract
- **THEN** it is rejected as outside this change and recorded for its roadmap owner rather than added to the virtual-store implementation
