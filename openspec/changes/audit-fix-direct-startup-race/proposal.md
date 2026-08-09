# Proposal: audit-fix-direct-startup-race

## Why

`DirectConsumer` registers itself into the shared `DirectRegistry` inside its
spawned `start()` task. Because the consumer never overrides `startup_mode()`,
it defaults to `Immediate` — the runtime pre-resolves the startup signal, and
`StartRoute` returns `Ok` the instant the task is spawned, not when the registry
insert actually runs. A producer route that starts next observes `None` in the
registry and returns `EndpointCreationFailed`, losing the exchange.

This race caused the harness timer test flake (mitigated with `await_exchanges`
in commit `45c476c7`) and can lose exchanges in production auto-startup scenarios
where a producer and consumer route start concurrently.

## What Changes

- `DirectConsumer` overrides `startup_mode()` to return
  `ConsumerStartupMode::Explicit` — the runtime will await readiness before
  completing `StartRoute`.
- `DirectConsumer::start()` calls `context.mark_ready()` immediately after the
  registration block's lock guard drops, before the event loop — the runtime
  sees readiness only after the registry insert is committed.
- `DirectProducer::poll_ready` and `fail_if_no_consumers` are **untouched**.
- Two new unit tests mirror HTTP's existing pattern (startup-mode assertion +
  mark-ready-after-registration handshake test).
- `CONTEXT.md` gets a startup-handshake note documenting the residual
  same-`startup_order` operator-ordering window.

## Acceptance criteria

- `cargo test -p camel-component-direct` — all existing tests stay green
- New test `test_direct_consumer_startup_mode_is_explicit` asserts Explicit mode
- New test `test_direct_consumer_marks_ready_after_registration` asserts the
  `StartupReceiver` resolves `Ok` only after the name is visible in the registry
- `test_poll_ready_endpoint_not_registered` stays **unchanged and green**
- `cargo clippy -p camel-component-direct --all-targets -- -D warnings` passes
- `camel-direct/CONTEXT.md` documents the startup handshake and residual window

## Risk budget

- **Zero data-structure changes** — no new fields on any struct.
- **Zero Tower-contract risk** — `poll_ready` untouched.
- **Full backward compatibility** — `fail_if_no_consumers` semantics unchanged;
  the `Explicit` mode uses existing runtime machinery (rc-w1u9).
- **Backstop**: if `mark_ready` were missed, `spawn_consumer_task`'s defensive
  fallback resolves the receiver — startup never hangs.

Bd: rc-ls1u
