# Tasks: multidoc

## Phase 1: Virtual store format and compile resolution

### Task 1.1: Define virtual-store index and v2 trailer codec

**Files:**
- `crates/camel-cli/src/compile/store.rs` (new)
- `crates/camel-dsl/src/embedded_store.rs` (new)
- `crates/camel-dsl/src/lib.rs` (modified)
- `crates/camel-cli/src/compile/trailer.rs` (modified)
- `crates/camel-cli/src/compile/manifest.rs` (modified)
- `crates/camel-cli/src/compile/mod.rs` (modified)

**Steps:**
1. Add canonical `VirtualDocumentStore`, `StoreEntry`, `StoreEntryKind`, `StoreIndex`, and `SourcePlan` in `camel-dsl`'s public embedded-store module, with `store_schema: 1`, typed route/job/config/include/profile entries, logical entry-point reference, configuration references, and ordered source-plan references; re-export the types from `camel-cli` without defining a second model.
2. Implement canonical UTF-8 JSON index encoding with lexicographically ordered object keys and deterministic array ordering; reject unsupported store schema, duplicate paths, noncanonical path order, missing references, unreferenced content, overlapping ranges, and out-of-bounds ranges during decode.
3. Extend the trailer codec with v2 content/index/manifest lengths and 76-byte footer fields, `CAMELTR1` family framing, kind discriminants `1=route` and `2=job`, and the exact `rust-camel-trailer-v2` checksum domain; retain v1 decode as a one-entry store.
4. Extend `Manifest` with `manifest_schema: 2` and `embedded_files` metadata while preserving environment-without-defaults and listener/component derivation; reject unknown manifest schemas independently of trailer version.
5. Preserve existing normalization exactly: valid UTF-8 only, remove one BOM, convert CRLF and lone CR to LF, preserve terminal-newline state, and enforce the aggregate 16 MiB embedded-byte limit.

**Tests:**
- `v2_trailer_round_trip_preserves_store_and_manifest`: setup two route entries, one config entry, canonical index, and manifest schema 2; action encode then decode v2; assert all entry paths/types/ranges, source plan, manifest schema, kind, and checksum are identical; command `cargo test -p camel-cli --lib compile::trailer::v2_trailer_round_trip_preserves_store_and_manifest`; expected pass after implementation and fail before it.
- `v2_trailer_uses_exact_footer_and_checksum_domain`: setup fixed content/index/manifest bytes; action encode v2; assert leading and terminal `CAMELTR1`, version 2, kind 1, zero flags, three little-endian lengths, 76-byte footer, and checksum over only the specified domain; command `cargo test -p camel-cli --lib compile::trailer::v2_trailer_uses_exact_footer_and_checksum_domain`; expected pass after implementation and fail before it.
- `v2_decoder_accepts_v1_as_single_entry_store`: setup a valid v1 artifact; action decode with v2 reader; assert one route/job entry with original source identity and payload; command `cargo test -p camel-cli --lib compile::trailer::v2_decoder_accepts_v1_as_single_entry_store`; expected pass after implementation and fail before it.
- `store_decoder_rejects_schema_ranges_references_and_order`: setup indexes with unknown schema, overlapping/out-of-bounds ranges, duplicate paths, missing source-plan targets, unreferenced content, and noncanonical order; action decode each; assert named format errors and no fallback; command `cargo test -p camel-cli --lib compile::store::store_decoder_rejects_schema_ranges_references_and_order`; expected pass after implementation and fail before it.
- `aggregate_normalization_preserves_utf8_bom_newlines_and_cap`: setup BOM/CRLF/lone-CR entries plus an aggregate over-16-MiB set; action normalize and validate; assert normalized bytes preserve terminal-newline state and oversized set returns `PayloadTooLarge`; command `cargo test -p camel-cli --lib compile::trailer::aggregate_normalization_preserves_utf8_bom_newlines_and_cap`; expected pass after implementation and fail before it.

