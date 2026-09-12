# Design: add-job-args-batch

## Approach

Three slices, one phase.

1. `--arg` flag. `JobArgs` gains `args: Vec<(String, String)>` parsed by a
   clap `value_parser`: split at the first `=`, reject a missing `=` or an
   empty name, allow an empty value. Clap renders failures as usage errors
   (exit 2) before any boot. In `send_with_startup_retry`, apply document
   `send.headers` first, then the `--arg` pairs in command-line order, so
   a CLI value overrides a colliding document header and a repeated name
   resolves to the last occurrence.

2. Batch grammar. `parse_job_document` accepts `batch` besides `one-shot`.
   `ExecuteSection.mode` becomes the enum `JobMode { OneShot, Batch }`.
   `JobDocError::BatchReserved` is removed. `UnsupportedMode` text names
   both accepted values. The existing rejection test flips to assert that
   `batch` parses and runs.

3. Batch runtime and drain. Depends on fix-job-multiroute-startup: worker
   routes must start. After the trigger send (same path as one-shot:
   startup retry window, `seda:` `waitForTaskToComplete=Always` rewrite,
   `--arg` headers), batch mode drains: wait until every document `seda:`
   consumer queue is empty, then outcome `Completed`, exit 0. A batch
   document with no `seda:` consumer routes has an empty expected queue
   set and completes immediately after the trigger send. The drain
   observer is a `BatchDepthProbe`. It implements
   `camel_api::MetricsCollector` (records the latest
   `set_queue_depth("seda:<name>", depth)` per queue plus a zero-streak
   counter per queue) and registers through `CamelContext::add_lifecycle`
   (`as_metrics_collector`) before `ctx.start()`. The context metrics cell
   is the shared late-bound `MetricsHandle`. SEDA consumers publish depth
   gauges every 250 ms. AS-BUILT (amended after task 2.4 root-caused a
   false-complete): the gauge does NOT cover route-pipeline residency —
   the fire-and-forget `ConsumerContext::send` drops the forwarder's
   `DepthGuard` claim when the envelope enters the route's internal
   pipeline channel, so the gauge reads 0 while a route holds an exchange
   between dequeue and its next `to: seda:` step. A short zero streak
   therefore false-completes cycling pipelines; the gate is
   `DRAIN_ZERO_SAMPLES_REQUIRED = 10` consecutive post-send zero samples
   (a 2.5 s quiescence window at the 250 ms cadence), sized above the
   sampler cadence so a 2 s-timeout verdict is scheduling-independent.
   KNOWN CEILING: a route with more than 2.5 s between dequeue and its
   next seda re-enqueue deterministically false-completes with outcome
   `Completed`; the sound signal (the route in-flight counter,
   `pub(crate)` in camel-core) is out of this change's zone and is
   tracked as bd rc-cd9y7. Drain counting starts only after the trigger
   send completes: generations observed pre-send never satisfy the drain
   (`reset()`); any nonzero sample resets the streak. Poll cadence is an
   implementation detail, never the completion signal, and never sleeps
   past the deadline. The overall deadline still bounds everything;
   expiry keeps outcome `Timeout`, exit 2. The batch teardown budget is
   the remaining deadline budget only: batch does NOT inherit the
   one-shot `MIN_SHUTDOWN_BUDGET` floor, so teardown cannot run past the
   mandatory overall deadline. On the expiry path the `Timeout` verdict
   already governs exit 2. A trigger-send pipeline failure is `Failed`,
   exit 1, under the unchanged taxonomy. Mid-drain failure accounting
   stays out of scope: metrics callbacks are observations, not
   authoritative outcomes.

Report: `mode` renders `batch`. The optional `shutdown_error` field
carries teardown failure detail when a shutdown failure follows a
recorded verdict. `error` keeps the pipeline or timeout verdict. Any
shutdown failure still forces exit 2.

## Affected crates

- camel-cli: `src/commands/job/mod.rs` (flag, batch branch, probe,
  report), `src/commands/job/document.rs` (mode enum, error text),
  `tests/job_one_shot_test.rs` (new tests, flipped batch test).

## Architecture boundaries

The change reads the metrics observation plane (camel-api
`MetricsCollector`) that components already publish. No core, component,
or DSL change. Zone lease: camel-cli jobs subdir plus the cli-jobs spec.
References: ADR-0069 (execute section), CONTEXT-MAP "camel job".

## Alternatives considered

- Scenario-tier settle (mock `received_count` stability): mock-only, no
  SEDA coverage. Rejected.
- `SedaComponent::has_active_consumer`: reports liveness, not backlog.
  Rejected.
- Mid-drain failure counting via `increment_errors`: metrics are
  observations, not outcomes (pre-flight ruling). Deferred until a
  completion-result seam exists.

## Test design notes

- The delayed-worker test proves the drain cannot complete from stale
  zero samples: a worker route holds the exchange in flight for a short
  delay before recording it to a `mock:` endpoint. The job must not exit
  before the mock evidence exists.
