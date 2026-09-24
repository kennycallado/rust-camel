# Tasks: protofix

Single-phase change. Tasks run in order. Task 1 introduces the seam
and error variant; Task 2 exercises `compile_proto` end to end;
Task 3 aligns docs; Task 4 runs gates. Spec delta ships in this change
dir (`specs/data-formats/spec.md`, blessed at spec stage).

## Task 1 — Protoc resolution seam, panic containment, typed error

- **Files**
  - `crates/services/camel-proto-compiler/src/compiler.rs` (modified)
  - `crates/services/camel-proto-compiler/src/lib.rs` (modified)
- **Steps**
  1. In `lib.rs`, add variant `ProtocUnavailable { detail: String }`
     to `ProtoCompileError` with display text
     `protoc unavailable: {detail}. Set PROTOC to a protoc binary, or use a build where the vendored protoc is present`.
     Remove the `VendoredProtoc` variant and its `#[from]` attribute
     (no in-workspace constructor, match, or `matches!` arm exists;
     verified by grep at proposal time and re-verify with
     `rg 'VendoredProtoc' crates/` before removal).
  2. In `compiler.rs`, add `pub(crate) fn resolve_protoc_with(vendored:
     impl FnOnce() -> Result<PathBuf, ProtoCompileError>) ->
     Result<PathBuf, ProtoCompileError>` per design D4: read
     `std::env::var_os("PROTOC")` first and return
     `Ok(PathBuf::from(value))` without calling `vendored`; otherwise
     run `std::panic::catch_unwind(AssertUnwindSafe(vendored))` and map
     `Ok(Ok(path))` to the path, `Ok(Err(e))` to `Err(e)` verbatim
     (pass-through, no wrapping), `Err(payload)` to
     `Err(ProtoCompileError::ProtocUnavailable { detail })` where
     `detail` comes from a `panic_payload_message(&payload) -> String`
     helper (downcast to `String` then `&str`; unknown payloads map to
     `"unknown panic payload"`).
  3. Add `pub(crate) fn resolve_protoc() ->
     Result<PathBuf, ProtoCompileError>` that calls
     `resolve_protoc_with` with the real closure:
     `|| protoc_bin_vendored::protoc_bin_path().map_err(|e|
     ProtoCompileError::ProtocUnavailable { detail: e.to_string() })`.
  4. In `compile_proto`, replace the eager
     `unwrap_or(protoc_bin_vendored::protoc_bin_path()?)` expression
     with a call to `resolve_protoc()`. No other behavior change.
  5. In the `lib.rs` test module, add the guard
     `let _guard = PROTOC_COMPILE_LOCK.lock().unwrap();` as the first
     statement of `test_concurrent_compiles_do_not_clobber` (the four
     spawned threads still run concurrently; the guard serializes the
     test against `PROTOC`-mutating tests per design Constraints).
  6. Add a `struct ProtocEnvGuard(Option<std::ffi::OsString>)` to the
     test module: constructor captures
     `std::env::var_os("PROTOC")` under `PROTOC_COMPILE_LOCK`; `Drop`
     restores the captured value (set or remove) under the same lock,
     with `SAFETY:` comments on both unsafe env calls stating the lock
     serializes all `PROTOC` access. The constructor and `Drop` run with
     the caller-held lock and MUST NOT acquire it (same-thread
     re-lock deadlocks); declare the guard after the lock binding so
     `Drop` restores before the lock releases. This makes restoration
     panic-safe (an assert failing mid-test still restores) and is
     reused by every test that mutates `PROTOC`.
  7. Add the four unit tests under Tests to the `lib.rs` test module.
     Every test that mutates `PROTOC` takes `PROTOC_COMPILE_LOCK`,
     constructs `ProtocEnvGuard`, then sets or removes `PROTOC` via
     unsafe `std::env::set_var` / `std::env::remove_var` with the same
     SAFETY discipline; restoration happens in the guard's `Drop`.
