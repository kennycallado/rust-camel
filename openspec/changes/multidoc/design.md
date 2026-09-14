# Design: multidoc

## Approach

Extend the CAMELTR1 trailer family with version 2. The v2 byte layout is `CAMELTR1 || content || index || manifest || footer`, where the footer is `CAMELTR1` (8), little-endian `u16 version=2`, `u8 kind` (`1=route`, `2=job`), zero `u8 flags`, little-endian `u64 content_len`, `u64 index_len`, `u64 manifest_len`, BLAKE3 (32), and terminal `CAMELTR1` (8), for 76 bytes total. The checksum domain is ASCII `rust-camel-trailer-v2`, one zero byte, the little-endian version/kind/three lengths, then content, index, and manifest; magic and flags are excluded. Keep the family marker so old readers recognize a marked artifact and reject unsupported versions instead of silently treating it as an ordinary executable. Version 1 remains readable by a v2 reader as a one-entry store; v1 readers are not required to parse v2 and may reject the marked artifact as unsupported.

The store has one logical root and entries keyed by normalized UTF-8 relative paths using `/`. Each entry records path, type (`route`, `job`, `config`, `include`, or `profile`), content offset, and byte length. The index is canonical UTF-8 JSON with lexicographically ordered object keys and arrays whose order is normative. R1 supports `store_schema: 1` and `manifest_schema: 2`; other values fail closed. The index records independent `store_schema`, the logical entry point, selected profile order, configuration/include references, and the ordered route-source plan. Content entries are written in canonical path order for stable bytes, while the route-source plan preserves declared pattern order and sorts each pattern's matches by normalized logical path. Duplicate canonical targets, absolute paths, traversal components, non-UTF-8 names, symlink escapes, and out-of-root sources fail before output creation. The aggregate normalized document/config bytes remain capped at 16 MiB.

The compiler accepts explicit `--config <Camel.toml>` and repeatable `--profile <name>` options. With no `--config`, it embeds no configuration and selects no profile; it never discovers ambient configuration. With `--config`, it resolves that file's ordered includes and requested profile inputs only while compiling. It captures `Camel.toml`, selected profile text, include text, and every route document before environment interpolation as typed store entries. Runtime receives a `VirtualDocumentStore` and a single entry plan; it builds in-memory `CamelConfig` from the indexed config/profile/include entries, then obtains route text by index lookup and invokes the existing DSL interpolation, parsing, provenance, lowering, boot, and lifecycle code. Runtime never opens source paths, loads ambient `Camel.toml`, expands globs, materializes files, or enables watch. `${env:NAME}` expressions remain in stored text and resolve only at deployment.

The operational manifest gains independent `manifest_schema` and `embedded_files` metadata. Trailer version describes framing; manifest schema describes JSON meaning. `--manifest` exposes the canonical manifest without boot. R1 keeps one logical entry point even though its store contains many documents; R3 may add multiple entry points without replacing the store.

## Affected crates

- `camel-cli`: compile source resolution, virtual-store/index and trailer codecs, manifest schema, artifact runtime, compile and confinement tests.
- `camel-dsl`: additive embedded-store discovery helper that parses multiple virtual documents without filesystem access and preserves source identity.
- `camel-config`: only additive compile-time source-resolution integration if existing route-file/config APIs cannot be reused; no runtime ambient-config path.
- Documentation: ADR 0075, `CONTEXT-MAP.md`, and `crates/camel-cli/CONTEXT.md`.

## Architecture boundaries

`camel-cli` owns deployment packaging and trailer decoding. `camel-dsl` owns document parsing, interpolation, route-source semantics, and diagnostic provenance. `camel-core` remains responsible for runtime lifecycle and component boot; it receives already parsed route definitions and gains no serialization contract. Runtime component file/network I/O remains deployment-time endpoint behavior, distinct from compile-time bundled documents. No control-plane capability or sandbox model changes.

## Phases

### Phase 1: Virtual store format and compile resolution
- **Goal:** Define and test canonical store/index/trailer v2 bytes, path confinement, deterministic source resolution, manifest schema, and compile output.
- **Dependencies:** CAMELTR1 v1 codec, existing route/job discovery and include/profile resolution, BLAKE3.
- **Externally-visible types/interfaces:** trailer v2, store schema, embedded-file manifest entries; `camel compile` accepts multi-document source plans.
- **Deliverable:** Artifact containing one logical entry and all resolved route documents.
- **Exit-criteria:** codec, ordering, bounds, path-confined resolution, duplicate, aggregate-cap, and compile CLI tests pass.

### Phase 2: Virtual-store runtime
- **Goal:** Run route and job documents from memory through existing lifecycles without source-tree access.
- **Dependencies:** Phase 1 store/index and additive embedded discovery seam.
- **Externally-visible types/interfaces:** virtual-store discovery/runtime input; unchanged narrow artifact arguments.
- **Deliverable:** Read-only, source-tree-free multi-document artifact runtime.
- **Exit-criteria:** routeFiles/includes/jobs, runtime environment, v1 compatibility, marked-corruption, no-I/O/no-glob, and no-extraction tests pass.

### Phase 3: Documentation and contract alignment
- **Goal:** Record virtual-store vocabulary, trailer evolution, MUST-NOT wall, and R2/R3 extension seams.
- **Dependencies:** Phases 1 and 2 behavior.
- **Externally-visible types/interfaces:** ADR and canonical context terms.
- **Deliverable:** Updated ADR 0075, context map, crate context, and OpenSpec delta.
- **Exit-criteria:** citation/context lint and OpenSpec validation pass; docs do not claim R2/R3 are shipped.

## Alternatives considered

- **Concatenate documents into one text:** rejected because document boundaries, logical paths, provenance, and future assets would be lost.
- **Serialize parsed routes or processors:** rejected by ADR 0075; runtime traits and deployment-time placeholders are not a stable artifact boundary.
- **Extract files at startup:** rejected because read-only roots and confinement require zero materialization.
- **Runtime discovery with an embedded directory:** rejected because it preserves forbidden globbing and ambient configuration semantics.
- **Replace CAMELTR1 with CAMELTR2:** rejected for family compatibility; retaining the marker gives old readers a safe unsupported-version failure.
