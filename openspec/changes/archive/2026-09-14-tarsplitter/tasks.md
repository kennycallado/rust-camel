# Tasks: tarsplitter

## camel-api contract

### Task 1.1: Add TAR archive formats to stream split contract

**Files:**
- `crates/camel-api/src/splitter.rs` (modified)
- `crates/camel-api/tests/splitter_schema.rs` (new)
- `schemas/ts/StreamSplitFormat.ts` (modified)
- `schemas/ts/StreamSplitConfig.ts` (modified)
- `schemas/canonical-route-spec.json` (modified)

**Steps:**
1. Add `Tar` and `TarGz` variants to the existing `StreamSplitFormat` enum beside `Zip`, preserving serialization names `tar` and `tar.gz` and the enum's public non-exhaustive policy.
2. Extend `StreamSplitConfig::validate` only where format-specific validation is required; retain existing cap defaults and reject invalid zero limits.
3. Add `#[serde(rename = "tar.gz")]` and `#[ts(rename = "tar.gz")]` to the `TarGz` variant so JSON, schema, and TypeScript use one public spelling.
4. Add schema assertions that require both serialized values and generated schema enum members, including exact TypeScript literal `"tar.gz"`.
5. Regenerate checked-in schema artifacts with `cargo xtask schema` and verify them with `cargo xtask schema --check`.

**Tests:**
- `stream_split_format_serializes_tar_and_tar_gz`: setup `StreamSplitConfig` with each new enum variant; action serialize to JSON; assert exact strings `"tar"` and `"tar.gz"`; command `cargo test -p camel-api --test splitter_schema`; expected pass after implementation and fail before it.
- `stream_split_schema_lists_archive_formats`: setup generated schema; action inspect `StreamSplitFormat` enum values; assert `tar`, `tar.gz`, and `zip` are present; command `cargo test -p camel-api --test splitter_schema`; expected pass after implementation and fail before it.
- `stream_split_typescript_uses_tar_gz_literal`: setup generated TypeScript binding; action inspect `StreamSplitFormat.ts`; assert exact literal `"tar.gz"` exists and `"tar_gz"` does not; command `cargo test -p camel-api --test splitter_schema`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-api --test splitter_schema` exits 0.
- `cargo xtask schema --check` exits 0 from the worktree.
- Checked-in JSON and TypeScript schema artifacts match generated output.
- No existing `Auto`, `Ndjson`, `Lines`, `Chunks`, or `Zip` serialization changes.

- [x] 1.1

## camel-processor archive splitter

### Task 2.1: Extract shared archive path validation and TAR metadata

**Files:**
- `crates/camel-processor/src/archive_splitter.rs` (new)
- `crates/camel-processor/src/zip_splitter.rs` (modified)
- `crates/camel-processor/src/tar_splitter.rs` (new)
- `crates/camel-processor/src/lib.rs` (modified)

**Steps:**
1. Move ZIP's existing `validate_entry_path` implementation and shared path-length constant into `archive_splitter.rs`, preserving ZIP error text and behavior.
2. Move `DuplicatePolicy` into `archive_splitter.rs` and re-export it from `zip_splitter` to preserve the existing public path.
3. Add `tar_splitter` module registration and define public TAR metadata constants `CAMEL_TAR_ENTRY_NAME`, `CAMEL_TAR_ENTRY_PATH`, `CAMEL_TAR_ENTRY_INDEX`, `CAMEL_TAR_ENTRY_SIZE`, and `CAMEL_TAR_ENTRY_IS_DIRECTORY`.
4. Define `TarSplitConfig` with explicit serde defaults, `deny_unknown_fields`, entry-count/per-entry/total/compressed/path caps, duplicate policy, and an `allow_empty_archive` flag defaulting to false.
5. Define TAR-specific error variants/messages for invalid paths, duplicate names, cap violations, malformed archives, and unsupported gzip members without exposing archive path bytes in filesystem operations.

**Tests:**
- `zip_path_validation_behavior_is_unchanged`: setup absolute, traversal, and safe ZIP names; action call shared validator through ZIP splitter; assert the same accepted/rejected outcomes and error text; command `cargo test -p camel-processor zip_splitter`; expected pass before and after implementation as a regression guard.
- `tar_split_config_rejects_unknown_fields`: setup JSON containing an unknown config key; action deserialize `TarSplitConfig`; assert deserialization fails; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.

**Acceptance:**
- ZIP uses the shared validator without changing existing public constants or error behavior.
- `cargo fmt --check --all` exits 0.
- `cargo clippy -p camel-processor --lib -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: Implement bounded TAR entry splitting

