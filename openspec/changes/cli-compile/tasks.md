# Tasks: cli-compile

## Phase 1: Trailer codec and compile pipeline

### Task 1.1: Implement deterministic trailer codec and manifest

**Files:**
- `crates/camel-cli/src/compile/mod.rs` (new)
- `crates/camel-cli/src/compile/trailer.rs` (new)
- `crates/camel-cli/src/compile/manifest.rs` (new)
- `crates/camel-cli/Cargo.toml` (modified)
- `crates/camel-cli/src/lib.rs` (modified)

**Steps:**
1. Add `blake3 = { workspace = true }` to `camel-cli/Cargo.toml`. Add `compile::trailer::{TrailerKind, TrailerError, Trailer, encode, decode, normalize_document}` with route/job kind, normalized UTF-8 payload, canonical manifest, and exact 68-byte footer constants. Add `compile::CompileError::{InvalidUtf8, PayloadTooLarge, UnsupportedAsset, InvalidDocument}` in `compile/mod.rs`; expose `manifest::Manifest.source_name`.
2. Encode little-endian version, kind, reserved byte, payload length, manifest length, BLAKE3, and terminal magic exactly as the blessed design specifies.
3. Hash domain bytes `rust-camel-trailer-v1`, `0x00`, encoded version/kind/length fields, payload, and manifest; exclude both magic fields and reserved byte.
4. Decode only a final footer with exact terminal magic; distinguish absent marker fallback from marked corruption; enforce lengths, version, kind, reserved byte, and checksum through named `TrailerError` variants.
5. Expose `compile::manifest::Manifest` JSON with deterministic key ordering, source name, runtime version, artifact kind, components, environment names without defaults, and listener declarations. Derive components from endpoint URI schemes, environment names from `${env:NAME}` tokens without defaults via the existing interpolation scan, and listener declarations from canonical REST/MCP port fields.

**Tests:**
- `trailer_round_trip_preserves_payload_and_manifest`: arrange route payload and canonical manifest; act `encode` then `decode`; assert exact payload, manifest, kind, and checksum; command `cargo test -p camel-cli --lib compile::trailer::trailer_round_trip_preserves_payload_and_manifest`; expected pass after implementation, fail before it.
- `trailer_encoding_uses_exact_68_byte_footer`: arrange fixed values; act `encode`; assert footer length, magic offsets, little-endian fields, zero reserved byte, and terminal magic; command `cargo test -p camel-cli --lib compile::trailer::trailer_encoding_uses_exact_68_byte_footer`; expected pass after implementation, fail before it.
- `trailer_rejects_marked_corruption_invalid_version_lengths_and_kind`: arrange encoded artifact; act mutate payload/checksum/version, overflow a length field, and mismatch footer kind against manifest kind while retaining terminal magic; assert named format/integrity error for each and no fallback; command `cargo test -p camel-cli --lib compile::trailer`; expected all cases pass after implementation and fail before it.
- `trailer_without_terminal_marker_is_absent`: arrange ordinary bytes or truncated trailer without terminal magic; act `decode`; assert absent-trailer result; command `cargo test -p camel-cli --lib compile::trailer::trailer_without_terminal_marker_is_absent`; expected pass after implementation, fail before it.
- `normalization_removes_bom_and_normalizes_line_endings`: arrange BOM, CRLF, and lone CR text; act `normalize_document`; assert UTF-8 LF payload with terminal-newline state preserved; command `cargo test -p camel-cli --lib compile::trailer::normalization_removes_bom_and_normalizes_line_endings`; expected pass after implementation, fail before it.
- `compile_manifest_lists_components_env_and_listeners`: arrange document with component URIs, `${env:NAME}`, REST/MCP listener ports; act manifest derivation; assert schemes, NAME, and literal/unresolved listener declarations; command `cargo test -p camel-cli --lib compile::manifest::compile_manifest_lists_components_env_and_listeners`; expected pass after implementation, fail before it.

**Acceptance:**
- Codec matches exact blessed byte layout and checksum domain.
- Manifest serialization is deterministic and does not include environment values with defaults.
- Unit tests pass; `cargo fmt --check` and affected-crate clippy pass.

- [x] 1.1

### Task 1.2: Add compile command and fail-closed asset policy

**Files:**
- `crates/camel-cli/src/commands/compile.rs` (new)
- `crates/camel-cli/src/commands/mod.rs` (modified)
- `crates/camel-cli/src/main.rs` (modified)
- `crates/camel-cli/src/compile/policy.rs` (new)
- `crates/camel-cli/tests/compile_command_test.rs` (new)

