# Proposal: fix-job-multiroute-startup

## Why

`camel job` force-stops every route except the send target
(`with_auto_startup(is_target)` in `crates/camel-cli/src/commands/job/mod.rs`).
A `DirectConsumer` registers its endpoint only inside `start()`. A producer
fails closed on an unregistered endpoint. A target route that hops with
`to: direct:enrich` therefore fails when the `enrich` route never started.
Multi-route composition in one job document is broken today. All existing
job tests are single-route, so the gap is untested.

The load-time consumer gate already rejects every `from:` scheme outside
`{direct, seda, log, mock}`. Every consumer entry point in a job document
is internal by construction; producer and sink `to:` endpoints remain
unrestricted. Force-stopping non-target routes adds no external safety. It
only breaks in-process composition.

bd: rc-itvbd (P2, discovered-from rc-pjm4).

## What Changes

- Start ALL routes of the job document, including routes configured
  `autoStartup: false`. The send target stays the single entry point.
- Keep the missing-target and ambiguous-target load checks unchanged.
- Amend the cli-jobs requirement "one-shot send with side-effect-safe route
  startup": the scenario "only the target route starts" becomes "all
  document routes start", and a new scenario covers a `to: direct:` hop
  end to end.
- Excluded: the consumer allowlist, the `seda:`
  `waitForTaskToComplete=Always` rewrite, and the send-target validation
  rules do not change. No other crate changes.

## Acceptance criteria

- A two-route document whose helper route is configured `autoStartup:
  false` still hops `to: direct:enrich` from the target route and
  completes end to end. It fails today with "no consumer registered".
  This proves the `autoStartup: false` helper was forced on.
- A three-route document (target, hop helper, unrelated `seda:`
  consumer) completes, and the unrelated `seda:` route's consumer is
  proven started by mock-endpoint evidence it produces.
- Single-route documents keep behavior and exit codes unchanged. All
  existing job tests stay green.

## Risk budget

- Low risk. The change removes a stop; it adds no machinery. The worst
  case is a document whose extra routes now consume from internal
  endpoints. Every such consumer passed the load gate. Out of bounds: any
  change outside `crates/camel-cli/src/commands/job/` and the cli-jobs
  spec.
