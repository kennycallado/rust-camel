# Tasks — sedamatch

Single delivery group. Execute in order: 1 → 2 → 3 → 4 (tasks 3 and 4
depend on 1+2; run sequentially). All paths are repo-relative. The
blessed specs live in openspec/changes/sedamatch/specs/ (seda-component,
error-taxonomy, dsl deltas). The design rationale, forgery analysis, and
test matrix live in design.md — read both before starting any task.

## Task 1 — camel-api: OpaqueErrorSource + EndpointCreationFailedWithSource + manual Clone

Files:
- crates/camel-api/src/error.rs (modified)
- crates/camel-api/src/lib.rs (modified — REQUIRED: line ~83 is an explicit re-export list; add `OpaqueErrorSource` alongside `CamelError`; tasks 2-4 depend on `camel_api::OpaqueErrorSource`)

Steps:
1. Add `pub struct OpaqueErrorSource(Arc<dyn std::error::Error + Send + Sync>)` in error.rs near `CamelError`. Private tuple field. Doc comment states the opacity contract: pointee-only `source()`, no public `Clone`, inner handle private, provenance not extractable outside camel-api (short of `unsafe`).
2. Implement for `OpaqueErrorSource`: `pub fn new(source: Arc<dyn std::error::Error + Send + Sync>) -> Self`; `impl fmt::Display` delegating to the pointee's Display; `impl std::error::Error` with `fn source(&self) -> Option<&(dyn std::error::Error + 'static)> { Some(self.0.as_ref()) }` (pointee directly — NO Arc wrapper hop); `#[derive(Debug)]` on the struct (Arc<dyn Error> is Debug). Add a PRIVATE method `fn clone_handle(&self) -> Self { Self(Arc::clone(&self.0)) }` (no `pub`, no `Clone` trait impl anywhere).
3. Add the variant to `CamelError` directly after `EndpointCreationFailed`:
   `#[error("Endpoint creation failed: {0}")] EndpointCreationFailedWithSource(String, #[source] OpaqueErrorSource),` with a doc comment mirroring `ProcessorErrorWithSource`'s ("preserves the source error chain for downstream inspection (e.g. typed gate-rejection classification)").
4. Replace `#[derive(Debug, Clone, Error)]` on `CamelError` with `#[derive(Debug, Error)]` and add a manual `impl Clone for CamelError` in error.rs below the enum: one match arm per variant cloning fields normally; the new variant arm uses `OpaqueErrorSource::clone_handle` via crate-private access. Comment: exhaustive like `variant_name()` — a new variant without an arm fails compilation.
5. Extend `variant_name()`: `Self::EndpointCreationFailedWithSource(_, _) => "EndpointCreationFailed",` (alias). Extend `classify()`: group into the `"endpoint"` arm with `Self::EndpointCreationFailed(_)`. Both matches are exhaustive — the compiler forces these arms.
6. Extend the `variant_name_covers_all_variants` test table (~error.rs:544-652): add the case `(EndpointCreationFailedWithSource sample, "EndpointCreationFailed")` and bump the table-length assertion from 25 to 26 (~lines 637-642). This table is the inventory-currency anchor the dsl guard scenario names — do not leave it stale.
7. Extend the existing `all_error_samples()` test helper (error.rs:285) with a sample: `CamelError::EndpointCreationFailedWithSource("x".into(), OpaqueErrorSource::new(Arc::new(SampleSource)))` where `SampleSource` is a minimal test type defined in the test module: `#[derive(Debug)] struct SampleSource;` with `impl fmt::Display` (writes `"sample source"`) and `impl std::error::Error`.
8. Add the tests listed under Tests.
9. Add the two `compile_fail` doctests to `OpaqueErrorSource`'s doc comment.

Tests (all in the existing error.rs test module unless noted):
- name: `opaque_error_source_exposes_only_pointee`
  setup: `let src = OpaqueErrorSource::new(Arc::new(SampleSource));`
  action: `let pointee = src.source().unwrap(); pointee.downcast_ref::<SampleSource>()`
  assert: `is_some()` (pointee reachable directly, no Arc wrapper hop).
  command: `cargo test -p camel-api --lib opaque_error_source_exposes_only_pointee`
  expected: RED before step 2 (type absent), GREEN after.