**Acceptance:**
- `cargo test -p camel-cli --lib` exits 0.
- v1 decode remains compatible; v2 framing, schemas, ranges, references, and checksum are deterministic and fail closed.
- `cargo fmt --check --all` and `cargo clippy -p camel-cli --lib -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Resolve and confine multi-document compile inputs

**Files:**
- `crates/camel-cli/src/compile/sources.rs` (new)
- `crates/camel-cli/src/compile/policy.rs` (modified)
- `crates/camel-cli/src/commands/compile.rs` (modified)
- `crates/camel-cli/src/compile/mod.rs` (modified)
- `crates/camel-cli/tests/compile_command_test.rs` (modified)
- `crates/camel-cli/src/compile/manifest.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified)

**Steps:**
1. Add `SourceSelection` with explicit optional `config_path` and ordered profile names, and add compile flags `--config <Camel.toml>` and repeatable `--profile <name>`; with neither flag, perform no ambient configuration discovery.
2. Resolve explicit `Camel.toml`, ordered includes, selected profiles, route-file patterns, and supported job route sources at compile time; capture each source before interpolation as a typed store entry.
3. Normalize logical names to UTF-8 relative `/` paths and verify every source remains under the selected root after symlink-aware canonicalization; reject absolute paths, empty/dot/traversal components, non-UTF-8 names, missing sources, symlink escapes, duplicate canonical targets, and duplicate normalized paths.
4. Preserve declared pattern order and sort each pattern's matches by normalized logical path; reject unsupported asset-bearing fields and compile-time `CAMEL_*` overrides before creating or replacing the output.
5. Update existing single-document rejection tests and policy branches so explicitly embedded routeFiles/includes/job sources are accepted while endpoint assets, compile-time overrides, and non-native targets remain rejected.
6. Build the v2 store/index/manifest, copy the current executable, append deterministic bytes, preserve executable permissions, and retain atomic temporary-output replacement behavior.

**Tests:**
- `compile_embeds_ordered_routes_config_includes_and_profiles`: setup explicit config with ordered includes/profile and route patterns whose directory enumeration is reversed; action run compile with `--config` and repeated `--profile`; assert v2 index has typed entries, stable plan order, and deterministic artifact bytes across two runs; command `cargo test -p camel-cli --test compile_command_test compile_embeds_ordered_routes_config_includes_and_profiles`; expected pass after implementation and fail before it.
- `compile_without_config_does_not_discover_ambient_config`: setup primary document beside an ambient Camel.toml with routes; action run compile without `--config`; assert no ambient file is embedded or read and output has no config/profile references; command `cargo test -p camel-cli --test compile_command_test compile_without_config_does_not_discover_ambient_config`; expected pass after implementation and fail before it.
- `compile_rejects_path_escape_duplicate_and_invalid_name`: setup traversal, absolute, symlink-escape, missing, duplicate-overlap, and invalid UTF-8 source cases; action run compile for each; assert exit 2, named confinement/duplicate diagnostic, and absent usable output; command `cargo test -p camel-cli --test compile_command_test compile_rejects_path_escape_duplicate_and_invalid_name`; expected pass after implementation and fail before it.
- `compile_embedded_bytes_enforce_aggregate_cap`: setup multiple valid source files whose normalized aggregate exceeds 16 MiB; action run compile; assert exit 2 with aggregate-cap diagnostic and no output; command `cargo test -p camel-cli --test compile_command_test compile_embedded_bytes_enforce_aggregate_cap`; expected pass after implementation and fail before it.
- `compile_copies_executable_with_v2_trailer`: setup supported multi-document route and writable output; action run compile; assert exit 0, executable permission, v2 trailer, manifest schema 2, and all embedded entries; command `cargo test -p camel-cli --test compile_command_test compile_copies_executable_with_v2_trailer`; expected pass after implementation and fail before it.
- `compile_rejects_non_native_target`: setup supported document and different `--target`; action run compile; assert exit 2, native-Linux-only diagnostic, and absent output; command `cargo test -p camel-cli --test compile_command_test compile_rejects_non_native_target`; expected pass after implementation and fail before it.

