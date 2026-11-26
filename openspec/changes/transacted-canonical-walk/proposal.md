# Proposal: transacted-canonical-walk

## Why

bd `rc-c3mjp` (p1, blocks the executor slice `rc-ggo3d`): the transacted
structural walk must hold on EVERY lowering entry point, or a block authored
through the canonical/programmatic path skips every load-time rejection the
declarative path enforces (parallel-split, multicast, wiretap, async, seda,
StreamList, foreign datasource, unnamed datasource, nested block).

State verified on `main` (`d84bfd804`): the shared walk landed with
`sqltrans-exec` step 3. Both lowering fan-ins call the ONE visitor in
`camel-core/src/transacted_walk.rs` — the declarative lowering
(`camel-dsl/src/compile.rs:1442` `walk_transacted_block_deferred`) and the
canonical lowering (`camel-core/src/lifecycle/application/commands.rs:1373`
`walk_transacted_block`, registry- and catalog-backed). The canonical fan-in
runs inside `canonical_to_route_definition`, before runtime registration, so
rejection is at load with the typed error shape.

What main does NOT yet have — and what this change delivers:

1. Most of the mission's five named tests do not exist. Two canonical-entry
   tests already pin two rejections
   (`canonical_fan_in_rejects_transaction_control_sql`,
   `canonical_fan_in_nested_block_reports_nested_not_manager_missing`); the
   canonical-entry rejections for parallel split, wiretap and multicast,
   foreign/unnamed datasource, StreamList and async/seda are unpinned
   (multicast has no canonical variant to test through lowering), and the
   sharing property (both entries call the same walk — no second copy of the
   list over the lowered step tree) is unpinned.
2. ADR-0101 still carries the slice-1 softened paragraph ("they do NOT run
   the structural walk; covering them at the lowering fan-in is a BLOCKING
   obligation of S2") — half false on main: false for the canonical
   lowering, still true for the programmatic `RouteDefinition` seam. There
   is no unsoftened original to restore (ADR-0101 has a single commit), so
   the paragraph is replaced with wording that is true of main, stating
   exactly which sentence remains true for the seam.
3. The spec canon (`openspec/specs/sql-transactions/spec.md`) nowhere states
   that load-time rejection applies to every lowering entry point, and the
   dynamic-target fail-closed rule (recipient list / routing slip / dynamic
   router inside a block) is not pinned as a load rejection — the frozen
   parent scenario (`openspec/changes/sqltrans/specs/sql-transactions/spec.md:246-250`)
   was implemented by the shared walk but never landed in the canon.
4. The mission's "pre-fix runs must fail" evidence cannot be produced on main
   (the wiring already landed); this change records the equivalent evidence:
   the named tests are mutation-checked against a temporarily bypassed walk
   and MUST fail there, outputs recorded.

## What Changes

- `camel-core` (`lifecycle/application/commands_tests.rs`,
  `transacted_walk.rs` tests): add the five mission-named tests pinning
  canonical-entry rejection and walk sharing. No production code changes —
  the walk is already wired at both fan-ins.
- `camel-dsl` (compile tests): the `declarative_and_canonical_share_one_walk`
  structural assertion (source-presence of both call sites + behavioural
  parity of the typed error).
- `docs/adr/0101-transacted-block-surface.md`: replace the softened S1
  paragraph with wording that is true of main; state exactly which sentence
  remains true for the programmatic `BuilderStep` seam
  (`CamelContext::add_route_definition`), which stays outside this change
  (bd follow-up `rc-ltfyp`, filed `discovered-from: rc-c3mjp`, with
  `rc-ggo3d` blocked-by it).
- `openspec/specs/sql-transactions/spec.md` (delta): load-time rejection
  applies to every lowering entry point; dynamic targets in a block are a
  load rejection; one shared walk, no duplicated list.

Explicitly excluded: wiring the walk into the context/programmatic fan-in
(`RuntimeBus::register_route` — bd `rc-ltfyp`, which blocks `rc-ggo3d`), any
change to the walk's structural set, and any runtime execution behaviour.
The dynamic-URI decision is reject-at-load — already implemented in the
shared walk; this change pins it. The slice-1 wiretap callee-side minor
(foreign-datasource check must see the callee side) is closed by the shared
walk's `static_step_uri` coverage of `WireTap` and is pinned by test 2.

## Acceptance criteria

- The five named tests exist and pass: `canonical_block_rejects_parallel_split`,
  `canonical_block_rejects_wiretap_and_multicast`,
  `canonical_block_rejects_nested_and_unnamed`,
  `canonical_block_dynamic_uri_is_rejected_at_load`,
  `declarative_and_canonical_share_one_walk`.
- Mutation evidence recorded: with the canonical fan-in's walk call
  temporarily bypassed, tests 1–3 fail (the documented pre-fix wrong
  behaviour); test 4 is mutation-checked separately by neutralizing the
  walk's dynamic-target arm; both mutations reverted afterwards.
- bd `rc-ltfyp` exists (`discovered-from: rc-c3mjp`, `rc-ggo3d` blocked-by
  it) before archive.
- ADR-0101 carries wording true of main plus the exact remaining sentence
  for the programmatic seam.
- `openspec validate transacted-canonical-walk --type change --json` passes.
- Affected gates green: `cargo fmt --check --all`, `cargo build --workspace`,
  clippy registry, `cargo test -p camel-core`, `cargo test -p camel-dsl`,
  `cargo test --workspace --lib`, hexagonal boundary test,
  `bash scripts/gates-local.sh`.

## Risk budget

Acceptable: doc-only drift risk in ADR wording; test-only crate changes.
Out of bounds: new error variants, walk structural-set changes, runtime
execution changes, touching the builder/context fan-in in this change.
