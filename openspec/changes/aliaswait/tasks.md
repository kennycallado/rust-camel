# Tasks: aliaswait

## scripts/xtask

### Task 1.1: Candidate-set resolver core — rewrite Imports collection, expansion, and class-aware path matching

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)

**Steps:**
1. Replace `type Imports = HashMap<String, String>` with
   `struct Imports { named: HashMap<String, Vec<String>>, items_value: HashSet<String>, items_type: HashSet<String>, globs: Vec<String> }`.
   `named` is a multimap (same-scope same-name imports push; never overwrite).
2. Rewrite `collect_use_tree`: `Name`/`Rename` arms push the accumulated
   target into `named`'s Vec; the `Glob` arm records the accumulated
   prefix in `globs`. `collect_imports` threads `ItemUse::leading_colon`
   so absolute roots are stored with a leading `"::"` marker.
3. Extend `collect_imports` to record per-module item names into
   `items_value` / `items_type`: `Item::Fn`, `Item::Const`, `Item::Static`
   → `items_value`; `Item::Struct` → `items_type` always, plus
   `items_value` only when fields are `syn::Fields::Unnamed` or `Unit`;
   `Item::Enum`, `Item::Union`, `Item::Trait`, `Item::Type`, `Item::Mod`
   → `items_type`; `Item::ExternCrate` with a rename → `named` entry
   mapping the rename to `vec!["::<crate-name>"]` (absolute).
4. In `scan_test_fn`, collect body bindings scope-aware:
   - Direct statements of `f.block` that are `Item::Use` populate a
     `body_top: Imports` (same struct; its items sets take fn-body
     `Item::Fn`/`Item::Const`/`Item::Static`/tuple-or-unit
     `Item::Struct` names as `items_value`).
   - A visitor collects everything deeper into a `body_nested: Imports`
     value (same struct; only its `named` multimap and `globs` are
     populated — nested `use` statements, named and glob; its item sets
     stay empty).
   - Replace the fn-wide `ShadowCollector` role: top-level body items
     land in `body_top.items_value` (terminal); every other local
     binding (let / closure / for / if-let / match-arm patterns,
     nested-block item statements) lands in a `non_terminal_locals:
     HashSet<String>` set. Keep the existing `PatIdents` helper.
5. Add `fn expand_readings(path: &str, chain: &[Imports], body_top: &Imports, body_nested: &Imports) -> Vec<String>`:
   if `path` starts with `"::"` return it as-is; otherwise rewrite the
   leading segment through `named` maps (innermost wins — consult
   `body_nested` first, then `body_top`, then the chain — and every
   reading of the multimap entry branches), and when the leading
   segment has no named binding, branch through every glob root
   visible in `chain` + `body_top` + `body_nested` (root + `"::"` +
   leading). Recurse
   per branch until the leading segment resolves nowhere or a LEADING
   NAME repeats on that branch (emit the pre-cycle string). Deduplicate
   and memoize within a single expansion call (`HashMap` local to the
   `expand_readings` invocation — never a cache shared across test fns,
   whose chains differ) so the branch product stays finite and cheap.
6. Rewrite the `ResolvesPaths` trait: replace `shadow()` with three
   accessors — `fn body_top(&self) -> &Imports`,
   `fn body_nested(&self) -> &Imports`, and
   `fn non_terminal_locals(&self) -> &HashSet<String>` (keep `chain()`;
   drop `resolve()`/`is_path_target`) — and add
   `fn candidates(&self, name: &str, value_ns: bool) -> Vec<String>`
   implementing the walk (innermost → outermost):
   `non_terminal_locals` (value lookups only) → add `<local>`, continue;
   `body_nested` → add expanded named readings and glob candidates,
   continue; `body_top` → add all named readings, continue; namespace-
   appropriate items hit → `<local>`, TERMINAL; globs → add candidates,
   continue; module scopes in the chain → same rules; nothing anywhere →
   empty vec. Then expose two class-aware matchers used by all callers:
   `fn is_bounding_path(&self, path: &syn::Path) -> bool` (true only
   when the resolved candidate set is a singleton equal — after
   stripping a leading `"::"` — to a `TIMEOUT_TARGETS` entry) and
   `fn is_wait_path(&self, path: &syn::Path) -> bool` (true when any
   candidate equals a `WAIT_CALL_TARGETS` entry). For both: if
   `path.leading_colon` is set, compare the joined literal path only;
   for multi-segment paths build full candidates as
   `candidate + "::" + rest`; when the first segment yields zero
   candidates, fall back to the joined literal path. `scan_test_fn`
   builds the three structures and passes them to all four implementors
   (`SpawnCollector`, `TimeoutCollector`, `TimeoutSeeker`,
   `WaitFinder`).
