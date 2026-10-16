# Proposal: master-start-error-classification

## Why

When the delegate of a `master:` route fails after it was created, the Master leadership task treats the error as fatal without classification. A transient error is not retried. Any error ends the task through `return Err(err)` and skips the graceful tail, so the awaited lease `step_down` never runs; on real platforms the lease is only cancelled by the handle drop and nothing waits for its termination. Construction errors are already classified and retried under the reconnect budget; start and run failures are not. Bd: rc-77wln. Sibling of rc-w02ns and rc-8yc0p; this change does not replace the rc-8yc0p fix in camel-jms.

## What Changes

- In camel-master, classify the error of a finished delegate task with `is_retryable_camel_error`.
- Retry a transient failure through the existing acquisition branch, under the existing reconnect policy and acquisition budget.
- On a permanent failure, or when the budget refuses a retry, leave the loop through the graceful tail (delegate stop and awaited lease `step_down`), then return the error.
- Skip classification and retry while a stop is in progress.
- Emit `master_delegate_lifecycle_total` with `event` = `start_error` and `reason` = `transient` or `permanent`, once per failed delegate task.

Affected crates: camel-master (code, tests, README); camel-test (one doc comment).

## Acceptance criteria

- A delegate that fails with a transient error and then succeeds leaves the Master consumer running, with `started` = 2 and `start_error`/`transient` = 1.
- A delegate that fails with a permanent error ends the Master task with that error, and the lease is stepped down.
- An exhausted acquisition budget after a delegate task failure ends the task and returns the last delegate error.
- `stop_propagates_delegate_start_error` keeps passing.

## Risk budget

A delegate whose background task fails transiently while it runs is now retried instead of ending the route. Each retry counts against `max_attempts` for the acquisition epoch, so the existing bound still holds; with `max_attempts = 0` a delegate that always fails is recreated about every 200 ms, as for construction errors. Construction-error handling, epoch idempotence, and the proxy re-wrap of the final error (supervision.rs, `ProcessorError(String)`) stay unchanged. Domain language follows CONTEXT-MAP.md.
