# Design: cli-compile

## Approach

Implement a small trailer codec and keep the runtime seam narrow. The compiler reads one normalized document as raw text before interpolation, validates the v1 asset allowlist, builds a manifest, and appends `payload || manifest || fixed footer` to a copy of `current_exe()`. Normalization means valid UTF-8 only, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and enforce a 16 MiB encoded-byte limit. The manifest is canonical UTF-8 JSON with lexicographically ordered keys and source-ordered arrays; source name is input path relative to compile working directory. The exact 68-byte footer is `CAMELTR1` magic (8), little-endian version `u16` 1, kind `u8` (`1=route`, `2=job`), zero reserved `u8`, little-endian payload and manifest lengths (`u64` each), BLAKE3 (32), and terminal `CAMELTR1` magic (8). Checksum covers ASCII bytes `rust-camel-trailer-v1` followed by one `0x00` byte, then the little-endian encoded version, kind, payload length, manifest length, payload, and manifest; it excludes both magic fields and reserved byte. The reader probes the final 68 bytes: no exact terminal magic means normal CLI; exact terminal magic with invalid fields, bounds, header magic, or checksum fails closed. A truncation that removes the terminal marker is indistinguishable from a trailer-free image and follows normal CLI behavior; marked truncation fails closed.

Self-detection runs before Clap parsing. A trailer-free image follows the existing CLI path. A valid artifact accepts only `--report <path>`, `--help`, `--version`, and `--manifest`; unknown or positional runtime arguments, duplicate exclusive flags, and missing report values exit 2. The embedded document records an explicit kind and virtual source identity. A route artifact uses the existing route run lifecycle with an in-memory document store and no filesystem discovery. A job artifact uses the existing single-document job execution/report lifecycle, with its route source constrained to the embedded document and a default in-memory configuration; any config, profile, include, or external route dependency is rejected during compilation. Route `--report <path>` writes JSON object `{ "kind": "route", "status": "completed"|"failed", "error": string|null }` after boot/runtime completion or failure; job reports retain their existing schema. The embedded document is passed to an embedded-text discovery helper that preserves existing `${env:}` interpolation, template handling, provenance, route parsing, lowering, component boot, context start, and signal shutdown. Watch is disabled. No temporary document extraction is permitted.

The v1 compiler uses a field-level matrix: route-source fields (`routeFiles`, `routeFilesFromRoot`, globs, includes, external paths), configuration fields (`Camel.toml`, profiles, `CAMEL_*` compile overrides), and asset-bearing endpoint fields (certificates, private keys, CA files, WASM/plugins, XSLT/XSD, SQL files, static directories, literal secret files, dynamic placeholders) are forbidden and named. Runtime endpoint URI paths, runtime `${env:}` values, and deploy-side network/file I/O remain permitted. The manifest exposes runtime version, artifact kind, embedded components, required environment variables without defaults, and listener declarations as literal ports or unresolved expressions. Route `--report <path>` writes `{ "kind": "route", "status": "completed"|"failed", "error": string|null }`; job artifacts use the existing job outcome report. Empty or zero-route input follows existing discovery validation and does not silently become a successful artifact.

## Affected crates

- `camel-cli`: `compile` command, trailer codec, self-detection, artifact argument guard, manifest/report output, compile and runtime tests.
- `camel-dsl`: embedded-text discovery entry point reusing the existing parse/interpolate/lower pipeline.
- `camel-api` / `camel-core`: no new serialized route model and no changes to control-plane or data-plane contracts.
- Documentation: ADR 0075, `CONTEXT-MAP.md`, and `crates/camel-cli/CONTEXT.md`.

## Architecture boundaries

The artifact embeds authoring text, not `RouteDefinition`, `BuilderStep`, processors, providers, or typed placeholders. DSL remains responsible for parsing and interpolation; runtime remains responsible for route lowering, component wiring, and lifecycle. The trailer is a deployment/control-plane packaging concern in `camel-cli`; runtime endpoint file, network, and environment access stays in existing components. No sandbox or new capability grant is introduced: deployment owns process capabilities and the artifact is trusted like its source executable.

## Phases

### Phase 1: Trailer and compile pipeline
- **Goal:** Deliver tested trailer codec, manifest, raw document capture, asset rejection, and `camel compile` output.
- **Dependencies:** Existing BLAKE3 workspace dependency; native Linux executable behavior; P0 benchmark evidence remains external.
- **Externally-visible types/interfaces:** `camel compile`, trailer format, manifest output.
- **Deliverable:** Immutable executable artifact with compile-time diagnostics and codec tests.
- **Exit-criteria:** Round-trip, checksum, truncation, version, asset rejection, and compile CLI tests pass.

### Phase 2: Artifact runtime
- **Goal:** Detect trailers before Clap and boot embedded text through existing discovery/runtime seams.
- **Dependencies:** Phase 1 trailer format and embedded-text discovery helper.
- **Externally-visible types/interfaces:** artifact argv contract and `--manifest` output.
- **Deliverable:** Single-document artifact runs without source files or temp extraction.
- **Exit-criteria:** Runtime interpolation, fallback CLI, argument exits, read-only-root, and no-watch tests pass.

### Phase 3: Decision and domain documentation
- **Goal:** Record format, trust, environment, platform, and scope decisions.
- **Dependencies:** Implemented and tested Phase 1–2 behavior.
- **Externally-visible types/interfaces:** ADR 0075 and canonical domain terms.
- **Deliverable:** Updated ADR, context map, crate context, and capability delta.
- **Exit-criteria:** Documentation lint and citation checks pass; no unsupported performance claim is present.

## Alternatives considered

- **ELF sections or libsui:** rejected for v1 because release targets and artifact handling require a format-agnostic result; append-only trailer also avoids a new dependency.
- **Serialized DSL AST or compiled steps:** rejected because `${env:}` placeholders are resolved during discovery and runtime route structures contain non-serializable traits and processors.
- **Compression:** rejected for small documents and zero-dependency discipline.
- **Temporary extraction:** rejected for read-only roots and unnecessary single-document I/O.
