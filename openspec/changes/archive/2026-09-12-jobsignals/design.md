# Design: jobsignals

## Approach

Port the established `camel run` signal contract into `run_job`, while adapting it to the one-shot command's linear send and report flow. Register Unix SIGINT and SIGTERM streams at function entry, before config loading. On non-Unix platforms, register the portable Ctrl+C listener. A signal received during boot is buffered by Tokio and consumed at the first signal-aware await rather than reaching the default process disposition.

The send phase races the existing overall deadline against a first-signal future using `tokio::select! { biased; signal first, operation second }`, so a simultaneously-ready signal wins deterministically. The same signal future remains active while batch drain runs, so interruption during send or drain follows one path. On interruption, the send or drain future is dropped and a `JobReport` with outcome `Interrupted`, `terminated_early: false`, and a signal error is created. One-shot teardown uses at least `MIN_SHUTDOWN_BUDGET`; batch teardown preserves the existing overall-deadline/no-floor budget rule. The report is written after teardown; interruption remains exit code 2, including when shutdown also fails.

After the first signal is consumed, move the already-registered signal streams into a force-exit task. The task waits for either stream and calls `std::process::exit(1)` if teardown has not completed. This ordering prevents the force-exit task from stealing the first signal. Abort the task after bounded shutdown completes. On non-Unix, the same task waits for a second Ctrl+C.

The outcome taxonomy stays string-compatible with existing reports. `Interrupted` is added to the report documentation and the exit-code mapping. Early validation and boot failures remain stderr-only because no send-phase report exists.

## Verification scenarios

- `job_report_interrupted_serializes`: arrange a report with `outcome: "Interrupted"` and a shutdown detail; act by serializing it; assert both fields are present.
- `job_exit_code_interrupted`: arrange an `Interrupted` report; act through the outcome mapping; assert exit code 2.
- `job_first_sigterm_interrupts_send`: arrange a spawned one-shot job held in a blocking `seda:` route; act by sending one SIGTERM after the flushed post-registration trust-model warning (`camel job trusts the current working directory...`); assert exit 2, JSON outcome `Interrupted`, and clean process teardown.
- `job_second_signal_force_exits`: arrange the same held job; act by sending SIGINT followed by SIGTERM; assert process exit 1 before the shutdown budget expires.
- `job_sigterm_during_batch_drain_interrupts`: arrange a batch job whose send completes while a drain probe remains non-empty; act by sending one SIGTERM after the flushed post-registration trust-model warning and during drain; assert exit 2 and JSON outcome `Interrupted`.
- `job_sigterm_during_boot_is_buffered`: arrange a subprocess that has registered streams but not completed boot; act by sending SIGTERM before the boot marker; assert no 130/143 shell signal exit and eventual `Interrupted` report with exit 2.
- `job_signal_wins_ready_tie`: extract private helper `async fn await_job_operation_or_signal<S, F>(signal: S, operation: F) -> JobWaitOutcome` with a biased signal-first select; arrange ready signal and operation futures; act through the helper; assert signal branch wins.
- `job_interrupted_shutdown_budget_by_mode`: arrange interrupted one-shot and batch jobs; act through teardown budget calculation; assert one-shot budget is at least `MIN_SHUTDOWN_BUDGET` and batch budget never exceeds the overall deadline.

## Affected crates

- `crates/camel-cli`: signal registration, cancellation/select logic, second-signal force exit, report outcome mapping, and tests.
- `crates/camel-cli/CONTEXT.md`: `camel job` signal contract and failure-mode table.

## Architecture boundaries

This change stays in the CLI control plane. It does not alter Runtime, DSL, Components, Services, Languages, or Functions. `BootHandle::shutdown_with_deadline` remains the existing lifecycle boundary, consistent with ADR-0003's coordinated lifecycle and the archived `cli-signal-contract` design. The signal handler only controls command cancellation and never changes data-plane pipeline outcome semantics.

## Alternatives considered

**Install a force-exit task at function entry.** Rejected because it could consume the first signal and violate graceful interruption.

**Use only `tokio::signal::ctrl_c()`.** Rejected because deployment supervisors resend SIGTERM, especially systemd and Docker.

**Return exit 1 for interruption.** Rejected because interruption is an apparatus/control event, and the job taxonomy reserves exit 2 for non-verdict failures.
