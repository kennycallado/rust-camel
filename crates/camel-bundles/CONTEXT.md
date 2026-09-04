# Bundles

The component registration cascade for rust-camel. This crate runs
`ComponentBundle::register_all` for every component of a `camel run` boot. It
also owns the teardown handle for that cascade. Established by
[ADR-0069](../../docs/adr/0069-integration-tier-testing-contract.md) section
10.

## Language

**boot**:
The free function `camel_bundles::boot(ctx, config, project_root)`. It
registers the cascade on a caller-owned context and returns a `BootHandle`.
The caller creates and pre-configures the context first. Route loading,
discovery, startup checks, and `ctx.start()` stay with the caller.
_Avoid_: startup, initialize (use boot for the registration act)

**Bundle cascade**:
The ordered registration sequence moved verbatim from
`crates/camel-cli/src/commands/run.rs`. It registers the built-in components
(timer, cron, log, direct, seda, mock, controlbus, validator, xslt, xj), the
always-registered bundles (http, ws, file, container, template, jms, cxf,
master, opensearch, redis, sql), and the feature-gated bundles (http-static,
kafka, mqtt, surrealdb, grpc, llm, mcp, wasm). Each bundle goes through the
`register_bundle` seam: it reads the bundle's `[components.<key>]` table
from the config, keyed by `config_key()`. A missing key falls back to an
empty table. The bundle then registers with its serde defaults. Conditional
gates (the CLI exec gate, the integration harness) call the same seam for
one bundle outside the always-on set.
_Avoid_: component setup, registration list

**BootHandle**:
The teardown sequencer returned by `boot`. It owns the JMS and CXF bridge
pools. `shutdown` and `shutdown_with_deadline` preserve the `camel run`
ordering: `begin_shutdown` on both pools, then `ctx.stop()`, then a
deadline-wrapped `pool.shutdown` for JMS and for CXF. Every step runs even
when an earlier step fails. Failures are logged by the handle
(`system-broken`) with the `camel run` message; callers that need the value
may inspect it. Pool timeouts warn and do not fail the shutdown.
_Avoid_: shutdown handle (unqualified), runner guard

**BridgeCleanup**:
A context `Lifecycle` registered by `boot`. It stops the XSLT, XJ, and XSD
validator bridge runtimes. `ctx.stop()` drains it, so its position in the
teardown is driven by the `BootHandle` ordering, not by a direct call.
_Avoid_: bridge hook, cleanup service

**Feature gates**:
Cargo features that mirror the `camel run` cfg lines one to one: `grpc`,
`wasm`, `http-static`, `llm`, `surrealdb`, `mqtt`, and `mcp` are default-on;
`kafka` is opt-in. `camel-cli` forwards each of its gates into this crate.
The `exec` gate stays with the CLI because its registration rule is
conditional on route content.
_Avoid_: bundle flags, component toggles

## Architecture notes

**Context ownership.** `boot` takes `&mut CamelContext`. The context has no
`Default` impl, so a `mem::take` dance is not viable, and the consuming
`with_lifecycle` builder cannot run on a borrowed context. `CamelContext`
offers `add_lifecycle(&mut self)` as the registration seam. `boot` uses it
for `BridgeCleanup`. See
[camel-core CONTEXT](../camel-core/CONTEXT.md).

**Datasource catalog.** `boot` builds the `RuntimeDatasourceCatalog` from
`config.datasources` against the prepared context's health registry. The Sql
and SurrealDb bundles receive it through `with_catalog`. This moved out of
`camel run`; the health wiring therefore sees the same registry the caller
prepared.

**WASM bundle base dir.** Under the `wasm` feature, `boot` constructs the
`WasmBundle` from `ctx.registry_arc()` and `project_root`. The CLI resolves
`project_root` as the canonicalized parent of the config file. WASM bean
loading stays with the caller; only the component bundle registers here.

**No process exit.** This crate never terminates the process. Cascade
failures return `CamelError::Config` with the message the inline cascade
produced. SQL and SurrealDb init failures log as `system-broken` per
ADR-0012, matching the prior CLI behavior.

## Related decisions

- [ADR-0069](../../docs/adr/0069-integration-tier-testing-contract.md) —
  the integration-tier testing contract. Section 10 defines this crate, the
  `BootHandle`, and the enumerated boot boundary.
- [ADR-0012](../../docs/adr/0012-log-level-convention-handler-contract-boundaries.md) —
  log-policy annotations on the `error!` sites moved from `camel run`.
