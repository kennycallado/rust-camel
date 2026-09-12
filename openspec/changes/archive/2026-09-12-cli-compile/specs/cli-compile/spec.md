## ADDED Requirements

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

The artifact SHALL feed the embedded document into the existing parse, runtime interpolation, lowering, boot, and start path without extracting files or enabling watch behavior. Route artifacts SHALL use virtual source identity `compiled://<manifest.source_name>` and the existing report writer for `--report <path>`. Job artifacts SHALL use the existing single-document job lifecycle with in-memory default configuration, the same virtual identity, the embedded document as its only route source, and existing job outcome reporting.

#### Scenario: Read-only deployment

- **GIVEN** a valid route or job artifact running with a read-only root filesystem and required runtime environment
- **WHEN** the artifact starts
- **THEN** it boots and runs without temporary extraction or writes other than an explicitly requested report

### Requirement: Restrict artifact arguments and expose manifest

The artifact SHALL accept `--report <path>`, `--help`, `--version`, and `--manifest`; duplicate exclusive flags, missing report values, positional arguments, and all other arguments SHALL exit 2. `--manifest` SHALL print runtime version, artifact kind, embedded components, required environment variables without defaults, and listener declarations as literal ports or unresolved expressions. Route `--report <path>` SHALL write JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }` after boot/runtime completion or failure; job reports SHALL use the existing job outcome schema. Route artifacts SHALL exit 0 after graceful completion, 1 on pipeline failure, and 2 on boot or report-write failure.

#### Scenario: Unknown artifact argument

- **GIVEN** a valid artifact
- **WHEN** the operator supplies an unsupported argument
- **THEN** the artifact exits 2 and reports the rejected argument

#### Scenario: Manifest inspection

- **GIVEN** a valid artifact
- **WHEN** the operator runs `./app --manifest`
- **THEN** the artifact prints its operational manifest and exits 0 without booting routes

### Requirement: Record the artifact decision

The project SHALL document the format and trust boundary in ADR 0075 and define `compiled artifact`, `EOF trailer`, `self-detect`, and `operational manifest` in the canonical context documentation with citations to the ADR.

#### Scenario: Documentation remains aligned

- **GIVEN** the implementation and capability delta are reviewed
- **WHEN** context and documentation lint runs
- **THEN** ADR 0075, `CONTEXT-MAP.md`, and `camel-cli/CONTEXT.md` describe the same native Linux preview scope and no unsupported performance claim
