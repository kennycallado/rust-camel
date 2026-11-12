# Proposal: clitesthook

## Why

`camel job`'s second-signal force-exit test (`job_signal_second_signal_forces_exit`,
bd `rc-af1y3`, P2) is flaky under runner load: the blind `kill -INT; kill -TERM`
pair can land before the send race is polled or after teardown has progressed,
so the second signal misses the force-exit guard (CI attempt 7 saw exit 0).
Mission 576 (`flakytests`, archived 2026-11-12) proved across three expert
rounds that every timing-only design has a concrete breaking schedule; the
deterministic test requires an OBSERVED marker proving worker pipeline entry
before the first INT. No such marker exists at any log level (delayer logs
nothing, seda logs only consumer start/stop, route runtime has no per-exchange
logs), so the marker needs an opt-in production seam — exactly what 576's park
prescribed as follow-up.

## What Changes

- **In scope:** a ~3-line env-opt-in test seam at route pipeline entry in
  `camel-core`'s sequential route controller (`route_controller_trait.rs`,
  immediately after the per-exchange `DrainGuard` claim), emitting one stderr
  marker line per dispatch when `CAMEL_JOB_PIPELINE_MARKER` is set (default
  off: no emission and no I/O when unset; the gate costs an env lookup and a
  branch — camel-cli precedent markers do the same); a deterministic
  rewrite of the flaky test (wait `CamelContext started` + pipeline-entry
  marker → INT → wait `Stopping CamelContext` → TERM; assertions byte-identical:
  exit 1, elapsed < 15 s, forcing-exit WARN); ≥50-run stability evidence.
- **Explicitly excluded:** any other production change; refactors of the
  delayer/seda/route runtime; the JS test (mission 576's scope).

## Acceptance criteria

- The flaky blind-pair test is replaced by the marker-synchronized test; no two
  competing tests of the same contract remain.
- The seam is default-off: with the env var unset, no emission and no I/O (the
  gate is a per-dispatch env lookup + branch, mirroring the
  `CAMEL_JOB_SIGNAL_MARKER`/`CAMEL_JOB_SHUTDOWN_MARKER` precedents).
- The rewritten test passes ≥50 consecutive unloaded runs; distribution recorded.
- `cargo fmt --check`, clippy `-D warnings` on touched crates, and
  `bash scripts/gates-local.sh` all pass.

## Risk budget

Acceptable: the seam is the first env-gated test marker outside `camel-cli`
(default-off keeps it inert for non-job runtimes — noted in the park).
Out of bounds: production behavior changes, refactors of the
delayer/seda/route runtime, teardown-budget semantics changes, spec-visible
CLI contract changes (none — see design; `skip_specs`).

Bd: rc-af1y3