- name: `endpoint_creation_failed_with_source_aliases_to_plain`
  setup: construct `CamelError::EndpointCreationFailedWithSource("d".into(), OpaqueErrorSource::new(Arc::new(SampleSource)))`.
  action: `e.variant_name()`, `e.classify()`, `e.to_string()`.
  assert: `"EndpointCreationFailed"`, `"endpoint"`, `"Endpoint creation failed: d"`.
  command: `cargo test -p camel-api --lib endpoint_creation_failed_with_source_aliases_to_plain`
  expected: RED before steps 3+5, GREEN after.
- name: `clone_preserves_variant_identity_for_all_error_samples`
  setup: `for e in all_error_samples()` (now including the new variant).
  action: `let c = e.clone();` compare `c.variant_name()`, `c.classify()`, `c.to_string()` to the original's.
  assert: equal for every sample (manual-Clone regression gate; catches a wrong-arm mapping).
  command: `cargo test -p camel-api --lib clone_preserves_variant_identity`
  expected: RED before step 4 (Clone via derive is absent once derive removed — or compile error), GREEN after.
- name: `camel_error_clone_preserves_source_provenance`
  setup: `let e = EndpointCreationFailedWithSource("d".into(), OpaqueErrorSource::new(Arc::new(SampleSource)))`.
  action: `let c = e.clone();` then walk `c.source().unwrap().downcast_ref::<SampleSource>()`.
  assert: `is_some()` (clone arc-clones the inner — provenance survives).
  command: `cargo test -p camel-api --lib camel_error_clone_preserves_source_provenance`
  expected: GREEN after steps 2-4.
- doctest name: `opaque_error_source_is_not_cloneable` (compile_fail)
  doc example body: `let s = OpaqueErrorSource::new(std::sync::Arc::new(MyError)); let _ = s.clone();` with a preceding line defining `MyError` as a minimal Display+Error type in the example.
  assert: does not compile (no public `Clone`).
  command: `cargo test -p camel-api --doc OpaqueErrorSource`
  expected: the doctest is marked compile_fail and "passes" by failing compilation, GREEN from landing. The filter is case-sensitive against the doc section title (`OpaqueErrorSource`), not snake_case.
- doctest name: `opaque_error_source_inner_is_not_destructurable` (compile_fail)
  doc example body: `let OpaqueErrorSource(inner) = OpaqueErrorSource::new(std::sync::Arc::new(MyError));` with `MyError` defined in the example.
  assert: does not compile (private field E0616 from an external position).
  command: `cargo test -p camel-api --doc OpaqueErrorSource`
  expected: compile_fail, GREEN from landing. After both doctests land, `cargo test -p camel-api --doc OpaqueErrorSource` reports exactly 2 executed doctests — a zero-test pass means the filter is wrong or the examples are outside the type's doc comment; fix before proceeding.

Acceptance:
- `cargo test -p camel-api --lib` exits 0.
- `cargo test -p camel-api --doc OpaqueErrorSource` exits 0 and reports exactly 2 doctests executed.
- `cargo clippy -p camel-api --all-targets -- -D warnings` exits 0.
- Spec coverage: error-taxonomy scenarios "aliases match the plain variant", "doTry catch-by-variant still catches" (mechanism = variant_name alias asserted above), "source chain remains inspectable without extraction", "cloning the error preserves provenance".

- [x] sedamatch-1

## Task 2 — seda: crate-private marker, constructor, bounded classification, three-site rewire

Files:
- crates/components/camel-component-seda/src/lib.rs (modified)