**Acceptance:**
- Explicit source selection is deterministic and no ambient configuration is read.
- Confinement, duplicate, invalid-name, unsupported-asset, aggregate-cap, and output-atomicity tests pass before any usable artifact is created.
- `cargo test -p camel-cli --test compile_command_test` and affected-crate fmt/clippy exit 0.

- [x] 1.2

## Phase 2: Virtual-store runtime

### Task 2.1: Add embedded virtual-store DSL discovery

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/src/embedded_store.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified)
- `crates/camel-dsl/tests/discovery_test.rs` (modified)

**Steps:**
1. Reuse the canonical store types from task 1.1, re-export them from `camel-dsl`, and add `discover_virtual_store` that accepts `VirtualDocumentStore`, one logical entry point, deployment environment lookup, and virtual source identity.
2. Build `CamelConfig` from embedded config/include/profile text in index order, then resolve only referenced route documents by index lookup; never call filesystem discovery, glob expansion, canonicalization, or temporary-file helpers.
3. Reuse existing interpolation, typed environment probing, template materialization, reserved-document validation, parsing, route lowering, and provenance logic for each virtual document.
4. Emit `compiled://<logical-path>` source identities and named errors for missing index references, unsupported store schema, malformed config, and invalid route source plans.

**Tests:**
- `discover_virtual_store_resolves_deployment_environment`: setup virtual config and two route entries containing `${env:NAME}`; action discover with deployment value; assert parsed routes contain deployment value and no compile-time value is required; command `cargo test -p camel-dsl --test discovery_test discover_virtual_store_resolves_deployment_environment`; expected pass after implementation and fail before it.
- `discover_virtual_store_builds_config_in_index_order`: setup config, include, and profile entries with values that require ordered merge; action discover virtual store; assert resulting `CamelConfig` matches filesystem discovery semantics; command `cargo test -p camel-dsl --test discovery_test discover_virtual_store_builds_config_in_index_order`; expected pass after implementation and fail before it.
- `discover_virtual_store_does_not_touch_filesystem`: setup valid in-memory store with no corresponding source files; action discover; assert successful route definitions and no filesystem access or extraction path; command `cargo test -p camel-dsl --test discovery_test discover_virtual_store_does_not_touch_filesystem`; expected pass after implementation and fail before it.
- `discover_virtual_store_preserves_virtual_provenance`: setup malformed second route entry at logical `routes/orders.yaml`; action discover; assert diagnostic names `compiled://routes/orders.yaml`; command `cargo test -p camel-dsl --test discovery_test discover_virtual_store_preserves_virtual_provenance`; expected pass after implementation and fail before it.

**Acceptance:**
- DSL discovery consumes only store entries and preserves existing interpolation/parser/lowering semantics.
- No new route or processor serialization exists; filesystem-free tests pass.
- `cargo test -p camel-dsl --test discovery_test` and `cargo clippy -p camel-dsl --all-targets -- -D warnings` exit 0.

- [x] 2.1

### Task 2.2: Run multi-document route and job artifacts

**Files:**
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Extend `EmbeddedRequest` to carry decoded `VirtualDocumentStore`, manifest, one entry point, and existing restricted artifact arguments while retaining v1 single-document adaptation.
2. Route runtime calls `discover_virtual_store`, registers all referenced routes, and reuses existing boot, context start, signal shutdown, report, and exit handling with `watch=false`.
3. Job runtime consumes embedded job/config/route entries through the existing job outcome lifecycle and does not reintroduce filesystem route discovery.
4. Ensure runtime performs no source/config reads, globbing, canonicalization, or extraction; permit deployment-time endpoint I/O and `${env:}` resolution only.
5. Validate decoded store before boot and return exit 2 for unknown schemas, invalid references, malformed ranges, kind mismatch, checksum failure, or missing configuration entries.

