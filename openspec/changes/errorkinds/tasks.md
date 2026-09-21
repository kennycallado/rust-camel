# Tasks: errorkinds

## Task 1 — Register three in-pipeline exception kinds

Files:
- crates/camel-dsl/src/compile.rs (modified)

Steps:
1. In `supported_exception_kinds()` (compile.rs ~line 904), insert three
   entries before `"ValidationError"` (mirroring 24ba1ee9 diff position):
   `"UnsupportedMediaType"`, `"NotAcceptable"`,
   `"ProcessorErrorWithSource"`.
2. Add a doc comment on `supported_exception_kinds()` documenting the
   intentionally-unmatchable variants (`ConfigValidation`, `EndpointUri` —
   startup fail-fast, never reach the route error handler, bd rc-5u8co)
   and the deferred one (`TemplateReload` — lifecycle-command path
   bypasses handlers, pending bd).
3. In `exception_kind_matches()` (compile.rs ~line 928), add three arms
   after the `"AuthProviderUnavailable"` arm, each with a rationale
   comment citing bd rc-5u8co and the raise site:
   - `"UnsupportedMediaType" => matches!(err, CamelError::UnsupportedMediaType { .. })`
     (raised in-pipeline by the media negotiation gate, media.rs:209 /
     content_negotiation.rs:144; 415-class)
   - `"NotAcceptable" => matches!(err, CamelError::NotAcceptable { .. })`
     (same gate, media.rs:241; 406-class)
   - `"ProcessorErrorWithSource" => matches!(err, CamelError::ProcessorErrorWithSource(_, _))`
     (raised in-pipeline by source-carrying producers: bean, exec,
     surrealdb; own kind per rc-2vm2y alias-distinction precedent —
     `variant_name()` aliases it to "ProcessorError" but on_exceptions
     matching is structural)
4. In test `test_compile_error_handler_kind_list_guard` (compile.rs:2431),
   add the three new kinds to the `expected` vector in the same positions
   (after `"ValidationError"`).
5. Add four new tests in the `mod tests` block of compile.rs, mirroring
   `test_compile_error_handler_auth_provider_unavailable_matches`
   (24ba1ee9 pattern): construct `DeclarativeErrorHandler` with
   `on_exceptions: Some(vec![DeclarativeOnException { kind: Some(THE_KIND.into()), message_contains: None, retry: None, steps: vec![], handled: None, continued: None }])`, `dead_letter_channel: None`, `retry: None`, `use_original_message: false` —    where THE_KIND is the literal kind string under test; call
   `compile_error_handler` with that handler; assert `config.policies.len() == 1`; assert `(config.policies[0].matches)` returns true for the corresponding variant instance and false for `CamelError::Io("other".into())`. Variant instances:
   - `CamelError::UnsupportedMediaType { consumed: "text/plain".into(), declared: "application/json".into() }`
   - `CamelError::NotAcceptable { accept: "application/xml".into(), produced: "application/json".into() }`
   - `CamelError::ProcessorErrorWithSource("exec failed".into(), std::sync::Arc::new(std::io::Error::other("boom")))` (toolchain proven: error.rs:292 uses `std::io::Error::other`).
   The fourth test is an alias pin: compile a policy with
   `kind: Some("ProcessorError".into())` and assert it does NOT match a
   `CamelError::ProcessorErrorWithSource` instance (structural matching
   stays distinct from the variant_name alias).
6. Add a test `test_compile_error_handler_startup_and_deferred_kinds_still_rejected`
   asserting `compile_error_handler` with `kind: "ConfigValidation"`,
   `"EndpointUri"`, and `"TemplateReload"` each returns `Err` whose
   Display contains both `"unknown exception kind"` and
   `"supported kinds:"` (the vocabulary listing is part of the spec'd
   error contract).

