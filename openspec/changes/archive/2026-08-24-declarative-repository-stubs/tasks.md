# Tasks: declarative-repository-stubs

## camel-cli

### Task 1.1: Repository stub document schema and eager validation

**Files:**
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests.rs` (modified)
- `crates/camel-cli/src/commands/test/document_tests/repositories.rs` (new)

**Steps:**
1. In `document.rs`, add `pub struct RepositoriesDoc` annotated `#[serde(deny_unknown_fields, rename_all = "camelCase")]` with three fields: `cache: Option<BTreeMap<String, String>>`, `idempotent: Option<BTreeMap<String, String>>`, `claim_check: Option<BTreeMap<String, String>>` (serde name `claimCheck` via the rename attribute).
2. Add field `pub repositories: Option<RepositoriesDoc>` to `TestDocument` (its existing `deny_unknown_fields, rename_all = "camelCase"` attributes pick the block up automatically).
3. Add `TestDocError::InvalidRepositories(String)` variant (mirrors `InvalidBeans(String)`; message carries the precise reason verbatim).
4. Add a validation function `validate_repositories(doc: &TestDocument) -> Result<(), TestDocError>` called from `parse_test_document` as step (i) in the documented validation order (extend the doc-comment list). For each declared registry map and each (`name`, `target`) pair: reject `target != "memory"` with a message naming the registry, the name, the unsupported target, and the only supported target `memory`; reject a `name` that is blank after trimming; reject `name == "memory"` with a message stating `memory` is a built-in repository name and cannot be stubbed.
5. Add accessor `pub fn repository_stubs(&self) -> Option<&RepositoriesDoc>` on `TestDocument` (mirrors `bean_decls()`; infallible because validation ran eagerly).
6. In `document_tests.rs`, add `mod repositories;`.

**Tests:** (new module `document_tests/repositories.rs`, same style as `beans.rs` — inline YAML strings through `parse_test_document` / `err_of`)
- `repos_absent_keeps_behavior`: minimal valid doc without `repositories:` parses; `doc.repository_stubs()` is `None`.
- `repos_cache_parses`: doc with `repositories: { cache: { persistent: memory } }` parses; `repository_stubs()` is `Some` and the `cache` map contains `persistent -> memory`.
- `repos_unknown_registry_kind_rejected`: doc with `repositories: { blob: { x: memory } }` fails; error message names the unknown field and lists `cache`, `idempotent`, `claimCheck`.
- `repos_unknown_target_rejected`: doc with `repositories: { cache: { persistent: rocksdb } }` fails with `InvalidRepositories`; message names `rocksdb` and the supported target `memory`.
- `repos_blank_name_rejected`: doc with `repositories: { cache: { "  ": memory } }` fails with `InvalidRepositories`; message states names must be non-blank.
- `repos_builtin_memory_name_rejected`: doc with `repositories: { cache: { memory: memory } }` fails with `InvalidRepositories`; message states `memory` is built-in.
- Command: `cargo test -p camel-cli --lib document_tests::repositories`. Expected: all fail before step 1-5 land (module missing / field unknown), pass after.

**Acceptance:**
- `cargo test -p camel-cli --lib document_tests::repositories` passes.
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.1

