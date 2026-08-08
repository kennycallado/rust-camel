# Tasks: audit-fix-docdrift-lint

## Task 1: Lint scaffolding — CLI variant, dispatch, file discovery, Rule A (path + anchor)

**Files:**
- `scripts/xtask/src/lint_context_citations.rs` (new)
- `scripts/xtask/src/main.rs` (modified)

**Steps:**
1. In `main.rs`: add `LintContextCitations` to the `Commands` clap enum
   (sibling of `LintUnwrap` at ~L71). No fields. Add `mod lint_context_citations;`
   near the existing `mod changelog;` declaration.
2. In `main.rs`: add a dispatch match arm for `Commands::LintContextCitations`
   after the `LintLogLevels` arm (~L208), mirroring its structure: call
   `lint_context_citations::lint_context_citations(&workspace_root)`, print
   `lint-context-citations: OK (0 violations)` on empty, else print each
   `Violation` and `eprintln!("\nlint-context-citations: FAILED")` + exit 1.
3. In `lint_context_citations.rs`: add `pub fn mask_fenced_code(content: &str) -> String`
   that replaces every fenced block (lines between ```` ``` ```` fences) with
   empty lines (preserving line count so line numbers in violations stay
   accurate). All per-file rules (A, B, C) run over the masked content.
4. In `lint_context_citations.rs`: add
   `pub fn lint_context_citations(workspace_root: &Path) -> Result<Vec<Violation>, String>`
   that: (a) discovers context files via recursive `walkdir` over
   `crates/`, `examples/`, `benchmarks/`, `platforms/` plus the root
   `CONTEXT-MAP.md`, excluding `target/`, `.worktrees/`, `node_modules/`,
   hidden dirs (starting `.`), and any `archive/` dir; (b) for each file
   reads content, masks fenced code, and calls the per-rule `check_*_src`
   functions; (c) runs the cross-file glossary pass (Rule D, wired in Task 3);
   (d) returns aggregated `Vec<Violation>`. The glossary pass and Rules B/C
   are stubs returning empty until Tasks 2/3.
5. In `lint_context_citations.rs`: add
   `pub fn check_paths_src(masked_content: &str, file_path: &str, context_dir: &Path, workspace_root: &Path) -> Vec<Violation>`
   implementing Rule A: find markdown links `[text](target)` and inline bare
   paths matching `[\w/]+\.\w+` containing `/`. For each target: skip if scheme
   is `http(s)`/`mailto` or if target starts with `#` (fragment-only). Strip
   any `#anchor`. Resolve: try `context_dir.join(path)` first for ANY path; if
   not found, try `workspace_root.join(path)` as a fallback (this covers both
   known-root prefixes like `crates/` and bare relative paths that happen to be
   workspace-root-relative). Reject `..` that escapes workspace root
   (`[path]` violation). For `.md` targets carrying `#anchor`, call
   `pub fn anchor_exists(target_md_path: &Path, anchor: &str) -> bool` which
   parses headings (`^#{1,6}\s+`), normalizes each via
   `pub fn normalize_anchor(heading: &str) -> String` (lowercase, trim, collapse
   whitespace to single hyphen, strip leading/trailing punctuation, drop
   non-`[a-z0-9-]`), normalizes the link anchor the same way, and checks
   membership. Missing anchor → `[anchor]` violation.
6. Wire `check_paths_src` into the per-file loop inside
   `lint_context_citations` (pass the masked content).

**Tests:**
- name: `lint_context_citations_discovers_context_files`
  setup: a temp workspace with `crates/camel-api/CONTEXT.md`,
    `examples/CONTEXT.md`, and `CONTEXT-MAP.md` — each containing a deliberate
    dangling citation `[x](./nonexistent.rs)` so it produces a violation; plus
    `target/CONTEXT.md` and `.hidden/CONTEXT.md` (also with dangling citations)
    that must NOT be discovered
  action: call `lint_context_citations(&temp_workspace)`
  assert: the returned violations reference exactly the 3 expected files
    (`crates/camel-api/CONTEXT.md`, `examples/CONTEXT.md`, `CONTEXT-MAP.md`),
    never `target/` or `.hidden/`
  command: `cargo test -p xtask lint_context_citations_discovers_context_files`
  expected: pass after implementation
- name: `check_paths_link_target_exists`
  setup: masked content `[cfg](./src/config.rs)` with `src/config.rs` existing
    relative to context_dir
  action: call `check_paths_src`
  assert: no `[path]` violation emitted
  command: `cargo test -p xtask check_paths_link_target_exists`
  expected: pass after implementation
- name: `check_paths_dangling_path_flagged`
  setup: masked content `[old](./src/old.rs)` where `old.rs` does not exist
  action: call `check_paths_src`
  assert: one `[path]` violation emitted naming the dangling reference
  command: `cargo test -p xtask check_paths_dangling_path_flagged`
  expected: pass after implementation
- name: `check_paths_anchor_resolves`
  setup: masked content `[x](./error.md#not-found-variant)`; `error.md` has
    heading `## Not Found Variant`
  action: call `check_paths_src`
  assert: no `[anchor]` violation
  command: `cargo test -p xtask check_paths_anchor_resolves`
  expected: pass after implementation
- name: `check_paths_dangling_anchor_flagged`
  setup: masked content `[x](./error.md#removed)`; `error.md` has no matching
    heading
  action: call `check_paths_src`
  assert: one `[anchor]` violation naming `removed` and `error.md`
  command: `cargo test -p xtask check_paths_dangling_anchor_flagged`
  expected: pass after implementation
- name: `check_paths_external_and_fragment_excluded`
  setup: masked content `[a](https://ex.com/d)` and `[b](#section)`
  action: call `check_paths_src`
  assert: zero violations (external scheme; same-document fragment)
  command: `cargo test -p xtask check_paths_external_and_fragment_excluded`
  expected: pass after implementation
- name: `check_paths_traversal_rejected`
  setup: masked content `[s](../../../etc/passwd)`
  action: call `check_paths_src`
  assert: one `[path]` violation (`..` escapes workspace)
  command: `cargo test -p xtask check_paths_traversal_rejected`
  expected: pass after implementation
- name: `mask_fenced_code_preserves_line_count`
  setup: content with a 3-line fenced block
  action: call `mask_fenced_code`
  assert: output line count equals input line count; fenced lines are blank
  command: `cargo test -p xtask mask_fenced_code_preserves_line_count`
  expected: pass after implementation
- name: `check_paths_inline_bare_path_exists`
  setup: masked content `see src/config.rs for details` (no markdown link) with
    `src/config.rs` existing relative to context_dir
  action: call `check_paths_src`
  assert: no `[path]` violation (inline bare path resolves)
  command: `cargo test -p xtask check_paths_inline_bare_path_exists`
  expected: pass after implementation

**Acceptance:**
- `cargo run -p xtask -- lint-context-citations` runs without panic and
  reports `OK` or lists path/anchor violations.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo test -p xtask` passes (including the 9 new tests above).

- [x] 1

## Task 2: Rule B — symbol validation via syn

**Files:**
- `scripts/xtask/src/lint_context_citations.rs` (modified)

**Steps:**
1. Add `fn parse_crate_items(src_dir: &Path) -> Vec<syn::Item>` that walks
   `src_dir/**/*.rs`, reads each file, and calls `syn::parse_file` collecting
   all top-level `syn::Item` into a flat `Vec`. (Reuse the existing
   `declaration_line` / `item_has_cfg_test` helpers' parsing approach; the
   existing lints already parse via syn.)
2. Add `fn type_exists(items: &[syn::Item], type_name: &str) -> bool` that
   returns true if any `Item::Struct(s)` with `s.ident == type_name`,
   `Item::Enum(e)` with `e.ident == type_name`, or `Item::Trait(t)` with
   `t.ident == type_name` exists.
3. Add `fn method_in_impl_for_type(items: &[syn::Item], type_name: &str, method: &str) -> bool`
   that walks every `Item::Impl(impl_block)`: extract the self-type and
   trait-type from `impl_block.self_ty` and `impl_block.trait_` (if `negative`
   is false and `trait_` is `Some`), stringify each, and if either contains
   `type_name` as an identifier-boundary match, scan `impl_block.items` for an
   `ImplItem::Fn(f)` with `f.sig.ident == method`. Return true on first match.
4. Add `fn check_symbols_src(masked_content: &str, file_path: &str, own_items: &[syn::Item], workspace_items: &[syn::Item]) -> Vec<Violation>`
   implementing Rule B: find backtick-quoted tokens matching a Rust definition
   pattern (`fn <ident>`, `struct <Ident>`, `enum <Ident>`, `trait <Ident>`,
   `<Type>::<method>`). For `fn`/`struct`/`enum`/`trait`, confirm a matching
   `Item` exists in `own_items` (or `workspace_items` if `file_path` is
   `CONTEXT-MAP.md`). For `<Type>::<method>`: if `type_exists` is false in BOTH
   `own_items` and `workspace_items`, SKIP (external type). If the type exists,
   call `method_in_impl_for_type` on the relevant item set; false → `[symbol]`
   violation. Bare `impl <Ident>` tokens (no `::method`) are not matched.
5. Wire `check_symbols_src` into the per-file loop: for each CONTEXT.md,
   determine the search set — a crate CONTEXT.md gets its own crate's parsed
   items as `own_items` and empty `workspace_items`; `CONTEXT-MAP.md` gets all
   workspace crate items as `workspace_items` and empty `own_items`. The crate
   `src/` directory is `context_md.parent().join("src/")` (works for both
   top-level crates like `crates/camel-api/` and nested crates like
   `crates/components/camel-component-llm/`); if that `src/` does not exist,
   `own_items` is empty (the file has no symbols to validate against).

**Tests:**
- name: `check_symbols_fn_exists`
  setup: masked content `` `fn process_exchange` ``; own_items contains
    `Item::Fn` with `sig.ident == "process_exchange"`
  action: call `check_symbols_src`
  assert: no `[symbol]` violation
  command: `cargo test -p xtask check_symbols_fn_exists`
  expected: pass after implementation
- name: `check_symbols_struct_renamed_flagged`
  setup: masked content `` `struct RouteConfig` ``; own_items has no
    `RouteConfig` struct (only `RouteDslConfig`)
  action: call `check_symbols_src`
  assert: one `[symbol]` violation naming `RouteConfig`
  command: `cargo test -p xtask check_symbols_struct_renamed_flagged`
  expected: pass after implementation
- name: `check_symbols_trait_impl_resolves`
  setup: masked content `` `Producer::poll_ready` ``; own_items has
    `Item::Impl` with `trait_ = Some("Service")`, `self_ty = "Producer"`,
    containing `ImplItem::Fn` `poll_ready`
  action: call `check_symbols_src`
  assert: no `[symbol]` violation (method found in trait impl for the type)
  command: `cargo test -p xtask check_symbols_trait_impl_resolves`
  expected: pass after implementation
- name: `check_symbols_direct_impl_resolves`
  setup: masked content `` `RouteErrorHandler::handle` ``; own_items has
    `Item::Impl` with `self_ty = "RouteErrorHandler"` (no trait_) containing
    `ImplItem::Fn` `handle`
  action: call `check_symbols_src`
  assert: no `[symbol]` violation (method found in direct impl for the type)
  command: `cargo test -p xtask check_symbols_direct_impl_resolves`
  expected: pass after implementation
- name: `check_symbols_context_map_workspace_scope`
  setup: masked content `` `fn compile_declarative_route_to_canonical` `` with
    `file_path = "CONTEXT-MAP.md"`; own_items empty, workspace_items contains
    the matching `Item::Fn`
  action: call `check_symbols_src`
  assert: no `[symbol]` violation (symbol resolves via workspace_items because
    the file is CONTEXT-MAP.md)
  command: `cargo test -p xtask check_symbols_context_map_workspace_scope`
  expected: pass after implementation
- name: `check_symbols_wrong_type_method_flagged`
  setup: masked content `` `CamelContext::poll_ready` ``; own_items has
    `CamelContext` struct but `poll_ready` only in `impl RouteChannelService`
  action: call `check_symbols_src`
  assert: one `[symbol]` violation
  command: `cargo test -p xtask check_symbols_wrong_type_method_flagged`
  expected: pass after implementation
- name: `check_symbols_external_type_skipped`
  setup: masked content `` `DynamicMessage::decode` ``; `DynamicMessage` not in
    own_items or workspace_items
  action: call `check_symbols_src`
  assert: zero violations (external type, skipped)
  command: `cargo test -p xtask check_symbols_external_type_skipped`
  expected: pass after implementation
- name: `check_symbols_non_symbol_not_validated`
  setup: masked content `` `config.watch` `` (config key, not a Rust pattern)
  action: call `check_symbols_src`
  assert: zero violations
  command: `cargo test -p xtask check_symbols_non_symbol_not_validated`
  expected: pass after implementation

**Acceptance:**
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo test -p xtask` passes (including the 8 new tests).

- [x] 2

## Task 3: Rule C (line-number sole-locator) + Rule D (glossary collision)

**Files:**
- `scripts/xtask/src/lint_context_citations.rs` (modified)

**Steps:**
1. Add `fn check_line_refs_src(masked_content: &str, file_path: &str) -> Vec<Violation>`
   implementing Rule C: scan each non-table prose line (lines NOT starting
   with `|`) for the pattern `\w+\.rs:L?\d+`. For each match, check whether the
   same line contains a Rule-B recognized symbol citation
   (`` `(fn|struct|enum|trait)\s+\w+` `` or `` `\w+::\w+` `` in backticks). If
   NO symbol citation is present on the line, emit a `[line-ref]` violation.
2. Wire `check_line_refs_src` into the per-file loop.
3. Add `fn collect_glossary_terms(masked_content: &str) -> Vec<(String, String)>`
   returning `(normalized_term, raw_term)` pairs: find heading lines whose
   normalized title is exactly `glossary`, `key terms`, or `terminology`
   (case-insensitive, trimmed — NOT prefix matches). Within that section (until
   the next heading of the same or higher level), collect `**<Term>:**` lines
   (bold, colon-terminated, at line start ignoring leading whitespace). Fenced
   blocks are already blanked by the masked input, so no separate fence
   handling is needed. Normalize each term: strip leading `**` and trailing
   `**:`, then lowercase, trim, collapse internal whitespace, strip any
   remaining trailing colon. The normalized form is the first tuple element.
4. Add `fn detect_glossary_collisions(terms_by_file: &[(String, Vec<(String,String)>)]) -> Vec<Violation>`
   implementing Rule D: group files by normalized term. For any term owned by
   2+ files, sort the file paths ascending; emit one `[glossary-collision]`
   violation per file after the first, naming the canonical (first) owner.
5. Wire the glossary pass into `lint_context_citations`: after the per-file
   loop, collect all glossary terms across files, call
   `detect_glossary_collisions`, append results to the violation list.

**Tests:**
- name: `check_line_refs_symbol_only_passes`
  setup: masked content `` see `fn run_steps` for details `` (no line number)
  action: call `check_line_refs_src`
  assert: zero `[line-ref]` violations
  command: `cargo test -p xtask check_line_refs_symbol_only_passes`
  expected: pass after implementation
- name: `check_line_refs_bare_line_number_flagged`
  setup: masked content `see config.rs:80 for the field` (no backtick symbol)
  action: call `check_line_refs_src`
  assert: one `[line-ref]` violation
  command: `cargo test -p xtask check_line_refs_bare_line_number_flagged`
  expected: pass after implementation
- name: `check_line_refs_symbol_plus_line_allowed`
  setup: masked content `` see `fn foo` at config.rs:80 ``
  action: call `check_line_refs_src`
  assert: zero violations (symbol present on same line)
  command: `cargo test -p xtask check_line_refs_symbol_plus_line_allowed`
  expected: pass after implementation
- name: `check_line_refs_code_block_ignored`
  setup: RAW content with a fenced block containing `error.rs:42`; the masking
    blanks the block BEFORE `check_line_refs_src` runs
  action: mask the content, then call `check_line_refs_src` on the masked output
  assert: zero `[line-ref]` violations (the reference was inside a code block)
  command: `cargo test -p xtask check_line_refs_code_block_ignored`
  expected: pass after implementation
- name: `collect_glossary_unique_term`
  setup: content with `## Glossary` then `**Exchange:**`
  action: call `collect_glossary_terms`
  assert: returns one term `("exchange", "**Exchange:**")`
  command: `cargo test -p xtask collect_glossary_unique_term`
  expected: pass after implementation
- name: `collect_glossary_prefix_heading_excluded`
  setup: content with `## Glossary conventions` then `**Term:**`
  action: call `collect_glossary_terms`
  assert: returns empty (prefix heading does not open a section)
  command: `cargo test -p xtask collect_glossary_prefix_heading_excluded`
  expected: pass after implementation
- name: `collect_glossary_section_terminates`
  setup: content with `## Glossary` + `**Foo:**`, then `## Notes` + `**Foo:**`
  action: call `collect_glossary_terms`
  assert: returns only one `Foo` (the one under Glossary)
  command: `cargo test -p xtask collect_glossary_section_terminates`
  expected: pass after implementation
- name: `collect_glossary_non_section_bold_ignored`
  setup: content with `**Questions:**` and `**Outcome:**` but no Glossary heading
  action: call `collect_glossary_terms`
  assert: returns empty
  command: `cargo test -p xtask collect_glossary_non_section_bold_ignored`
  expected: pass after implementation
- name: `collect_glossary_fenced_bold_ignored`
  setup: RAW content with `## Glossary` then a fenced block containing
    `**FakeTerm:**`, then `**Real:**` outside the fence; mask first
  action: mask content, then call `collect_glossary_terms`
  assert: returns only `Real`, not `FakeTerm` (fenced bold blanked by masking)
  command: `cargo test -p xtask collect_glossary_fenced_bold_ignored`
  expected: pass after implementation
- name: `detect_glossary_collision_two_files`
  setup: terms_by_file with file A and file B both owning `canonical route spec`
  action: call `detect_glossary_collisions`
  assert: one `[glossary-collision]` violation on file B naming file A
  command: `cargo test -p xtask detect_glossary_collision_two_files`
  expected: pass after implementation
- name: `detect_glossary_normalized_collision`
  setup: file A has `**Exchange:**`, file B has `**exchange :**`
  action: call `detect_glossary_collisions`
  assert: one collision violation (normalized to `exchange` in both)
  command: `cargo test -p xtask detect_glossary_normalized_collision`
  expected: pass after implementation
- name: `mask_fenced_code_excludes_from_all_rules`
  setup: raw content with a fenced block containing `` `struct Fake` `` and
    `config.rs:99` and `**FakeTerm:**` under a `## Glossary` heading; mask
    first, then run `check_symbols_src`, `check_line_refs_src`, and
    `collect_glossary_terms` over the masked output
  action: mask content, call all three extractors
  assert: zero violations from all three (fenced tokens blanked before rules)
  command: `cargo test -p xtask mask_fenced_code_excludes_from_all_rules`
  expected: pass after implementation

**Acceptance:**
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo test -p xtask` passes (including the 12 new tests).

- [x] 3

## Task 4: CI + AGENTS.md wiring + baseline cleanup

**Files:**
- `AGENTS.md` (modified)
- `.github/workflows/ci.yml` (modified)
- any `CONTEXT.md` / `CONTEXT-MAP.md` files whose citations the lint flags (modified, as discovered)

**Steps:**
1. Add a `lint-context-citations` entry to the `## QUALITY GATES` block in
   `AGENTS.md` (after the `lint-log-levels` entry), with `name: lint-context-citations`
   and `run: cargo xtask lint-context-citations`.
2. Add a `lint-context-citations` step to `.github/workflows/ci.yml` in the
   quality-gates job, sibling to the `lint-unwrap` / `lint-log-levels` steps,
   with `run: cargo xtask lint-context-citations`.
3. Run `cargo run -p xtask -- lint-context-citations` against the full tree.
   For every violation reported: either fix the CONTEXT.md citation (correct
   the path/anchor/symbol, replace a bare line-number with a symbol citation,
   resolve a glossary collision by removing the duplicate term from the
   non-canonical owner), OR — if the lint is over-matching (false positive) —
   tighten the rule in `lint_context_citations.rs` and re-run. Iterate until the lint exits 0.
4. Record in the task result: the count and nature of baseline violations
   found and how each was resolved (fixed-citation vs lint-tightened).

**Tests:**
- name: `gate_present_in_agents_md`
  setup: AGENTS.md after edit
  action: grep the `## QUALITY GATES` block
  assert: a `lint-context-citations` entry exists AND its run line is exactly
    `cargo xtask lint-context-citations`
  command: `grep -A1 "lint-context-citations" AGENTS.md | grep -c "cargo xtask lint-context-citations"`
  expected: count ≥ 1
- name: `gate_present_in_ci_yml`
  setup: ci.yml after edit
  action: grep for the step
  assert: a `lint-context-citations` step exists AND its run line is
    `cargo xtask lint-context-citations`
  command: `grep -A1 "lint-context-citations" .github/workflows/ci.yml | grep -c "cargo xtask lint-context-citations"`
  expected: count ≥ 1
- name: `lint_clean_on_tree`
  setup: the full workspace tree post-cleanup
  action: run the lint
  assert: exits 0 with `lint-context-citations: OK (0 violations)`
  command: `cargo run -p xtask -- lint-context-citations`
  expected: exit 0

**Acceptance:**
- `grep -c "lint-context-citations" AGENTS.md` ≥ 1.
- `grep -c "lint-context-citations" .github/workflows/ci.yml` ≥ 1.
- `cargo run -p xtask -- lint-context-citations` exits 0.
- `cargo clippy -p xtask -- -D warnings` exits 0.
- `cargo test -p xtask` passes.

- [x] 4
