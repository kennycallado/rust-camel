# Tasks: docsweep

## Testing documentation

### Task 1.1: Clarify the testing build matrix

**Files:**
- `docs/src/testing/index.md` (modified)

**Steps:**
1. Locate the build matrix row currently labelled `default` and describing
   only the `fake:` path.
2. Rename that row to identify the `--no-default-features` build and add or
   revise the default-build row so it reflects the current
   `integration-http` and `integration-sql` defaults without enumerating
   unrelated features.
3. Preserve the existing table structure and surrounding prose.

**Tests:**
- `testing-build-table-default-row`: setup is the edited Markdown table;
  action is a textual review against `crates/camel-cli/Cargo.toml` default
  features; assert the featureless row is distinct and the default row names
  `integration-http` and `integration-sql`; command `git diff --check`;
  expected pass.

**Acceptance:**
- `docs/src/testing/index.md` distinguishes no-default-features from the
  default build and names both current integration defaults.
- `git diff --check` exits 0 for the task changes.

- [x] 1.1

## Processor documentation

### Task 2.1: Regenerate processor catalog citations

**Files:**
- `crates/camel-processor/CONTEXT.md` (modified)

**Steps:**
1. Extract every processor module name from the catalog and map it to the
   matching `pub mod <name>;` declaration in
   `crates/camel-processor/src/lib.rs`.
2. Replace each stale `src/lib.rs:N` citation with its exact declaration
   line, preserving catalog order, wording, and Markdown formatting.
3. Confirm the diff changes citations only.

**Tests:**
- `processor-catalog-citations-match-source`: setup is the edited catalog
  and `src/lib.rs`; action is to compare each catalog module citation with
  the declaration line; assert every citation equals the source line;
  command `cargo xtask lint-context-citations`; expected pass.

**Acceptance:**
- Every processor catalog citation points to the exact `pub mod` line.
- No processor source or unrelated context page changes are included.
- `cargo xtask lint-context-citations` exits 0.

- [x] 2.1

## DSL documentation

### Task 3.1: Correct stale route AST comments

**Files:**
- `crates/camel-dsl/src/route_ast.rs` (modified)

**Steps:**
1. Verify the existing null visitor test and dependency behavior: serde_json
   rejects `parameters: null`, while noyalib routes YAML null through
   `deserialize_map` to `visit_map` over an empty access; the direct
   deserializer tests pin `visit_unit`/`visit_none` and produce an empty map.
2. Rewrite the stale comment to state those format-specific outcomes.
3. Delete the note claiming `ResequenceStreamYaml` is intentionally omitted,
   because the struct is defined immediately below it.
4. Keep the edit comment-only and run Rust formatting.

**Tests:**
- `null_parameters_route_through_null_visitor_arms`: setup is the existing
  route AST null-handling test; action is `cargo test -p camel-dsl
  null_parameters_route_through_null_visitor_arms`; assert the test passes
  and documents the behavior used by the comment; command exactly as shown;
  expected pass.
- `route_ast_comments_are_comment_only`: setup is the task diff; action is
  inspect changed hunks; assert no non-comment Rust tokens change; command
  `git diff --check`; expected pass.

**Acceptance:**
- The null comment distinguishes serde_json rejection from noyalib visitor
  routing and empty-map behavior.
- The false omission note is removed.
- `cargo fmt --check --all` exits 0 and the targeted test passes.
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-dsl --no-deps` exits 0.

- [x] 3.1

## Component documentation

### Task 4.1: Document component b-prime labels

**Files:**
- `crates/components/camel-ws/CONTEXT.md` (modified)
- `crates/components/camel-file/CONTEXT.md` (modified)
- `crates/components/camel-timer/CONTEXT.md` (new)

**Steps:**
1. Add one context bullet to the WS page for the existing
   `b-prime:ws:message-dispatch` metric site.
2. Add one context bullet to the file page for the existing
   `b-prime:file:poll-send` metric site.
3. Create the timer context page using the existing component context-page
   structure and document `b-prime:timer:fire-send` with a live source
   citation.
4. Run context citation and metric-label checks; do not modify the shared
   component test harness or production source.

**Tests:**
- `component-context-labels-match-source`: setup is the three context pages
  and their existing source sites; action is `cargo xtask lint-context-citations
  && cargo xtask lint-metric-labels`; assert both commands exit 0 and all
  three exact labels are present; expected pass.
- `timer-context-page-is-present`: setup is the component directory; action
  is `test -f crates/components/camel-timer/CONTEXT.md`; assert exit 0;
  command exactly as shown; expected pass.

**Acceptance:**
- The three exact b-prime labels are documented in the matching context
  pages.
- The new timer page follows repository context-page conventions and all
  cited paths resolve.
- No test harness, production implementation, or unrelated component file
  changes are included.

- [x] 4.1
