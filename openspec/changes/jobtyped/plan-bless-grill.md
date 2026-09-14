# Plan-bless self-grill record — jobtyped (Mission 96, bd rc-g4ruv)

Expert: plan-bless (rotated after e_glm pre-flight, e_gpt spec-bless).
Skill: self-grill-proposals.
Artifact hash under bless: sha256:938eabc89e26e177a252ba9279b73cd008ee3367dac6ad0e85f114e302d2dee1
Verdict: **BLESS-WITH-FIXES** (2 minor executability fixes; no design or scope defects).

All 5 code-reality anchors were read and cross-referenced. Every task claim
about existing seams holds.

## Anchors verified against code

- `JobArgumentDeclaration` is a 3-field struct (`required`/`default`/`description`)
  at `document.rs:129`; adding `arg_type` is additive. (`document.rs:128-136`)
- `job_argument_declaration` match arms on `required|default|description|other`
  at `document.rs:636-682`; the `other =>` arm returns `UnknownArgumentField`.
  Admitting a `type` key means inserting a new arm BEFORE `other`. Single source
  confirmed — no second strict parser in the crate (Task 1 step 1 guard is
  belt-and-suspenders, correctly cheap).
- `normalize_job_args` at `document.rs:616-631` is the shared entry both
  `parse_job_document_impl` (`document.rs:445`) and `parse_job_document_for_help`
  (`document.rs:574`) call — so a typed-default coercion check placed inside
  `normalize_job_args` is inherited by `--help` for free. Task 4's
  `help_declaration_error_exits_2` test is therefore satisfiable with no help.rs
  change beyond the render column. VERIFIED.
- `resolve_job_args` at `document.rs:701-726` iterates pairs (unknown-name),
  then `declarations.entries` (missing-required + default fill). Task 2's
  "final pass after default fill, BTreeMap order" appends cleanly; precedence
  unknown-name > missing-required > coercion is structurally guaranteed because
  each earlier check `return Err`s first. VERIFIED.
- `interpolate_declared_fields` (`document.rs:778-800`) consumes the resolved
  `BTreeMap<String,String>` verbatim; Task 3's "no production change" claim is
  correct — canonical strings flow through unchanged. VERIFIED.
- help.rs hardcoded `"  string  "` at `help.rs:73`; `name_width` per-job strategy
  at `help.rs:44-49`. Task 4's `type_width` mirror is exact. VERIFIED.
- compile.rs `run_compile` does NOT parse job docs today; `text` is available at
  `compile.rs:122` (post-normalize) and `kind` at `:146`; write happens at `:175`.
  A `validate_job_declarations_for_compile(&text)` guarded by `kind ==
  TrailerKind::Job` inserts cleanly between `:166` and `:170`. VERIFIED.
- runtime.rs `TrailerKind::Job` dispatches to `run_embedded_job` at
  `runtime.rs:346-347`; `run_embedded_job` calls `parse_job_document_with_args(
  .., &[])` at `mod.rs:735` — shared parser, empty pairs. Task 5 step 3's
  "verify only, no restructure" claim holds; embedded typed defaults coerce
  through the same seam. VERIFIED.

---

## Q&A

**Questions generated:**
1. [glossary] Does `JobArgType` / "canonical form" / "coercion" collide with
   any existing CONTEXT-MAP glossary term or the landed A2 `type`-free vocabulary?
2. [sharpen] Task 5 step 2 says "print the existing loud diagnostic style and
   exit 2." Is "exit 2" a literal or a named constant, and does a mid-tier worker
   have an unambiguous instruction for the compile.rs failure path?
3. [scenario] Can a worker construct the delta scenario "compiled artifact
   coerces embedded typed defaults" AND the compile-rejection half from the tasks
   without an open decision? Is every delta scenario (incl. the compile-time
   sentence) covered by a task test?
4. [cross-ref] Does the plan touch camel-dsl / camel-http / cli-compile-spec, or
   widen the compile seam past "argument declarations only" as the spec promises?
5. [scenario] Task 3 batch test: does the referenced fixture shape actually exist
   in `tests/job_one_shot_test.rs`, and does `tests.rs` have the named helpers,
   so a worker can port it without inventing a harness?

**Answers (with citations):**