### Task 1.2: Runner registration and cache runtime proof

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/tests/test_repository_stubs.rs` (new)

**Steps:**
1. Extend `boot_context` signature to take `repo_stubs: Option<&RepositoriesDoc>` (import from `super::document`). After `builder().build()` and after the five component registrations, for each registry map in `repo_stubs`: call `ctx.register_cache_repository(name.clone(), Arc::new(MemoryCacheRepository::new(name.clone(), 10_000)))`, `ctx.register_idempotent_repository(name.clone(), Arc::new(MemoryIdempotentRepository::new(name.clone())))`, or `ctx.register_claim_check_repository(name.clone(), Arc::new(MemoryClaimCheckRepository::new(name.clone())))` — all three APIs take `(name, repo)` and return `Result<(), RegistryError>` (crates/camel-core/src/context.rs:854-903). Collision is impossible (parse validation rejected blank and `memory` names), so handle the `Result` with `.expect("repository stub registration must succeed"); // allow-unwrap`, mirroring `context_builder.rs:248-249`. Imports via the shallow re-exports matching existing usage in `crates/camel-config/src/context_ext.rs`: `camel_core::cache::MemoryCacheRepository`, `camel_core::idempotent::MemoryIdempotentRepository`, `camel_core::claim_check::MemoryClaimCheckRepository` — verify the exact re-export paths against `camel-core`'s lib re-exports. Registration happens inside `boot_context`, which `run_test_doc` calls BEFORE `run_phases` adds and compiles routes, so names resolve at compile time.
2. In `run_test_doc`, pass `doc.repository_stubs()` to `boot_context` alongside the existing `intercepts` and `beans` arguments. Error paths (`TestDocResult` with `doc_error`) stay unchanged.
3. In the new integration test file, verify the exact YAML keys for the cache step against `crates/camel-dsl/src/route_ast.rs` (search `cache`) and existing cache parse tests before authoring fixtures — do not invent keys.
4. Author the runtime tests (below) fixtures-first: write the test file BEFORE landing steps 1-2, run it, and capture the pre-implementation failure (unknown-repository compile error — the same gate as `cache_clear_unknown_repository_fails_compile` in `crates/camel-core/src/lifecycle/adapters/step_compilers/core.rs`), then land steps 1-2 and re-run to green. Do not use git stash to toggle implementation state.

**Tests:** (new `tests/test_repository_stubs.rs`, `#[tokio::test]`, driving `run_test_doc` with inline-route documents — mirror fixture style from `tests/test_beans.rs`)
- `cache_stub_miss_then_hit`: route `from direct:in` with a `cache` step (`repository: persistent`, fixed key) whose miss branch sends to `mock:miss` and then continues to `mock:out`; document declares `repositories: { cache: { persistent: memory } }` and two inputs with equal bodies; expectations `mock:miss` count 1 and `mock:out` count 2. Assert both expectations pass (first input takes the miss branch, second is a cache hit that skips it).
- `undeclared_repository_name_fails_route_load`: same document but the route references repository `persistant` (typo) while the stub declares `persistent`; assert `TestDocResult.doc_error` is `Some` and its message names the unknown repository — identical failure shape as a run without the `repositories:` block.
- Command: `cargo test -p camel-cli --test test_repository_stubs`. Expected pre-implementation (fixtures written before steps 1-2): `cache_stub_miss_then_hit` fails with the unknown-repository compile error (same gate as `cache_clear_unknown_repository_fails_compile`); `undeclared_repository_name_fails_route_load` already passes (it asserts the failure shape, which exists pre-change and must remain identical post-change). Post-implementation: both pass.

**Acceptance:**
- `cargo test -p camel-cli --test test_repository_stubs` passes.
- `git diff --stat` shows NO change under `crates/camel-core/` (registration APIs consumed as-is).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.2

### Task 1.3: Idempotent and claimCheck runtime proofs

**Files:**
- `crates/camel-cli/tests/test_repository_stubs.rs` (modified)

**Steps:**
1. Verify the exact YAML keys for the idempotent-consumer step against `crates/camel-dsl/src/route_ast.rs` (search `idempotent`) and `crates/camel-processor/src/idempotent_consumer.rs` (key derivation: message id source) before authoring fixtures.
2. Verify the exact YAML keys for claim-check register/retrieve against `crates/camel-dsl/src/route_ast.rs` (search `claim_check` / `claimCheck`) and `crates/camel-processor/src/claim_check.rs`.
3. Author the two tests below; confirm the runner needs no further change (Task 1.2 registration already covers all three registries). If a registry map is missing from `boot_context` wiring, extend step 1.2's loop rather than adding a parallel mechanism.

