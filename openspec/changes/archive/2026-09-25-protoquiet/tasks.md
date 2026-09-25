# Tasks: protoquiet

## 1. Scoped panic-hook silencer around the vendored catch_unwind window

**Files**

- `crates/services/camel-proto-compiler/src/compiler.rs`
- `crates/services/camel-proto-compiler/src/lib.rs` (tests module only)

**Steps**

1. In `compiler.rs`, add a private `SilencePanicHook` guard:
   - `new()` calls `std::panic::take_hook()`, stores the previous hook
     (`Option<Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Sync + Send>>`),
     then installs a no-op hook via `std::panic::set_hook`.
   - `Drop` takes the stored hook out of the `Option` and reinstalls it
     with `set_hook`. Never leaks the silent hook globally.
2. Construct the guard immediately BEFORE the `catch_unwind` call in
   `resolve_protoc_with`, bound to a local so it drops immediately AFTER
   the `match` completes (both `Ok` and panic paths restore the hook).
   Guard must cover ONLY the vendored window — the `PROTOC` env-var early
   return stays outside.
3. Update the `resolve_protoc_with` doc comment: the window is hook-silent
   so the contained panic cannot double-report as `thread panicked` stderr
   noise before the typed error surfaces.
4. Add the test below to the tests module in `lib.rs`, following the
   existing `PROTOC_COMPILE_LOCK` + `ProtocEnvGuard` discipline (lock
   first, guard second, `SAFETY:` comments on unsafe env calls).

**Tests (exact)**

- `vendored_panic_does_not_invoke_panic_hook`
  - Arrange: hold `PROTOC_COMPILE_LOCK`; `ProtocEnvGuard`; `remove_var("PROTOC")`.
    Install a recording hook (`static HOOK_FIRED: AtomicBool`) over the
    current hook, saving the previous hook for restore.
  - Act: `resolve_protoc_with(|| panic!("internal: protoc not found /baked/registry/bin/protoc"))`.
  - Assert: result is `Err(ProtocUnavailable)` whose `detail` contains
    `internal: protoc not found`; `HOOK_FIRED` is `false` (production
    silencer suppressed the hook); restore the previous hook.
- Existing `vendored_panic_contained_as_protoc_unavailable` must still
  pass unchanged (containment + payload extraction intact).

**Acceptance**

- `cargo test -p camel-proto-compiler` green, including the new test.
- `cargo fmt --check --all` clean.
- `cargo clippy --workspace --all-features --exclude camel-cli
  --exclude camel-component-kafka --exclude security-keycloak
  --exclude security-wasm-policy -- -D warnings` clean.
- No new dependencies; no public API change; no global hook leak
  (guard restores on every path).

- [x] 1.1 implement silencer guard + test
