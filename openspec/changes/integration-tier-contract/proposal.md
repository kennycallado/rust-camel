# Proposal: integration-tier-contract

## Why

ADR-0069 (ratified 2026-09-03, commit f5d90c72) binds the integration-tier
testing contract sketched in ADR-0064 section 4. This change implements its
Phases 1 through 3.

The pain is measured. `crates/camel-test/tests/` holds roughly 30
hand-written integration tests behind the `integration-tests` feature. They
probe free ports, sleep fixed intervals, and assert by hand. The HTTP bridge
header corruption (rc-eoft, rc-f0cn) passed every existing check. `camel
test` also resolves no `${env:}` placeholders while `camel run` does, so the
two commands already disagree on the same configuration.

## What Changes

Phase 1: extract the component-bundle registration cascade from `camel run`
into a new crate `camel-bundles`, with a `BootHandle` lifecycle handle. Migrate
`camel run` onto it. Prove parity with tests.

Phase 2: new crate `camel-integration-test` with the pure tier-derivation
function, the scenario runner (`send`, `receive` with mandatory deadline,
`sleep`, `validate`, scenario variables), the layered hermetic environment
source, the failure taxonomy, and the tier report. Wire `camel test
--unit` / `--integration` filters.

Phase 3: activate HTTP end to end, both directions, through partner-side
loopback scenarios. CI runs a dedicated `integration-http` job with path
filters.

Excluded, each filed on its own merit: WS and gRPC adapters (demand-gated),
broker adapters (Docker), `testcontainer` and `user-provided` provisioning
(reserved grammar values, rejected in v1), structured `parallel`, `camel test
--watch` (rc-hi9y), int-leaf env coercion (rc-v1sw), bound-address API for
port 0.

## Affected crates

- `camel-bundles` (new): bundle cascade + `BootHandle`.
- `camel-integration-test` (new): scenario model, runner, partner adapters.
- `camel-cli`: `run.rs` migrates to `camel-bundles`; `test` subcommand gains
  tier filters and the scenario path.
- `camel-config`: unchanged ownership. Receives the layered environment
  source as an input, not a process-global rewrite.
- `camel-core`: no change. The ADR-0069 section 6 fences hold.
- `camel-test`: no change.

## Acceptance criteria

- Parity: `camel run` and the harness register identical bundles from the
  same `Camel.toml`, proven by test.
- Tier derivation is total and implements the sealed rules: `scenario:`
  forces FULL, exact `skipTo` intercepts subtract from the closure
  (`divertCopyTo` does not), dynamic dispatch forces FULL, the lean set stays
  byte-identical.
- Mixed vocabulary (`scenario:` plus `inputs`/`expects`/`intercepts`) fails
  at load with `doc-validation`.
- Filters are symmetric. An explicitly named nonmatching document fails with
  `tier-filter-collision`. Both flags together exit 2.
- An HTTP bridge scenario runs green through full boot and a partner
  listener on `127.0.0.1:0`, in the `integration-http` CI job.
- The hexagonal boundaries test stays green and `camel-core` gains no
  dependency on any new crate.
- Default test-suite runtime does not change. The integration job is opt-in.

## Risk budget

Acceptable: publish-surface growth from two crates, `run.rs` refactor risk
bounded by parity tests. Out of bounds: any `camel-core` change, any lean-boot
set growth, any default-suite slowdown, any process-environment mutation from
the harness.

Bd: rc-kk69 (epic). Adjacent: rc-v1sw, rc-hi9y.