7. Update the four call sites: `TimeoutCollector::visit_expr_call` and
   `TimeoutSeeker::visit_expr_call` use `is_bounding_path`;
   `SpawnCollector::is_spawn_expr` and `WaitFinder::visit_expr_await`
   use `is_wait_path`. Delete the old `is_path_target`.
8. Add the new tests below to the in-file `mod tests` (mirror the
   existing `findings(src)` helper), and UPDATE the existing
   `glob_imported_timeout_bounds_no_inner_awaits` test: keep its
   fixture, change the assertion from `vec![5]` to `is_empty()`, and
   rewrite its comment (a resolved glob import now bounds; flip is
   deliberate per the blessed spec).

**Tests:** (all via the `findings(&str) -> Vec<usize>` helper; `cargo test -p xtask lint_unbounded_wait`)

- `module_alias_timeout_bounds_inner_await`: fixture `"use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let r = clock::timeout(d, async { rx.recv().await; }).await;\n}\n"` → `findings` is empty (was reported before the fix).
- `module_alias_timeout_at_bounds_inner_await`: `"use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let r = clock::timeout_at(deadline, async { rx.recv().await; }).await;\n}\n"` → `findings` is empty.
- `module_alias_spawn_await_reported`: `"use tokio::task as task;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn(work()).await;\n}\n"` → `vec![5]` (was missed before the fix).
- `module_alias_spawn_blocking_await_reported`: `"use tokio::task as task;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn_blocking(work()).await;\n}\n"` → `vec![5]`.
- `module_alias_tcp_connect_await_reported`: `"use tokio::net as net;\n\n#[tokio::test]\nasync fn t() {\n    let s = net::TcpStream::connect(addr).await;\n}\n"` → `vec![5]`.
- `transitive_alias_timeout_bounds_inner_await`: `"use tokio::time as clock;\nuse clock::timeout as t;\n\n#[tokio::test]\nasync fn f() {\n    let r = t(d, async { rx.recv().await; }).await;\n}\n"` → empty.
- `alias_cycle_terminates_and_reports`: `"use beta::x as alpha;\nuse alpha::y as beta;\n\n#[tokio::test]\nasync fn t() {\n    let r = alpha(d, async { rx.recv().await; }).await;\n}\n"` → `vec![6]` (expansion stops at the repeated leading name; nothing bounds).
- `glob_prefix_qualified_spawn_await_reported`: `"use tokio::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = task::spawn(work()).await;\n}\n"` → `vec![5]`.
- `glob_imported_timeout_bounds_await_loop`: `"use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    loop {\n        let r = timeout(d, q.recv()).await;\n    }\n}\n"` → empty (the glob singleton bounds the loop; the rc-orivx case).
- `glob_imported_timeout_bounds_no_inner_awaits` (UPDATED): fixture `"use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"`, assertion → empty, comment rewritten (a resolved glob import now bounds; flip is deliberate per the blessed spec).
- `recv_inside_aliased_timeout_block_not_reported`: `"use tokio::time::timeout as with_deadline;\n\n#[tokio::test]\nasync fn t() {\n    let r = with_deadline(d, async { rx.recv().await; }).await;\n}\n"` → empty (non-vacuous companion to the preserved mission-201 test).
- `leading_colon_path_bypasses_alias`: `"use crate::fake as tokio;\n\n#[tokio::test]\nasync fn t() {\n    let r = ::tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n"` → empty (absolute path resolves literally).
- `aliased_extern_name_cannot_bypass_resolution`: `"use crate::fake as tokio;\n\n#[tokio::test]\nasync fn t() {\n    let r = tokio::time::timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![5]` (no literal short-circuit; the alias produces the non-target candidate).
- `absolute_import_target_immune_to_alias_rewrite`: `"use crate::fake as tokio;\nuse ::tokio::time::timeout as t;\n\n#[tokio::test]\nasync fn f() {\n    let r = t(d, async { rx.recv().await; }).await;\n}\n"` → empty.
- `extern_crate_alias_spawn_await_reported`: `"extern crate tokio as runtime;\n\n#[tokio::test]\nasync fn t() {\n    let r = runtime::task::spawn(work()).await;\n}\n"` → `vec![5]`.
- `glob_prefix_inside_alias_target_spawn_reported`: `"use tokio::*;\nuse task::spawn as s;\n\n#[tokio::test]\nasync fn t() {\n    let r = s(work()).await;\n}\n"` → `vec![6]`.
- `nested_block_glob_spawn_reported`: `"#[tokio::test]\nasync fn t() {\n    {\n        use tokio::task::*;\n        let r = spawn(work()).await;\n    }\n}\n"` → `vec![5]`.
- All 39 pre-existing tests still pass (`shadowed_spawn_fn_not_reported` survives via the terminal local `fn`; `recv_inside_aliased_timeout_not_reported` preserved unchanged; `tcp_stream_connect_free_fn_await_reported` survives via the literal fallback).