**Steps:**
1. Add `commands::compile::CompileArgs` and `run_compile` for `camel compile <document> -o <artifact>`, rejecting non-Linux hosts or any `--target` value different from the native target triple with exit 2 and a native-Linux-only diagnostic.
2. Read raw document bytes, normalize before interpolation, identify single route/job kind, and reject invalid UTF-8 or oversized payloads through `CompileError::{InvalidUtf8, PayloadTooLarge, UnsupportedAsset, InvalidDocument}`.
3. Add `crates/camel-cli/src/compile/policy.rs` with `reject_unsupported_assets(document_text: &str, kind: TrailerKind) -> Result<(), CompileError>` and inspect exact forbidden fields: `routeFiles`, `routeFilesFromRoot`, route glob patterns, `profiles`, `includes`, `cert`, `key`, `client_ca`, `wasm`, `plugin`, `xslt`, `xsd`, `sql`, `static_dir`, and file-valued secret fields; reject `${env:NAME}` expressions in those fields while allowing runtime endpoint URI paths and runtime environment expressions. `run_compile` separately rejects `Camel.toml` in the compile working directory and any `CAMEL_*` variable in the compile environment before output creation.
4. Build `Manifest` without embedding compile-time environment values; copy `current_exe()` and append trailer only after all validation succeeds.
5. Set executable permissions on Unix and report named errors with exit 2; never leave a usable partial artifact.

**Tests:**
- `compile_writes_executable_with_valid_trailer`: arrange supported single route fixture and output path; act invoke `run_compile`; assert exit 0, executable output, and decodable route artifact; command `cargo test -p camel-cli --test compile_command_test compile_writes_executable_with_valid_trailer`; expected pass after implementation, fail before it.
- `compile_rejects_external_assets_before_output`: arrange documents using `routeFilesFromRoot`, `Camel.toml`, a certificate, and dynamic asset path; act invoke `run_compile`; assert exit 2, named rejection, and absent output; command `cargo test -p camel-cli --test compile_command_test compile_rejects_external_assets_before_output`; expected pass after implementation, fail before it.
- `compile_preserves_env_expression_not_value`: arrange document with `${env:NAME}` and compile environment value; act `run_compile`; assert artifact bytes contain expression and not value; command `cargo test -p camel-cli --test compile_command_test compile_preserves_env_expression_not_value`; expected pass after implementation, fail before it.
- `compile_rejects_invalid_utf8_and_oversize_payload`: arrange invalid bytes and payload over 16 MiB; act `run_compile`; assert exit 2 and no output for each case; command `cargo test -p camel-cli --test compile_command_test compile_rejects_invalid_utf8_and_oversize_payload`; expected pass after implementation, fail before it.
- `compile_rejects_non_native_target`: arrange a supported document and `--target` set to a different triple; act invoke `run_compile`; assert exit 2, native-Linux-only diagnostic, and absent output; command `cargo test -p camel-cli --test compile_command_test compile_rejects_non_native_target`; expected pass after implementation, fail before it.
- `compile_rejects_job_with_external_dependencies`: arrange a `*.job.yaml` with external route source and config dependency; act invoke `run_compile`; assert exit 2, named job dependency rejection, and absent output; command `cargo test -p camel-cli --test compile_command_test compile_rejects_job_with_external_dependencies`; expected pass after implementation, fail before it.

**Acceptance:**
- Native Linux compile command produces immutable single-document artifact only after validation.
- All forbidden asset classes fail closed before output creation.
- Compile tests pass; worker runs `cargo fmt --check` and affected-crate clippy.

- [x] 1.2

## Phase 2: Artifact runtime

### Task 2.1: Add embedded-text discovery seam

**Files:**
- `crates/camel-dsl/src/discovery.rs` (modified)
- `crates/camel-dsl/src/lib.rs` (modified)
- `crates/camel-dsl/tests/discovery_test.rs` (modified)

**Steps:**
1. Add `EmbeddedDocumentKind::{Route, Job}` and `discover_embedded_text(text: &str, source_name: &str, kind: EmbeddedDocumentKind, env_lookup: &dyn Fn(&str) -> Option<String>) -> Result<Vec<RouteDefinition>, DiscoveryError>` as a public internal-facing helper accepting normalized text, virtual source identity, route/job kind, and the existing discovery environment lookup seam.
2. Reuse existing interpolation, typed env probing, template materialization, parsing, provenance, reserved-document validation, and lowering rather than duplicating discovery logic.
3. Preserve source hash and diagnostic names using `compiled://<manifest.source_name>`.
4. Ensure no helper reads config, globs, external route files, or writes temporary files.