Steps:
1. Add `#[derive(Debug, Clone, PartialEq, Eq)] enum NoActiveConsumersGate { Single, Fanout }` (CRATE-PRIVATE, near the existing `is_no_active_consumers_gate` at ~line 546). `impl fmt::Display`: `"seda no-active-consumers gate rejection (single mode)"` / `"seda no-active-consumers gate rejection (fanout mode)"` — deliberately NON-canonical diagnostic text. `impl std::error::Error for NoActiveConsumersGate {}`. Doc comment: provenance marker; Display never participates in classification and never equals a canonical gate message.
2. Add `impl NoActiveConsumersGate { fn rejection(&self, endpoint_name: &str) -> CamelError }` (crate-private) returning `CamelError::EndpointCreationFailedWithSource(detail, OpaqueErrorSource::new(Arc::new(self.clone())))` — the marker embedded is the method's own kind (`self`), Single or Fanout, never a hardcoded kind. Build `detail` via a `fn detail(&self, endpoint_name: &str) -> String` returning `format!("SEDA endpoint '{endpoint_name}' has no active consumers")` for Single and the `has no active subscribers` wording for Fanout; `rejection` calls `self.detail(...)`. Import `OpaqueErrorSource` via the camel_api path CamelError already arrives through.
3. Add the three crate-private site functions delegating to `rejection`: `fn single_mode_gate_rejection(name: &str) -> CamelError` (Single kind), `fn fanout_preenqueue_gate_rejection(name: &str) -> CamelError` (Fanout kind), `fn fanout_subscriber_list_gate_rejection(name: &str) -> CamelError` (Fanout kind).
4. Rewire the three production sites to call the site functions: the single/fanout mode match at the producer entry (~lines 1100-1110: `SedaMode::Single` arm → `single_mode_gate_rejection(&state.config.name)`, `SedaMode::Fanout` arm → `fanout_preenqueue_gate_rejection(&state.config.name)`), and the fanout subscriber-list empty branch (~line 1196: `fanout_subscriber_list_gate_rejection(&state.config.name)`). Delete the inline `format!` blocks. Keep the surrounding comments; update them to cite the typed-provenance doctrine (rc-3px7o) instead of wording-matching.
5. Add `const MAX_SOURCE_HOPS: usize = 8;` and `fn unwrap_arc_dyn_error<'a>(src: &'a (dyn std::error::Error + 'static)) -> &'a (dyn std::error::Error + 'static)` — copy the camel-redis transport_error.rs helper semantics (downcast_ref::<Arc<dyn Error + Send + Sync>> → &**arc, else identity) with a citation comment to db512039.
6. Add `fn gate_from_error(err: &CamelError) -> Option<NoActiveConsumersGate>`: return None unless the error is `EndpointCreationFailedWithSource`; starting from the variant's `OpaqueErrorSource` pointee (`err.source()` walk from the error itself is equivalent — probe via the matched field), loop up to MAX_SOURCE_HOPS hops: unwrap_arc_dyn_error, try `downcast_ref::<NoActiveConsumersGate>()` (clone it out), else follow `.source()`. Marker at hop depth 1..=8 → Some; deeper or absent → None.
7. Rewrite `is_no_active_consumers_gate` to `gate_from_error(err).is_some()` and update its doc comment: classification is by typed provenance in the source chain (bounded 8-hop walk); text never classifies; cite rc-3px7o and the retryclass/rediserr doctrine.
8. Rewrite `is_direct_startup_race` to: `match err { CamelError::EndpointCreationFailed(_) => true, CamelError::EndpointCreationFailedWithSource(..) => !is_no_active_consumers_gate(err), _ => false }` — keep its doc contract (non-gate endpoint-creation failure is retryable) and update the rc-utx98/rc-fr20u comment block to the typed form.
9. Convert ALL behavioral wording assertions from `.contains("no active consumers")` to byte-exact equality — there are five: two producer-path tests (~lines 1603 and 1763) plus `producer_gate_wording_single_mode` / `producer_gate_wording_fanout_mode` (~lines 2509 and 2532) and the wording test at ~line 3523. Each becomes `assert_eq!` of the captured error's matched payload against the exact canonical string for that test's endpoint name (`SEDA endpoint '<name>' has no active consumers` for single mode, `SEDA endpoint '<name>' has no active subscribers` for fanout mode).
10. Rewrite the three existing tests that assert plain-literal gate wordings classify as gates (they WILL flip and fail under typed classification):
    - `gate_predicate_matches_both_modes_only` (~line 2471): its positive rows use plain `EndpointCreationFailed` literals — replace the whole test with a version asserting the same canonical strings as PLAIN variants now classify `is_no_active_consumers_gate == false` / `is_direct_startup_race == true` (superseded by `foreign_text_never_classifies_gate` rows c/d — fold those rows in and delete the redundant positive assertions), keeping the non-EndpointCreationFailed negative rows.
    - `seda_gate_single_fails_fast` (~line 2571) and `seda_gate_fanout_fails_fast` (~line 2581): they fabricate gates from plain literals; repoint their gate construction to the behavioral capture pattern (SedaComponent endpoint with no consumer, as `startup_retry_pipeline_tests.rs` does) so the genuine typed gate still asserts fail-fast.
