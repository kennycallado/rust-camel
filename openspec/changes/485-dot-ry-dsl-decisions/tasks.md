# Tasks: 485-dot-ry-dsl-decisions

Docs and ADR only. No Rust source changes. Each task's "Tests" block lists the
exact executable verification; run it from the worktree root.

## ADR amendments

### Task 1.1: Amend ADR-0025 with caught-property retention and `CamelExceptionHandled`

**Files:**
- `docs/adr/0025-outcome-aware-structural-eips.md` (modified)

**Steps:**
1. Append a `## Amendment: exception property retention (2026-11-01)` section
   after the existing status updates. State that `Exchange::clear_error()`
   removes `CamelExceptionMessage`, `CamelExceptionKind`, and
   `CamelExceptionCaught` on both `Handled` and `Continued`, for doTry and the
   route-level handler alike; this intentionally diverges from Java Camel's
   `EXCEPTION_CAUGHT` retention. Cite bd rc-zaqjd.
2. In the same amendment, record the supported workaround: read or copy the
   exception inside the catch body (for example into a user property or
   header). State that steps after a `Handled`/`Continued` exit SHALL NOT rely
   on `CamelExceptionCaught`.
3. State the two rejected options with the literal labels `Rejected — restore in
   \`DoTrySegment\`` and `Rejected — restore in \`clear_error()\``, each with its
   one-line reason: the first splits doTry from the route-level handler; the
   second leaves stale exception state across redelivery and exchange reuse.
4. Add `## Amendment: CamelExceptionHandled canonical rule (2026-11-01)` with
   the six normative points: (a) control flow keys off `PipelineOutcome` only;
   (b) the property is Camel-compatibility only and no core logic reads it;
   (c) only a `Handled` arm running through `Exchange::handle_error()` sets it
   (the doTry `Handled` arm and the route-level `Handled` paths that execute
   `on_exception` steps); a `Handled` arm that dispatches to the dead-letter
   channel clears without setting it; `Continued` **does not set** it; (d) `Handled` stops the block while `Continued` advances
   past it; (e) `DoTrySegment` removes a stale marker on entry, so absent,
   removed-on-entry, and retained-from-nested-`Handled` are distinct states;
   (f) `clear_error()` does not touch the marker, so an outer `Continued` does
   not clear a marker set by a nested `Handled`. Cite bd rc-ho9j1, the existing
   spec scenarios, and the deferred scenario ticket rc-ud3rl.
5. Keep the existing ADR sections untouched; the amendment is additive.

**Tests:** executable spec — name: command → expected.
- `adr0025_names_both_properties`: setup = ADR-0025 edited; action =
  `rg -c "CamelExceptionCaught" docs/adr/0025-outcome-aware-structural-eips.md`
  and `rg -c "CamelExceptionHandled" docs/adr/0025-outcome-aware-structural-eips.md`;
  assert = each command prints a count >= 1.
- `adr0025_cites_tickets`: `rg -c "rc-zaqjd"` >= 1, `rg -c "rc-ho9j1"` >= 1,
  `rg -c "rc-ud3rl"` >= 1, all against `docs/adr/0025-outcome-aware-structural-eips.md`.
- `adr0025_six_rule_points`: each of these six `rg -c "<phrase>" docs/adr/0025-outcome-aware-structural-eips.md`
  commands prints a count >= 1: `PipelineOutcome`, `Camel-compatibility`, `does not set`,
  `stops the block`, `removes a stale marker`, `does not touch the marker`.
- `adr0025_two_rejected_options`: `rg -c "Rejected — restore in" docs/adr/0025-outcome-aware-structural-eips.md`
  → count 2, one per rejected restoration option.
- `adr0025_context_lint`: `cargo xtask lint-context-citations` → exit 0.
- `adr0025_book_parses`: `cd docs && mdbook test` → exit 0 (no new code
  blocks, so no new doctests; proves the page still parses).

**Acceptance:**
- Both amendments present with the six handled-marker points and the two
  rejected restoration options; each is covered by a test above.
- `cargo xtask lint-context-citations` exits 0.

- [x] 1.1

### Task 1.2: State the rules in `error-handling.md` and `do-try.md`

**Files:**
- `docs/src/concepts/error-handling.md` (modified)
- `docs/src/eip/do-try.md` (modified)

