# Proposal: exec-cli-startup-guard

## Why

`camel run` built with **default features** aborts at startup for *any* route
that does not use exec, because `exec` is in `default` and `run.rs` registers
`ExecBundle` unconditionally. `ExecBundle::from_toml(<empty>)` runs
`ExecGlobalConfig::validate()` (ADR-0033, fail-closed), which hard-errors on
zero profiles. Reproduced: a `timer:tick -> log` route exits with
`Configuration error: Failed to load exec config: exec: no profiles
configured (fail-closed)`. Control run: the same route starts fine once a
dummy exec profile is added — the only variable is the zero-profile check
firing globally at startup.

This is an over-firing of ADR-0033: the fail-closed property should govern
routes that actually exercise the exec capability, not block unrelated routes.

## What Changes

- The camel-cli `run` command registers the ExecBundle **when a discovered
  route references an `exec:` endpoint OR the operator explicitly declared
  `[components.exec]`**. The bundle is skipped only when exec is neither used
  nor configured (the actual bug case).
- A reusable scheme-presence scanner is added to `camel-core` (mirroring the
  existing `scan_route_definitions_for_sql_checks` walker).
- The ExecBundle itself is **unchanged** — it stays fail-closed per ADR-0033.
- A regression/positive example `examples/camel-cli-no-exec` is added.
- Out of scope: changing camel-cli default features; weakening exec fail-closed.

## Acceptance criteria

- `camel run` (default features) with a `timer -> log` route, no exec config,
  and no exec profiles: the CamelContext starts, exchanges are processed, and
  on a stop signal the process exits with code 0 (no fail-closed error).
- `camel run` with a route using `exec:echo` and no profiles still aborts with
  the fail-closed error (ADR-0033 preserved).
- `camel run` with a route using `exec:echo` and a configured `echo` profile
  starts and runs.
- `camel run` with NO route using exec but an explicit `[components.exec]`
  section (zero profiles) still aborts fail-closed — explicit declaration is
  validated.
- No existing `scan_route_definitions_for_sql_checks` behavior regresses.

## Risk budget

Acceptable: a localized reordering of exec bundle registration to after route
discovery in `run.rs`; a small refactor of the private URI walker for reuse
(guarded by existing SQL scanner tests). Out of bounds: any change to
`ExecBundle`/`ExecGlobalConfig` validation semantics; removing `exec` from
default features. Hot-reload that newly introduces exec usage when no exec
route existed at startup is a documented known limitation (restart required).

Bd: rc-71sc
