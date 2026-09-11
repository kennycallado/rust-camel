# Tasks: cli-signal-contract

## Implementation

### Task 1: Arm the SIGINT stream at run entry

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. Register a SIGINT stream (`SignalKind::interrupt()`) at `run` entry next
   to the rc-z5zch SIGTERM stream, with the same `expect`/`allow-unwrap`
   idiom.
2. Await the registered stream in the shutdown select (replacing the fresh
   `tokio::signal::ctrl_c()` call), so a boot-time interrupt is consumed
   instead of default-killing the process.

**Tests:**
- `sigint-during-boot-graceful`: `crates/camel-cli/tests/run_signal_test.rs`
  `sigint_during_boot_shuts_down_gracefully` — SIGINT at the mid-boot marker
  exits 0 with the `Received Ctrl+C` log; red on pre-fix code.

**Acceptance:**
- SIGINT mid-boot no longer default-kills; exit code is 0.

- [x] 1

### Task 2: Second-signal force exit during teardown

**Files:**
- `crates/camel-cli/src/commands/run.rs` (modified)

**Steps:**
1. Replace the Ctrl+C-only force-exit task with a `tokio::select!` over the
   entry-registered SIGINT and SIGTERM streams (moved into the task). Either
   second signal warns `Second ... — forcing exit` and exits with code 1.
2. Keep the task teardown-scoped: spawned after the first signal is
   consumed, aborted once shutdown completes.
3. Keep the non-unix build on the Ctrl+C-only arm.

**Tests:**
- `second-signal-force-exit`: `crates/camel-cli/tests/run_signal_test.rs`
  `second_sigterm_during_teardown_force_exits` — an INT+TERM pair at the
  mid-boot marker exits 1 with the `forcing exit` WARN; red on pre-fix code.

**Acceptance:**
- A second SIGTERM or SIGINT during teardown exits 1.
- One signal during normal running still exits 0 (existing suites).

- [x] 2

## Documentation

### Task 3: Document the signal handling contract

**Files:**
- `crates/camel-cli/CONTEXT.md` (modified)
- `openspec/changes/cli-signal-contract/specs/cli-startup/spec.md` (new)

**Steps:**
1. Add the "Signal handling contract" section to `crates/camel-cli/CONTEXT.md`:
   streams armed before boot, first signal graceful (exit 0), second signal
   force-exit (exit 1).
2. Add the spec delta under `specs/cli-startup/` with the repeated-stop-signal
   requirement and its scenarios.

**Tests:**
- `openspec-validate`: `openspec validate cli-signal-contract --type change
  --json` passes with zero delta-structure errors.

**Acceptance:**
- Contract documented in CONTEXT.md and the spec delta validates.

- [x] 3
