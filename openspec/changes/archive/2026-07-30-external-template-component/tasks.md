# Tasks: external-template-component

<!--
  Multi-phase change. The WHOLE tasks.md — every Phase block — is plan-blessed
  ONCE in PHASE 2, then implemented phase-group by phase-group in PHASE 3.
  Boundaries mirror design.md ## Phases. Autopilot budget is global across phases.
-->

## Phase 1: types

### Task 1.1: Add CamelError::TemplateReload variant

**Files:**
- `crates/camel-api/src/error.rs` (modified)

**Steps:**
1. Add variant `TemplateReload(String)` to `CamelError` (after `ValidationError(String)`, `error.rs:124`).
2. Add arm `Self::TemplateReload(_) => "template"` to `classify(&self)` (`error.rs:128`). The string `template` is 8 ASCII chars (≤15).
3. Add arm `Self::TemplateReload(_) => "TemplateReload"` to `variant_name(&self)` (`error.rs:159`). This match is exhaustive in-crate; omitting it is a compile error.
4. Add `CamelError::TemplateReload("reload failed".to_string())` to `all_error_samples()` (`error.rs:207`).
5. Add `(CamelError::TemplateReload("x".into()), "TemplateReload")` to the `cases` Vec in `variant_name_covers_all_variants` (`error.rs:383`).

**Tests:**
- `template_reload_classifies_as_template`: setup = a `CamelError::TemplateReload("boom".into())` sample; action = call `.classify()`; assert = returns `"template"`. command = `cargo test -p camel-api --lib classify`. expected = pass after step 2.
- `template_reload_variant_name`: setup = same sample; action = call `.variant_name()`; assert = returns `"TemplateReload"`. command = `cargo test -p camel-api --lib variant_name`. expected = pass after step 3.

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-api --lib` passes (incl. `variant_name_covers_all_variants`, `test_classify_output_is_ascii_and_short`).

- [x] 1.1

### Task 1.2: Extract public engine module in camel-language-minijinja

**Files:**
- `crates/languages/camel-language-minijinja/src/engine.rs` (new)
- `crates/languages/camel-language-minijinja/src/lib.rs` (modified)

**Steps:**
1. Create `src/engine.rs`. Move into it, from `lib.rs`, verbatim: `MinijinjaExpression` (`:120-202`), `ResolvedLimits` (`:71-111`), `build_context_bounded` (`:273-317`), `template_name_for` (`:206-210`), `BodyAsJson` (`:323-344`), `MeasurementCtx` (`:346-352`), and the `spawn_blocking` + `tokio::time::timeout` render block (`:234-261`).
2. In `engine.rs` expose `pub async fn render(source: &str, context: &minijinja::Value, limits: ResolvedLimits) -> Result<String, LanguageError>` that wraps the moved compile+render logic (compile via `MinijinjaExpression::compile`, render via the moved `spawn_blocking`+`timeout` closure).
3. Make `ResolvedLimits` and `MinijinjaExpression` `pub` in `engine.rs` (they already are in `lib.rs`; keep visibility). Re-export from `lib.rs`: `pub mod engine;` and `pub use engine::{render, MinijinjaExpression, ResolvedLimits};`.
4. Expose the autoescape validator for reuse by the external Component (Task 4.1): change `mod autoescape_validator;` (`lib.rs:9`) to `pub mod autoescape_validator;` and add `pub use autoescape_validator::validate_autoescape_wrapper;` to `lib.rs`. (The fn is the ADR-0047 top-level `{% autoescape %}` wrapper check.) Keep behavior identical — only visibility changes.
5. Rewrite `Expression::evaluate` (`lib.rs:212-265`) to RENDER THE PRE-COMPILED `self.environment()` directly (the moved `spawn_blocking`+`timeout`+`LimitedWriter` closure operating on `self.environment().get_template(...)`). Do NOT route `evaluate` through `engine::render` — that would recompile on every evaluate and silently break the compile-once invariant (AC: a fresh internal expression masks the recompile from the per-instance `compile_count` probe). `engine::render` stays a standalone one-shot helper for other callers, never reached by `evaluate`. No logic change vs the original inline render path.

**Tests:**
- `engine_render_inline_unchanged`: setup = an existing inline-render unit test in the crate (e.g. one asserting `MinijinjaLanguage` renders `Hello {{name}}`); action = run the crate's existing tests; assert = identical output. command = `cargo test -p camel-language-minijinja --lib`. expected = pass (zero behavior change).
- `engine_render_is_public`: setup = a new test in `lib.rs` calling `camel_language_minijinja::engine::render("{{x}}", &value, ResolvedLimits::default()).await`; action = await render; assert = returns the rendered value. command = `cargo test -p camel-language-minijinja --lib engine_render_is_public`. expected = pass after step 2.

**Acceptance:**
- `cargo clippy -p camel-language-minijinja -- -D warnings` exits 0.
- `cargo test -p camel-language-minijinja --lib` passes (no existing test altered).
- `engine::render` is reachable as a public path.

- [x] 1.2

### Task 1.3: Scaffold camel-template crate with types

**Files:**
- `Cargo.toml` (modified — root, add `[workspace.dependencies] camel-template`, `rustix`, `windows-sys`)
- `crates/components/camel-template/Cargo.toml` (new)
- `crates/components/camel-template/src/lib.rs` (new)
- `crates/components/camel-template/src/config.rs` (new)
- `crates/components/camel-template/src/error.rs` (new)

**Steps:**
1. Add to root `Cargo.toml` `[workspace.dependencies]`: `camel-template = { path = "crates/components/camel-template", version = "=0.25.1" }` (mirror L101 `camel-xslt`); `rustix = { version = "0.38", features = ["fs"] }`; `windows-sys = { version = "0.59", features = ["Win32_Foundation", "Wdk_Storage_FileSystem", "Win32_Storage_FileSystem", "Win32_Security"] }`. (Verify `regex`, `futures`, `tempfile`, `bytes`, `minijinja`, `toml` are already workspace deps; if any is missing, add it here too.)
2. Create `crates/components/camel-template/Cargo.toml`: `[package]` name `camel-template`, version `0.25.1`, edition `2024`; `[dependencies]` `camel-api.workspace`, `camel-component-api.workspace`, `camel-language-api.workspace`, `camel-language-minijinja.workspace`, `minijinja.workspace`, `arc-swap.workspace`, `blake3.workspace`, `tokio.workspace` (features `["rt","time","sync","macros"]`), `async-trait.workspace`, `serde.workspace`, `thiserror.workspace`, `tracing.workspace`, `tower.workspace`, `url.workspace`, `regex.workspace`, `futures.workspace`; cfg-gated: `[target.'cfg(unix)'.dependencies] rustix.workspace`, `[target.'cfg(windows)'.dependencies] windows-sys.workspace`; `[dev-dependencies]` `tempfile.workspace`, `bytes.workspace`.
3. Create `src/error.rs`: `#[derive(Debug, Clone, thiserror::Error)] pub enum TemplateReloadError` with variants `Acquire(String)`, `Compile(String)`, `PathEscape(String)`, `Cycle(String)`, `DuplicateIdentity(String)`, `BoundExceeded(&'static str)`, `Timeout`, `StaleGeneration`. Impl `From<TemplateReloadError> for CamelError` mapping to `CamelError::TemplateReload(e.to_string())`.
4. Create `src/config.rs`: `#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)] #[serde(rename_all="kebab-case", deny_unknown_fields)] pub struct ExternalTemplateLimitsConfig` with five `Option` fields: `max_total_source_bytes: Option<usize>`, `max_include_count: Option<u32>`, `max_include_depth: Option<u32>`, `max_template_size: Option<usize>`, `reload_timeout_ms: Option<u64>` (all default `None`, mirror `MinijinjaLimitsConfig` shape). Add `pub fn resolve(&self) -> ResolvedExternalTemplateLimits` folding to finite non-zero defaults (`max_total_source_bytes` 16 MiB, `max_include_count` 64, `max_include_depth` 16, `max_template_size` 1 MiB, `reload_timeout_ms` 5000); `resolve()` returns `Err(TemplateReloadError::BoundExceeded("zero value not permitted"))` if any provided value is 0.
5. Define `ResolvedExternalTemplateLimits` (same fields, non-`Option`, `Copy`) in `config.rs`.
6. Create `src/lib.rs`: `pub mod config; pub mod error; pub use config::{ExternalTemplateLimitsConfig, ResolvedExternalTemplateLimits}; pub use error::TemplateReloadError;`. No Component yet.