1. [glossary] No collision. CONTEXT-MAP `Declared job arguments (args:)` at
   `CONTEXT-MAP.md:189` currently enumerates `required`/`default`/`description`
   only and says nothing about `type`. `JobArgType`, `coerce_argument`, and
   "canonical string form" are NEW identifiers with no glossary counterpart.
   Task 5 step 5 extends the SAME entry in its existing authority-citation style
   (`Authority: cli-jobs spec. (camel-cli + camel-dsl + camel-lint)`), which is
   the correct home. The delta spec MODIFIED "declared job arguments" requirement
   (`spec.md:107-123`) matches the glossary's grammar. CONFIRM.

2. [sharpen] **Minor executability gap.** `compile.rs:28` defines
   `const EXIT_REJECTION: i32 = 2;` and every failure path returns
   `EXIT_REJECTION` with the prefix `eprintln!("camel compile: {e}")`
   (`compile.rs:157,163`). Task 5 step 2 says "exit 2" (literal) without naming
   `EXIT_REJECTION` or the `"camel compile: "` prefix. A mid-tier worker COULD
   write `return 2;` — compiling correctly but breaking the crate's single-source
   exit-code convention and diverging from the loud-diagnostic prefix. FIX
   required: pin the constant and prefix. (Not a design defect; the value is 2,
   so the exit taxonomy 2>1>0 is untouched.)