11. Add the tests listed under Tests to the existing test module.

Tests (all in the existing seda lib.rs test module):
- name: `gate_rejection_single_round_trip`
  setup: `let e = single_mode_gate_rejection("q");`
  action: `is_no_active_consumers_gate(&e)`, `is_direct_startup_race(&e)`, `e.to_string()`.
  assert: true; false; `"Endpoint creation failed: SEDA endpoint 'q' has no active consumers"`.
  command: `cargo test -p camel-component-seda gate_rejection_single_round_trip`
  expected: RED before steps 2-8 (function absent), GREEN after.
- name: `gate_rejection_fanout_round_trip`
  setup: `let e = fanout_preenqueue_gate_rejection("q");`
  action/assert: gate true; race false; Display `"Endpoint creation failed: SEDA endpoint 'q' has no active subscribers"`.
  command: `cargo test -p camel-component-seda gate_rejection_fanout_round_trip`
  expected: RED before, GREEN after.
- name: `gate_rejection_nested_source_chain_classifies_within_bound`
  setup: two wrapper error types (define `struct WrapA;`/`struct WrapB;` with Display+Error, `WrapB::source()` returning the gate marker, `WrapA::source()` returning WrapB) carried as `EndpointCreationFailedWithSource("outer".into(), OpaqueErrorSource::new(Arc::new(WrapA)))`.
  action: `is_no_active_consumers_gate(&e)`, `is_direct_startup_race(&e)`.
  assert: true; false.
  command: `cargo test -p camel-component-seda gate_rejection_nested_source_chain`
  expected: RED before step 6, GREEN after.
- name: `gate_rejection_at_hop_limit_classifies`
  setup: a chain of SEVEN linked wrapper errors (`ChainHop(u16)` whose `source()` returns the next hop; hop 7's source is the marker) carried in `EndpointCreationFailedWithSource` — seven wrappers place the marker at exactly hop depth 8 (1 = the variant's own source, 2-8 = the wrapper chain).
  action/assert: gate true; race false.
  command: `cargo test -p camel-component-seda gate_rejection_at_hop_limit`
  expected: RED before step 6, GREEN after.
- name: `gate_rejection_beyond_hop_limit_stays_retryable`
  setup: same chain shape but EIGHT wrappers before the marker — marker at hop depth 9, beyond the limit.
  action/assert: gate false; race true.
  command: `cargo test -p camel-component-seda gate_rejection_beyond_hop_limit`
  expected: RED before step 6, GREEN after.
- name: `marker_display_is_non_canonical`
  setup: gate error; extract marker via `gate_from_error` (crate-private access) or match the source pointee.
  action: `format!("{}", marker)`.
  assert: equals `"seda no-active-consumers gate rejection (single mode)"` (and the fanout string for the Fanout kind); NOT equal to either canonical gate message.
  command: `cargo test -p camel-component-seda marker_display_is_non_canonical`
  expected: GREEN with steps 1-6.