**Tests:**
- `limits_resolve_defaults`: setup = `ExternalTemplateLimitsConfig::default()`; action = `.resolve()`; assert = `Ok` with the five documented non-zero defaults. command = `cargo test -p camel-template --lib limits_resolve_defaults`. expected = pass after step 4.
- `limits_reject_zero`: setup = config with `max_include_count: Some(0)`; action = `.resolve()`; assert = `Err(TemplateReloadError::BoundExceeded(_))`. command = `cargo test -p camel-template --lib limits_reject_zero`. expected = pass after step 4.
- `deny_unknown_fields`: setup = TOML `max-include-count = 3\nbogus = 1`; action = deserialize into `ExternalTemplateLimitsConfig`; assert = serde error. command = `cargo test -p camel-template --lib deny_unknown_fields`. expected = pass after step 4.

**Acceptance:**
- `cargo build -p camel-template` succeeds.
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib` passes.

- [x] 1.3

### Task 1.4: camel-config re-export of ExternalTemplateLimitsConfig

**Files:**
- `crates/camel-config/Cargo.toml` (modified — add `camel-template.workspace` dependency)
- `crates/camel-config/src/lib.rs` (modified)

**Steps:**
1. Add `camel-template = { workspace = true }` to `crates/camel-config/Cargo.toml` `[dependencies]`.
2. In `crates/camel-config/src/lib.rs:16-19` (the `pub use camel_language_api::{...}` re-export block) add `camel_template::ExternalTemplateLimitsConfig` to the re-exports so the type is reachable from the TOML config layer. (Mirror how `MinijinjaLimitsConfig` is re-exported.) Full TOML component-block wiring lands in Task 4.5 (`TemplateBundle`); this task only makes the type importable from `camel-config`.

**Tests:**
- `config_reexports_external_limits`: setup = `use camel_config::ExternalTemplateLimitsConfig;`; action = construct `ExternalTemplateLimitsConfig::default()`; assert = compiles and is the same type as `camel_template::ExternalTemplateLimitsConfig`. command = `cargo test -p camel-config --lib config_reexports_external_limits`. expected = pass after step 1.

**Acceptance:**
- `cargo clippy -p camel-config -- -D warnings` exits 0.
- `cargo test -p camel-config --lib` passes.

- [x] 1.4

## Phase 2: path-policy

### Task 2.1: OwnedHandle cfg-gated file handles + FileIdentity

**Files:**
- `crates/components/camel-template/src/path_util.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified — add `mod path_util;`)

**Steps:**
1. Create `src/path_util.rs`. Define `pub(crate) struct OwnedHandle` wrapping a cfg-gated inner: `#[cfg(unix)] OwnedFd` (from `std::os::fd`), `#[cfg(windows)] std::os::windows::io::OwnedHandle`.
2. Add `impl OwnedHandle { pub fn open_relative(root: &OwnedHandle, name: &str, max_bytes: usize) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError> }`. `name` MAY be multi-component (e.g. `partials/page.html`); to confine, traverse EACH path component handle-relative (a single `openat(..., O_NOFOLLOW, ...)` only rejects a symlink in the TRAILING component — an intermediate symlink like `partials/link/page.html` where `link` points outside root would escape — Critical C1):
   - Split `name` on `/`. Reject any component that is empty, `.`, `..`, or absolute (leading `/`).
   - Walk components: start from `root`. For each INTERMEDIATE component, `rustix::fs::openat(curr.as_fd(), comp, OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW, Mode::empty())` — `O_DIRECTORY | O_NOFOLLOW` rejects both a non-directory AND a symlink at that component; advance `curr` to the returned handle. For the FINAL component, `openat(curr.as_fd(), last, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty())`.
   - Windows equivalent: per-component `NtCreateFile` with `OBJECT_ATTRIBUTES.RootDirectory` chained, `FILE_OPEN_REPARSE_POINT`/reparse-point rejection at every component (NOT `CreateFileW`). `Wdk_Storage_FileSystem` feature.
   - `max_bytes` is passed to the bounded read (Task 2.3), not used in opening.
3. Define `#[derive(Debug, Clone, PartialEq, Eq, Hash)] pub(crate) struct FileIdentity` cfg-gated: Unix `{ inode: u64, length: u64, mtime_nsec: i64 }`, Windows `{ volume_serial: u32, file_index_high: u32, file_index_low: u32, length: u64, last_write_100ns: i64 }`. Populate from `rustix::fs::StatExt`/`statx` (Unix) and `FILE_INTERNAL_INFO`+`FILE_STANDARD_INFO`+`FILE_FS_VOLUME_INFORMATION` (Windows).
4. Add `pub(crate) fn open_root(root_abs_path: &std::path::Path) -> Result<(OwnedHandle, FileIdentity), TemplateReloadError>` that opens the configured root directory (entry template's parent) by absolute path and returns its handle + identity. Reject paths containing `..` after normalization; reject a missing parent with `Err(TemplateReloadError::PathEscape("missing parent"))`. (API is `&Path`, NOT `&str` — standardize all path params on `&Path` across Tasks 2.1/2.4/4.4/5.1.)

> NOTE: the Windows `NtCreateFile` + `OBJECT_ATTRIBUTES.RootDirectory` path and the `windows-sys` feature set (`Wdk_Storage_FileSystem`) cannot be validated on the Linux CI host. Unix validation is the gate here; add a Windows build check (cross-compile or a Windows CI job) before relying on the Windows path in production.

**Tests:**
- `owned_handle_open_relative_unix`: setup = a `tempfile::tempdir()` with a child file, root handle opened on the dir; action = `OwnedHandle::open_relative(&root, "child", 1024)`; assert = `Ok`, identity `length > 0`. command = `cargo test -p camel-template --lib owned_handle_open_relative_unix` (cfg unix). expected = pass after step 2.
- `open_relative_rejects_symlink_escape`: setup = a child that is a symlink pointing outside the root; action = `open_relative(&root, "link", 1024)`; assert = `Err(PathEscape(_))` (trailing-component `O_NOFOLLOW` rejects the leaf symlink). command = `cargo test -p camel-template --lib open_relative_rejects_symlink_escape` (cfg unix). expected = pass after step 2.
- `open_relative_rejects_intermediate_symlink_escape`: setup = `partials/` is a real dir, but `partials/link` is a symlink to a dir outside root, and the target is `partials/link/page.html`; action = `open_relative(&root, "partials/link/page.html", 1024)`; assert = `Err(PathEscape(_))` (the per-component walk rejects the intermediate `link` symlink via `O_DIRECTORY | O_NOFOLLOW` — Critical C1). command = `cargo test -p camel-template --lib open_relative_rejects_intermediate_symlink_escape` (cfg unix). expected = pass after step 2.
- `open_root_rejects_dotdot`: setup = path `Path::new("/srv/../etc")`; action = `open_root(path)`; assert = `Err(TemplateReloadError::PathEscape(_))`. command = `cargo test -p camel-template --lib open_root_rejects_dotdot`. expected = pass after step 4.

**Acceptance:**
- `cargo build -p camel-template` succeeds on unix target.
- `cargo clippy -p camel-template -- -D warnings` exits 0.

- [x] 2.1

### Task 2.2: TemplateEndpointConfig URI parser (parse-only)

**Files:**
- `crates/components/camel-template/src/config.rs` (modified)
- `crates/components/camel-template/src/uri.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified — add `mod uri;`)

**Steps:**
1. Create `src/uri.rs`. Define `pub(crate) struct TemplateEndpointConfig { entry_abs_path: PathBuf, limits: ExternalTemplateLimitsConfig }`.
2. Add `pub(crate) fn parse_template_uri(uri: &str, limits: ExternalTemplateLimitsConfig) -> Result<TemplateEndpointConfig, CamelError>`. Logic: require scheme `template`, require next scheme part exactly `file`, require a non-empty absolute path (`file:///...`). Reject bare-path (`template:/abs/path` — no `file` part) and any non-`file` scheme (`template:http://...`) with `CamelError::Config("template URI must be file:///<abs-path>: ...")`. Do NOT touch the filesystem here.
3. Validate the path is absolute (`Path::is_absolute`) and contains no `..` segments.

**Tests:**
- `parse_valid_file_uri`: setup = `template:file:///srv/t/page.html`; action = `parse_template_uri(...)`; assert = `Ok`, `entry_abs_path == /srv/t/page.html`. command = `cargo test -p camel-template --lib parse_valid_file_uri`. expected = pass after step 2.
- `parse_rejects_bare_path`: setup = `template:/srv/t/page.html`; action = parse; assert = `Err(CamelError::Config(_))`. command = `cargo test -p camel-template --lib parse_rejects_bare_path`. expected = pass.
- `parse_rejects_non_file_scheme`: setup = `template:http://h/p`; action = parse; assert = `Err(CamelError::Config(_))`. command = `cargo test -p camel-template --lib parse_rejects_non_file_scheme`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib parse_` passes (3 tests).

- [x] 2.2

### Task 2.3: StableTemplateReader + acquire_closure iterative DFS

**Files:**
- `crates/components/camel-template/src/closure.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified — add `mod closure;`)