**Acceptance:**
- `cargo test -p xtask lint_unbounded_wait` green: 55 total (39 pre-existing incl. 1 flipped + 16 net-new).
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: Namespace, scope, and shadow adversarial test battery

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)

**Steps:**
1. Add the tests below to `mod tests`. Write each test FIRST, run it
   against the Task 1.1 resolver, and fix any resolution gap it
   reveals before moving on (the fixtures are the blessed spec's
   scenarios; do not weaken a fixture to make it pass).
2. If a fixture reveals a resolver gap, fix the resolver per the
   blessed design's candidate walk (terminality: only namespace-known
   items; imports never terminal; globs and locals non-terminal).

**Tests:** (all via `findings`; `cargo test -p xtask lint_unbounded_wait`)

- `local_shadow_fn_beats_glob_import`: `"use tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    fn timeout<T>(d: T, f: T) -> T { f }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![6]`.
- `same_scope_explicit_and_glob_stay_ambiguous`: `"use my::timeout;\nuse tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![6]`.
- `type_only_import_does_not_mask_glob_spawn`: `"use types::spawn;\nuse tokio::task::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `scope_local_fn_item_beats_glob_import`: `"mod m {\n    use tokio::time::*;\n    fn timeout<T>(d: T, f: T) -> T { f }\n    #[tokio::test]\n    async fn t() {\n        let r = timeout(d, async { rx.recv().await; }).await;\n    }\n}\n"` → `vec![6]`.
- `inner_glob_does_not_suppress_outer_spawn_import`: `"use tokio::task::spawn;\nmod inner {\n    use super::*;\n    use crate::helpers::*;\n    #[tokio::test]\n    async fn t() {\n        let r = spawn(work()).await;\n    }\n}\n"` → `vec![7]`.
- `nested_block_import_cannot_wrongly_bound`: `"use my::timeout;\n\n#[tokio::test]\nasync fn t() {\n    { use tokio::time::timeout; }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![6]`.
- `conflicting_sibling_block_imports_stay_ambiguous`: `"#[tokio::test]\nasync fn t() {\n    { use tokio::time::timeout; }\n    { use crate::helpers::timeout; }\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![5]`.
- `qualified_alias_immune_to_value_shadow`: `"use tokio::time as clock;\n\n#[tokio::test]\nasync fn t() {\n    let clock = 5;\n    let r = clock::timeout(d, async { rx.recv().await; }).await;\n}\n"` → empty.
- `tuple_struct_ctor_beats_glob_import`: `"struct timeout<D, F>(D, F);\nuse tokio::time::*;\n\n#[tokio::test]\nasync fn t() {\n    let r = timeout(d, async { rx.recv().await; }).await;\n}\n"` → `vec![6]`.
- `top_level_body_import_and_outer_import_ambiguous`: `"use tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    use crate::helpers::spawn;\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `dual_namespace_imports_keep_both_readings`: `"use tokio::task::spawn;\nuse types::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `enum_name_does_not_suppress_spawn_call`: `"enum spawn { A }\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `named_field_struct_does_not_suppress_spawn_call`: `"struct spawn { x: u32 }\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `type_alias_does_not_suppress_spawn_call`: `"type spawn = ();\nuse tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n}\n"` → `vec![6]`.
- `late_local_binding_does_not_suppress_earlier_detection`: `"use tokio::task::spawn;\n\n#[tokio::test]\nasync fn t() {\n    let r = spawn(work()).await;\n    let spawn = helper;\n}\n"` → `vec![5]`.

**Acceptance:**
- `cargo test -p xtask lint_unbounded_wait` green: 15 new tests plus everything from Task 1.1.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

### Task 1.3: Docs, ratchet evidence, and rc-orivx adjudication

**Files:**
- `scripts/xtask/src/lint_unbounded_wait.rs` (modified)
- `scripts/xtask/ratchet-unbounded-wait.max` (modified — header comment ONLY; the integer stays 394)

**Steps:**
1. Update the module doc (`//!` header) resolution section: describe
   the candidate-set contract — imports/module aliases/transitive
   aliases/globs resolved; only namespace-known items terminate;
   imports never terminal; ambiguity never bounds and never suppresses
   a wait finding; absolute-marker and leading-colon semantics; the
   fn-wide shadow note replaced by the scope-aware model.