- name: `foreign_text_never_classifies_gate`
  setup: table of plain `CamelError::EndpointCreationFailed(String)` messages: (a) `"kafka topic 'orders' has no active consumers (broker=1)"`, (b) `"ws channel 'ch' has no active subscribers upstream"`, (c) `"SEDA endpoint 'q' has no active consumers"` (byte-exact single canonical), (d) `"SEDA endpoint 'q' has no active subscribers"` (byte-exact fanout canonical), (e) `"WARNING: SEDA endpoint 'q' has no active consumers (attempt 1)"`, (f) `"SEDA endpoint 'q' has no active consumers and 2 more issues"`, (g) `"seda endpoint 'q' HAS NO ACTIVE CONSUMERS"`, (h) `"SEDA endpoint '' has no active consumers"`.
  action/assert per row: `is_no_active_consumers_gate == false` AND `is_direct_startup_race == true`.
  command: `cargo test -p camel-component-seda foreign_text_never_classifies_gate`
  expected: RED before step 8 for rows a-f and row h (the substring matches regardless of the empty endpoint name, so row h classifies as gate today); row g (uppercase) already passes today — case-sensitive `contains` never matched it. GREEN for all rows after.
- name: `typed_non_gate_source_stays_retryable`
  setup: `EndpointCreationFailedWithSource("foreign".into(), OpaqueErrorSource::new(Arc::new(WrapA)))` (WrapA = non-gate source).
  action/assert: gate false; race true.
  command: `cargo test -p camel-component-seda typed_non_gate_source_stays_retryable`
  expected: RED before step 8, GREEN after.
- name: `other_seda_wordings_stay_retryable`
  setup: table of plain `EndpointCreationFailed`: `"SEDA queue 'q' is full (size=1)"`, `"SEDA producer timeout enqueueing on 'q' (1000ms)"`, `"SEDA fanout timeout on 'q' (1000ms)"`, `"multipleConsumers=true with waitForTaskToComplete != Never is not supported — a single request cannot have N valid replies without aggregator semantics"`.
  action/assert per row: gate false; race true.
  command: `cargo test -p camel-component-seda other_seda_wordings_stay_retryable`
  expected: GREEN before AND after (regression pin).
- name: `single_gate_site_detail_byte_exact`
  setup/action: `let e = single_mode_gate_rejection("site-q");`
  assert: Display `"Endpoint creation failed: SEDA endpoint 'site-q' has no active consumers"`; `is_no_active_consumers_gate(&e)` true.
  command: `cargo test -p camel-component-seda single_gate_site_detail_byte_exact`
  expected: GREEN with steps 2-7.
- name: `fanout_preenqueue_gate_site_detail_byte_exact`
  setup/action: `let e = fanout_preenqueue_gate_rejection("site-q");`
  assert: Display `"Endpoint creation failed: SEDA endpoint 'site-q' has no active subscribers"`; gate true.
  command: `cargo test -p camel-component-seda fanout_preenqueue_gate_site_detail`
  expected: GREEN with steps 2-7.
- name: `fanout_subscriber_list_gate_site_detail_byte_exact`
  setup/action: `let e = fanout_subscriber_list_gate_rejection("site-q");`
  assert: Display `"Endpoint creation failed: SEDA endpoint 'site-q' has no active subscribers"`; gate true.
  command: `cargo test -p camel-component-seda fanout_subscriber_list_gate_site_detail`
  expected: GREEN with steps 2-7.
- Existing tests updated by step 9 (the two behavioral producer wording tests) keep the same test names and now assert equality.

Acceptance:
- `cargo test -p camel-component-seda` exits 0 (full crate, including the rewritten tests from step 10).
- `cargo clippy -p camel-component-seda --all-targets -- -D warnings` exits 0.
- `grep -n 'contains("no active' crates/components/camel-component-seda/src/lib.rs` matches nothing in test assertions (byte-exact equality everywhere per step 9); doc-comment doctrine citations may mention the historical wording.
- Spec coverage: seda-component scenarios "genuine single-mode gate fails fast", "genuine fanout-mode gate fails fast", "gate marker deeper in the source chain still classifies", "marker beyond the walk limit stays retryable", "foreign message containing the consumer wording stays retryable", "foreign message containing the subscriber wording stays retryable", "byte-exact imitation of the consumer wording stays retryable", "byte-exact imitation of the subscriber wording stays retryable", "decorated gate wording stays retryable", "typed endpoint failure without the gate marker stays retryable", "other SEDA endpoint-creation failures stay retryable", "canonical wording and aliases are preserved".

