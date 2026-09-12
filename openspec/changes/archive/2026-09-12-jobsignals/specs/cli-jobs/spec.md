## MODIFIED Requirements

### Requirement: exit-code taxonomy and JSON report

`camel job` SHALL exit 0 when the pipeline completes, 1 when the
pipeline fails, and 2 for load, validation, boot, interruption,
drain-timeout, shutdown, and report-write errors; when classes mix, exit
2 SHALL outrank exit 1. A first SIGINT or SIGTERM SHALL cancel an in-flight
send or batch drain, perform bounded teardown, and report outcome
`Interrupted`. A second SIGINT or SIGTERM received during that teardown SHALL
force-exit with code 1. The signal streams SHALL be armed before config
loading so signals during boot are buffered rather than default-killing the
process. For outcomes that reach the send (verdict, interruption, or
timeout), and for shutdown failures after a recorded verdict, the command
SHALL emit a JSON report
(`{document, mode, outcome, terminated_early, duration_ms, reply?, error?, shutdown_error?}`)
to stdout, or to the `--report` path when given; early exit-2 classes remain
stderr-only. When `capture-reply` is set and a reply exchange is returned,
the report SHALL carry the reply body and headers. A batch drain timeout
SHALL report outcome `Timeout` and preserve the existing exit-2 behavior.
When a signal event is ready at the same poll point as send completion or
deadline expiry, signal handling SHALL win and report `Interrupted`. The
implementation SHALL use signal-first selection ordering to make this
precedence deterministic.

#### Scenario: first signal interrupts an active job

- **GIVEN** a `camel job` process has booted and its send is in flight
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the send is cancelled, teardown runs under at least
  `MIN_SHUTDOWN_BUDGET`, the report outcome is `Interrupted`, and the
  process exits with code 2

#### Scenario: signal during boot is buffered

- **GIVEN** a `camel job` process is loading config or booting components
- **WHEN** the operator sends SIGINT or SIGTERM
- **THEN** the signal is handled by the job contract after boot reaches its
  signal-aware phase, rather than default-killing the process

On non-Unix platforms, the portable Ctrl+C listener is installed when first
polled; the boot-buffer guarantee applies to Unix SIGINT/SIGTERM streams.

#### Scenario: second signal force-exits during teardown

- **GIVEN** the first signal was consumed and bounded job teardown is running
- **WHEN** a second SIGINT or SIGTERM arrives before teardown completes
- **THEN** the process exits with code 1 without waiting for teardown

#### Scenario: interruption keeps apparatus precedence

- **GIVEN** a first signal interrupts a job and teardown also reports an error
- **WHEN** the JSON report is emitted
- **THEN** outcome remains `Interrupted`, shutdown detail is recorded, and
  the process exits with code 2

#### Scenario: signal interrupts batch drain

- **GIVEN** a batch job's send completed and its drain wait is still active
- **WHEN** the operator sends SIGINT or SIGTERM once
- **THEN** the drain wait is cancelled, teardown runs under the existing
  batch overall-deadline budget without a floor, the report outcome is
  `Interrupted`, and the process exits with code 2

#### Scenario: completed job with captured reply

- **GIVEN** a job completes and `capture-reply` is enabled
- **WHEN** the report is emitted
- **THEN** it contains outcome `Completed`, the captured reply body and
  headers, no `error` or `shutdown_error` for successful teardown, and the
  process exits 0

#### Scenario: failed pipeline exits 1

- **GIVEN** the target route pipeline fails during execution
- **WHEN** the report is emitted
- **THEN** it contains outcome `Failed` with a non-empty error and the
  no shutdown detail when teardown succeeds, and the process exits 1

#### Scenario: timeout and shutdown failure preserve precedence

- **GIVEN** send or batch drain exceeds the mandatory overall timeout, or
  teardown fails after a recorded verdict
- **WHEN** the report is emitted
- **THEN** timeout reports outcome `Timeout`, shutdown failure records
  `shutdown_error` only when teardown had a non-zero remaining budget, no
  shutdown detail when teardown succeeds, and either case exits 2

#### Scenario: shutdown failure after a verdict preserves both details

- **GIVEN** a pipeline verdict was recorded and teardown fails with a
  non-zero remaining budget
- **WHEN** the report is emitted
- **THEN** `error` preserves the pipeline or timeout verdict,
  `shutdown_error` carries teardown detail, and the process exits 2

#### Scenario: signal-first tie is deterministic

- **GIVEN** a signal and send completion become ready at the same poll point
- **WHEN** the command selects its next operation
- **THEN** the signal branch wins and the report outcome is `Interrupted`

#### Scenario: zero-budget shutdown failure is stderr-only

- **GIVEN** the overall deadline has already expired and teardown has no
  remaining budget
- **WHEN** shutdown reports its expected zero-budget failure
- **THEN** the report keeps outcome `Timeout`, the shutdown detail is written
  to stderr without replacing the verdict, and the process exits 2

#### Scenario: batch drain completion reports Completed

- **GIVEN** a batch job's send completes and all expected SEDA queues drain
- **WHEN** the report is emitted
- **THEN** it contains mode `batch`, outcome `Completed`, and the process
  exits 0

#### Scenario: shutdown failure after a verdict forces exit 2

- **GIVEN** a job whose pipeline completed but whose teardown fails or
  exceeds its budget
- **WHEN** the report is emitted
- **THEN** the report carries the shutdown detail in `shutdown_error` and
  the process exits 2

#### Scenario: failed verdict plus shutdown failure keeps both details

- **GIVEN** a job whose pipeline failed and whose teardown also fails
- **WHEN** the report is emitted
- **THEN** `error` holds the pipeline failure, `shutdown_error` holds the
  teardown detail, and the process exits 2