Tests:
- name: test_compile_error_handler_unsupported_media_type_matches
  setup: compile.rs tests mod, DeclarativeErrorHandler/DeclarativeOnException in scope
  action: compile_error_handler with kind "UnsupportedMediaType"
  assert: one policy; matches CamelError::UnsupportedMediaType; not Io
  command: cargo test -p camel-dsl --lib test_compile_error_handler_unsupported_media_type_matches
  expected: fails before step 3 (unknown-kind Config error), passes after
- name: test_compile_error_handler_not_acceptable_matches
  setup/action/assert: same shape with kind "NotAcceptable" and CamelError::NotAcceptable
  command: cargo test -p camel-dsl --lib test_compile_error_handler_not_acceptable_matches
  expected: fails before, passes after
- name: test_compile_error_handler_processor_error_with_source_matches
  setup/action/assert: same shape with kind "ProcessorErrorWithSource" and CamelError::ProcessorErrorWithSource(msg, Arc source)
  command: cargo test -p camel-dsl --lib test_compile_error_handler_processor_error_with_source_matches
  expected: fails before, passes after
- name: test_compile_error_handler_processor_error_alias_distinction
  action: compile policy with kind "ProcessorError"
  assert: does NOT match CamelError::ProcessorErrorWithSource; DOES match CamelError::ProcessorError("plain")
  command: cargo test -p camel-dsl --lib test_compile_error_handler_processor_error_alias_distinction
  expected: passes before AND after (pin — must not regress)
- name: test_compile_error_handler_startup_and_deferred_kinds_still_rejected
  action: compile with each of the three rejected kinds
  assert: Err with "unknown exception kind" AND "supported kinds:" in Display
  command: cargo test -p camel-dsl --lib test_compile_error_handler_startup_and_deferred_kinds_still_rejected
  expected: passes before AND after (rejection pin)

Acceptance:
- cargo test -p camel-dsl --lib exits 0
- cargo fmt --check exits 0
- cargo clippy -p camel-dsl -- -D warnings exits 0
- `grep -c '"UnsupportedMediaType"\|"NotAcceptable"\|"ProcessorErrorWithSource"' crates/camel-dsl/src/compile.rs` >= 8 (vocab + matcher + guard + tests)

## Task 2 — Vocabulary classification guard test

Files:
- crates/camel-dsl/src/compile.rs (modified, tests only)
- crates/camel-api/src/error.rs (modified, tests only)

Steps:
1. In the compile.rs tests mod, add:
   `const ALL_CAMEL_ERROR_VARIANTS: &[&str]` listing the 25 variant ids
   (source of truth: the enum arms in crates/camel-api/src/error.rs):
   AlreadyConsumed, AuthProviderUnavailable, ChannelClosed, CircuitOpen,
   ComponentNotFound, Config, ConfigValidation, ConsumerStopping,
   DeadLetterChannelFailed, EndpointCreationFailed, EndpointUri,
   HttpOperationFailed, InvalidUri, Io, NotAcceptable, ProcessorError,
   ProcessorErrorWithSource, RouteError, StreamLimitExceeded,
   TemplateReload, TypeConversionFailed, Unauthenticated, Unauthorized,
   UnsupportedMediaType, ValidationError.
   `const STARTUP_UNMATCHABLE_VARIANTS: &[&str] = &["ConfigValidation", "EndpointUri"];`
   `const DEFERRED_VARIANTS: &[&str] = &["TemplateReload"];`
2. Add test `test_exception_kind_vocabulary_classification_guard`:
   - assert every entry of `supported_exception_kinds()` is in
     `ALL_CAMEL_ERROR_VARIANTS` (no pseudo-kinds — bd rc-5u8co stale-note
     resolution);
   - assert every variant in `ALL_CAMEL_ERROR_VARIANTS` is in exactly one
     of: `supported_exception_kinds()`, `STARTUP_UNMATCHABLE_VARIANTS`,
     `DEFERRED_VARIANTS` (disjoint union, no dupes across the three sets);
   - assert `ALL_CAMEL_ERROR_VARIANTS.len() == 25`.
   Include a comment: adding a CamelError variant requires extending this
   table and the exhaustive `variant_name_covers_all_variants` test in
   crates/camel-api/src/error.rs (bd rc-5u8co) — reviewed manual step.
