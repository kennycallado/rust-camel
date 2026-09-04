# Design: integration-tier-contract

Normative source: ADR-0069. Where this design and the ADR differ, the ADR
wins.

## Approach

Phase 1 extracts the bundle cascade from `crates/camel-cli/src/commands/run.rs`
into `camel-bundles`. The crate runs `ComponentBundle::register_all` over the
same bundle set, driven by the same `Camel.toml` tables, and returns a
`BootHandle` owning bridge cleanup and JMS/CXF pool teardown through an
explicit `shutdown()`. `camel-config` keeps context composition
(`configure_context_with_beans`); the new crate calls it, never re-homes it.
The CLI keeps the watcher, signal handling, the second-Ctrl+C path, the
conditional exec guard, and operator logging. Parity tests compare CLI and harness composition through the extracted
cascade, including registered bundles, feature forwarding, and teardown.

Phase 2 builds `camel-integration-test`. The tier function is pure and total:
`scenario:` forces FULL; otherwise the component closure over parsed
`RouteDefinition`s from any route source, traversed recursively, minus endpoints replaced by exact `skipTo` intercepts (`divertCopyTo` delivers
a copy; it subtracts nothing), plus `inputs`/`expects` schemes, decides lean
against the closed set. `recipient_list`, `routingSlip`, `dynamic_router`,
and `toD`-style steps force FULL. The function lives in test tooling and reads
the camel-core route model (`RouteDefinition`, `InterceptAction`). Core never
learns of tiers.

The scenario runner executes ordered actions: `send`, `receive` with a
mandatory deadline, `sleep`, `validate`, and variable extraction. Partner-side
assertions are normative: the harness binds its own listener. Environment is
layered and explicit: harness-provisioned bindings first (reserved keys such
as a bound partner address), document `env` second, allowlisted ambient
third, defaults last, otherwise unresolved. The pinned profile and this
layered source are inputs to the DSL and config loaders. The harness never
mutates the process environment. The tier function reads the camel-core
route model (`RouteDefinition`, `InterceptAction`) through camel-dsl
parsing and URI scheme resolution.

Phase 3 activates HTTP both directions. Outbound scenarios point route
producers at a harness listener on `127.0.0.1:0`. Inbound scenarios use an
explicitly configured loopback port, because the readiness signal exposes no
port-0 address. Readiness is consumed through the operator surface only
(`ConsumerStartupMode::Explicit`, `mark_ready`).

## Affected crates

- `camel-bundles` (new, publishable): bundle cascade, `BootHandle`,
  feature-flag forwarding.
- `camel-integration-test` (new, publishable): tier function, document
  parsing for scenario docs, action runner, validators, HTTP partner driver,
  tier-aware JUnit report.
- `camel-cli`: `run.rs` calls `camel_bundles`; `test` subcommand gains
  `--unit`/`--integration`, the scenario execution path, and the tier report.
- `camel-config`: loader seams accept the layered environment source and the
  explicit profile. No ownership change.
- `camel-core`, `camel-test`: unchanged.

## Architecture boundaries

Runtime: `camel-bundles` sits at the composition level beside `camel-cli`,
never inside `camel-core`. The six ADR-0069 fences hold; the hexagonal
boundaries test enforces them. DSL: the tier function reads the camel-core
route model through camel-dsl parsing and reuses URI scheme resolution.
Components: partner listeners belong
to the harness, not to any component. No mock producer reply mode
(rc-i2qf closed by recorded rejection). Test workspace: `camel-test` remains
the unit-tier leaf sink per ADR-0055; nothing publishable depends on it.

## Phases

Phase 1 — camel-bundles extraction. Exit: parity tests compare CLI and
harness composition through the extracted cascade, and no legacy inline
cascade remains in `run.rs`.

Phase 2 — runner and filters. Exit: tier function total with unit tests over
each sealed rule; scenario executor unit tests pass against typed
fake-adapter fixtures, and no scenario document uses the lean boot; filters
symmetric; failure taxonomy exits verified.

Phase 3 — HTTP activation. Exit: outbound bridge scenario and inbound
consumer scenario green through full boot in the `integration-http` CI job;
default suite runtime unchanged.

## Resolved during implementation

Fake-only scenario dispatch (Phase-3 review): scenario documents whose
endpoints are all in-memory (`fake:` scheme) take the in-memory smoke
path in every build, feature on or off. They validate document shape
and action grammar; no system-under-test surface is involved, so a boot
adds no information. Documents declaring any real wire scheme (for v1:
`http:` under `integration-http`) take the full boot when the feature
is compiled in, and the `infra-unavailable` path otherwise. Recorded in
the mock-testkit delta requirement wording above.