**Files:**
- `crates/camel-processor/src/tar_splitter.rs` (modified)

**Steps:**
1. Implement sequential TAR reading over materialized bytes, validate each entry name before body use, skip directories/links/devices, and emit regular files in archive order with zero-based emitted-entry indices.
2. Read each regular entry through `take(max_entry_bytes + 1)`, reject overflow before retaining the body, and enforce entry-count, total-decoded, compressed-input, and path-length caps.
3. Apply the shared archive duplicate policy (TAR's own contract; ZIP's reader-level collapse is pinned historical behavior, not a parity target), remove parent `Content-Length` and `Content-Type` headers, and set TAR metadata properties with exact constants from Task 2.1.
4. Preserve split cancellation/error propagation and avoid any filesystem access or link-target following.

**Tests:**
- `tar_split_emits_regular_files_in_header_order`: setup TAR with two files, a directory, symlink, hard link, and device entry; action split; assert two fragments in header order, indices `0,1`, bodies equal file bytes, and all non-regular entries omitted; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_split_rejects_traversal_and_absolute_names`: setup TAR entries named `../escape` and `/absolute`; action split each; assert path-validation error and no filesystem access; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_split_enforces_all_bounds`: setup archives exceeding entry count, per-entry bytes, total decoded bytes, compressed input, and path length separately; action split; assert the matching bounded error for each; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_split_empty_and_directory_only_archives_emit_zero`: setup empty and directory-only TAR archives with `allow_empty_archive: true`; action split; assert successful zero-fragment output; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-processor tar_splitter` exits 0.
- TAR and TAR.GZ never write to disk or follow link targets.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 2.2

### Task 2.3: Add TAR.GZ support and shared duplicate policy

**Files:**
- `crates/camel-processor/src/tar_splitter.rs` (modified)
- `crates/camel-processor/src/data_format/gzip.rs` (modified)

**Steps:**
1. Wrap the TAR splitter's decode path in the existing bounded single-member GZIP decoder and reject unsupported multi-member behavior only for TAR.GZ stream splitting; do not change standalone `GzipDataFormat` semantics.
2. Enforce compressed-input limits before decompression acceptance while retaining total decoded and per-entry limits.
3. Apply shared duplicate policy to TAR names and preserve deterministic indexed-name behavior for `AllowWithIndex`.

**Tests:**
- `tar_gz_split_matches_tar_metadata`: setup equivalent TAR and single-member TAR.GZ; action split both; assert same bodies, names, paths, indices, and sizes; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_split_applies_shared_duplicate_policy`: setup duplicate names; action split under reject and allow-with-index policies; assert reject in first mode and deterministic collision-free indexed names in second; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_gz_compressed_input_limit_is_checked`: setup TAR.GZ exceeding compressed-input limit; action split; assert bounded input error before decompression; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `tar_gz_multi_member_is_rejected`: setup a concatenated two-member GZIP stream containing TAR data; action split; assert an explicit unsupported-multi-member error rather than silently accepting only the first member; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.
- `gzip_data_format_first_member_behavior_is_unchanged`: setup a concatenated two-member GZIP payload; action unmarshal with standalone `GzipDataFormat`; assert existing first-member behavior remains unchanged; command `cargo test -p camel-processor gzip`; expected pass before and after implementation as a regression guard.
- `tar_split_empty_default_is_rejected`: setup an empty TAR with default configuration; action split; assert fail-closed rejection until `allow_empty_archive` is enabled; command `cargo test -p camel-processor tar_splitter`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-processor tar_splitter` exits 0.
- Single-member TAR.GZ output matches TAR output for equivalent archives.
- `cargo clippy -p camel-processor --all-targets -- -D warnings` exits 0.

- [x] 2.3

## camel-core and camel-dsl wiring

### Task 3.1: Parse TAR formats through DSL configuration

**Files:**
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/compile.rs` (modified)

