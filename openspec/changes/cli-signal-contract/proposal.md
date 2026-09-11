# Proposal: cli-signal-contract

## Why

`camel run` handles stop signals inconsistently. A SIGINT during boot hit the
default disposition and killed the process (rc-ukwlt; the SIGTERM twin was
fixed in rc-z5zch). After the boot-window fix, the SIGTERM stream stayed
registered through teardown, so a second TERM during a hung graceful shutdown
was buffered and ignored — only a second Ctrl+C still force-exited
(rc-kz85m). Orchestrators (systemd, `docker stop`) resend the stop signal
after their grace period, so the escape hatch must accept the signal they
actually send.

## What Changes

- Arm both signal streams (SIGINT and SIGTERM) at the start of `run`, before
  boot, so a signal arriving mid-boot is buffered and consumed by the
  shutdown select instead of default-killing the process.
- Extend the teardown force-exit arm: a second SIGINT or SIGTERM after the
  first was consumed exits with code 1. The entry-registered streams are
  moved into the force-exit task.
- Document the signal handling contract in `crates/camel-cli/CONTEXT.md` and
  add the spec delta under `specs/cli-startup/`.
- Add subprocess regression tests: SIGINT during boot exits 0; a second stop
  signal force-exits with 1.

## Explicitly excluded

- No change to SIGHUP, SIGQUIT, or other signal dispositions.
- No new shutdown hooks to make teardown artificially interruptible.
- No change to the exit code of a graceful shutdown (stays 0).

## Acceptance criteria

- `kill -INT` during boot no longer default-kills: the run exits 0.
- A second SIGTERM or SIGINT during teardown exits 1.
- One signal during normal running still exits 0.
- `openspec validate cli-signal-contract --type change` passes.
- New regression tests pass; `cargo test -p camel-cli --lib` green.