**Steps:**
1. Create `src/closure.rs`. Define `pub(crate) trait StableTemplateReader: Send + Sync { fn read_relative(&self, root: &OwnedHandle, name: &str, max_bytes: usize) -> Result<(OwnedHandle, FileIdentity, Vec<u8>), TemplateReloadError>; }`. Implementations MUST read bounded: open via `OwnedHandle::open_relative(root, name, max_bytes)`, then read up to `max_bytes` bytes, rejecting (without first allocating the whole file) when the file exceeds `max_bytes` — use a bounded reader loop, return `Err(BoundExceeded("max_template_size"))` as soon as the limit is exceeded.
2. Define `#[derive(Debug)] pub(crate) struct TemplateFile { name: String, bytes: Vec<u8>, identity: FileIdentity }`.
3. Define `pub(crate) struct ClosureSnapshot { entries: BTreeMap<String, TemplateFile> }` with `pub fn deterministic_hash(&self) -> [u8;32]` using `blake3` over length-delimited `(name, bytes)` tuples.
4. Implement `pub(crate) fn acquire_closure(reader: &dyn StableTemplateReader, entry: String, root: &OwnedHandle, limits: ResolvedExternalTemplateLimits) -> Result<ClosureSnapshot, TemplateReloadError>`. Single-pass DFS: a `Vec<WalkItem>` stack + `HashMap<String, VisitState>` (Gray=on-stack, Black=done). `root` is BORROWED (`&OwnedHandle`) so the same root handle is reused across reloads (Task 5.1). For each source read: (a) enforce `max_template_size` and running `max_total_source_bytes`; (b) parse static include/extends/import/from targets via a regex over the MiniJinja source (only string-literal targets — variable targets are rejected as not statically discoverable); (c) reject symlink/`..`/absolute targets; (d) push child reads transitively (read-then-walk, so edges are known); (e) cycle → on-stack Gray hit → `Err(Cycle)`; (f) duplicate identity → `Err(DuplicateIdentity)`; (g) `max_include_count`/`max_include_depth` bounds enforced.
5. Export `ClosureSnapshot` field accessors used by Phase 4 (`entries`, `deterministic_hash`).

**Tests:**
- `acquire_closure_flat`: setup = tempdir with `page.html` (no includes) + a `FilesystemTemplateReader`; action = `acquire_closure(...)`; assert = `Ok`, one entry. command = `cargo test -p camel-template --lib acquire_closure_flat`. expected = pass after step 4.
- `acquire_closure_transitive`: setup = `a.html` includes `b.html` includes `c.html`; action = acquire; assert = 3 entries. command = `cargo test -p camel-template --lib acquire_closure_transitive`. expected = pass.
- `acquire_closure_rejects_cycle`: setup = `a` includes `b`, `b` includes `a`; action = acquire; assert = `Err(Cycle(_))`. command = `cargo test -p camel-template --lib acquire_closure_rejects_cycle`. expected = pass.
- `acquire_closure_rejects_escape`: setup = `a` includes `../secret`; action = acquire; assert = `Err(PathEscape(_))`. command = `cargo test -p camel-template --lib acquire_closure_rejects_escape`. expected = pass.
- `acquire_closure_rejects_symlink`: setup = `a` includes a symlinked name pointing outside the root; action = acquire; assert = `Err(PathEscape(_))` (openat-relative opens + identity check must reject the redirect). command = `cargo test -p camel-template --lib acquire_closure_rejects_symlink`. expected = pass.
- `acquire_closure_rejects_dynamic`: setup = `a` includes `{{x}}`; action = acquire; assert = `Err(Acquire(_))`. command = `cargo test -p camel-template --lib acquire_closure_rejects_dynamic`. expected = pass.
- `acquire_closure_rejects_duplicate_identity`: setup = two names hardlinked to same inode; action = acquire; assert = `Err(DuplicateIdentity(_))`. command = `cargo test -p camel-template --lib acquire_closure_rejects_duplicate_identity`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib acquire_closure` passes (6 tests).

- [x] 2.3

### Task 2.4: FilesystemTemplateReader + bounded snapshot assembly

**Files:**
- `crates/components/camel-template/src/closure.rs` (modified)
- `crates/components/camel-template/src/path_util.rs` (modified)

**Steps:**
1. Implement `pub(crate) struct FilesystemTemplateReader;` (in `closure.rs`) impl `StableTemplateReader`: `read_relative` opens via `OwnedHandle::open_relative`, reads up to `max_template_size` bytes (rejecting oversize with `BoundExceeded("max_template_size")`), returns `(OwnedHandle, FileIdentity, Vec<u8>)`.
2. Add `pub(crate) fn build_snapshot(entry: &std::path::Path, root: &OwnedHandle, limits: ResolvedExternalTemplateLimits) -> Result<ClosureSnapshot, TemplateReloadError>`: the caller opens root once (Task 2.1 `open_root`) and passes it BY REFERENCE; `build_snapshot` does NOT open root itself. The `entry` is the absolute entry path (caller passes `&PathBuf` which coerces to `&Path`). Construct `FilesystemTemplateReader`, call `acquire_closure(reader, entry.file_name()...to_string(), root, limits)` (the entry NAME relative to root is the file_name; reject if `entry.file_name()` is None with `PathEscape`), return the snapshot. The root handle is retained by the caller (ReloadHandler in Task 5.1) so reload re-acquires against the same handle.

**Tests:**
- `build_snapshot_real_files`: setup = tempdir with `page.html` + `header.html` (included); action = `open_root(tempdir)` then `build_snapshot(entry, &root, limits)`; assert = `Ok`, snapshot has 2 entries, deterministic_hash is stable across two calls (same root reused). command = `cargo test -p camel-template --lib build_snapshot_real_files`. expected = pass after step 2.
- `build_snapshot_rejects_oversize`: setup = a file larger than `max_template_size`; action = `open_root` then `build_snapshot` with a tight limit; assert = `Err(BoundExceeded(_))`. command = `cargo test -p camel-template --lib build_snapshot_rejects_oversize`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib build_snapshot` passes.

- [x] 2.4

## Phase 3: producer-start-lifecycle-spi

### Task 3.1: StepLifecycle::start hook

**Files:**
- `crates/camel-api/src/step_lifecycle.rs` (modified)

**Steps:**
1. In `step_lifecycle.rs:30-36` add to the `StepLifecycle` trait a default method: `async fn start(&self) -> Result<(), CamelError> { Ok(()) }`. Keep `#[async_trait]`. Existing implementors (`ResequencerService`, `AggregatorService`, test fakes) are unaffected by the default. Generation is NOT threaded through `ProducerContext`; it lives on the `ReloadHandler` (Phase 5).

