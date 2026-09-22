## ADDED Requirements

### Requirement: Bounded-stop stall repros share one harness

The bounded-stop regression repros in camel-otel (tests that pin `stop()`
boundedness when a provider's export path stalls) SHALL run their arrange /
act / assert bodies through one shared `bounded_repro` helper that owns the
dedicated-thread + current-thread-runtime + panic-channel + outer-timeout
scaffold. Repro thread names, panic diagnostics, and timeout constants SHALL
be preserved verbatim per repro.

#### Scenario: harness is single-source (unit-testable)

- **GIVEN** the camel-otel crate sources after the refactor
- **WHEN** searching for the harness scaffold pattern
  (`mpsc::channel::<Result<(), String>>` and
  `recv_timeout(Duration::from_secs(60))`)
- **THEN** each pattern occurs exactly once — inside the shared helper — and
  both stall repros (`test_stop_bounded_when_metric_export_stalls`,
  `test_stop_bounded_when_span_export_stalls`) invoke `bounded_repro`
  instead of embedding the scaffold

#### Scenario: metric-path repro is preserved (unit-testable)

- **GIVEN** the metric-export stall repro (`rc-q74u`)
- **WHEN** it runs through the shared harness
- **THEN** it spawns thread `q74u-repro`, keeps the 15s bounded-stop
  deadline assertion with its exact message, and reports a hang as
  "stop() hung (rc-q74u regression)"

#### Scenario: span-path repro is preserved (unit-testable)

- **GIVEN** the span-export stall repro (`rc-6ju71`)
- **WHEN** it runs through the shared harness
- **THEN** it spawns thread `q6ju71-repro`, keeps the 15s bounded-stop
  deadline assertion with its exact message, and reports a hang as
  "stop() hung (rc-6ju71 regression)"

### Requirement: OtelService unit tests live in a sibling module

The unit tests of `OtelService` SHALL live in
`crates/services/camel-otel/src/service_tests.rs`, wired from `service.rs`
as a `#[cfg(test)] #[path = "service_tests.rs"] mod tests;` declaration, so
test paths remain `camel_otel::service::tests::*` and `service.rs` carries
no inline test module. The crate's public API SHALL NOT change.

#### Scenario: test identity survives the move (unit-testable)

- **GIVEN** the baseline `cargo test -p camel-otel -- --list` output at
  commit 6b110708 (91 tests)
- **WHEN** the same listing runs after the move
- **THEN** the output is byte-identical: same test names, same count, same
  module paths, and the full suite passes

#### Scenario: crate context documents the layout (unit-testable)

- **GIVEN** `crates/services/camel-otel/CONTEXT.md`
- **WHEN** the refactor lands
- **THEN** it notes the sibling test layout (stall repros in
  `service_tests.rs` sharing `bounded_repro`), its ADR-0012 line anchors
  match the shifted `service.rs` lines, and
  `cargo xtask lint-context-citations` passes
