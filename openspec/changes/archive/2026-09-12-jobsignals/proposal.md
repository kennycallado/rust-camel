# Proposal: jobsignals

## Why

`camel job` currently leaves SIGINT and SIGTERM at their default process disposition. An operator or deployment supervisor can therefore kill a one-shot job without `BootHandle` teardown or its JSON report. This breaks the job execution contract and is unsafe for compiled or deployed job artifacts (bd rc-x72sf).

## What Changes

- Arm SIGINT and SIGTERM before job configuration loading.
- Treat the first signal as an interruption: cancel the send, tear down under `MIN_SHUTDOWN_BUDGET`, emit report outcome `Interrupted`, and return exit code 2.
- Treat a second signal during interruption teardown as force-exit code 1.
- Add the `Interrupted` outcome and exit semantics to the `camel job` failure-mode documentation.
- Add regression coverage for graceful interruption and repeated-signal force exit.

Affected crate: `crates/camel-cli`.

## Acceptance criteria

- A first SIGINT or SIGTERM during boot or send never uses the default kill disposition.
- A first signal produces an `Interrupted` JSON report when a job document has reached the send phase and exits 2 after bounded teardown; one-shot teardown has the `MIN_SHUTDOWN_BUDGET` floor and batch teardown remains bounded by its overall deadline without a floor.
- A second SIGINT or SIGTERM during teardown exits 1.
- Existing completed, pipeline-failed, timeout, and shutdown-failure behavior remains unchanged.

## Risk budget

Acceptable risk: dropping an in-flight send future as part of cancellation; context teardown remains the cleanup boundary. Out of scope: changing route execution, signal semantics for `camel run`, or adding a new public API. Signal coalescing follows Tokio semantics and is documented rather than reimplemented.
