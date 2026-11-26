# Design: transacted-canonical-walk

## Approach

The structural walk already exists as ONE visitor:
`camel_core::transacted_walk::walk_block`
(`crates/camel-core/src/transacted_walk.rs:160`) — the single
structural + per-arm list over the lowered `BuilderStep` tree. Three public
wrappers thread resolvers into it; the mission's two fan-ins both call it:

- **Declarative fan-in**: `camel-dsl/src/compile.rs:1442`
  (`walk_transacted_block_deferred(&built, resolve_manager)`) in the
  declarative `Transacted` lowering arm; the camel-dsl canonical-dark arm
  (`compile.rs:668`) calls `walk_transacted_block_deferred_unresolved`.
- **Canonical fan-in**: `camel-core/src/lifecycle/application/commands.rs:1373`
  (`walk_transacted_block(&built, &manager_resolver, &url_resolver)`) in
  `canonical_step_to_builder_step_at`'s `Transacted` arm, depth-0 only,
  inside `canonical_to_route_definition` — which the register command and
  journal replay call BEFORE `execution.register_route`, so the failure is a
  load failure with the typed `CamelError::Transaction(..)` shape, not a
  route-add runtime error.

This change adds NO production wiring. It pins the property set with tests,
replaces the ADR-0101 paragraph, and writes the spec canon. Test placement:

- Tests 1–3 (`canonical_block_rejects_parallel_split`,
  `canonical_block_rejects_wiretap_and_multicast`,
  `canonical_block_rejects_nested_and_unnamed`):
  `crates/camel-core/src/lifecycle/application/commands_tests.rs` (wired via
  `#[path]` at `commands.rs:1390`). They lower a canonical
  `CanonicalStepSpec::Transacted` block through
  `canonical_step_to_builder_step_at` with a REGISTERED fake manager
  (`registry_with_pg()` and the `WalkSeamManager` fixture already in that
  file; `fake_manager` is private to the walk tests) — proving the
  structural rejection wins over `ManagerMissing`, i.e. exactly the
  slice-2-manager incident the mission describes. Multicast has no
  `CanonicalStepSpec` variant (`#[non_exhaustive]` enum with a fail-closed
  `_` arm), so its half asserts the shared walk rejects
  `BuilderStep::Multicast` in-block — the same function the canonical fan-in
  calls. The wiretap half also pins the callee-side datasource identity
  (`static_step_uri`, transacted_walk.rs:477-486, covers `WireTap`):
  a wiretap to a foreign-datasource sql URI yields `ForeignDatasource`
  (slice-1 minor, closed by the shared walk).
- Test 4 (`canonical_block_dynamic_uri_is_rejected_at_load`):
  `crates/camel-core/src/transacted_walk.rs` test module, next to the
  existing `walk_rejects_dynamic_targets_in_block`. Decision: **reject at
  load**, never membership proof — a recipient-list/routing-slip target is
  chosen by an expression evaluated at exchange time, so block membership is
  not statically provable; the frozen requirement defaults fail-closed. The
  canonical enum cannot even express a dynamic target (authoring-side
  fail-closed); the test pins the walk-side rejection through the SAME entry
  and resolver shape the existing `walk_rejects_dynamic_targets_in_block`
  uses, and exists separately only because the mission names it and the
  canonical entry cannot construct the input. Mutation check for this test
  is its own: neutralize the walk's dynamic-target arm
  (transacted_walk.rs:347-355), the test MUST fail.
- Test 5 (`declarative_and_canonical_share_one_walk`):
  `crates/camel-dsl` compile test module. Structural half: `include_str!`
  source assertions — camel-dsl `compile.rs` contains
  `camel_core::transacted_walk::walk_transacted_block_deferred`, and
  camel-core `commands.rs` (read textually via the sibling-crate relative
  path; no build dependency) contains
  `transacted_walk::walk_transacted_block` — both entries call the same
  module, no second copy of the list over the lowered step tree. Behavioural
  half: the same offending block —
  a recipient list inside a transacted block — yields the SAME typed variant
  (`DynamicTargetUnprovable { step: "recipient-list" }`) from the declarative
  compile entry and from `camel_core::transacted_walk::walk_transacted_block`
  directly. The dynamic-target case is chosen precisely because the S1 AST
  validator defers it (`transacted_validation.rs:316-318`): a
  `ParallelInBlock` would be caught by the AST gate first and would not prove
  the lowered walk ran.