**Steps:**
1. In `docs/src/concepts/error-handling.md`, add a `## Divergence from Apache
   Camel` subsection after the error-disposition table. State that after a
   `Handled` or `Continued` disposition this runtime clears `CamelExceptionCaught`
   together with the error, while Java Camel retains `EXCEPTION_CAUGHT`. Show
   the copy-inside-catch workaround in one short sentence, and link to
   `../adr/0025-outcome-aware-structural-eips.md`.
2. In the same subsection, state the `CamelExceptionHandled` rule in user terms:
   it is a Camel-compatibility marker, only a `Handled` disposition running
   `on_exception` steps through `handle_error()` sets it (`doTry` and the
   route-level `Handled` paths), a `Handled` arm that dispatches to the
   dead-letter channel clears without setting it, `Continued` never sets it,
   and an outer `Continued` does not remove a marker set by a nested `Handled`
   block. Point to the ADR.
3. In `docs/src/eip/do-try.md`, add one short paragraph after the disposition
   paragraph covering both rules: (a) a handled or continued catch clears the
   caught exception, so downstream steps must not read `CamelExceptionCaught`;
   read it inside the catch body instead; link
   `../concepts/error-handling.md#divergence-from-apache-camel`. (b)
   `CamelExceptionHandled` is Camel-compatibility only, only `Handled` sets it,
   and an outer `Continued` does not remove a marker set by a nested `Handled`
   block; point to the ADR.
4. Do not add a code block that would need a new doctest; use inline
   backticks for identifiers.

**Tests:** executable spec — name: command → expected.
- `error_handling_has_divergence_section`:
  `rg -n "Divergence from Apache Camel" docs/src/concepts/error-handling.md`
  → one match.
- `both_docs_name_the_properties`:
  `rg -n "CamelExceptionCaught|CamelExceptionHandled" docs/src/concepts/error-handling.md docs/src/eip/do-try.md`
  → both files match.
- `docs_context_lint`: `cargo xtask lint-context-citations` → exit 0.
- `docs_book_parses`: `cd docs && mdbook test` → exit 0.

**Acceptance:**
- Both docs name the divergence and link the ADR, directly or through the
  `error-handling.md#divergence-from-apache-camel` anchor.
- No doctest was added; `mdbook test` and the context lint pass.

- [x] 1.2

### Task 1.3: Clarify ADR-0049 scope for the DSL AST enums

**Files:**
- `docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Append a `## Scope clarification: camel-dsl ASTs (2026-11-01)` section to
   ADR-0049. State that `RouteDslStep` (`crates/camel-dsl/src/route_ast.rs`) and
   `DeclarativeStep` (`crates/camel-dsl/src/model.rs`) are intentionally
   exhaustive: they are authoring ASTs, not contract-crate enums, and their
   out-of-crate exhaustive matches are the compile-time guard that every new DSL
   step is wired through compile, lint, and CLI. Cite bd rc-b48du.
2. State the release rule: a new AST variant (authorable or internal/doc-hidden)
   is a breaking change for external exhaustive matchers and must appear in
   release notes. `DeclarativeStepKind` is the intended external discriminant
   (`crates/camel-dsl/src/contract.rs:2-51`); it names the authorable kinds and
   omits internal-only variants such as `ResponseMarshal`. It is also an
   exhaustive public enum, so adding a kind breaks downstream exhaustive matches
   and needs the same breaking-change release discipline. The internal
   `ResponseMarshal` variants stay undocumented for construction
   (`#[doc(hidden)]` deferred to a code PR). Note that no versioned CHANGELOG
   file exists; `cargo xtask changelog` renders release notes from commit
   subjects, so the rule is enforced by release discipline recorded here.
3. In `CONTEXT-MAP.md`, extend the ADR-0049 summary line (the line starting
   `- [0049](...`) to contain the literal `camel-dsl AST`, and extend the
   ADR-0025 summary line (the line starting `- [0025](...`) to contain the
   literal `CamelExceptionCaught`. Keep both entries one line.

**Tests:** executable spec — name: command → expected.
- `adr0049_has_scope_clarification`:
  `rg -n "Scope clarification: camel-dsl ASTs" docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md`
  → one match.
- `adr0049_names_enums_and_ticket`:
  `rg -n "RouteDslStep|DeclarativeStep|rc-b48du" docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md`
  → all three appear.
- `context_map_summaries_updated`:
  `rg -n "^- \[0049\].*camel-dsl AST" CONTEXT-MAP.md` → one match;
  `rg -n "^- \[0025\].*CamelExceptionCaught" CONTEXT-MAP.md` → one match.
