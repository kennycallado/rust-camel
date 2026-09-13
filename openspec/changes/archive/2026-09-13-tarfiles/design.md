# Design: tarfiles

## Approach

Implement three `DataFormat` implementations in `camel-processor`: `TarDataFormat`, `GzipDataFormat`, and `TarGzDataFormat`. Each accepts `Body::Bytes` and `Body::Text` for unmarshal and the same materializable body types accepted by ZIP for marshal. Marshal emits one archive entry named `payload`; unmarshal consumes archive bytes in memory and returns bytes.

TAR has no central directory, so unmarshal walks entries in stream order. It selects the first regular file, rejects an archive with no regular file, and applies `allow_multi_entry` when additional entries exist. Non-regular entries are never followed or materialized. A fixed marshal entry name avoids attacker-controlled paths. Existing path-confinement precedent `rc-0ks57` remains authoritative for any future disk extraction, but v1 performs no disk I/O.

`gzip` is a standalone bounded `GzEncoder`/`GzDecoder` format. `tar.gz` directly composes TAR creation with gzip compression and decompression with TAR reading. Thus users may compose `tar` then `gzip`, while the combined format avoids requiring callers to manage the intermediate representation. `tar.gz` output must decode through `gzip` then `tar`, and composed `tar` then `gzip` output must decode through `tar.gz`. For compressed TAR, `max_decompressed_size` caps every byte emitted by the gzip decoder before TAR parsing, including headers, padding, and skipped entries. All formats use bounded input and output limits and fail closed on unknown configuration fields, following ZIP and ADR-0032/0033/0038 trust and DoS-limit rules.

Register names in the built-in factory and public re-exports. Update builder documentation, lint/schema format metadata, processor context API inventory, and the data-format catalog. Do not modify `zip_splitter`; explicitly document that entry-per-exchange splitting is deferred.

## Affected crates

- `camel-processor`: format implementations, factory registration, public exports, tests.
- `camel-builder`: supported-format documentation.
- `camel-dsl` / `camel-lint`: format validation/schema metadata if their closed lists require updates.
- `docs`: data-format catalog and format behavior documentation.

## Architecture boundaries

This is a processor data-plane capability. It does not add runtime state, control-plane commands, components, services, languages, or functions. DSL resolution continues through the existing built-in factory. The implementation stays in memory and uses existing workspace `tar` and `flate2` dependencies.

## Phases

### Phase 1: Archive format implementations and registry
- **Goal:** Deliver bounded in-memory `tar`, `gzip`, and `tar.gz` formats and make them resolvable through the processor factory.
- **Dependencies:** Existing workspace `tar` and `flate2` dependencies; ZIP data-format body and limit conventions.
- **Externally-visible types/interfaces:** `TarConfig`, `TarDataFormat`, `GzipConfig`, `GzipDataFormat`, `TarGzConfig`, `TarGzDataFormat`; factory names `tar`, `gzip`, `tar.gz`.
- **Deliverable:** Processor implementations, unit tests, manifest wiring, public exports, and registry tests.
- **Exit-criteria:** Named archive unit tests, `cargo fmt --check`, and `cargo clippy -p camel-processor -- -D warnings` pass.

### Phase 2: Documentation and validation metadata
- **Goal:** Align public documentation, context inventory, and schema/lint validation with the new formats without adding a splitter.
- **Dependencies:** Phase 1 public names and registry behavior.
- **Externally-visible types/interfaces:** Builder Rustdoc and documented format catalog/context entries.
- **Deliverable:** Updated builder docs, processor context, data-format catalog, and validation metadata only where a closed list requires it.
- **Exit-criteria:** `cargo xtask schema --check`, `cargo xtask lint-context-citations`, and formatting pass; docs state splitter and extraction are out of scope.

## Alternatives considered

- Only `tar.gz`: rejected because it prevents explicit `tar`/`gzip` composition.
- Entry-per-exchange unmarshal: rejected; that is splitter behavior and would change the `DataFormat` body contract.
- Generalizing `zip_splitter`: deferred; archive splitting is a separate EIP surface and needs its own parity and security design.
- Filesystem extraction: rejected for v1 because it expands path, symlink, and device security scope.