- **Tests** (in `crates/services/camel-proto-compiler/src/lib.rs`)
  - name: `protoc_env_short_circuits_vendored_resolver`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured; set
    `PROTOC` to `/protofix/fake/protoc`; injected resolver closure is
    `|| panic!("vendored resolver must not run")`
    action: call `resolve_protoc_with(injected_resolver)`
    assert: result is `Ok(PathBuf::from("/protofix/fake/protoc"))`;
    closure not invoked (its panic would surface as
    `Err(ProtocUnavailable)` and fail the assert); restore `PROTOC`
    command: `cargo test -p camel-proto-compiler protoc_env_short_circuits_vendored_resolver`
    expected: fails before the seam exists (function not found), passes
    after
  - name: `vendored_panic_contained_as_protoc_unavailable`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured;
    `PROTOC` removed; injected resolver closure is
    `|| panic!("internal: protoc not found /baked/registry/path/bin/protoc")`
    action: call `resolve_protoc_with(injected_resolver)`
    assert: result is
    `Err(ProtoCompileError::ProtocUnavailable { detail })` with
    `detail` containing `internal: protoc not found`, and
    `err.to_string()` containing `Set PROTOC`; the test completing is
    proof the panic was contained; restore `PROTOC`
    command: `cargo test -p camel-proto-compiler vendored_panic_contained_as_protoc_unavailable`
    expected: fails before the seam exists (function not found,
    compile error); passes after. Note for green runs: the default
    panic hook prints one `thread panicked at` line to stderr when
    the injected resolver panics; that line is expected noise from the
    containment test, not a bug.
  - name: `vendored_err_passes_through_seam_verbatim`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured;
    `PROTOC` removed; injected resolver returns
    `Err(ProtoCompileError::ProtocUnavailable { detail:
    "facade unsupported-platform".into() })`
    action: call `resolve_protoc_with(injected_resolver)`
    assert: result is the same `Err(ProtocUnavailable)` with `detail ==
    "facade unsupported-platform"` (single-pass mapping: the seam does
    not wrap or rewrite the closure's error); restore `PROTOC`
    command: `cargo test -p camel-proto-compiler vendored_err_passes_through_seam_verbatim`
    expected: fails before the seam exists, passes after
  - name: `resolve_protoc_vendored_present_returns_file`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured;
    `PROTOC` removed (CI and dev shape: vendored binary present)
    action: call `resolve_protoc()` (real vendored closure)
    assert: result is `Ok(path)` and `path.is_file()`; restore `PROTOC`
    command: `cargo test -p camel-proto-compiler resolve_protoc_vendored_present_returns_file`
    expected: fails before the seam exists, passes after
- **Acceptance**
  - `cargo test -p camel-proto-compiler` passes (all new and
    pre-existing tests).
  - `rg 'VendoredProtoc' crates/` returns zero hits.
  - `cargo fmt --check --all` and
    `cargo clippy -p camel-proto-compiler -- -D warnings` exit 0.
- [x] task-1

## Task 2 — compile_proto end-to-end override tests

- **Files**
  - `crates/services/camel-proto-compiler/tests/helloworld.desc` (new,
    binary fixture: descriptor set for `tests/helloworld.proto`)
  - `crates/services/camel-proto-compiler/src/lib.rs` (modified, test
    module additions only)
- **Steps**
  1. Generate the fixture deterministically with a throwaway
     generator: add a temporary `#[test] #[ignore] fn
     generate_helloworld_desc_fixture()` that resolves
     `protoc_bin_vendored::protoc_bin_path()`, spawns that binary with
     `--descriptor_set_out=<crate>/tests/helloworld.desc
     --include_imports -I <crate>/tests
     <crate>/tests/helloworld.proto`, and asserts
     the fixture is non-empty. Run it once with
     `cargo test -p camel-proto-compiler -- --ignored
     generate_helloworld_desc_fixture`, then delete the generator
     test. The fixture stays committed.
  2. In the `lib.rs` test module, add helper `fn write_fake_protoc(
     dir: &std::path::Path) -> std::path::PathBuf` that writes an
     executable shell script `fake-protoc` into `dir`: the script
     appends `invoked` to a `marker` file next to itself (path derived
     from `$(dirname "$0")`), then for each argument matching
     `--descriptor_set_out=<path>` copies the fixture to `<path>`.
     The fixture path is baked into the script as an absolute literal
     at write time from `env!("CARGO_MANIFEST_DIR")` (the fixture
     lives in the crate's `tests/` dir, not in the temp dir). The
     script exits 0. Make it executable with
     `std::os::unix::fs::PermissionsExt::from_mode(0o755)`.
  3. Add the two tests under Tests. Both take
     `PROTOC_COMPILE_LOCK` and use Task 1's `ProtocEnvGuard` for
     capture/restore.
- **Tests** (in `crates/services/camel-proto-compiler/src/lib.rs`)
  - name: `protoc_env_marker_script_serves_compilation`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured; temp
    dir with `write_fake_protoc`; `marker` absent; `PROTOC` set to the
    script path
    action: call `compile_proto` on `tests/helloworld.proto`
    assert: result is `Ok(pool)` with
    `pool.get_message_by_name("helloworld.HelloRequest").is_some()`;
    the `marker` file exists (the script, not the vendored binary,
    served the compilation); restore `PROTOC`
    command: `cargo test -p camel-proto-compiler protoc_env_marker_script_serves_compilation`
    expected: on vendored-present machines this passes before and after
    (the eager pre-fix lookup runs but its value is discarded; `PROTOC`
    still wins). It is red on machines where the vendored binary is
    absent: the pre-fix eager lookup panics and kills the process
    before the script runs. The test pins the override contract end to
    end.
  - name: `protoc_env_broken_override_fails_without_fallback`
    setup: under `PROTOC_COMPILE_LOCK`; prior `PROTOC` captured;
    `PROTOC` set to `/protofix/definitely/missing/protoc`
    action: call `compile_proto` on `tests/helloworld.proto`
    assert: result is `Err(ProtoCompileError::Io(_))` (spawn of the
    missing binary fails; the vendored fallback is not attempted, per
    the D1 short-circuit proven in Task 1); restore `PROTOC`
    command: `cargo test -p camel-proto-compiler protoc_env_broken_override_fails_without_fallback`
    expected: on vendored-present machines this passes before and after
    (`Some(PROTOC)` wins the `unwrap_or`, spawn fails, `Io`), so it
    pins the contract rather than going red first; on vendored-absent
    machines the pre-fix eager lookup panics first. The no-fallback
    guarantee itself is proven red-first by task-1's
    `protoc_env_short_circuits_vendored_resolver`.
- **Acceptance**
  - `cargo test -p camel-proto-compiler` passes including both new
    tests.
  - `tests/helloworld.desc` committed and non-empty.
  - `cargo fmt --check --all` and
    `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings`
    exit 0.
- [x] task-2

## Task 3 — Docs truth on protoc resolution

- **Files**
  - `docs/src/data-formats/protobuf.md` (modified)
  - `crates/services/camel-proto-compiler/README.md` (modified)
  - `crates/services/camel-proto-compiler/src/lib.rs` (modified, crate
    doc comment and `ProtoCache` doc comment only)
- **Steps**
  1. In `docs/src/data-formats/protobuf.md`, qualify the intro claim:
     keep "no compile-time code generation" only as the statement that
     schemas compile at runtime, and add that a `protoc` binary must
     exist at runtime.
  2. Add a `## Protoc resolution` section after `## Construction`
     stating the order: `PROTOC` environment override (honored
     verbatim, never re-resolved, a broken value surfaces the ordinary
     execution error), vendored fallback (present in standard builds),
     typed load failure whose display starts with `protoc unavailable:`
     and ends with the `Set PROTOC` remedy when
     neither exists. Include the `PROTOC=/path/to/protoc` export as
     the operator remedy example.
  3. In `README.md`, update the Features list and the Known limitation
     section to state the same order and the typed failure.
  4. In `lib.rs`, update the crate-level doc comment (and the
     `ProtoCache` doc comment sentence that references
     `protoc_bin_vendored::protoc_bin_path()`) to state the resolution
     order: `PROTOC` override first, vendored fallback second, typed
     `ProtocUnavailable` failure third.
- **Tests**
  - name: `docs-consistency`
    setup: the three files from Files
    action: read the resolution-order statements in each surface
    assert: all three name the same order (`PROTOC` override, vendored
    fallback, typed failure) and none claims the format works without
    a runtime protoc; no stale mention of `VendoredProtoc` or
    "eager" resolution remains
    command: `rg -i 'vendored|PROTOC' docs/src/data-formats/protobuf.md crates/services/camel-proto-compiler/README.md crates/services/camel-proto-compiler/src/lib.rs`
    expected: every hit is consistent with the order above
- **Acceptance**
  - The `rg` command above shows no contradicting or stale statement.
  - `cargo fmt --check --all` exits 0 (doc comments are in Rust
    source).
- [x] task-3

## Task 4 — Gates and change validation

- **Files**
  - none (verification only; commit any resulting fixes inside the
    zone files above)
- **Steps**
  1. `cargo fmt --check --all` (worktree).
  2. `cargo clippy -p camel-proto-compiler --all-targets -- -D warnings`.
  3. API changed (variant added and removed), so clippy the dependents
     with their gating features enabled so the consumer code actually
     compiles: `cargo clippy -p camel-dsl --features protobuf -- -D
     warnings`, `cargo clippy -p camel-dataformat-protobuf -- -D
     warnings`, `cargo clippy -p camel-component-grpc -- -D warnings`,
     and `cargo clippy -p camel-cli --features grpc -- -D warnings`.
  4. `cargo test -p camel-proto-compiler`.
  5. `openspec validate protofix --type change --json` (worktree).
  6. Record each gate result in the commit message body of the final
     commit (one line per gate: pass or N/A with reason).
- **Tests**
  - name: `all-gates-green`
    setup: Tasks 1-3 complete and committed
    action: run the gate list above
    assert: every command exits 0
    command: as listed
    expected: passes
- **Acceptance**
  - All listed gates exit 0.
  - `openspec validate protofix --type change` reports valid with zero
    issues.
- [x] task-4
