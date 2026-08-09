# Proposal: audit-fix-misc-correctness

## Why

The v1.0 quality audit surfaced seven independent P2 correctness defects
across seven crates. Each is a latent runtime bug, API-stability gap, or
test-coverage hole that is cheaper to fix before the 1.0 freeze than after.
No two issues share a root cause or a fix shape — they are grouped here
solely because they share the same urgency window.

## What Changes

1. **camel-log** (rc-3smd): `String::truncate` panics when `max_chars` lands
   inside a multibyte UTF-8 sequence. Replace with char-boundary-safe
   truncation.
2. **camel-component-seda** (rc-exa2): `concurrentConsumers > 1` is ignored —
   `start()` spawns one forwarder regardless. Spawn N forwarders sharing one
   receiver.
3. **camel-proto-compiler** (rc-gr8k): Descriptor file path uses a
   process-local counter that resets to zero across processes, clobbering
   parallel builds. Replace with a unique-per-invocation temp file.
4. **camel-container** (rc-xvuk): `cleanup_tracked_containers()` ignores the
   configured `docker_host`, silently failing cleanup on non-default sockets.
   Thread the configured host through the cleanup path.
5. **camel-ws** (rc-jh8s): `mark_ready()` fires before the TLS listener binds
   for `wss://` routes. Defer readiness until after the bind succeeds.
6. **camel-bean** (rc-sfy1): `BeanError` pub enum lacks `#[non_exhaustive]`.
   Adding a variant post-1.0 is breaking. Add the attribute.
7. **camel-endpoint-macros** (rc-7ka6): Zero trybuild compile-fail tests for
   15 proc-macro error-message sites. Add a trybuild regression suite.

Excluded: anything outside these seven issues.

## Acceptance criteria

- No `truncate()` panic on multibyte input in camel-log.
- SEDA `concurrentConsumers=4` delivers four concurrent forwarder tasks.
- Proto-compiler descriptor files are unique across concurrent processes.
- `cleanup_tracked_containers()` respects configured `docker_host`.
- `wss://` routes do not signal readiness before the TLS listener binds.
- `BeanError` carries `#[non_exhaustive]`.
- `cargo test -p camel-endpoint-macros` includes trybuild compile-fail cases.
- All quality gates pass (fmt, clippy, xtask lints, build).

## Risk budget

- SEDA concurrency: medium behavioral change — the single-receiver shared
  by N tasks must not reorder or lose envelopes.
- WS readiness: must not break the existing plain-`ws://` synchronous bind.
- All other fixes are localised and low-risk.