**Tests:**
- `compiled_multidocument_route_runs_without_source_tree`: setup artifact with config, includes, and two routes, then remove source tree and working-directory config; action run artifact; assert all embedded routes boot and execute; command `cargo test -p camel-cli --test compiled_artifact_test compiled_multidocument_route_runs_without_source_tree`; expected pass after implementation and fail before it.
- `compiled_job_uses_embedded_route_plan_and_report`: setup job artifact with indexed route sources and config; action run with existing job report option; assert existing job outcome schema and no source-tree read; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_uses_embedded_route_plan_and_report`; expected pass after implementation and fail before it.
- `compiled_multidocument_resolves_deployment_environment`: setup multi-document artifact containing `${env:NAME}` and different compile environment; action run with deployment value; assert route observes deployment value only; command `cargo test -p camel-cli --test compiled_artifact_test compiled_multidocument_resolves_deployment_environment`; expected pass after implementation and fail before it.
- `compiled_multidocument_does_not_extract_glob_or_watch`: setup read-only root, no source files, and valid artifact; action run; assert successful boot, no extraction, no globbing, and no watcher activation; command `cargo test -p camel-cli --test compiled_artifact_test compiled_multidocument_does_not_extract_glob_or_watch`; expected pass after implementation and fail before it.
- `compiled_multidocument_ignores_post_compile_decoy`: setup valid artifact, remove original source tree, place a new route file beside the artifact; action run artifact; assert decoy route is not loaded and only indexed routes execute; command `cargo test -p camel-cli --test compiled_artifact_test compiled_multidocument_ignores_post_compile_decoy`; expected pass after implementation and fail before it.
- `compiled_v1_artifact_uses_single_entry_adapter`: setup valid v1 artifact; action run without source/config files; assert existing single-document route behavior and exit/report semantics through the v2 runtime adapter; command `cargo test -p camel-cli --test compiled_artifact_test compiled_v1_artifact_uses_single_entry_adapter`; expected pass after implementation and fail before it.
- `compiled_runtime_rejects_invalid_store_before_boot`: setup artifacts with unknown schema, missing reference, and kind mismatch; action run each; assert exit 2 and route boot counter remains zero; command `cargo test -p camel-cli --test compiled_artifact_test compiled_runtime_rejects_invalid_store_before_boot`; expected pass after implementation and fail before it.

**Acceptance:**
- Multi-document route and job artifacts run with no source/config tree, no extraction, no runtime discovery, and documented exit/report behavior.
- v1 artifacts still run through the adapter path and all invalid v2 forms fail before boot.
- `cargo test -p camel-cli --test compiled_artifact_test` and affected-crate fmt/clippy exit 0.

- [x] 2.2

### Task 2.3: Self-detect v2 artifacts and expose manifest

**Files:**
- `crates/camel-cli/src/main.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)

**Steps:**
1. Probe `current_exe()` before Clap parsing and dispatch v1/v2 trailer decoding while preserving normal CLI fallback when no terminal marker exists.
2. Route valid v2 artifacts into virtual-store runtime and reject marked corruption, unsupported trailer/store/manifest schema, invalid kind, bounds, references, and checksum before argument parsing or boot.
3. Keep artifact runtime arguments limited to `--report`, `--help`, `--version`, `--manifest`, and sanctioned R4 verification; make `--manifest` print embedded-file metadata without boot.
4. Preserve truncation behavior: removal of terminal marker falls through to normal CLI; marked truncation fails closed.

