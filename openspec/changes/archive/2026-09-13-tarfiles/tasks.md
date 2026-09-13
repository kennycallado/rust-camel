# Tasks: tarfiles

## Phase 1: Archive format implementations and registry

### camel-processor TAR

#### Task 1.1: Implement bounded TAR format

**Files:**
- `crates/camel-processor/Cargo.toml` (modified)
- `crates/camel-processor/src/data_format/tar.rs` (new)

**Steps:**
1. Add `tar.workspace = true` and `flate2.workspace = true` to the processor manifest without adding new workspace dependencies.
2. Add `TarConfig` and `TarDataFormat` with `serde(default, deny_unknown_fields)`, ZIP-matching input/decompressed caps, and `allow_multi_entry`.
3. Implement marshal from materialized bytes, rejecting `Body::Empty` and `Body::Stream`, enforcing `max_input_size`, and writing one regular-file entry named `payload`.
4. Implement unmarshal from bytes/text by scanning TAR entries, ignoring paths and non-regular entries, selecting the first regular file, bounding returned bytes, and enforcing regular-file cardinality.
5. Add archive fixtures and unit tests for round trips, regular-file policy and warning, non-regular/malicious paths, malformed headers, stream/empty rejection, zero-length materialized bodies, and unknown configuration.

**Tests:**
- `tar_round_trip_bytes`: setup non-empty `Body::Bytes`; action marshal then unmarshal with `TarDataFormat`; assert original bytes; command `cargo test -p camel-processor --lib data_format::tar::tests::tar_round_trip_bytes`; expected fail before implementation, pass after.
- `tar_regular_entry_policy`: setup zero, one, and two regular files plus directories/links; action unmarshal with `allow_multi_entry` false and true; assert zero errors, one succeeds, multiple errors or returns first regular file, and the multi-entry path emits a warning; command `cargo test -p camel-processor --lib data_format::tar::tests::tar_regular_entry_policy`; expected fail before implementation, pass after.
- `tar_non_regular_entries_and_malicious_paths_are_ignored`: setup directory, symlink, hardlink, device, `../escape`, and absolute-path entries before a regular file; action unmarshal; assert regular-file bytes return, no filesystem path is accessed, and non-regular entries are not returned; command `cargo test -p camel-processor --lib data_format::tar::tests::tar_non_regular_entries_and_malicious_paths_are_ignored`; expected fail before implementation, pass after.
- `malformed_tar_input_rejected`: setup garbage and truncated TAR headers; action unmarshal; assert established `CamelError::TypeConversionFailed`; command `cargo test -p camel-processor --lib data_format::tar::tests::malformed_tar_input_rejected`; expected fail before implementation, pass after.
- `tar_materialized_empty_and_stream_bodies`: setup zero-length `Body::Bytes`, zero-length `Body::Text`, `Body::Empty`, and `Body::Stream`; action marshal/unmarshal; assert materialized empty values follow TAR semantics, empty body fails, and stream fails without consumption; command `cargo test -p camel-processor --lib data_format::tar::tests::tar_materialized_empty_and_stream_bodies`; expected fail before implementation, pass after.

**Acceptance:**
- TAR uses no filesystem I/O and never follows archive paths or links.
- `max_input_size` and regular-file/output limits are enforced before unbounded materialization.
- Named TAR tests pass; `cargo fmt --check` and `cargo clippy -p camel-processor -- -D warnings` exit 0.

- [x] 1.1

### camel-processor GZIP

#### Task 1.2: Implement bounded standalone GZIP format

**Files:**
- `crates/camel-processor/src/data_format/gzip.rs` (new)

**Steps:**
1. Add `GzipConfig` and `GzipDataFormat` with bounded input/decompressed sizes, optional validated compression level, and fail-closed serde parsing.
2. Implement marshal with `GzEncoder` over materialized bytes and unmarshal with `GzDecoder` wrapped by a `take(max_decompressed_size + 1)` limit.
3. Reject `Body::Empty` and `Body::Stream`, preserve zero-length materialized body semantics, and add malformed, limit, round-trip, cross-format, and configuration tests.

**Tests:**
- `gzip_round_trip_bytes`: setup non-empty bytes; action marshal then unmarshal; assert original bytes; command `cargo test -p camel-processor --lib data_format::gzip::tests::gzip_round_trip_bytes`; expected fail before implementation, pass after.
- `gzip_decompression_limit_covers_full_stream`: setup compressed bytes exceeding configured decompressed cap; action unmarshal; assert `CamelError` before bytes beyond cap are materialized; command `cargo test -p camel-processor --lib data_format::gzip::tests::gzip_decompression_limit_covers_full_stream`; expected fail before implementation, pass after.
- `malformed_gzip_input_rejected`: setup truncated and invalid gzip data; action unmarshal; assert `CamelError::TypeConversionFailed`; command `cargo test -p camel-processor --lib data_format::gzip::tests::malformed_gzip_input_rejected`; expected fail before implementation, pass after.

**Acceptance:**
- GZIP decompression is bounded before materialization and invalid levels/configuration fail closed.
- Named GZIP tests pass; `cargo fmt --check` and `cargo clippy -p camel-processor -- -D warnings` exit 0.

- [x] 1.2

### camel-processor TAR.GZ