**Tests:**
- `embedded_text_resolves_runtime_environment`: arrange text containing `${env:NAME}` and runtime environment; act `discover_embedded_text`; assert runtime value appears in parsed route and compile value is not needed; command `cargo test -p camel-dsl --test discovery_test embedded_text_resolves_runtime_environment`; expected pass after implementation, fail before it.
- `embedded_text_preserves_typed_probe_and_source_identity`: arrange typed placeholder and virtual source name; act `discover_embedded_text`; assert typed probe behavior and diagnostic/source identity use virtual name; command `cargo test -p camel-dsl --test discovery_test embedded_text_preserves_typed_probe_and_source_identity`; expected pass after implementation, fail before it.
- `embedded_text_does_not_touch_filesystem`: arrange valid text with no source file; act `discover_embedded_text`; assert successful parse without file reads or extraction path; command `cargo test -p camel-dsl --test discovery_test embedded_text_does_not_touch_filesystem`; expected pass after implementation, fail before it.

**Acceptance:**
- Existing discovery semantics remain shared by filesystem and embedded paths.
- No `RouteDefinition` or processor serialization is introduced.
- DSL tests pass; worker runs `cargo fmt --check` and affected-crate clippy.

- [x] 2.1

### Task 2.2: Run embedded routes and jobs through existing lifecycles

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/camel-cli/src/commands/job/mod.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (new)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (new)

**Steps:**
1. Define `compile::runtime::{EmbeddedRequest, ArtifactArgs, ArtifactArgError}` and `run_embedded_document(request: EmbeddedRequest) -> ExitCode`; `ArtifactArgs::parse` accepts `--report <path>`, `--help`, `--version`, and `--manifest`, rejects duplicate/missing-value/unknown/positional forms with exit 2, and owns `RouteReport` JSON serialization.
2. Refactor existing `run()` lifecycle around `run_embedded_document`, accepting default in-memory config, embedded source, route/job kind, and `watch=false`; reuse boot, route registration, context start, signals, and shutdown.
3. Add job runtime entry using existing single-document report/outcome lifecycle and embedded document as its sole route source.
4. Dispatch validated requests, disable watcher unconditionally, reject runtime attempts to resolve compile-time assets, and write `RouteReport { kind: "route", status: "completed"|"failed", error: Option<String> }` to `--report` with exit 0 for completed, 1 for pipeline failure, 2 for boot/report-write failure.
5. Verify read-only root operation and runtime endpoint I/O remain deployment-time behavior.

**Tests:**
- `compiled_route_runs_without_source_tree`: arrange compiled route and remove source/config files; act run artifact; assert route boots through embedded text; command `cargo test -p camel-cli --test compiled_artifact_test compiled_route_runs_without_source_tree`; expected pass after implementation, fail before it.
- `compiled_job_uses_existing_outcome_report`: arrange compiled single job; act run with report path; assert existing job outcome schema and exit precedence; command `cargo test -p camel-cli --test compiled_artifact_test compiled_job_uses_existing_outcome_report`; expected pass after implementation, fail before it.
- `compiled_artifact_resolves_deploy_environment`: arrange artifact with `${env:NAME}`; act run with deployment value; assert route/job observes deployment value; command `cargo test -p camel-cli --test compiled_artifact_test compiled_artifact_resolves_deploy_environment`; expected pass after implementation, fail before it.
- `compiled_artifact_does_not_extract_or_watch`: arrange read-only root and valid artifact; act run; assert successful boot and no temp/source extraction or watcher activation; command `cargo test -p camel-cli --test compiled_artifact_test compiled_artifact_does_not_extract_or_watch`; expected pass after implementation, fail before it.
- `compiled_route_report_writes_status_json`: arrange valid route artifact; act run with `--report`, send SIGTERM after boot, and wait for graceful shutdown; assert `{"kind":"route","status":"completed","error":null}` and exit 0; command `cargo test -p camel-cli --test compiled_artifact_test compiled_route_report_writes_status_json`; expected pass after implementation, fail before it.

