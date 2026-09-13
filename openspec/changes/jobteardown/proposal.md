# Proposal: jobteardown

## Why

`camel job` boots a runtime before it validates and starts the job. Several failure paths after boot return exit code 2 without shutting down the booted context. With bridge-backed components, those leaked tasks and processes can keep the command alive and block later runs. This is tracked by bd `rc-7wl19`.

## What Changes

- Ensure every failure path after `BootHandle` acquisition performs the existing bounded shutdown sequence.
- Preserve the current signal, transport, success, and batch shutdown contracts.
- Add a regression test that observes shutdown on an early failure while retaining exit code 2.

Explicitly excluded: changes to bridge behavior, signal force-exit semantics, job discovery, and public APIs outside `camel-cli`.

## Acceptance criteria

- Early failures after boot do not return before bounded context teardown.
- Existing shutdown budget machinery is used for all new teardown paths.
- A regression test fails without the fix and passes with it.
- `camel-cli` formatting, clippy, tests, and required documentation checks pass.

## Risk budget

The change is limited to job command control flow and its tests. It may reorganize `execute_job` internally, but must not alter diagnostics, exit codes, signal handling, or successful job behavior. No new runtime dependency or unbounded wait is acceptable.