**Tests:** (append to `tests/test_repository_stubs.rs`)
- `idempotent_stub_filters_duplicates`: route `from direct:in` with an idempotent-consumer step referencing repository `redis`, then `to mock:out`; document declares `repositories: { idempotent: { redis: memory } }` and delivers two inputs with the SAME message id (per the key derivation verified in step 1); expectation `mock:out` count exactly 1. Assert it passes (the duplicate is filtered).
- `claimcheck_stub_roundtrip`: route `from direct:in` that claim-check registers the body to repository `redb`, then retrieves it, then `to mock:out`; document declares `repositories: { claimCheck: { redb: memory } }`; expectation asserts the output body equals the input body. Assert it passes.
- Command: `cargo test -p camel-cli --test test_repository_stubs`. Expected: both fail with route-load errors when the stub block is removed (manual check), pass with it.

**Acceptance:**
- `cargo test -p camel-cli --test test_repository_stubs` passes (all four tests).
- `cargo fmt --check --all` and `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.3

### Task 1.4: R-REPOSITORY-STUB per-run warning

**Files:**
- `crates/camel-cli/src/commands/test.rs` (modified)
- `crates/camel-cli/tests/test_repository_stubs.rs` (modified)

**Steps:**
1. In `run_tests` (`commands/test.rs`), after a document parses successfully and before `run_test_doc` is invoked for it, if `doc.repository_stubs()` declares AT LEAST ONE (registry, name) pair (a `repositories:` block whose maps are all empty or absent emits nothing — the spec words stubs as declared names), write ONE warning line to the `err` writer: `R-REPOSITORY-STUB: cache=persistent idempotent=redis claimCheck=redb stubbed as memory; backend semantics not exercised (cache: prefix purge, TTL/stale timing, disk offload, stats; idempotent/claim-check: persistence; all: backend failure) — cover them in the integration tier`, listing only the registries that actually carry stubs, each as `registry=name` pairs joined by spaces (multiple names in one registry appear as repeated `registry=name` pairs). Documents without stub pairs emit nothing; parse-error documents are unaffected.
2. Author the warning tests (below).

**Tests:** (append to `tests/test_repository_stubs.rs`, calling `run_tests(files, out, err)` directly with a temp-dir `*.test.yaml` — mirror the existing `run_tests` harness usage in `tests/test_runner.rs`)
- `stub_warning_emitted_per_run`: document declaring `repositories: { cache: { persistent: memory } }` runs green; the `err` buffer contains `R-REPOSITORY-STUB`, the string `cache`, and the name `persistent`.
- `no_stub_no_warning`: same route document WITHOUT the `repositories:` block (repository renamed to the built-in `memory` in the route); the `err` buffer does NOT contain `R-REPOSITORY-STUB`.
- Command: `cargo test -p camel-cli --test test_repository_stubs`. Expected before step 1: `stub_warning_emitted_per_run` fails (no warning in `err` buffer); `no_stub_no_warning` already passes (absence assertions hold pre-change). After step 1: both pass.

**Acceptance:**
- `cargo test -p camel-cli --test test_repository_stubs` passes (all six tests).
- Full local gate set green: `cargo build --workspace`; `cargo test --workspace --lib`; `cargo test -p camel-core --test hexagonal_architecture_boundaries_test`; `cargo fmt --check --all`; the three clippy commands — `cargo clippy --workspace --all-features --exclude camel-cli --exclude camel-component-kafka --exclude security-keycloak --exclude security-wasm-policy -- -D warnings`, `cargo clippy -p camel-component-kafka --all-targets -- -D warnings`, `cargo clippy -p camel-cli -- -D warnings`; and the `cargo xtask` gates `lint-unwrap`, `lint-secrets`, `lint-non-exhaustive`, `lint-log-levels`, `lint-ignore`, `lint-publish-cycles`, `lint-component-deps`, `lint-context-citations`. `schema-check` and `cargo-audit` are out of this task's acceptance: camel-cli adds no schemars/`JsonSchema` types and no dependency changes. `lint-commits` is a CI gate (remote fetch), not run here.
- Spec scenario "camel run ignores the repositories block" holds structurally: no diff outside `camel-cli` touches route loading, and `cargo test -p camel-cli` (existing suites incl. `test_runner.rs`) stays green.

- [x] 1.4