- [x] sedamatch-2

## Task 3 — camel-cli: adversarial classification tests through the job delegation

Files:
- crates/camel-cli/src/commands/job/startup_retry_classification_tests.rs (modified)

Steps:
1. Add the four unit-table tests listed under Tests. They call the existing `is_retryable_startup_failure` (which delegates to `camel_component_seda::is_direct_startup_race`) — no change to job/mod.rs.
2. Add the two behavioral gate tests: reuse the SedaComponent setup pattern already present in `startup_retry_pipeline_tests.rs` (import `SedaComponent` as that file does, register an endpoint with no consumer, capture the producer error) and assert non-retryability through `is_retryable_startup_failure`. Do NOT modify `startup_retry_pipeline_tests.rs` — `gate_fails_fast_with_exactly_one_side_effect` and `fanout_gate_fails_fast_with_exactly_one_side_effect` must keep passing unchanged.
3. Remove or repoint any helper in this file that fabricates gate errors from literal message strings (e.g. `gate_single()`/`gate_fanout()` helpers constructing plain `EndpointCreationFailed` literals): plain literals are now by definition NOT gates — replace their use in fail-fast rows with the behavioral capture from step 2.

Tests:
- name: `foreign_byte_exact_single_imitation_stays_retryable`
  setup: `let e = CamelError::EndpointCreationFailed("SEDA endpoint 'q' has no active consumers".into());`
  action: `is_retryable_startup_failure(&e)`.
  assert: true.
  command: `cargo test -p camel-cli --lib foreign_byte_exact_single_imitation`
  expected: GREEN on landing (task 2 has already landed under the mandated 1→2→3 ordering; the classification flip is proven RED→GREEN by the seda crate's own `foreign_text_never_classifies_gate` in task 2 — these tests pin the consuming delegation).
- name: `foreign_byte_exact_fanout_imitation_stays_retryable`
  setup: same with `"SEDA endpoint 'q' has no active subscribers"`.
  action/assert: retryable true.
  command: `cargo test -p camel-cli --lib foreign_byte_exact_fanout_imitation`
  expected: GREEN on landing (consuming-delegation pin; RED proof lives in task 2).
- name: `foreign_wording_collision_stays_retryable`
  setup: `CamelError::EndpointCreationFailed("kafka topic 'orders' has no active consumers (broker=1)".into())`.
  action/assert: retryable true.
  command: `cargo test -p camel-cli --lib foreign_wording_collision`
  expected: GREEN on landing (consuming-delegation pin; RED proof lives in task 2).
- name: `typed_non_gate_source_stays_retryable`
  setup: `CamelError::EndpointCreationFailedWithSource("foreign".into(), camel_api::OpaqueErrorSource::new(Arc::new(ForeignSource)))` where `ForeignSource` is a minimal `#[derive(Debug)]` struct with Display+Error impls defined locally in the test module.
  action/assert: retryable true.
  command: `cargo test -p camel-cli --lib typed_non_gate_source_stays_retryable`
  expected: GREEN on landing (consuming-delegation pin; RED proof lives in task 2).
- name: `behavioral_single_gate_fails_fast_classification`
  setup: SedaComponent single-mode endpoint registered in a context, no consumer (pattern from startup_retry_pipeline_tests.rs).
  action: capture the producer send error; `is_retryable_startup_failure(&err)`.
  assert: false (non-retryable).
  command: `cargo test -p camel-cli --lib behavioral_single_gate_fails_fast`
  expected: GREEN before AND after task 2 (genuine gates stay non-retryable — regression pin).
- name: `behavioral_fanout_gate_fails_fast_classification`
  setup: fanout endpoint, no subscriber.
  action: capture the producer send error; `is_retryable_startup_failure(&err)`.
  assert: false (non-retryable).
  command: `cargo test -p camel-cli --lib behavioral_fanout_gate_fails_fast`
  expected: GREEN before AND after (regression pin).

Acceptance:
- `cargo test -p camel-cli --lib job` exits 0 (includes the untouched pipeline tests).
- `cargo clippy -p camel-cli -- -D warnings` exits 0.
- `git diff --stat crates/camel-cli/src/commands/job/startup_retry_pipeline_tests.rs` is EMPTY.
- Spec coverage: reinforces seda-component foreign/imitation/typed scenarios at the consuming delegation; jobreplay cli-jobs contract (non-gate stays retryable) validated end-to-end.

- [x] sedamatch-3

## Task 4 — camel-dsl: family grouping for the EndpointCreationFailed kind value

Files:
- crates/camel-dsl/src/compile.rs (modified)

Steps:
1. In `exception_kind_matches` (~line 945), extend the arm: `"EndpointCreationFailed" => matches!(err, CamelError::EndpointCreationFailed(_) | CamelError::EndpointCreationFailedWithSource(..)),` with a comment citing the sedamatch dsl delta: the kind value denotes the endpoint-creation failure family; intentional exception to the variant-exact ProcessorErrorWithSource precedent; no distinct kind value exists.
2. Do NOT add `"EndpointCreationFailedWithSource"` to the kind vocabulary array (`exception_kind_vocabulary`, ~line 904-935) — it must stay an unknown kind.
3. Extend `test_exception_kind_vocabulary_classification_guard` (~lines 2529-2573): the guard asserts a disjoint union over exactly three sets (vocabulary / STARTUP_UNMATCHABLE_VARIANTS / DEFERRED_VARIANTS) with `hits == 1` per variant. Add a FOURTH category, e.g. `GROUPED_UNDER_KIND: &[(&str, &str)]` mapping variant name → the kind value it is grouped under — add `("EndpointCreationFailedWithSource", "EndpointCreationFailed")`. Extend the guard so every variant in the enumerated inventory gets `hits == 1` across the FOUR sets (grouped variants hit the grouped set, not the vocabulary set), and bump both variant-count assertions from 25 to 26 (~lines 2530-2541). Do NOT add the variant to the vocabulary set (it must stay an unknown kind) and do NOT leave it out of the inventory (the guard must classify it — silently omitting it drops the spec scenario and re-triggers `hits == 0` drift detection later).
4. Add the tests listed under Tests.

Tests:
- name: `endpoint_creation_failed_kind_matches_both_variants`
  setup: `let plain = CamelError::EndpointCreationFailed("d".into());` and `let with_src = CamelError::EndpointCreationFailedWithSource("d".into(), camel_api::OpaqueErrorSource::new(Arc::new(DslTestSource)))` where `DslTestSource` is a minimal `#[derive(Debug)]` struct with Display+Error impls defined locally in the test module (or an existing test double already used by compile.rs tests, if one exists).
  action: `exception_kind_matches("EndpointCreationFailed", &plain)` and `exception_kind_matches("EndpointCreationFailed", &with_src)`.
  assert: both true.
  command: `cargo test -p camel-dsl --lib endpoint_creation_failed_kind_matches_both_variants`
  expected: RED before step 1 (source variant not matched), GREEN after.
- name: `endpoint_creation_failed_with_source_kind_value_rejected`
  setup: a route document with `error_handler.on_exceptions: [{kind: "EndpointCreationFailedWithSource", handled: true}]` (follow the exact route-document shape existing unknown-kind tests in compile.rs use — find the existing unknown-kind test and mirror its document).
  action: compile the route.
  assert: compilation fails with the unknown-kind error listing the supported kinds.
  command: `cargo test -p camel-dsl --lib endpoint_creation_failed_with_source_kind_value_rejected`
  expected: GREEN before AND after (no vocabulary entry added — regression pin).

Acceptance:
- `cargo test -p camel-dsl --lib` exits 0 (full crate, includes the guard).
- `cargo clippy -p camel-dsl --all-targets -- -D warnings` exits 0.
- Spec coverage: dsl scenarios "EndpointCreationFailed kind matches both endpoint-creation variants", "no distinct EndpointCreationFailedWithSource kind value", "inventory guard classifies the grouped endpoint-creation variant"; preserved original scenarios verified by the untouched vocabulary/unknown-kind tests.

- [x] sedamatch-4
