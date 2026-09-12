# Proposal: add-job-args-batch

## Why

`camel job` documents are closed: a run cannot receive values from the
caller, and `mode: batch` is reserved and rejected at load. bd rc-6308x
(P3) asks for light scripting: repeatable `--arg` values on the CLI, and a
batch mode that drains queues until empty. The design is sealed by the
e_opus ruling recorded in the bd: `--arg` injects exchange headers at send
time, with no document interpolation syntax; batch is a drain-until-empty
loop.

Depends on fix-job-multiroute-startup: batch worker routes must start.
The old startup suppression would keep every non-target consumer route
stopped, and a drain over stopped queues could never make progress.

## What Changes

- `--arg NAME=VALUE` flag, repeatable, raw strings, applied at send time
  as exchange headers. A CLI value overrides a document `send.headers`
  entry with the same name. When the same name repeats on the command
  line, the last occurrence wins. A malformed pair (no `=`, or an empty
  name) fails as a usage error (exit 2) before any boot.
- `mode: batch` accepted at load. The job sends the same single trigger
  exchange, then drains: it waits until every `seda:` consumer queue of
  the document is empty, then reports `Completed` with mode `batch` and
  exits 0. A batch document with no `seda:` consumer routes completes
  immediately after the trigger send. The mandatory overall `timeout`
  still bounds boot, send, drain, and teardown. Expiry reports `Timeout`
  with exit 2.
- Unknown modes still fail at load exactly as today; the error names both
  accepted values.
- The JSON report gains an optional `shutdown_error` field. A shutdown
  failure after a verdict keeps its detail in the report even when `error`
  is already occupied by the pipeline verdict. Any shutdown failure still
  forces exit 2. (Sweep item rc-zpdme #1 lands through this delta; the
  code change rides in the sweep commit of this mission.)
- Excluded: document interpolation syntax (`${job.args.x}`), batch caps
  (`max-duration`/`max-idle`/`max-messages`), mid-drain failure
  accounting, and signal streams (rc-x72sf).

## Acceptance criteria

- `--arg` single and repeated: headers reach the route, proven by mock
  endpoint evidence.
- `--arg` overrides a colliding document header. A malformed `--arg`
  exits 2 as a usage error.
- A batch document with a fan-out pipeline drains until empty and exits 0.
- A batch document whose worker holds a message in flight for a short
  delay does NOT complete from a stale zero-depth sample: drain completion
  happens only after the worker recorded the exchange.
- Batch combined with `--arg` works. An unknown mode still exits 2 at
  load.
- The current `batch mode is rejected at load` test is replaced by the new
  acceptance tests atomically with the grammar change.

## Risk budget

- Medium. Batch drain reads the existing `camel_queue_depth` metrics gauge
  (250 ms sampling) through a collector registered on the context. No
  other crate changes. Main risks: vacuous drain completion (mitigated by
  preseeding the expected queue set from the document's `seda:` consumer
  bases and requiring two distinct zero samples per queue), and detached
  work the gauge cannot see (documented residual, bounded by the overall
  timeout). Out of bounds: changes outside
  `crates/camel-cli/src/commands/job/` and the cli-jobs spec.