**Tests:**
- `start_default_is_noop`: setup = a `FakeStep` (mirror `route_controller_tests.rs:1520`) that does NOT override `start`; action = `FakeStep.start().await`; assert = `Ok(())`. command = `cargo test -p camel-api --lib start_default_is_noop`. expected = pass after step 1.

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo test -p camel-api --lib` passes; existing `ResequencerService`/`AggregatorService` StepLifecycle impls unchanged; `ProducerContext` untouched.

- [x] 3.1

### Task 3.2: Endpoint::lifecycle() accessor + endpoints.rs wiring

**Files:**
- `crates/components/camel-component-api/src/endpoint.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/endpoints.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/step_compilers/mod.rs` (modified — if `resolve_producer` lives here)

**Steps:**
1. In `endpoint.rs:42-52` add to the `Endpoint` trait a default method: `fn lifecycle(&self) -> Option<std::sync::Arc<dyn StepLifecycle>> { None }`. (`StepLifecycle` is in `camel-api`, already a dep of `camel-component-api`.)
2. In `endpoints.rs` `To` arm (`:29-42`): after `create_producer`, capture `let lifecycle = endpoint.lifecycle();` and pass it into `CompiledStep::Process { ..., lifecycle }` instead of `lifecycle: None`.
3. In `endpoints.rs` `WireTap` arm (`:46-52`): extend `resolve_producer` (and/or this arm) to also resolve the endpoint's `lifecycle()` and pass it into the `CompiledStep::Process.lifecycle` field at `:52` instead of `None`. If `resolve_producer` does not currently surface the endpoint, add a parallel `resolve_producer_with_lifecycle` or return the lifecycle alongside, ensuring the WireTap `Process` step carries it.

**Tests:**
- `endpoint_lifecycle_default_none`: setup = a trivial `Endpoint` impl not overriding `lifecycle`; action = `.lifecycle()`; assert = `None`. command = `cargo test -p camel-component-api --lib endpoint_lifecycle_default_none`. expected = pass after step 1.
- `to_arm_propagates_lifecycle`: setup = a route `To` step whose endpoint overrides `lifecycle` to `Some(Arc::new(FakeStep))`; action = compile the step; assert = `CompiledStep::Process.lifecycle == Some(_)`. command = `cargo test -p camel-core --lib to_arm_propagates_lifecycle`. expected = pass after step 2.
- `wiretap_arm_propagates_lifecycle`: setup = a `WireTap` step whose endpoint overrides `lifecycle`; action = compile; assert = `Process.lifecycle == Some(_)`. command = `cargo test -p camel-core --lib wiretap_arm_propagates_lifecycle`. expected = pass after step 3.

**Acceptance:**
- `cargo clippy -p camel-component-api -- -D warnings` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib` passes (existing endpoint tests unaffected — default `None`).

- [x] 3.2

### Task 3.3: start_route awaits start() with reverse-order rollback

**Files:**
- `crates/camel-core/src/lifecycle/adapters/route_controller_trait.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/route_controller_tests.rs` (modified)

**Steps:**
1. In `start_route` (`route_controller_trait.rs:30`), after assembly and BEFORE the pipeline spawn (`:146`) and Consumer spawn (`:257`), collect all lifecycle handles from the assembled route in order. Await `handle.start().await` on each in order.
2. On the Nth `start()` returning `Err`: call `handle.shutdown(StepShutdownReason::RouteStop).await` on handles `1..N` in reverse order (best-effort; log errors), then return the `Err` from `start_route`. Do not spawn the pipeline/Consumer.
3. If all `start()` succeed, proceed to the existing pipeline+Consumer spawn unchanged.
4. Add a test fake `FailingStartStep` (overrides `start` to `Err`) and a recording `ShutdownSpy` in `route_controller_tests.rs`.

**Tests:**
- `start_route_awaits_start_in_order`: setup = a route with two recording lifecycle handles; action = `start_route`; assert = `start()` called on both, in order, before pipeline spawn. command = `cargo test -p camel-core --lib start_route_awaits_start_in_order`. expected = pass after step 1.
- `start_route_rolls_back_on_failure`: setup = handle 1 starts OK, handle 2 `start()` returns `Err`; action = `start_route`; assert = handle 1 `shutdown(RouteStop)` called, pipeline NOT spawned, `start_route` returns `Err`. command = `cargo test -p camel-core --lib start_route_rolls_back_on_failure`. expected = pass after step 2.
- `start_route_default_start_noop_unaffected`: setup = an existing route whose handles use the default no-op `start`; action = `start_route`; assert = identical to pre-change behavior (regression). command = `cargo test -p camel-core --lib start_route_default_start_noop_unaffected`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib start_route` passes.
- `cargo test -p camel-core --test hexagonal_architecture_boundaries_test` passes (start_route is in the adapters slice; ensure no forbidden import added).

- [x] 3.3

## Phase 4: render

### Task 4.1: TemplateSet + SharedTemplates + compile via engine

**Files:**
- `crates/components/camel-template/src/template_set.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified)

**Steps:**
1. Create `src/template_set.rs`. Define `pub(crate) struct TemplateSet { env: minijinja::Environment<'static>, entry: String }`. Add `pub fn empty() -> Self` returning a set with a default `Environment` and `entry = String::new()` (used only as the ArcSwap seed before `start()` replaces it; never rendered).
2. Add `impl TemplateSet { pub fn compile(snapshot: &ClosureSnapshot, entry: &str, render_limits: MinijinjaLimitsConfig) -> Result<Self, TemplateReloadError> }`: build a `minijinja::Environment`, set `set_undefined_behavior(Strict)`, `set_fuel`, `set_recursion_limit` from `ResolvedLimits::from_config(&render_limits)`. BEFORE `add_template_owned` on the entry, call `camel_language_minijinja::validate_autoescape_wrapper(&entry_source)` (the ADR-0047 top-level autoescape check, made pub in Task 1.2 step 4); on `Err` map to `TemplateReloadError::Compile`. Add each snapshot entry via `add_template_owned`, return `Self`. Other errors map to `TemplateReloadError::Compile`.
3. Define `pub(crate) type SharedTemplates = std::sync::Arc<arc_swap::ArcSwap<TemplateSet>>;` (mirror gRPC `server.rs:43`).
4. Add `pub async fn render_entry(&self, context: minijinja::Value, render_limits: ResolvedLimits) -> Result<String, TemplateReloadError>`. This renders the ALREADY-COMPILED entry against `context` — do NOT call `engine::render` (which recompiles from source). Instead reuse ONLY the `spawn_blocking` + `tokio::time::timeout` + `LimitedWriter` pattern from the engine module against `self.env.get_template(&self.entry)` and the configured `max_output`/`execution_timeout_ms`. Re-export `ResolvedLimits` usage.
5. ADR-0047 requires a top-level `{% autoescape "html"|"json"|"none" %}` wrapper; `TemplateSet::compile` (step 2) enforces it by calling the now-public `validate_autoescape_wrapper` on the entry before adding it to the Environment.

**Tests:**
- `template_set_compile_and_render`: setup = a `ClosureSnapshot` with one entry `page.html` = `{% autoescape "none" %}Hi {{name}}{% endautoescape %}`; action = `TemplateSet::compile(...)` then `render_entry(value!{"name"=>"k"}, limits)`; assert = `Ok("Hi k")`. command = `cargo test -p camel-template --lib template_set_compile_and_render`. expected = pass after step 4.
- `template_set_compile_strict_undefined`: setup = entry references undefined `{{nope}}`; action = render; assert = `Err(Compile(_))` (strict-undefined). command = `cargo test -p camel-template --lib template_set_compile_strict_undefined`. expected = pass.
- `template_set_compile_requires_autoescape`: setup = entry WITHOUT a top-level `{% autoescape %}` block; action = `TemplateSet::compile(...)`; assert = `Err(Compile(_))` (explicit-autoescape enforced per ADR-0047). command = `cargo test -p camel-template --lib template_set_compile_requires_autoescape`. expected = pass after step 5.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib template_set` passes.

- [x] 4.1

### Task 4.2: TemplateProducer (Service<Exchange>) render semantics

**Files:**
- `crates/components/camel-template/src/producer.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified)