**Tests:**
- `trailer_free_binary_keeps_normal_cli`: setup executable without terminal marker; action invoke normal CLI command; assert unchanged Clap behavior; command `cargo test -p camel-cli --test compiled_artifact_test trailer_free_binary_keeps_normal_cli`; expected pass after implementation and fail before it.
- `artifact_manifest_lists_virtual_store_without_boot`: setup valid v2 artifact; action run `--manifest`; assert exit 0, runtime version, manifest schema 2, every embedded logical path, and zero route boot; command `cargo test -p camel-cli --test compiled_artifact_test artifact_manifest_lists_virtual_store_without_boot`; expected pass after implementation and fail before it.
- `artifact_rejects_v2_corruption_and_unknown_schemas`: setup artifacts mutated in content, footer, index schema, and manifest schema while retaining terminal marker; action start each; assert exit 2, integrity/format diagnostic, and zero boot; command `cargo test -p camel-cli --test compiled_artifact_test artifact_rejects_v2_corruption_and_unknown_schemas`; expected pass after implementation and fail before it.
- `artifact_truncated_without_marker_keeps_clap_fallback`: setup v2 artifact truncated through terminal marker; action invoke normal CLI argument; assert normal fallback behavior; command `cargo test -p camel-cli --test compiled_artifact_test artifact_truncated_without_marker_keeps_clap_fallback`; expected pass after implementation and fail before it.
- `artifact_rejects_unknown_positional_and_duplicate_args`: setup valid v2 artifact; action invoke unknown, positional, missing-report, and duplicate-exclusive arguments; assert exit 2 and rejected argument diagnostics; command `cargo test -p camel-cli --test compiled_artifact_test artifact_rejects_unknown_positional_and_duplicate_args`; expected pass after implementation and fail before it.

**Acceptance:**
- Self-detection happens before Clap, v1 fallback remains intact, and v2 corruption/schema errors fail closed.
- `--manifest` is boot-free and exposes independent manifest/store metadata.
- Focused compiled-artifact tests and `cargo clippy -p camel-cli --all-targets -- -D warnings` exit 0.

- [x] 2.3

## Phase 3: Documentation and contract alignment

### Task 3.1: Document virtual-store artifact contract

**Files:**
- `docs/adr/0075-self-contained-executable-artifact-format.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)
- `crates/camel-dsl/CONTEXT.md` (modified)
- `crates/camel-config/CONTEXT.md` (modified)
- `crates/camel-config/src/config_tests/virtual_store_file_parity_tests.rs` (new, phase-review follow-up)

**Steps:**
1. Update ADR 0075 with v2 virtual-store framing, exact schema separation, explicit compile source selectors, path confinement, config embedding, v1 compatibility, and aggregate cap.
2. Add canonical terms for virtual document store, store index, embedded file, and compile-time source selection to `CONTEXT-MAP.md`, each citing ADR 0075 and preserving the MUST-NOT wall.
3. Update camel-cli context with compile/runtime ownership, no-I/O/no-glob guarantee, route/job behavior, report/manifest behavior, and R2/R3 extension boundaries.
4. Update camel-dsl context with the embedded-store discovery seam, no-filesystem guarantee, typed configuration assembly, and `compiled://<logical-path>` provenance contract.
5. Run citation and OpenSpec validation checks and remove claims that imply assets, compression, signing, cross-target, or multi-entry support shipped in R1.

**Tests:**
- `context_citations_lint_covers_virtual_store_terms`: setup updated ADR and context docs; action run `cargo xtask lint-context-citations`; assert exit 0 and all virtual-store terms cite ADR 0075; command `cargo xtask lint-context-citations`; expected pass after implementation and fail before aligned docs.
- `openspec_validation_accepts_final_multidoc_delta`: setup proposal, design, specs, and completed task plan; action run `openspec validate multidoc --type change --json`; assert JSON `valid:true`; command `openspec validate multidoc --type change --json`; expected pass after implementation and fail before complete delta.

**Acceptance:**
- ADR, context map, and camel-cli context describe identical v2 framing, config/source selection, confinement, runtime wall, and deferred R2/R3 scope in English.
- camel-dsl context documents the public virtual-store seam and its no-filesystem runtime contract.
- `cargo xtask lint-context-citations` and `openspec validate multidoc --type change --json` exit 0.
- No documentation claims unsupported performance, asset, signing, compression, cross-target, or multi-entry behavior.

- [x] 3.1