**Acceptance:**
- Embedded route and job lifecycles run without source/config files, use no watcher, and return documented route/job exits.
- Runtime tests pass; worker runs `cargo fmt --check` and affected-crate clippy.

- [x] 2.2

### Task 2.3: Self-detect artifacts before CLI parsing

**Files:**
- `crates/camel-cli/src/main.rs` (modified)
- `crates/camel-cli/src/compile/runtime.rs` (modified)
- `crates/camel-cli/tests/compiled_artifact_test.rs` (modified)
- `crates/camel-cli/tests/job_one_shot_test.rs` (modified only for shared helper coverage)

**Steps:**
1. Probe `current_exe()` and decode a marked trailer before Clap parses arguments.
2. Fall through unchanged for absent trailer; fail closed for marked corruption, unsupported version, invalid kind, or checksum failure.
3. Decode valid payload into `EmbeddedRequest` and call `run_embedded_document`; print manifest without boot for `--manifest` and print version/help with deterministic exit 0 behavior.

**Tests:**
- `trailer_free_binary_keeps_normal_cli`: arrange normal executable without trailer; act invoke standard command; assert existing Clap behavior; command `cargo test -p camel-cli --test compiled_artifact_test trailer_free_binary_keeps_normal_cli`; expected pass after implementation, fail before it.
- `artifact_manifest_exits_without_boot`: arrange valid artifact; act run `--manifest`; assert exit 0, required manifest fields, and no route boot; command `cargo test -p camel-cli --test compiled_artifact_test artifact_manifest_exits_without_boot`; expected pass after implementation, fail before it.
- `artifact_rejects_unknown_and_positional_args`: arrange valid artifact; act run duplicate exclusive flags, missing `--report` value, unknown flag, and positional argument; assert each exits 2 and names rejected argument; command `cargo test -p camel-cli --test compiled_artifact_test artifact_rejects_unknown_and_positional_args`; expected pass after implementation, fail before it.
- `artifact_rejects_marked_corruption`: arrange valid artifact then mutate payload/footer retaining terminal marker; act start; assert nonzero integrity diagnostic and no boot; command `cargo test -p camel-cli --test compiled_artifact_test artifact_rejects_marked_corruption`; expected pass after implementation, fail before it.
- `artifact_truncated_without_marker_keeps_clap_fallback`: arrange artifact truncated through terminal marker; act invoke normal CLI argument; assert unchanged Clap fallback; command `cargo test -p camel-cli --test compiled_artifact_test artifact_truncated_without_marker_keeps_clap_fallback`; expected pass after implementation, fail before it.
- `artifact_help_and_version_exit_zero`: arrange valid artifact; act run `--help` and `--version`; assert each exits 0 without boot; command `cargo test -p camel-cli --test compiled_artifact_test artifact_help_and_version_exit_zero`; expected pass after implementation, fail before it.

**Acceptance:**
- Probe runs before Clap, absent trailers preserve normal CLI fallback, and marked corruption fails closed.
- Artifact argument surface, manifest behavior, and exit codes match spec.
- Focused CLI tests pass; worker runs `cargo fmt --check` and affected-crate clippy.

- [x] 2.3

## Phase 3: Decision and domain documentation

### Task 3.1: Document compiled artifact vocabulary and decision

**Files:**
- `docs/adr/0075-self-contained-executable-artifact-format.md` (new)
- `CONTEXT-MAP.md` (modified)
- `crates/camel-cli/CONTEXT.md` (modified)

**Steps:**
1. Keep ADR 0075 aligned with implemented footer, checksum, environment, trust, native Linux preview, and deferred scope.
2. Add definitions for compiled artifact, EOF trailer, self-detect, and operational manifest to canonical context docs with ADR citation.
3. Document route/job scope, report behavior, read-only-root guarantee, asset rejection, and absence of performance claims before P0.

**Tests:**
- `context_citations_lint_covers_compile_terms`: arrange updated context docs and ADR; act run `cargo xtask lint-context-citations`; assert exit 0 and ADR 0075 citations; command `cargo xtask lint-context-citations`; expected pass after implementation, fail before docs are added.
- `openspec_validation_accepts_final_delta`: arrange all change artifacts; act run `openspec validate cli-compile --type change --json`; assert JSON `valid:true`; command `openspec validate cli-compile --type change --json`; expected pass after implementation, fail before valid delta exists.

**Acceptance:**
- Docs describe exact implementation and no deferred feature as shipped.
- Context and citation lint pass; prose remains English.

- [x] 3.1
