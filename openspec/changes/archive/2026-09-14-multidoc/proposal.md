# Proposal: multidoc

## Why

`camel compile` currently seals one route or job document. Documents that use `routeFiles`, `routeFilesFromRoot`, or includes therefore cannot become deployment-equivalent artifacts. Runtime would need the source tree, ambient configuration, or file discovery, which violates the sealed-artifact contract established by ADR 0075 and blocks asset embedding and multi-entry jobs.

This change is R1 of the compile roadmap (bd `rc-86m92`): introduce one in-memory virtual document store that carries every resolved route document needed by one logical entry point. The store is the keystone for later asset and multi-entry work, but does not implement those features now.

## What Changes

- Resolve route sources and supported job route sources at compile time, using the selected `Camel.toml` root and deterministic logical relative paths.
- Embed normalized pre-interpolation route, job, `Camel.toml`, include, and selected-profile text plus a canonical store index in the executable trailer.
- Add a separate `manifest_schema` field and embedded-file metadata. Bump the trailer format version without reusing it as the manifest schema.
- Run embedded documents from memory with no runtime globbing, source-tree reads, configuration discovery, extraction, watch, or hot reload.
- Preserve runtime `${env:}` interpolation and the narrow artifact argument surface.
- Keep unsupported deploy-time assets rejected; R2 owns asset embedding.

Affected crates: `camel-cli`, `camel-dsl`, and the compile-related documentation. `camel-core` and route processor contracts remain unchanged.

## Acceptance criteria

- A route document with ordered multi-file sources and selected configuration compiles into one artifact whose virtual store preserves logical paths, configuration references, and deterministic source order.
- A compiled artifact boots from its store after its source tree and ambient `Camel.toml` are unavailable, with zero extraction and zero runtime glob/file discovery.
- Jobs preserve their existing route-source semantics while consuming the store; one logical entry point remains the R1 limit.
- Trailer integrity, schema/version checks, path confinement, duplicate detection, malformed index handling, and the 16 MiB aggregate cap fail closed before boot.
- Existing single-document artifacts remain readable, and all MUST-NOT capabilities remain rejected.

## Risk budget

Acceptable: trailer version evolution, a new canonical index codec, and additive discovery/runtime seams. Out of bounds: AST or processor serialization, ambient configuration fallback, runtime source discovery, asset embedding, compression, signing, cross-target compilation, watch/hot reload, or a wider artifact argument surface. The aggregate embedded text cap remains 16 MiB until R2 makes caps configurable.
