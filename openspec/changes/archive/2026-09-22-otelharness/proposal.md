# Proposal: otelharness

## Why

The bounded-stop (BSP) stall repro harness — the ~50-line
`mpsc`/`std::thread`/`catch_unwind`/`recv_timeout` scaffold that guards the
rc-q74u/rc-6ju71 unbounded-wait class — is duplicated verbatim across two
tests in `crates/services/camel-otel/src/service.rs`
(`test_stop_bounded_when_metric_export_stalls`, rc-q74u; and
`test_stop_bounded_when_span_export_stalls`, rc-6ju71). A third provider path
(logs) would copy it a third time. Additionally, `service.rs` is ~1.28k lines,
~720 of which are the inline `mod tests` (r_glm minors on rc-6ju71, tracked as
bd rc-qr9pc).

## What Changes

- Extract the shared repro harness into one `bounded_repro(name, regression, body)`
  helper so every stall repro calls it; thread names, panic diagnostics, and
  timeout constants are preserved exactly.
- Move the entire inline `mod tests` (lines 561–1280) from `service.rs` to a
  sibling file `src/service_tests.rs`, wired as `#[cfg(test)] #[path =
  "service_tests.rs"] mod tests;` — repo precedent `51a7d49f`
  (`metrics_tests.rs`) and the existing `sampler_tests.rs` sibling in the same
  crate. Test paths stay `camel_otel::service::tests::*`.
- Note the new test layout in `crates/services/camel-otel/CONTEXT.md` and
  refresh its ADR-0012 line anchors (they shift by +3 from the wiring lines).

Excluded: any assertion or behavior change. This is a behavior-preserving
refactor — the only permitted edits are the harness call-sites. No public API
of camel-otel changes; `mod tests` is `#[cfg(test)]`-gated and referenced
nowhere outside `service.rs` (verified by repo-wide grep).

## Acceptance criteria

- `cargo test -p camel-otel -- --list` output is byte-identical before and
  after (91 tests, same names/paths).
- `cargo test -p camel-otel` green, including both stall repros.
- The harness scaffold (`mpsc::channel::<Result<(), String>>` +
  `recv_timeout(Duration::from_secs(60))`) appears exactly once in the crate.
- `service.rs` drops to ~565 lines; `service_tests.rs` carries the tests.
- `cargo fmt --check` and `cargo clippy -p camel-otel -- -D warnings` clean.
- CONTEXT.md notes the layout; `cargo xtask lint-context-citations` green.

## Risk budget

Acceptable: mechanical move noise (large diff, zero semantic change), the two
stall tests' inner bodies remain un-deduped (only the scaffold is shared).
Out of bounds: weakening any stall assertion, renaming tests, touching
production `service.rs` logic beyond deleting the inline test mod, or changes
outside `crates/services/camel-otel` (CONTEXT.md of that crate excepted).

Bd: rc-qr9pc