3. [scenario] Yes, coverage is complete. Every delta scenario maps to a task test:
   int-canonical→`resolve_int_coerces_canonical_form`+`typed_args_interpolate...`;
   int-reject→`resolve_int_rejects_non_integer`; bool→`resolve_bool_*`;
   enum-member/outsider→`resolve_enum_member_verbatim_and_outsider_lists_members`;
   typed-default-no-pair→`resolve_typed_default_canonicalizes_without_pair`+
   `compiled_job_coerces_typed_default`; typed-default-fails-load-and-help→
   `arg_type_typed_default_failing_coercion_fails_load` (Task 1) +
   `help_declaration_error_exits_2` (Task 4); unknown-name-precedes→
   `resolve_unknown_name_precedes_coercion`; missing-required-precedes→
   `resolve_missing_required_precedes_coercion`; all-fields→
   `typed_args_interpolate_canonical_forms_all_fields`. The compile-time sentence
   in the coercion requirement (`spec.md:21-25` "Compiling a job document SHALL
   run the argument-declaration checks ... and exit 2 without producing an
   artifact") is covered by `compile_rejects_bad_typed_default` +
   `compile_rejects_malformed_declaration` + `compiled_job_coerces_typed_default`
   (Task 5). The declared-args MODIFIED scenarios (each-type-spelling, unknown
   word, malformed enum, trimmed members) map to Task 1's document_tests. NO gap.
   **One sharpening (minor):** Task 3's `typed_args_interpolate_canonical_forms_all_fields`
   uses `target: {type: "enum[direct:in,direct:out]"}` — an enum member containing
   a `:`. The grammar (Task 1 step 4 / `spec.md:116`) forbids `,`/`[`/`]`/CR/LF
   in members but ALLOWS `:`. So `direct:out` is a legal member. The task already
   says "adapt the enum members to the job's real consumer route names", so this
   is self-consistent — but a worker should be told the member's `:` is
   deliberately legal (it is not in the forbidden set). Optional clarity note.

4. [cross-ref] No scope leak. Task files touch only `commands/job/{document,help}.rs`,
   `commands/compile.rs`, `compile/runtime.rs` (verify + comments only, step 3
   says STOP-and-report if the path diverges), the four test modules, and
   `CONTEXT-MAP.md`. No camel-dsl edit (`interpolate_declared_fields` at
   `document.rs:778` CONSUMES `camel_dsl::interpolate_with_args` unchanged —
   `document.rs:742`). No camel-http. No `cli-compile` spec delta (Task 5 edits
   compile.rs CODE, not the compile spec — the compile-time behavior is defined
   in the `typed argument coercion` requirement of THIS change's cli-jobs spec,
   `spec.md:21-25`, which is the correct owner since it is a job-document
   declaration check, not a general artifact-compilation contract). The compile
   seam is exactly as narrow as the spec: UNTYPED `serde_yaml::Value`, extract
   `args:` only, `normalize_job_args`, drop result (Task 5 step 1) — no
   document-structure or execution-value validation, matching `spec.md:24`
   "no other execution-value validation SHALL run at compile time". CONFIRM.

5. [scenario] Verified by grep: `tests/job_one_shot_test.rs` has 22 hits for
   `mode: batch|seda:|mock:` (the batch fan-out fixture is real). `tests.rs`
   defines `job_test_binary` (`tests.rs:21`), `run_camel_job` (`:48`), and
   `write_tap_route` (`:69`), exactly the helpers Task 3 names. `tests.rs` has NO
   existing batch harness (0 hits for `mode: batch`), so Task 3 step 3's "port the
   fixture shape into tests.rs beside the existing harness" is both necessary and
   executable. `batch_typed_arg_coerces_and_drains` declares `batch-id: {type: int}`
   — note `batch-id` contains a hyphen, which VIOLATES the argument-identifier
   grammar `[A-Za-z_][A-Za-z0-9_]*` (`document.rs:602-609`; `is_argument_identifier`
   rejects `-`). **This test as written would fail at `InvalidArgumentName`, not
   test coercion.** FIX required: rename to a legal identifier (e.g. `batch_id`).
   Same latent bug does not appear elsewhere (other tests use `count`/`verbose`/
   `tier`/`name`/`target`/`wait`, all legal). CONFIRM after fix.

**Outcome:** refine (2 required fixes + 1 optional clarity note)
**Self-grill mode:** self-grill-proposals skill

---

## Required fixes (BLESS-WITH-FIXES)

**FIX 1 — Task 5 step 2, exit-code + diagnostic convention (executability).**
Replace "print the existing loud diagnostic style and exit 2" with the concrete
convention a worker can copy: on `Err(e)`, `eprintln!("camel compile: {e}")` and
`return EXIT_REJECTION;` (the `const EXIT_REJECTION: i32 = 2` already defined at
`compile.rs:28`). This keeps the single-source exit constant and the loud prefix
that every other `run_compile` failure path uses (`compile.rs:157,163`). Do NOT
introduce a literal `return 2;`.

**FIX 2 — Task 3 test `batch_typed_arg_coerces_and_drains`, illegal identifier.**
The argument name `batch-id` fails `is_argument_identifier` (`document.rs:602`;
`-` is not in `[A-Za-z0-9_]`), so the document would be rejected with
`InvalidArgumentName` before any coercion runs — the test would assert on the
wrong error. Rename the declared argument to a legal identifier such as
`batch_id` (declaration `batch_id: {type: int}`, run `--arg batch_id=007`, assert
header `batch_id=7`). Update the test body and its assertion accordingly.

## Optional clarity note (non-blocking)

Task 3 `typed_args_interpolate_canonical_forms_all_fields` and delta
scenario `spec.md:89` use enum members with a `:` (`direct:in`, `direct:out`).
This is intentional and legal: the enum-member forbidden set is `,`/`[`/`]`/CR/LF
only (`spec.md:116`, Task 1 step 4), so `:` is permitted. A worker need not
"fix" it. No change required; recorded to pre-empt a false-positive during apply.

## Dimensions cleared

1. Mid-tier executability: YES for Tasks 1,2,4 as written; Tasks 3 and 5 need
   FIX 2 and FIX 1 respectively before an unassisted worker succeeds.
2. Spec coverage incl. compile-time sentence: COMPLETE (Q3).
3. Symbol consistency (`JobArgType`+`render`, `parse_arg_type`, `coerce_argument`,
   `InvalidArgumentType`/`ArgumentCoercion`, `validate_job_declarations_for_compile`):
   consistent across tasks/design/spec; single `render` reused by help + diagnostics
   (Task 1 step 2, Task 4 step 2, design item 2).
4. Back-compat: untyped docs bit-identical — `arg_type` defaults to `String`,
   coercion skipped for `String` (Task 1 step 6, Task 2 step 2 "non-`String`"
   guard), `resolve_untyped_values_stay_verbatim` + `untyped_document_behavior_unchanged`
   + `help_untyped_job_renders_string_column_unchanged` pin it. Compile seam
   UNTYPED-Value narrow, matching spec promise (Q4).
5. Exit taxonomy 2>1>0 untouched: all new errors are exit-2 class; `EXIT_REJECTION==2`.
6. No camel-dsl/camel-http/cli-compile-spec scope leak (Q4).
7. Test commands executable: all use `cargo test -p camel-cli <module/test-path>`;
   module paths (`commands::job::document_tests`, `::tests`, `::help_tests`) and
   integration targets (`--test compiled_artifact_test`) match the crate layout.