3. In crates/camel-api/src/error.rs, extend the existing test
   `variant_name_covers_all_variants` (line ~554): add an assertion that
   its table has exactly 25 entries, and a comment cross-referencing the
   camel-dsl classification guard (when adding a variant, update
   `test_exception_kind_vocabulary_classification_guard` in
   crates/camel-dsl/src/compile.rs and make the register-or-document
   decision).

Tests:
- name: test_exception_kind_vocabulary_classification_guard
  setup: Task 1 landed (vocab includes the three new kinds)
  action: walk vocab vs ALL_CAMEL_ERROR_VARIANTS vs the two classification sets
  assert: vocab ⊆ variants; variants == disjoint union of (vocab, startup, deferred); len 25
  command: cargo test -p camel-dsl --lib test_exception_kind_vocabulary_classification_guard
  expected: fails before Task 1 (PEWS/UMT/NA unclassified), passes after Tasks 1+2
- name: variant_name_covers_all_variants (extended)
  action: existing exhaustive walk + new len==25 assertion
  assert: table length 25
  command: cargo test -p camel-api --lib variant_name_covers_all_variants
  expected: passes before AND after (25 variants unchanged by this change)

Acceptance:
- cargo test -p camel-dsl --lib exits 0
- cargo test -p camel-api --lib variant_name_covers_all_variants exits 0
- cargo fmt --check exits 0; cargo clippy -p camel-dsl -p camel-api -- -D warnings exits 0

## Task 3 — File bds for the controversial variants

Files:
- (no repo files; bd operations from repo root /home/kenny/dev/rust-camel)

Steps:
1. File bd for TemplateReload matchability:
   `bd create "TemplateReload not matchable by on_exceptions kind — decide registration vs documented exclusion" --description="Audit rc-5u8co classified TemplateReload as deferred. Raised via From<TemplateReloadError> at endpoint creation (startup, camel-template/src/component.rs) and by TemplateReloadRegistry::reload_route stale-generation/timeout paths invoked from lifecycle reload commands (camel-core runtime_bus.rs / RouteLifecycleCommand::Reload) — these bypass route error handlers. Open question: whether any per-exchange render path also surfaces TemplateReload in-pipeline; if yes, register it (24ba1ee9 pattern) with vocab + matcher arm + tests; if no, move it to STARTUP_UNMATCHABLE_VARIANTS in the classification guard and document in the supported_exception_kinds doc comment." -t task -p 3 --deps discovered-from:rc-5u8co --json`
2. File bd for EndpointUri/InvalidUri consistency:
   `bd create "EndpointUri vs InvalidUri on_exceptions vocabulary consistency" --description="Audit rc-5u8co: EndpointUri raised at YAML parse (camel-dsl/src/yaml.rs:811 DuplicateKey) and endpoint build (startup fail-fast) — classified intentionally unmatchable. But InvalidUri IS in the vocabulary while being the same startup-family kind. Decide: either document why InvalidUri is special (any runtime raise path? dynamic endpoint creation?) or remove/keep both consistently; update classification guard + doc comment accordingly." -t task -p 3 --deps discovered-from:rc-5u8co --json`
3. Record both new bd ids in the task result for the park report.

Tests:
- name: bd-filings-created
  setup: bd CLI available at repo root
  action: run the two bd create commands
  assert: both commands exit 0 and return JSON with distinct ids; `bd show <id> --json` shows dependency discovered-from:rc-5u8co
  command: bd show <id> --json (per created id)
  expected: both bds exist, linked to rc-5u8co

Acceptance:
- Two bd ids exist, both with discovered-from:rc-5u8co dependencies
- No code changes in this task (repo diff unaffected)

- [x] task-1-register-kinds
- [x] task-2-classification-guard
- [x] task-3-bd-filings
