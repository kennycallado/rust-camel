# Proposal: bootfreshness

## Why

The `boot_freshness` integration test can intermittently observe rows from
boot A during boot B when both boots use the same named shared-cache SQLite
URI. This violates the scenario-tier contract that an in-memory datasource
dies with its boot and wastes CI cycles. The failure is tracked by bd
`rc-yw0e6`, and is distinct from JWKS cooldown, HTTP readiness, and macOS
errno flake classes.

## What Changes

- Extend the landed SQL datasource teardown contract with regression coverage
  for its deterministic, observable happens-before boundary for named
  in-memory SQLite pools.
- Harden the boot freshness fixture so its URI is unique to the test while
  remaining stable across the two boots it compares.
- Document the root cause and determinism argument in the integration-test
  context.

This change does not add parallel boot execution, replace the real SQL pool,
or use sleep-based synchronization.

## Acceptance criteria

- `named_shared_memory_uri_dies_with_its_boot` passes 20 consecutive runs
  under concurrent build load with no boot-B row leakage.
- Teardown either proves all datasource pool handles are drained or returns a
  deterministic error; it does not silently race a later boot.
- Tests and docs explain why the fix is deterministic and contain no
  unconditional sleep used as synchronization.

The close seam and pool drain implementation already landed in `camel-core`
and `camel-component-sql` (`efd92d6f` and `41028d20`). This change does not
re-implement them. It evaluates the existing contract and changes timeout
behavior only if the regression proves the current warning-only path can
silently violate boot freshness.

The pre-flight ruling classifies this as a new SQLite named-memory lifetime
class, not rc-7dfyq JWKS cooldown, the camel-http readiness class, or
rc-62me6 macOS errno. Therefore it is not parked as a duplicate.

## Verification matrix

The regression evidence covers three execution shapes. The serial focused
run passed `1 passed; 0 failed`; the default-thread focused run passed `1
passed; 0 failed`; and 20 consecutive full-suite raw-binary invocations each
passed `192 passed; 0 failed` with zero boot-B row leakage while a whole-
workspace `cargo check --workspace --tests` load ran for 17m54s.

The planned `cargo build --workspace --tests` load was attempted first, but
the fresh load target exhausted the shared disk after consuming 9.8 GB.
The check-based load is the recorded, disk-safe substitute: it still drives
parallel workspace rustc work and scheduler pressure while the real linked
integration-test binary runs. Future re-verification may use the original
build command when sufficient disk is available.

## Risk budget

The affected crates are `camel-integration-test` and its context/docs; the
already-landed `camel-core` and `camel-component-sql` adapter contracts are
the behavior under
test. Acceptable risk is a focused fixture or documentation change. Out of
scope are re-implementing landed teardown, unrelated readiness,
authentication cooldown, platform errno fixes, schema changes, and
production database behavior.
