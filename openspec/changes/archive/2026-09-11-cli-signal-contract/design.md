# Design: cli-signal-contract

## Approach

tokio signal streams only protect the process while they are registered. The
shutdown `tokio::select!` in `camel run` armed its streams late: a signal
arriving before that point hit the default disposition and killed the
process. rc-z5zch fixed the SIGTERM half by registering the stream at `run`
entry (commit 063b1918); rc-ukwlt is the SIGINT twin and registers the
interrupt stream the same way. tokio buffers a pending signal, so a boot-time
signal is consumed by the shutdown select as soon as it awaits — boot
completes, then shutdown is graceful.

## Second-signal escape hatch (rc-kz85m)

After the boot fix, the TERM stream stayed armed through teardown, so a
second TERM during a hung graceful shutdown was buffered and ignored. Before
that fix the accidental force exit existed because the handler was dropped
after the select arm. The explicit fix extends the post-select force-exit
task with a `tokio::select!` over the entry-registered SIGINT and SIGTERM
streams (moved into the task). Whichever fires: warn and
`std::process::exit(1)`.

Why the entry-registered streams instead of a fresh
`tokio::signal::ctrl_c()`:

- The escape hatch must be the signal orchestrators actually send. systemd
  (`TimeoutStopSec`) and `docker stop` resend SIGTERM after the grace period,
  so a TERM-only-listening hatch is useless in exactly the hung-teardown case
  it exists for.
- The entry-registered streams keep buffered boot-time signals
  force-exit-eligible and avoid registering a duplicate SIGINT listener.
- tokio coalesces repeats of the same signal (one permit slot per stream), so
  a same-signal double-tap can be lost; INT and TERM have independent slots.

The force-exit task is spawned after the first signal is consumed and aborted
once shutdown completes, so the semantics are unchanged for normal running:
one signal, graceful exit 0.

## Affected crates

- `crates/camel-cli`: `commands/run.rs` (stream arming, shutdown select,
  force-exit arm), `CONTEXT.md` (contract section), integration tests.

## Test notes

The regression tests signal at the earliest post-arm boot marker (the
CWD-trust WARN). tokio coalesces same-signal bursts, so the second-signal
test sends an INT+TERM pair in one `sh -c`: one is consumed as the graceful
first signal, the other is the consumable second signal during teardown. The
tests cover the second-signal window, not a deterministically hung teardown —
the harness has no hook to stall `BootHandle::shutdown`.

## Alternatives considered

**Document TERM-during-teardown as ignored.** Rejected: the escape hatch
must exist for the signal orchestrators send; documenting it away leaves
hung runs killable only by SIGKILL.

**Third-signal force exit.** Rejected: orchestrators resend once after the
grace period; waiting for a third delays the escape hatch past its purpose.