2. Update the `ratchet-unbounded-wait.max` header comment: replace
   "imports and aliases resolved" with the new resolution contract
   summary (one sentence). Do NOT touch the `394` integer or the
   inventory block.
3. Evidence run: `cargo run -p xtask -- lint-unbounded-wait` from the
   worktree root — capture the dispatcher's PRINTED finding count from
   stdout and assert it is EXACTLY 394 (exit code alone is
   insufficient: the dispatcher exits 0 on a decrease too). If the
   count deviates in EITHER direction, capture the printed findings
   list / headroom line and adjudicate per ratchet convention BEFORE
   proceeding: a decrease lowers the ceiling with a burn-down note; an
   increase requires new TRUE findings, each inventoried — known
   movement vector is `use super::*;` globs coexisting with explicit
   timeout/spawn imports (same-scope ambiguity now reports those
   sites); such sites are Rust-truth bounded and get
   `// allow-test-wait: conservative ambiguity — explicit import beats
   same-scope glob` markers rather than a ceiling raise. Record the
   captured count and any adjudication in the task result for the park
   file.
4. rc-orivx adjudication: the `glob_imported_timeout_bounds_await_loop`
   test from Task 1.1 is the rc-orivx scenario (glob-imported timeout
   bounding a loop subtree). Note in the task result that rc-orivx is
   subsumed and will be closed with a supersede reference at closeout.

**Tests:**
- `ratchet_unchanged_at_394`: with pipefail enabled (`set -o pipefail`), run `cargo run -p xtask -- lint-unbounded-wait 2>/dev/null | tee /tmp/ubwait.out` and assert `grep -qx "lint-unbounded-wait: OK (394 findings = max 394)" /tmp/ubwait.out` exits 0 (exact line, dispatcher's equality branch); `rg -n "^394$" scripts/xtask/ratchet-unbounded-wait.max` → exactly one hit; `git diff scripts/xtask/ratchet-unbounded-wait.max` shows comment-line changes only.

**Acceptance:**
- `cargo run -p xtask -- lint-unbounded-wait` exits 0 at ceiling 394 (integer untouched).
- `cargo fmt --check --all`, `cargo clippy -p xtask -- -D warnings`, `cargo test -p xtask` all green.
- Module doc and ratchet header describe the candidate-set resolution contract.

- [x] 1.3

### Task 1.4: Precedence-rule bounding (revised blessed spec addendum)

Added after the implementation falsified the design's zero-live-glob
premise (see spec revision, e_gpt re-bless 2026-09-22): bounding
follows provenance precedence rules; evidence gate exactly 395 (394
base + 1 inventoried true finding from the fixed false-negative
class). Implementation contract, tests, docs, and evidence live in
`.superpowers/sdd/task-1.4-brief.md` and the task-1.3 report.

- [x] 1.4
