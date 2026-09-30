# Tasks: shuttokens

## camel-component-api

### Task 1.1: Document the registration-time token-lineage boundary on the trait

**Files:**
- `crates/components/camel-component-api/src/component_context.rs` (modified)
- `crates/components/camel-component-api/CONTEXT.md` (modified)

**Steps:**
1. Extend the doc comment on `ComponentContext::shutdown_token` (crates/components/camel-component-api/src/component_context.rs, the `fn shutdown_token` default-method block around line 49-58) with a `# Binding-time boundary` paragraph stating: token lineage binds at component REGISTRATION time — production registration sites construct slot-bound contexts (`RegistryComponentContext::with_shutdown_slot`, re-written by every `CamelContext::start`) and hand them to long-lived components (e.g. `WasmComponent`'s captured `Arc<dyn ComponentContext>`); endpoint-creation-time adapter contexts (`ControllerComponentContext`, `MasterDelegateContext`) must NOT snapshot tokens — they keep the `None` default so no stale-across-stop/start lineage can leak, and callers mint a local root. Keep the existing prose; append, do not rewrite.
2. In `crates/components/camel-component-api/CONTEXT.md`, append one sentence to the existing shutdown_token paragraph (around lines 57-59): "Token lineage binds when the component is registered; endpoint-creation adapter contexts (controller, master delegate) resolve `None` by design."

**Tests:**
- `shutdown_token_default_is_none` (existing, component_context.rs test mod): unchanged and still passing — proves the default was not altered by the doc edit.
- `cargo doc -p camel-component-api --no-deps` with `RUSTDOCFLAGS="-D warnings"`: doc edit introduces no broken intra-doc links (reference private items in plain backticks only).

**Acceptance:**
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-api --no-deps` exits 0.
- `cargo test -p camel-component-api --lib` green (under the mission systemd-run scope).
- No code change in the crate — doc comments and CONTEXT.md only.

- [x] 1.1

## camel-core

### Task 1.2: Boundary comment + boundary-lock test on ControllerComponentContext; poison-comment wording fix

**Files:**
- `crates/camel-core/src/lifecycle/adapters/controller_component_context.rs` (modified)
- `crates/camel-core/src/lifecycle/adapters/controller_component_context_tests.rs` (modified)
- `crates/camel-core/src/lifecycle/application/context_lifecycle.rs` (modified)

**Steps:**
1. Add a doc comment on the `impl ComponentContext for ControllerComponentContext` block (or directly above the impl, controller_component_context.rs) stating the boundary decision: `shutdown_token` intentionally keeps the `None` default — no per-call token consumer is reachable through this context (the only production trait-method callers are wasm `producer.rs:150` and `bean.rs:58`, which resolve from the registration-time slot-bound `RegistryComponentContext`, camel-bundles lib.rs:421-429 / camel-cli run.rs:385-393); snapshotting here would be dead code plus a stale-across-stop/start hazard. Reference bd rc-4bfnk.
2. In `controller_component_context_tests.rs`, add test `shutdown_token_is_none_boundary_lock`: build a context via the existing `build_ctx(false)` helper, call `ComponentContext::shutdown_token(&*ctx)` (explicit trait call), assert `.is_none()` with a message stating this is a boundary lock — if a slot-backed accessor is ever added to the trait, update this test deliberately, do not delete it.
3. Fix the poison-comment wording in `context_lifecycle.rs` around lines 101-110: replace "Poison-tolerant by design — a poisoned slot must not fail the boot; leaving a stale token is fail-safe (resolvers keep a lineage that this boot never cancels)" with wording that describes the actual degraded path: a poisoned slot must not fail the boot; readers fail CLOSED to `None` (`lock().ok()?` in `RegistryComponentContext::shutdown_token`), so resolvers mint an uncancelled local root — Runtime-shutdown observation is lost for that boot (same observable as unbound contexts), never born-cancelled; poison requires a panic while holding a one-line read/write lock (near-zero probability).

**Tests:**
- `shutdown_token_is_none_boundary_lock`: `build_ctx(false)` → `ComponentContext::shutdown_token(&*ctx)` → assert `is_none()`; command `cargo test -p camel-core --lib shutdown_token_is_none_boundary_lock`; expected: BEFORE the fn is added the filter matches 0 tests (cargo succeeds with "0 passed" — do not mistake that for a pass); AFTER: exactly 1 match, passing.
- Existing poison-related/context lifecycle tests unchanged and green: `cargo test -p camel-core --lib context_lifecycle` (module path may differ — run `cargo test -p camel-core --lib lifecycle::application` if the first filter matches nothing).

**Acceptance:**
- `cargo fmt --check --all` exits 0 (or `cargo fmt -p camel-core` applied).
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib shutdown_token_is_none_boundary_lock` green (under the mission systemd-run scope).
- No behavior change: comments + one `#[cfg(test)]` fn only.

- [x] 1.2

## camel-master

### Task 1.3: Boundary comment, stale-comment rewrite + boundary-lock test on MasterDelegateContext

**Files:**
- `crates/components/camel-master/src/endpoint.rs` (modified)
- `crates/components/camel-master/src/tests/producer_passthrough.rs` (modified)

**Steps:**
1. Rewrite the stale field-visibility comment on `MasterDelegateContext` (endpoint.rs, the `pub(crate)` fields comment around lines 63-66): the current text says the fields CAN be narrowed to private after a leadership.rs extraction — wrong on two counts: the extraction already happened (reconcile builds this struct in `leadership.rs` around line 283), and that construction uses field-init syntax, so the fields must STAY `pub(crate)`. New comment states exactly that.
2. Add a doc comment on `impl ComponentContext for MasterDelegateContext` (endpoint.rs) stating the boundary decision with evidence: `shutdown_token` keeps the `None` default by design — wasm delegates created through this context resolve tokens from the registration-time slot-bound context captured by `WasmComponent` (lib.rs:70-84, endpoint construction passes `self.registry.clone()`), never from the `ctx` passed here; the only trait-method callers are wasm producer.rs:150 / bean.rs:58. Reference bd rc-4bfnk.
3. In `tests/producer_passthrough.rs`, add test `master_delegate_shutdown_token_is_none_boundary_lock`: construct a `MasterDelegateContext` directly (delegate_component: `Arc<dyn Component>` from the file's existing fake/no-op component, metrics: `Arc::new(camel_api::NoOpMetrics)`, platform_service: `Arc::new(camel_api::NoopPlatformService::default())` — match whatever the existing tests in this file already use), call `ComponentContext::shutdown_token(&ctx)`, assert `.is_none()` with a boundary-lock message.

**Tests:**
- `master_delegate_shutdown_token_is_none_boundary_lock`: construct MasterDelegateContext → explicit trait call → assert `is_none()`; command `cargo test -p camel-master --lib master_delegate_shutdown_token_is_none_boundary_lock`; expected: BEFORE the fn is added the filter matches 0 tests (cargo succeeds with "0 passed" — do not mistake that for a pass); AFTER: exactly 1 match, passing.
- Existing producer-passthrough tests green: `cargo test -p camel-master --lib`.

**Acceptance:**
- `cargo fmt --check --all` exits 0 (or `cargo fmt -p camel-master` applied).
- `cargo clippy -p camel-master -- -D warnings` exits 0.
- `cargo test -p camel-master --lib` green (under the mission systemd-run scope).

- [x] 1.3

## xtask + bd

### Task 1.4: Ratchet 9→7, lint verification, bd documentation

**Files:**
- `scripts/xtask/ratchet-cancel-tokens.max` (modified)

**Steps:**
1. Run `CARGO_BUILD_JOBS=6 cargo xtask lint-cancel-tokens` in the worktree; confirm the reported production site count is exactly 7 and OK against max 9.
2. Edit `scripts/xtask/ratchet-cancel-tokens.max`: change the ceiling integer 9 → 7; delete the two stale seed-inventory lines for `camel-component-wasm/src/bean.rs` and `camel-component-wasm/src/producer.rs` (both now use `unwrap_or_default()`; their remaining `CancellationToken::new()` sites are under `#[cfg(test)]`, bean.rs 362+, producer.rs 515+). Keep all other inventory lines and header comments.
3. Re-run `CARGO_BUILD_JOBS=6 cargo xtask lint-cancel-tokens`; confirm OK (7 sites == max 7).
4. From the REPO ROOT (`/home/kenny/dev/rust-camel`) — never from the worktree — file the P4 follow-up: `bd create "lint-cancel-tokens blind spot: Option<CancellationToken>::unwrap_or_default mints an uncounted root" -t task -p 4 --deps discovered-from:rc-4bfnk --json` with a description noting wasm producer.rs:150/bean.rs:58 fall back to `unwrap_or_default()` when the context is unbound, which the lint cannot see.
5. From the REPO ROOT, add the audit-outcome comment on the bd: `bd comment rc-4bfnk "<text>"` recording: both adapters audited, None is CORRECT (evidence chain: registration-time slot-bound lineage; only trait callers are wasm producer.rs:150/bean.rs:58; adapters never reach a token consumer), boundary documented in trait doc + both adapters + camel-component-api/CONTEXT.md, locked by two boundary-lock tests, poison-comment corrected (readers fail closed to None → uncancelled local root, not born-cancelled — supersedes e_glm note 1's mechanism), ratchet 9→7 with stale inventory lines removed (note 2 resolved).

**Tests:**
- Ratchet verification (no unit test — machine-checked by the lint itself): `CARGO_BUILD_JOBS=6 cargo xtask lint-cancel-tokens` → expected `OK (7 sites == max 7)` before edit: `OK (7 sites < max 9)`.
- `bd show rc-4bfnk --json` from root shows the audit-outcome comment and the new P4 issue exists in `bd show <new-id> --json`.

**Acceptance:**
- `cargo xtask lint-cancel-tokens` green with ceiling 7.
- Ratchet file diff contains only: 9→7 + two deleted inventory lines.
- bd rc-4bfnk carries the audit-outcome comment; P4 follow-up bd exists with discovered-from link.

- [x] 1.4
