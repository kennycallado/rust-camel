# Design: fix-job-multiroute-startup

## Approach

Replace the per-route `with_auto_startup(is_target)` map in `run_job` with
`with_auto_startup(true)` for every discovered route. A route configured
`autoStartup: false` in the document is also forced on. A job document is
a closed composition, and the load gate is the safety boundary.

Update the module doc comment and the inline rationale in
`crates/camel-cli/src/commands/job/mod.rs`: side-effect safety comes from
the fail-closed consumer allowlist plus the single send, not from startup
suppression.

Keep `target_route_ids` and both load checks exactly as they are. With all
routes started, two consumer routes on one base would round-robin both the
target send and any `to:` hops. The check stays load-time and fail-closed.
Update only its doc-comment rationale in `document.rs`.

Tests: the two-route hop test configures the helper route with
`autoStartup: false`, proving the forcing. A three-route variant adds an
unrelated `seda:` consumer route whose mock-endpoint output proves its
consumer started.

## Affected crates

- camel-cli: `src/commands/job/mod.rs` (startup map, comments),
  `src/commands/job/document.rs` (doc comment on `target_route_ids`),
  `tests/job_one_shot_test.rs` (two-route hop test, non-target-start
  evidence, single-route regression stays covered by the existing suite).

## Architecture boundaries

CLI layer only. No camel-core, component, or DSL change. Route startup
already flows through the `camel run` seams; the job only changes the
`auto_startup` value it stamps before `add_route_definition`. References:
ADR-0069 (job runner boot path), CONTEXT-MAP "camel job" zone.

## Alternatives considered

- Start only routes reachable from the target (graph walk over `to:` URIs):
  rejected. The allowlist already guarantees internality, and reachability
  analysis adds machinery with no extra safety.
- Keep suppression, register `direct:` endpoints eagerly outside `start()`:
  rejected. It would touch camel-direct or camel-core, which the zone lease
  forbids, and it would start consumers implicitly.