**Pre-fix evidence.** The wrong behaviour this mission documents (canonical
blocks skipping the walk) cannot be reproduced on `main` — the wiring landed
with `sqltrans-exec` (archived 2026-11-22). Recorded equivalent: with the
commands.rs walk call temporarily commented out (scratch mutation, never
committed), tests 1–3 MUST fail (they lower through
`canonical_step_to_builder_step_at` at depth 0, which still reaches the
`commands.rs:1373` walk call; the multicast half of test 2 calls the walk
directly and is unaffected, but test 2 still fails through its wiretap
halves (a)/(b)); test 4 does not traverse that call, so it
gets its own mutation (neutralize transacted_walk.rs:347-355, test 4 MUST
fail). Outputs recorded in the park report. This
proves the tests detect exactly the regression class the mission names.

**ADR-0101 paragraph.** The softened sentences (S1 section, current lines
191–200) are replaced — this is new wording, not a restoration (ADR-0101 has
a single commit, `059aedd7b`); the replacement states what is true of main
and names the one condition that remains. Per the mission's alternative, the
sentence that remains true is stated exactly: the programmatic
`BuilderStep` seam (`CamelContext::add_route_definition` →
`RuntimeBus::register_route` → `handle_register_internal`) does not run the
registry/catalog-backed walk. A hand-authored `BuilderStep::Transacted`
block skips the structural and per-arm checks there; every pre-lowered
definition skips manager and dialect resolution. Today such a block fails
closed at route-add with `TransactionError::JoinSessionMissing`
(`StepCompilerRegistry::compile_step`, step_compilers/mod.rs:293-306), and
the typed variant is stringified into a route error at the port boundary
(runtime_bus.rs:350-353, commands.rs:292-295). That seam is OUT OF SCOPE
here (no production code changes in this change); bd `rc-ltfyp` (filed
`discovered-from: rc-c3mjp`, `rc-ggo3d` blocked-by it) owns the seam walk.
Verbatim replacement for ADR-0101 §(h) S1 bullet, lines 191–200:

> Those rejections hold on the DECLARATIVE load path and on the canonical
> lowering path through ONE shared load walk over the lowered step tree
> (`sqltrans-exec` step 3; pinned by `transacted-canonical-walk`). A
> `RouteDefinition` registered already lowered
> (`CamelContext::add_route_definition` → `RuntimeBus::register_route` →
> `handle_register_internal`) passes no lowering entry and does not run the
> registry-backed walk: a hand-authored block skips the structural and
> per-arm checks, and every pre-lowered block skips manager and dialect
> resolution. Today such a block fails closed at route-add with the typed
> `Transaction(TransactionError::JoinSessionMissing { datasource })` error,
> surfaced stringified; the executor slice may not remove that backstop
> before the walk runs there (bd rc-ltfyp, blocks `rc-ggo3d`). Bundle boot
> registers one manager per catalog datasource, so a block on a catalog
> datasource passes load; `Managed` keeps the current endpoint-creation
> rejection.

## Affected crates

- `camel-core`: test-only (commands_tests.rs, transacted_walk.rs tests).
- `camel-dsl`: test-only (compile test module, source-assertion test).
- `camel-api`: unchanged (error variants already exist).
- docs/adr: ADR-0101 paragraph replacement (new wording, single-commit file).

## Architecture boundaries

Runtime boundary untouched: no step-compilation, no execution semantics.
DSL boundary: tests only; the declarative validator
(`camel_dsl::transacted_validation`) stays as the AST-level gate, the
camel-core walk stays the lowered-tree gate. The hexagonal boundary test is
unaffected (no new cross-crate code dependencies; the cross-crate
`include_str!` in test 5 is a textual read, not a dependency edge).

## Alternatives considered

- **Wire the walk into `RuntimeBus::register_route`** (context fan-in) in
  this change: rejected — the mission order scopes the two lowering fan-ins;
  the registry/catalog access and error-shape mapping
  (`DomainError` stringification) deserve their own blessed slice
  (bd `rc-ltfyp`, which blocks `rc-ggo3d` so the executor slice cannot
  remove the `JoinSessionMissing` backstop first). Recorded, not silently
  dropped.
- **Membership proof for dynamic targets**: rejected — routing expressions
  evaluate at exchange time; no load-time proof exists. Reject-at-load is
  the frozen requirement's fail-closed default.
- **Re-run the S1 AST validator on the canonical path**: rejected — it
  operates on `DeclarativeRoute`, which the canonical path never builds;
  the shared `BuilderStep` walk is the single-site mechanism.