**Steps:**
1. Add YAML names `tar` and `tar.gz` to the closed stream-format match and construct the corresponding `StreamSplitFormat` variants.
2. Update canonical DSL conversion and retain fail-closed errors for unknown names.

**Tests:**
- `test_streaming_true_with_format_tar_produces_tar_def`: setup YAML split with `stream.format: tar`; action parse route; assert `StreamSplitFormat::Tar`; command `cargo test -p camel-dsl test_streaming_true_with_format_tar`; expected pass after implementation and fail before it.
- `test_streaming_true_with_format_tar_gz_produces_tar_gz_def`: setup YAML split with `stream.format: tar.gz`; action parse route; assert `StreamSplitFormat::TarGz`; command `cargo test -p camel-dsl test_streaming_true_with_format_tar_gz`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-dsl --lib` exits 0.
- `cargo clippy -p camel-dsl -- -D warnings` exits 0.

- [x] 3.1

### Task 3.2: Compile TAR formats through processor and core

**Files:**
- `crates/camel-processor/src/stream_codec.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/splitting.rs` (modified)

**Steps:**
1. Add TAR and TAR.GZ branches to stream format resolution and materialized archive dispatch, preserving ZIP dispatch unchanged.
2. Thread `TarSplitConfig` through bytes, text, and bounded stream compiler paths with the existing `StreamingSplitSegment` aggregation contract.

**Tests:**
- `tar_stream_split_compiles_from_bytes_and_stream`: setup canonical route definitions for byte and stream bodies; action compile; assert TAR splitter is selected and unknown format remains rejected; command `cargo test -p camel-core --lib splitting`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-core --lib` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.

- [x] 3.2

## Integration and documentation

### Task 4.1: Add TAR splitter integration coverage and update docs

**Files:**
- `crates/camel-test/tests/tar_split_integration.rs` (new)
- `docs/src/eip/tar-splitter.md` (new)
- `docs/src/eip/streaming-splitter.md` (modified)
- `docs/src/data-formats/index.md` (modified)
- `docs/src/eip/zip-splitter.md` (modified)
- `docs/src/SUMMARY.md` (modified)

**Steps:**
1. Copy the ZIP integration harness shape into `tar_split_integration.rs`, generate TAR and TAR.GZ fixtures with multiple entries, and assert arrival metadata and bodies through mock endpoints.
2. Add docs for YAML/API configuration, archive-order index, materialization and all caps, skipped entry kinds, path-confinement boundary citing rc-0ks57's nearest-existing-ancestor confinement precedent, single-member GZIP, and `AggregationStrategy::Original` non-round-trip behavior.
3. Replace the data-format claim that ZIP is the only archive splitter and cross-link TAR and ZIP guidance without claiming a generic archive abstraction.
4. Add the new page to the documentation summary and run the documentation build for all touched public crates.

**Tests:**
- `test_tar_split_multi_entry`: setup a two-file TAR route and mock sink; action run the integration harness; assert two bodies and indices in archive order; command `cargo test -p camel-test --test tar_split_integration`; expected pass after implementation and fail before it.
- `test_tar_gz_split_multi_entry`: setup equivalent single-member TAR.GZ route; action run harness; assert same bodies and metadata as TAR; command `cargo test -p camel-test --test tar_split_integration`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-test --test tar_split_integration` exits 0.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p camel-processor --no-deps` exits 0.
- Documentation contains no statement that archive entry splitting is ZIP-only.

- [x] 4.1