#### Task 1.3: Implement combined TAR.GZ format and cross-format behavior

**Files:**
- `crates/camel-processor/src/data_format/tar_gz.rs` (new)

**Steps:**
1. Add `TarGzConfig` and `TarGzDataFormat` with TAR entry policy, gzip compression level, input cap, and full decoded-stream cap.
2. Implement marshal as one `payload` TAR regular file wrapped in GZIP and unmarshal by capping all GZIP output, then applying TAR regular-file selection.
3. Add tests proving `tar.gz` output decodes through `gzip` then `tar`, composed `tar` then `gzip` decodes through `tar.gz`, and malformed/non-regular/multi-entry cases remain bounded and safe.

**Tests:**
- `gzip_and_tar_gz_cross_decode`: setup input bytes; action marshal as tar then gzip and as tar.gz; assert both directions decode through gzip+tar and tar.gz to original bytes; command `cargo test -p camel-processor --lib data_format::tar_gz::tests::gzip_and_tar_gz_cross_decode`; expected fail before implementation, pass after.
- `tar_gz_decompression_limit_includes_tar_stream`: setup compressed TAR whose decoded headers/padding/skipped entries exceed the cap; action unmarshal; assert failure before cap overrun; command `cargo test -p camel-processor --lib data_format::tar_gz::tests::tar_gz_decompression_limit_includes_tar_stream`; expected fail before implementation, pass after.

**Acceptance:**
- `tar.gz` and composed `tar` plus `gzip` are wire-compatible in both specified directions.
- Full decoded GZIP stream, not only selected payload, is capped; no extraction occurs.
- Named TAR.GZ tests pass; `cargo fmt --check` and `cargo clippy -p camel-processor -- -D warnings` exit 0.

- [x] 1.3

### camel-processor registry

#### Task 1.4: Register and expose archive formats

**Files:**
- `crates/camel-processor/src/data_format/mod.rs` (modified)
- `crates/camel-processor/src/lib.rs` (modified)

**Steps:**
1. Add module declarations and public re-exports for all six archive config/data-format types.
2. Add exact factory arms for `tar`, `gzip`, and `tar.gz` using each config parser.
3. Add factory tests for exact names and unknown configuration fields.

**Tests:**
- `builtin_archive_formats_resolve`: setup built-in factory; action resolve `tar`, `gzip`, and `tar.gz`; assert each format name matches; command `cargo test -p camel-processor --lib data_format::tests::builtin_archive_formats_resolve`; expected fail before implementation, pass after.
- `builtin_archive_config_rejects_unknown_fields`: setup unknown key for each format; action call factory; assert `CamelError::RouteError`; command `cargo test -p camel-processor --lib data_format::tests::builtin_archive_config_rejects_unknown_fields`; expected fail before implementation, pass after.

**Acceptance:**
- All six types compile through `camel_processor` public exports.
- Factory resolves exact names and does not add splitter registration.
- `cargo test -p camel-processor --lib data_format` passes.

- [x] 1.4

## Phase 2: Documentation and validation metadata

### Documentation and context

#### Task 2.1: Align builder docs, processor context, and format catalog

**Files:**
- `crates/camel-builder/src/lib.rs` (modified)
- `crates/camel-processor/CONTEXT.md` (modified)
- `docs/src/data-formats/index.md` (modified)

**Steps:**
1. Update marshal/unmarshal Rustdoc lists with `tar`, `gzip`, and `tar.gz`.
2. Add the six public archive types to the processor API inventory with valid citations.
3. Add catalog rows and prose describing single-entry TAR semantics, standalone gzip composition, combined tar.gz, limits, and splitter/extraction non-goals.

**Tests:**
- `context_citations_for_archive_api`: setup updated processor context; action run `cargo xtask lint-context-citations`; assert exit 0 and all referenced symbols/locations resolve; command `cargo xtask lint-context-citations`; expected fail only if citations are malformed before implementation, pass after.

**Acceptance:**
- Builder docs, catalog, and processor context list all three names and do not claim TAR splitting or disk extraction.
- `cargo xtask lint-context-citations` and `cargo fmt --check --all` exit 0.

- [x] 2.1

### Validation metadata

#### Task 2.2: Verify schema and format validation remain aligned

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (modified only if a closed format validation list is found)
- `crates/camel-lint/schema/route-schema.json` (modified only if a closed format enum is found)

**Steps:**
1. Inspect route schema and lint validation paths and confirm `marshal`/`unmarshal` accept arbitrary format strings before registry resolution.
2. If a closed list exists in the checked-in revision, add exact `tar`, `gzip`, and `tar.gz` values without removing existing values; otherwise leave both files unchanged.
3. Run schema validation against routes using each new format and record the result in the task review.

**Tests:**
- `schema_accepts_archive_format_names`: setup route schema inputs using `tar`, `gzip`, and `tar.gz`; action validate each route; assert no schema enum rejection; command `cargo xtask schema --check`; expected pass on current schema and after any required update.

**Acceptance:**
- `cargo xtask schema --check` exits 0.
- No unnecessary edits are made to schema/lint files when format names are intentionally open strings.
- Schema inspection confirmed `marshal` and `unmarshal` are open strings; `cargo xtask schema --check` passed and routes using `tar`, `gzip`, and `tar.gz` produced no schema diagnostics.

- [x] 2.2