- `adr0049_context_lint`: `cargo xtask lint-context-citations` → exit 0.
- `non_exhaustive_policy_lint`: `cargo xtask lint-non-exhaustive` → exit 0 (no
  Rust change; proves the policy lint still accepts the tree).

**Acceptance:**
- ADR-0049 states the exhaustive-by-design ruling and the release rule.
- CONTEXT-MAP summaries reflect both amendments.
- Both xtask lints pass.

- [x] 1.3

## Verification

### Task 1.4: Prove no behavior change and run the docs gate set

**Files:**
- `openspec/changes/485-dot-ry-dsl-decisions/evidence/verification.md` (new)

**Steps:**
1. Create `openspec/changes/485-dot-ry-dsl-decisions/evidence/verification.md`
   with a table: `command | exit code | output summary` for every command in
   steps 3 and 4, plus the `git diff` evidence from step 1.
2. Confirm the diff contains no Rust source and no spec delta. Set
   `BASE=$(git merge-base HEAD main)` and run `git diff "$BASE" --name-only`; it
   lists only `docs/**`, `CONTEXT-MAP.md`, and
   `openspec/changes/485-dot-ry-dsl-decisions/**`.
3. Confirm `openspec/changes/485-dot-ry-dsl-decisions/.openspec.yaml` carries
   `skip_specs: true` and the change has no `specs/` directory.
4. Run the gate set: `cargo fmt --check --all` (N/A but cheap),
   `cargo xtask lint-context-citations`, `cargo xtask lint-non-exhaustive`,
   `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
   -p camel-builder -p camel-dsl -p camel-endpoint
   -p camel-component-validator -p camel-xslt --no-deps`,
   `cd docs && mdbook test`, and
   `openspec validate --all --strict`.
5. Link resolution: run the CI command `mdbook build docs && lychee --offline
   --no-progress docs/book` if `lychee` is installed; otherwise record
   "link validation deferred to CI (.github/workflows/docs.yml)" with the
   `command -v lychee` output as evidence, and state that the new
   `#divergence-from-apache-camel` anchor is checked only by CI lychee/mdbook
   build, not by `mdbook test`.
6. Add a line to the evidence file naming bd `rc-ud3rl` (P4,
   discovered-from `rc-ho9j1`) as the deferred scenario ticket. Do not run bd
   from the worktree and do not create or close it; the conductor verified it
   during planning.

**Tests:** executable spec — name: command → expected.
- `no_rust_in_diff`: with `BASE=$(git merge-base HEAD main)`,
  `git diff "$BASE" --name-only | rg '\.rs$'` → no output (exit 1).
- `no_spec_delta`:
  `test ! -d openspec/changes/485-dot-ry-dsl-decisions/specs && echo no-specs`
  → prints `no-specs`.
- `docs_gate_set`: each command in step 3 → exit 0.
- `link_validation_disposition`: `command -v lychee` → a path (then run the CI
  link command and require exit 0) or no output (record the CI deferral); the
  evidence string is recorded either way.
- `evidence_file_written`:
  `rg -n "rc-ud3rl|exit code" openspec/changes/485-dot-ry-dsl-decisions/evidence/verification.md`
  → both terms appear.

**Acceptance:**
- Zero Rust files in the diff; no spec delta.
- Every listed gate exits 0; results, the lychee outcome, and the `rc-ud3rl`
  existence check are recorded in `evidence/verification.md`, which the
  conductor folds into the park JSON.

- [x] 1.4

## Post-bless amendments

- **2026-11-01 — marker-setting precision (STAGE 4 consistency check).** The
  plan said "only the `Handled` arm sets it". That is true, but it read as if
  every `Handled` disposition sets it. `crates/camel-processor/src/error_handler.rs`
  has four `handle_error()` sites (`DoTrySegment`, retry-service
  `execute_on_steps`, step-level `on_exception` steps, boundary `on_steps`) and
  dead-letter dispatch paths that use `clear_error()` and never set the marker.
  The task blocks, `design.md`, ADR-0025, and `docs/src/concepts/error-handling.md`
  now state the rule semantically ("only a `Handled` arm that runs
  `on_exception` steps through `handle_error()`") instead of enumerating one or
  two internal variants. No behavior change; the canonical rule ("only a
  `Handled` arm can set it; `Continued` never does") is unchanged.
