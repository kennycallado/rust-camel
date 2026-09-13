# Design: jobteardown

## Approach

Restructure the post-boot portion of `execute_job` behind one async teardown border. The inner operation returns the existing exit outcome for validation, route-registration, start, transport, signal, success, and batch cases. The border owns the already-established shutdown budget calculation and invokes `shutdown(&mut ctx, &boot_handle, budget)` for every early failure that currently exits before shutdown. Existing paths that already shut down remain behaviorally unchanged and are not duplicated.

The implementation must preserve each existing stderr diagnostic and exit code. The early-failure border is signal-agnostic for embedded jobs (`signals: None`) and does not change the signal branch's force-exit guard. Tests add an opt-in `CAMEL_JOB_SHUTDOWN_MARKER` seam beside the existing signal marker, then use `common::spawn_camel_job_with_args`, `spawn_drained`, `wait_for_marker`, and `wait_exit_code_bounded` to run a deterministic ambiguous-target fixture. The test asserts the marker, the existing ambiguity diagnostic, and exit code 2.

The deterministic transport regression uses a rejected `direct:` option rather than a closed HTTP port: job target validation permits only job-safe consumer schemes, so an HTTP target cannot reach the outer transport-error branch. A persistent `direct:go?block=true` endpoint-creation failure exercises that branch without a network dependency. A dedicated `ctx.start()` failure fixture is not required because the single `EarlyJobFailure` result type structurally routes every setup failure through the same teardown border.

## Affected crates

- `camel-cli`: post-boot job execution control flow and regression tests.

## Architecture boundaries

This is control-plane lifecycle handling in the CLI. It does not change route data-plane processing, component implementations, DSL parsing, or bridge protocols. The design follows the lifecycle ownership and coordinated shutdown vocabulary documented in `CONTEXT-MAP.md`; the existing bounded shutdown helper remains the single teardown mechanism.

## Alternatives considered

- **Async RAII guard:** rejected because `Drop` cannot await shutdown safely and blocking on the runtime can deadlock or panic.
- **Copy shutdown calls into each return branch:** rejected because it duplicates budget logic and makes future early exits easy to leak again.
- **Change only the test fixture:** rejected because the defect is lifecycle control flow, not test orchestration.