**Steps:**
1. Create `src/producer.rs`. Define `#[derive(Clone)] pub(crate) struct TemplateProducer { templates: SharedTemplates, render_limits: ResolvedLimits, rt: Option<Arc<dyn RuntimeObservability>>, route_id: String }`.
2. `impl Service<Exchange> for TemplateProducer` (`Response=Exchange`, `Error=CamelError`, `Future=Pin<Box<...>>`) mirroring `camel-xslt/src/producer.rs:74-128`. `poll_ready` returns `Ready(Ok(()))`.
3. Add a PRIVATE `async fn render_into(&self, exchange: &mut Exchange) -> Result<(), CamelError>` (the testable seam — `Service::call` consumes the Exchange so a failed render is otherwise unobservable): build a `minijinja::Value` context from `exchange.in_body()` + headers + properties (reuse the context-building approach from `engine`/`build_context_bounded`); load the current set via `self.templates.load_full()`; call `render_entry`. On `Ok(rendered)`: set `exchange.input.body = Body::from(rendered)` (headers and properties untouched). On `Err(e)`: return `Err(CamelError::from(e))` WITHOUT mutating the body (the caller's `&mut Exchange` stays as-is). The Exchange body/headers/properties are NEVER consulted to select the entry or root (zero-override).
4. `Service::call(&mut self, mut exchange)` delegates: `self.render_into(&mut exchange).await?; Ok(exchange)` — so all observable behavior (incl. body-unchanged-on-error) lives in `render_into`, which tests can call directly on a borrowed Exchange.

**Tests:**
- `producer_replaces_body_on_success`: setup = a `SharedTemplates` seeded with `{{body}}`-echoing entry; action = `producer.call(exchange_with_body("x"))`; assert = body becomes rendered output, headers preserved. command = `cargo test -p camel-template --lib producer_replaces_body_on_success`. expected = pass after step 3.
- `producer_leaves_body_on_render_error`: setup = a `TemplateProducer` and a mutable `Exchange` with a known body, plus a set whose entry has an undefined variable under strict-undefined; action = `producer.render_into(&mut exchange).await`; assert = `Err(CamelError::TemplateReload(_))` AND the SAME mutable `Exchange`'s body is unchanged (observable because `render_into` borrows `&mut Exchange` rather than consuming it). command = `cargo test -p camel-template --lib producer_leaves_body_on_render_error`. expected = pass after step 3.
- `producer_ignores_override_header`: setup = exchange header `X-Template-File=/etc/passwd`; action = `producer.call(exchange)`; assert = renders the operator-configured entry, header ignored. command = `cargo test -p camel-template --lib producer_ignores_override_header`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib producer_` passes.

- [x] 4.2

### Task 4.3: TemplateComponent + TemplateEndpoint (producer-only)

**Files:**
- `crates/components/camel-template/src/component.rs` (new)
- `crates/components/camel-template/src/endpoint.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified)

**Steps:**
1. Create `src/component.rs`. Define `#[derive(Default)] pub struct TemplateComponent { limits: ExternalTemplateLimitsConfig, render_limits: MinijinjaLimitsConfig }` (constructible from config; `Default` derives `MinijinjaLimitsConfig::default()`). `impl Component for TemplateComponent`: `scheme()` → `"template"`; `create_endpoint(uri, ctx)` → `let config = uri::parse_template_uri(uri, self.limits.clone())?;` (parse-only, NO filesystem access); capture `route_id` from `ctx` (the current route being built — `ComponentContext` exposes the in-construction route id the same way xslt's `create_endpoint` calls `ctx.register_current_route_health_check`); resolve acquisition limits here `let limits = config.limits.resolve()?;` (returns `Result`, so `?` is valid in `create_endpoint` — a zero value fails closed at endpoint construction with `CamelError::TemplateReload`); return `Box::new(TemplateEndpoint::new(uri.to_string(), config.entry_abs_path, limits, self.render_limits.clone(), route_id))`.
2. Create `src/endpoint.rs`. Define `pub(crate) struct TemplateEndpoint { uri: String, entry_abs_path: PathBuf, limits: ResolvedExternalTemplateLimits, render_limits: MinijinjaLimitsConfig, route_id: String, shared: SharedTemplates, rt: std::sync::Mutex<Option<Arc<dyn RuntimeObservability>>> }`. In `Endpoint::new`, seed `shared = Arc::new(ArcSwap::from_pointee(TemplateSet::empty()))` (`TemplateSet::empty()` — add a constructor returning a set with an empty `Environment` and no entry; it is never rendered because `start()` replaces it before any request). `impl Endpoint`: `uri()` → `&self.uri` (required by the trait, mirror xslt); `body_contract()` → `None`; `create_producer(rt, ctx)` → stash `*self.rt.lock() = Some(Arc::clone(&rt));` then return `Ok(BoxProcessor::new(TemplateProducer { templates: Arc::clone(&self.shared), render_limits: ResolvedLimits::from_config(&self.render_limits), rt: Some(rt), route_id: self.route_id.clone() }))`. This IS the real producer; it renders the set that `start()` (Task 4.4) populates. A request cannot arrive before `start()` completes because `start_route` awaits `start()` before serving (Task 3.3).
3. `create_consumer` → `Err(CamelError::EndpointCreationFailed("template is producer-only"))` (mirror xslt `endpoint.rs:53-60`).
4. `lifecycle()` → `let rt = self.rt.lock().expect("rt cell poisoned").clone();` then `Some(Arc::new(StartupBuildHandle { shared: Arc::clone(&self.shared), entry_abs_path: self.entry_abs_path.clone(), render_limits: self.render_limits.clone(), limits: self.limits, rt, route_id: self.route_id.clone(), handler: std::sync::Mutex::new(None) }))`. (Phase 4 declares only the `handler` field — the `guard` field is added in Task 5.3 when registration is wired. `Mutex::lock()` returns a `LockResult`; always `.expect(...)` it, never `.unwrap()`.) (No `?` here — `limits` is already the resolved `ResolvedExternalTemplateLimits` from step 1.) `StartupBuildHandle` impls `StepLifecycle` (Task 4.4 defines `start()`).

**Tests:**
- `component_scheme_is_template`: setup = `TemplateComponent::default()`; action = `.scheme()`; assert = `"template"`. command = `cargo test -p camel-template --lib component_scheme_is_template`. expected = pass after step 1.
- `endpoint_create_consumer_errors`: setup = a `TemplateEndpoint`; action = `create_consumer(...)`; assert = `Err(...)`. command = `cargo test -p camel-template --lib endpoint_create_consumer_errors`. expected = pass after step 3.
- `endpoint_lifecycle_returns_handle`: setup = a `TemplateEndpoint`; action = `.lifecycle()`; assert = `Some(_)`. command = `cargo test -p camel-template --lib endpoint_lifecycle_returns_handle`. expected = pass after step 4.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib` passes.

- [x] 4.3

### Task 4.4: StartupBuildHandle — startup acquire + compile + seed + metrics

**Files:**
- `crates/components/camel-template/src/lifecycle.rs` (new)
- `crates/components/camel-template/src/reload.rs` (new — ReloadHandler struct shell only; methods added in Task 5.1)
- `crates/components/camel-template/src/endpoint.rs` (modified — use `StartupBuildHandle`)
- `crates/components/camel-template/src/lib.rs` (modified)

**Steps:**
1. Create `src/lifecycle.rs`. Define `pub(crate) struct StartupBuildHandle { shared: SharedTemplates, entry_abs_path: PathBuf, render_limits: MinijinjaLimitsConfig, limits: ResolvedExternalTemplateLimits, rt: Option<Arc<dyn RuntimeObservability>>, route_id: String, handler: std::sync::Mutex<Option<Arc<ReloadHandler>>> }`. (The `guard: Mutex<Option<RegistrationGuard>>` field is added in Task 5.3 — it does not exist in Phase 4.) Create `src/reload.rs` with the struct SHELL ONLY (no methods yet): `pub(crate) struct ReloadHandler { shared: SharedTemplates, entry_abs_path: PathBuf, render_limits: MinijinjaLimitsConfig, limits: ResolvedExternalTemplateLimits, generation: std::sync::Mutex<u64>, root: Arc<OwnedHandle>, rt: Option<Arc<dyn RuntimeObservability>>, route_id: String }`. Add `pub mod lifecycle; pub mod reload;` to `lib.rs`. (The `build`/`commit` methods + `StagedSet` are added in Task 5.1; defining the struct shell here keeps Phase 4 self-contained — `lifecycle.rs::start()` can construct and store it. `root` is `Arc<OwnedHandle>` so it can be cloned into `spawn_blocking` in Task 5.1.)
2. `impl StepLifecycle for StartupBuildHandle`: `name()` → `"template-startup"`. `async fn start(&self)`: extract the parent dir `let parent = self.entry_abs_path.parent().ok_or_else(|| CamelError::from(TemplateReloadError::PathEscape("entry has no parent".into())))?;` (`Path::parent()` returns `Option<&Path>`); open root ONCE via `path_util::open_root(parent)` → `(root, _id)` (an `OwnedHandle`); wrap it `let root = Arc::new(root);` (ReloadHandler.root is `Arc<OwnedHandle>`); on `Err(e)` return `Err(CamelError::from(e))`. Call `closure::build_snapshot(&self.entry_abs_path, root.as_ref(), self.limits)` (`as_ref()` → `&OwnedHandle`; `&self.entry_abs_path` is `&PathBuf` coercing to `&Path`); on `Err` return `Err`. `TemplateSet::compile(&snapshot, entry, self.render_limits.clone())`; on `Err` return `Err`. On `Ok(set)`: `self.shared.store(Arc::new(set))`; build the `ReloadHandler { shared, entry_abs_path, render_limits, limits, generation: Mutex::new(0), root, rt, route_id }` (RETAINS the `Arc<OwnedHandle>`) and store it in `*self.handler.lock().expect("handler poisoned") = Some(Arc::new(handler));`; return `Ok(())`. Do NOT record `template_reloads_total` at startup (startup is not a reload; the metric fires once per route-scoped reload at the intercept — Task 5.4). (The `ReloadHandler` struct + its `build`/`commit` are defined in Task 5.1; Phase 4 only constructs it with generation 0 and retains it — it does not wire the registry yet.)
3. `async fn shutdown(&self, _reason)`: Phase 4 no-op (Phase 5 adds reload-registration guard drop via the retained handler). Return `Ok(())`.

**Tests:**
- `startup_build_compiles_and_seeds`: setup = tempdir with a valid `page.html`, a `SharedTemplates::from_pointee(empty)`; action = `handle.start().await`; assert = `Ok(())`, `shared.load_full()` renders the entry. command = `cargo test -p camel-template --lib startup_build_compiles_and_seeds`. expected = pass after step 2.
- `startup_build_fails_closed_on_missing_file`: setup = `entry_abs_path` pointing at a non-existent file; action = `handle.start().await`; assert = `Err(CamelError::TemplateReload(_))`, `shared` still empty. command = `cargo test -p camel-template --lib startup_build_fails_closed_on_missing_file`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib startup_build` passes.

- [x] 4.4

### Task 4.5: Register template: scheme + TemplateBundle config + integration test

**Files:**
- `crates/camel-cli/Cargo.toml` (modified — add `camel-template.workspace`)
- `crates/camel-cli/src/commands/run.rs` (modified)
- `crates/components/camel-template/Cargo.toml` (modified — add `toml.workspace` for the bundle)
- `crates/components/camel-template/src/bundle.rs` (new)
- `crates/components/camel-template/src/lib.rs` (modified)
- `crates/components/camel-template/tests/template_render_integration.rs` (new)

**Steps:**
1. Add `camel-template = { workspace = true }` to `crates/camel-cli/Cargo.toml` `[dependencies]`, `toml = { workspace = true }` to `crates/components/camel-template/Cargo.toml` `[dependencies]`, and the INTEGRATION-TEST dev-deps to `crates/components/camel-template/Cargo.toml` `[dev-dependencies]`: `camel-core.workspace`, `camel-builder.workspace`, and `camel-component-timer.workspace` (the inbound test component used to drive a route and send Exchanges through the template producer). Register via the BUNDLE path (not a bare `register_component(default)`) so configured acquisition/render limits actually apply (Important 3): in `run.rs` add `register_bundle!(camel_template::TemplateBundle)` (mirror `register_bundle!(camel_component_grpc::GrpcBundle)` at `run.rs:412`, but with NO feature gate — always-on built-in). The bundle's `register_all` constructs `TemplateComponent` from the `[template]` config block when present, else from defaults — so `TemplateComponent::default()` is never registered directly (which would ignore configured limits). Follow the no-feature-flag convention used by built-in components.
2. Create `src/bundle.rs` implementing `ComponentBundle` (`camel-component-api/src/bundle.rs:7-15`): `config_key() -> "template"`, `from_toml(toml)` deserializing `ExternalTemplateLimitsConfig` + `MinijinjaLimitsConfig`, `register_all` constructing a `TemplateComponent { limits, render_limits }` and calling `ctx.register_component_dyn(...)`. Export `pub struct TemplateBundle` and re-export from `lib.rs`. This makes both limit layers operator-configurable (design "Two limit layers").
3. Create `tests/template_render_integration.rs`: register a `TemplateComponent` into a minimal `CamelContext`, define a route `To("template:file:///<tempdir>/page.html")` with a real temp file `page.html` = `{% autoescape "html" %}<h1>{{title}}</h1>{% endautoescape %}`, start the context (which awaits `start()`), send an Exchange with body/headers, and assert the response body is `<h1>Hi</h1>` with headers preserved and zero additional FS reads on a second request.

**Tests:**
- `template_renders_end_to_end`: as step 2. command = `cargo test -p camel-template --test template_render_integration`. expected = pass after steps 1-2.
- `template_compile_once_no_hot_io`: setup = start a route with a real template file; AFTER `start()` completes, delete (or rename) the source file on disk; action = send two requests; assert = both renders still succeed with the compiled output (the hot path holds the compiled set in memory and does NOT re-read the file — proving zero hot-path FS I/O without needing access to private readers). command = `cargo test -p camel-template --test template_render_integration template_compile_once_no_hot_io`. expected = pass.
- `missing_template_fails_route_closed`: setup = a route `To("template:file:///<tempdir>/missing.html")` where the file does not exist; action = start the context; assert = `start_route` returns `Err` (the `start()` hook fails closed) and the route enters `Failed`, serving no requests (the spec "startup compile failure fails the route" scenario). command = `cargo test -p camel-template --test template_render_integration missing_template_fails_route_closed`. expected = pass.

**Acceptance:**
- `cargo build -p camel-cli` succeeds with the registration.
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --test template_render_integration` passes.
- `cargo xtask schema --check` exits 0 (confirm the new `template:` scheme needs no schema-tooling change; if it does, update the schema in this task).
- AC 1, 2, 3, 4, 7, 9 demonstrably met.

- [x] 4.5

## Phase 5: reload

> Architecture note (Critical dependency-inversion): `camel-core` cannot depend on
> `camel-template` (components depend on core, never vice versa). The reload
> registry + erased target contract therefore live in `camel-component-api`
> (mirroring `TlsReloadRegistry` at `tls_source.rs:146`), which both `camel-core`
> (RuntimeBus) and `camel-template` (ReloadHandler impl) can see.

### Task 5.1: ReloadHandler build + infallible commit via spawn_blocking

**Files:**
- `crates/components/camel-template/src/reload.rs` (modified — add `StagedSet` + `build`/`current_generation`/`commit`)
- `crates/components/camel-template/src/lifecycle.rs` (modified)

**Steps:**
1. In `src/reload.rs` (struct shell from Task 4.4, `root` already `Arc<OwnedHandle>`), add `pub(crate) struct StagedSet { set: TemplateSet, read_generation: u64 }`. All `Mutex` access uses `.lock().expect("<field> poisoned")` (NEVER `.unwrap()` — `cargo xtask lint-unwrap` forbids it).
2. `pub async fn build(&self) -> Result<(Box<dyn camel_component_api::template_reload::TemplateReloadStaged>, u64), CamelError>`: run the bounded FS read + compile inside `tokio::task::spawn_blocking` (sync work CANNOT be interrupted by `tokio::time::timeout` — Critical 4). Clone `Arc<OwnedHandle>` + entry path + limits into the blocking closure; call `closure::build_snapshot(&entry, &root, limits)` then `TemplateSet::compile`; on `Err` return `Err(CamelError::from(e))` WITHOUT storing (prior set retained). On `Ok(set)`: `let read_generation = *self.generation.lock().expect("generation poisoned");` return `(Box::new(StagedSet { set, read_generation }), read_generation)`. Do NOT store or increment here.
3. `pub fn current_generation(&self) -> u64 { *self.generation.lock().expect("generation poisoned") }`.
4. `pub fn commit(&self, staged: Box<dyn camel_component_api::template_reload::TemplateReloadStaged>)`: this is INFALLIBLE — it is only ever called by `reload_route` AFTER validation (Task 5.2), so no generation recheck is needed here. Downcast via the staged's `into_any`: `let concrete = staged.into_any().downcast::<StagedSet>().expect("staged type matches its builder");` then `*self.generation.lock().expect("generation poisoned") += 1; self.shared.store(Arc::new(concrete.set));`. Do NOT record `template_reloads_total` here (it is recorded ONCE per route-scoped aggregation at the RuntimeBus intercept, Task 5.4 — not per target, not at startup). The `expect` is safe: only `ReloadHandler::build` produces a `StagedSet` and only `ReloadHandler::commit` consumes it.
5. Refactor `StartupBuildHandle` (Task 4.4) so `start()` seeds generation 0 by constructing the `ReloadHandler` (root retained as `Arc<OwnedHandle>`), storing it in `self.handler`, and doing an initial compile+store directly (the registry is wired in Task 5.3).

**Tests:**
- `reload_build_does_not_store_on_compile_error`: setup = seeded S0; action = make file invalid, `handler.build().await`; assert = `Err`, `shared.load_full()` still S0. command = `cargo test -p camel-template --lib reload_build_does_not_store_on_compile_error`. expected = pass after step 2.
- `reload_commit_swaps_on_valid_change`: setup = seeded v1; action = mutate file, `build().await` then `commit(staged)`; assert = `shared.load_full()` renders v2, generation incremented. command = `cargo test -p camel-template --lib reload_commit_swaps_on_valid_change`. expected = pass.
- `reload_commit_is_infallible`: setup = a valid StagedSet built by this handler; action = `commit(staged)`; assert = returns `()` (no Result), set swapped. command = `cargo test -p camel-template --lib reload_commit_is_infallible`. expected = pass after step 4.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib reload_` passes (3 tests).
- `build()` body is wrapped in `spawn_blocking`; `commit` returns `()` (infallible); no `.unwrap()` in reload.rs.

- [x] 5.1

### Task 5.2: TemplateReloadRegistry + erased contract in camel-component-api

**Files:**
- `crates/components/camel-component-api/src/template_reload.rs` (new)
- `crates/components/camel-component-api/src/lib.rs` (modified — `pub mod template_reload;`)
- `crates/components/camel-component-api/Cargo.toml` (modified — ensure `tokio` feature `["sync"]`, `async-trait`, `futures` present)

**Steps:**
1. Create `crates/components/camel-component-api/src/template_reload.rs`. Define the erased staged marker with an owned-downcast accessor: `pub trait TemplateReloadStaged: Send { fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>; }`. (`Box<dyn Trait>` has no inherent `downcast`; `into_any` returns `Box<dyn Any>` which DOES — this is the standard object-downcast idiom and resolves the compile error.)
2. Define `#[async_trait] pub trait TemplateReloadTarget: Send + Sync { fn route_id(&self) -> &str; fn reload_timeout(&self) -> std::time::Duration; fn current_generation(&self) -> u64; async fn build(&self) -> Result<(Box<dyn TemplateReloadStaged>, u64), CamelError>; fn commit(&self, staged: Box<dyn TemplateReloadStaged>); }`. `commit` is INFALLIBLE (returns `()`) — it is only called after `reload_route` validates every staged generation, so all-or-nothing holds structurally (Critical 1). There is NO single-producer `reload()` on the trait — the ONLY reload path is `reload_route`.
3. Define `pub struct TemplateReloadRegistry { handlers: std::sync::Mutex<Vec<RegisteredTarget>>, route_locks: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>> }` with `pub fn global() -> &'static TemplateReloadRegistry` (OnceLock, mirror `tls_source.rs:146-186`), `pub fn register(&self, target: Arc<dyn TemplateReloadTarget>) -> RegistrationGuard`, and `pub fn find_all(&self, route_id: &str) -> Vec<Arc<dyn TemplateReloadTarget>>` (`pub` so `camel-template` integration tests can assert registration — Important 2). `RegisteredTarget` carries a unique `u64` id (atomic counter) + `route_id`. All `Mutex::lock()` uses `.expect("...")`, never `.unwrap()`.
4. Define `pub struct RegistrationGuard { id: u64, registry: &'static TemplateReloadRegistry }` impl `Drop`: remove by `id` (NOT by route_id — an old generation cannot remove a newer registration).
5. `pub async fn reload_route(&self, route_id: &str) -> Result<(), CamelError>` (ALL-OR-NOTHING, returns `()`): acquire the per-route `tokio::sync::Mutex` (serialize concurrent `reload_route` for this route — and since there is no other reload path, NO concurrent bump is possible). `find_all(route_id)`; if empty return `Err(CamelError::Config("no template target for route"))`. `timeout = targets.iter().map(|t| t.reload_timeout()).min().unwrap_or(Duration::from_millis(5000));` (the TIGHTEST deadline wins — registration order must not define the route deadline — Important 4). Wrap the rest in `tokio::time::timeout(timeout, async { ... })`:
   - Build phase: `futures::join_all(targets.iter().map(|t| t.build()))` → `Vec<(Box<dyn TemplateReloadStaged>, u64)>`. If ANY build returns `Err`: return `Err` immediately (NOTHING committed — all-or-nothing).
   - Validate phase: for each `(staged, read_gen, target)`, assert `read_gen == target.current_generation()`. If ANY mismatch: return `Err(CamelError::TemplateReload("stale generation"))` (commit none). (Under the per-route mutex with no other reload path this never fires in practice — it is the structural guarantee of the spec "delayed stale build does not swap" scenario.)
   - Commit phase (infallible): only reached if every build AND every validation succeeded. Call `target.commit(staged)` for each. `commit` cannot fail, so all-or-nothing is structurally guaranteed. Return `Ok(())`.
   - On timeout: return `Err(CamelError::TemplateReload("reload timeout"))`. Dropped `build` futures never produce a staged set, so `commit` is never reached for them.

**Tests:**
- `registry_register_find_all_remove`: setup = `TemplateReloadRegistry::global()` + a fake target; action = `register`, `find_all(route)`; assert = len 1; drop guard; `find_all` empty. command = `cargo test -p camel-component-api --lib registry_register_find_all_remove`. expected = pass after step 4.
- `reload_route_all_or_nothing`: setup = two fake targets, second `build()` returns `Err`; action = `reload_route(route)`; assert = `Err`, `commit` called on NEITHER (commit-spy), both retain prior sets. command = `cargo test -p camel-component-api --lib reload_route_all_or_nothing`. expected = pass after step 5.
- `reload_route_commits_all_on_success`: setup = two fakes both build OK; action = `reload_route(route)`; assert = `Ok(())`, both `commit` called. command = `cargo test -p camel-component-api --lib reload_route_commits_all_on_success`. expected = pass.
- `reload_route_timeout_no_commit`: setup = a fake whose `build()` sleeps past `reload_timeout()`; action = `reload_route(route)`; assert = `Err`, `commit` never called. command = `cargo test -p camel-component-api --lib reload_route_timeout_no_commit`. expected = pass.
- `reload_route_rejects_stale_no_commit`: setup = a fake whose `build()` returns staged `read_generation = G`, but `current_generation()` returns `G+1` (simulating a concurrent bump before validation); action = `reload_route(route)`; assert = `Err(CamelError::TemplateReload(_))`, `commit` called on NONE (validate phase rejected before any commit). command = `cargo test -p camel-component-api --lib reload_route_rejects_stale_no_commit`. expected = pass.
- `reload_route_serializes_concurrent`: setup = a fake whose `build()` records entry into a shared counter; action = spawn two concurrent `reload_route(route)` calls; assert = the second does not start building until the first completes (the per-route mutex serializes them). command = `cargo test -p camel-component-api --lib reload_route_serializes_concurrent`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-component-api -- -D warnings` exits 0.
- `cargo test -p camel-component-api --lib` passes (registry tests).
- No reference to `camel_template` anywhere in `camel-component-api` (`rg camel_template crates/components/camel-component-api` returns nothing).

- [x] 5.2

### Task 5.3: ReloadHandler impls target + registration lifecycle

**Files:**
- `crates/components/camel-template/src/reload.rs` (modified)
- `crates/components/camel-template/src/lifecycle.rs` (modified)
- `crates/components/camel-template/src/endpoint.rs` (modified — add the `guard` field to the `StartupBuildHandle` construction in `lifecycle()`)

**Steps:**
1. `impl camel_component_api::template_reload::TemplateReloadStaged for StagedSet { fn into_any(self: Box<Self>) -> Box<dyn std::any::Any> { self } }`.
2. `#[async_trait] impl TemplateReloadTarget for ReloadHandler`: `route_id()` → `&self.route_id`; `reload_timeout()` → `Duration::from_millis(self.limits.reload_timeout_ms)`; `current_generation()` → delegate to Task 5.1; `build()` → delegate to Task 5.1's spawn_blocking build; `commit(staged)` → delegate to Task 5.1's infallible commit.
3. Add the `guard` field to `StartupBuildHandle` (declared WITHOUT it in Task 4.4): change the struct in `lifecycle.rs` to add `guard: std::sync::Mutex<Option<camel_component_api::template_reload::RegistrationGuard>>`, AND update the construction in `endpoint.rs::lifecycle()` (Task 4.3) to include `guard: std::sync::Mutex::new(None)`. Registration lifecycle (Critical 3): in `start()`, AFTER the initial compile+store+ReloadHandler construction, call `TemplateReloadRegistry::global().register(Arc::clone(&handler) as Arc<dyn TemplateReloadTarget>)` and store the returned `RegistrationGuard` into `*self.guard.lock().expect("guard poisoned") = Some(guard)`. In `shutdown(RouteStop|HotSwap)`: `*self.guard.lock().expect("guard poisoned") = None;` (drops the guard → RAII unregister). On route restart: `start()` re-registers and stores a fresh guard; the guard's unique id ensures a stopped-generation guard cannot evict a restarted-generation registration.
4. `use camel_component_api::template_reload::{TemplateReloadRegistry, TemplateReloadTarget, TemplateReloadStaged, RegistrationGuard};` in `lifecycle.rs`.

**Tests:**
- `reload_handler_impls_target`: setup = a `ReloadHandler`; action = call `TemplateReloadTarget::build` then `commit` via the trait; assert = set swapped, generation incremented. command = `cargo test -p camel-template --lib reload_handler_impls_target`. expected = pass after step 2.
- `start_registers_shutdown_unregisters`: setup = a `StartupBuildHandle` whose `start()` registers; action = `start().await` then `shutdown(RouteStop).await`; assert = after start, `TemplateReloadRegistry::global().find_all(route)` len 1; after shutdown, len 0. command = `cargo test -p camel-template --lib start_registers_shutdown_unregisters`. expected = pass after step 3.
- `restart_re_registers_new_guard`: setup = start → shutdown → start again; action = second `start()`; assert = registered again with a NEW guard (unique id), `find_all` len 1; the first (dropped) guard did not evict the new one. command = `cargo test -p camel-template --lib restart_re_registers_new_guard`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --lib` passes (registration lifecycle tests).
- `StartupBuildHandle` declares both `handler` and `guard` fields (set in Task 4.4); `start()` populates them, `shutdown()` clears `guard`.

- [x] 5.3

### Task 5.4: RuntimeCommand::ReloadTemplates + RuntimeBus intercept

**Files:**
- `crates/camel-api/src/runtime.rs` (modified)
- `crates/camel-core/src/lifecycle/application/runtime_bus.rs` (modified)

**Steps:**
1. In `runtime.rs:489` add variant `ReloadTemplates { route_id: String, command_id: String, causation_id: Option<String> }` to `RuntimeCommand`. Extend `command_id()`/`causation_id()` accessors (`:499-525`) with the arm.
2. Add `TemplatesReloaded { route_id: String }` to `RuntimeCommandResult` (mirror `TlsCertsReloaded` at `:541`).
3. In `runtime_bus.rs:173` add an intercept BEFORE `ensure_journal_recovered` (`:196`) and `dedup.first_seen` (`:198`): match `RuntimeCommand::ReloadTemplates { route_id, .. }`, call `camel_component_api::template_reload::TemplateReloadRegistry::global().reload_route(route_id).await` (registry lives in `camel-component-api`, which `camel-core` already depends on — NOT `camel-template`). On `Ok(())`: record `template_reloads_total` ONCE via the runtime metrics (`self.metrics.record_counter("template_reloads_total", 1.0, &[("route", route_id)])` — the route-scoped aggregation increment, NOT per-target, NOT at startup), then `return Ok(RuntimeCommandResult::TemplatesReloaded { route_id })`. On `Err(e)`: `return Err(e)`. Add the safety-net arm in `commands.rs:62`: `ReloadTemplates { .. } => Err(CamelError::Config("ReloadTemplates not handled: should be intercepted in execute()".into()))`.

**Tests:**
- `reload_templates_bypasses_dedup`: setup = a registry target; action = issue `ReloadTemplates` with the same `command_id` 3 times; assert = `reload_route` invoked 3 times (mirror `tls_reload_test.rs:131`). command = `cargo test -p camel-core --test template_reload_test reload_templates_bypasses_dedup`. expected = pass after step 3.
- `reload_templates_does_not_require_journal`: setup = no UoW attached; action = issue `ReloadTemplates`; assert = succeeds (intercept precedes journal recovery). command = `cargo test -p camel-core --test template_reload_test reload_templates_does_not_require_journal`. expected = pass.
- `reload_templates_route_status_unchanged`: setup = a started route; action = issue `ReloadTemplates`; assert = `RouteStatus` unchanged, zero journal writes. command = `cargo test -p camel-core --test template_reload_test reload_templates_route_status_unchanged`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` and `cargo clippy -p camel-core -- -D warnings` exit 0.
- `cargo test -p camel-api --lib` and `cargo test -p camel-core --test template_reload_test` pass.

- [x] 5.4

### Task 5.5: Reload integration tests + CONTEXT-MAP ADR-0047 index

**Files:**
- `crates/components/camel-template/tests/template_reload_integration.rs` (new)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Create `tests/template_reload_integration.rs`: register a `TemplateComponent`, start a route with a real tempdir template file `page.html` (with a top-level `{% autoescape "html" %}` wrapper). Assert: initial render = v1; issue `RuntimeCommand::ReloadTemplates` after mutating the file to v2 → subsequent renders = v2 (atomic swap); issue `ReloadTemplates` after making the file invalid → renders STILL v2 (retention of the last good set, per AC 6); hot-path compile-once proved by deleting the source file after a valid reload and confirming subsequent renders still succeed (compiled set held in memory, no hot-path FS I/O — no private-reader access needed).
2. Multi-producer all-or-nothing: register a route with TWO `To("template:...")` producers sharing a tempdir; mutate both files to valid v2 → both swap; mutate so the second producer's file is invalid → `ReloadTemplates` returns `Err` and BOTH producers retain their prior sets (all-or-nothing).
3. In `CONTEXT-MAP.md` ADR index (`:33-70`, currently stops at 0046), add the entry for ADR-0047 referencing `docs/adr/0047-template-rendering-engine.md` and the `template rendering language` Key Term (`:127`).

**Tests:**
- `reload_valid_swaps_atomic`: step 1 valid-change. command = `cargo test -p camel-template --test template_reload_integration reload_valid_swaps_atomic`. expected = pass.
- `reload_invalid_retains_last_good`: step 1 invalid-after-v2 retains v2. command = `cargo test -p camel-template --test template_reload_integration reload_invalid_retains_last_good`. expected = pass.
- `reload_multi_producer_all_or_nothing`: step 2. command = `cargo test -p camel-template --test template_reload_integration reload_multi_producer_all_or_nothing`. expected = pass.

**Acceptance:**
- `cargo clippy -p camel-template -- -D warnings` exits 0.
- `cargo test -p camel-template --test template_reload_integration` passes.
- `CONTEXT-MAP.md` ADR index includes ADR-0047.
- AC 5, 6, 8, 10 demonstrably met.

- [x] 5.5
