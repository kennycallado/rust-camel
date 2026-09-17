# Proposal: kafka-feature-wiring

## Why

Every published `camel` release binary today ships without usable kafka
support, while paying the full cost of building it:

- `release.yml` passes `--features cmake-build` on the five kafka-capable
  legs (linux-gnu ×2, macOS ×2, Windows). `cmake-build` activates the
  optional dependency — compiling and linking librdkafka via cmake, the
  slowest step of those legs — but never enables the `kafka` feature that
  gates component registration.
- Registration is doubly gated by `#[cfg(feature = "kafka")]`: the lint
  registry (`crates/camel-cli/src/lib.rs`) and the boot cascade
  (`crates/camel-bundles/src/lib.rs`). Both stay off. `kafka:` URIs fail
  at runtime with `Component not found: kafka` — the exact symptom
  camel-bundles' own gating test asserts for the feature-off case
  (`crates/camel-bundles/src/lib.rs:437-463`).
- `kafka-static` carries the same trap: it activates the dependency
  without enabling `kafka`.
- Nothing catches this class today: no test runs against the shipped
  artifact.

Found during the distribution-flavor analysis (bd epic rc-5t5fo); expert
verdict in `docs/audits/2026-09-17-distribution-flavor-verdict.md`.
Fixes bd rc-5t5fo.1.

## What Changes

- Collapse camel-cli's kafka feature surface from four names to two:
  - `kafka` — capability: activates the component and its registration
    (librdkafka source build, the rdkafka default)
  - `dynamic-linking` — same capability, linked against a system
    librdkafka; SHALL imply `kafka`
  - `cmake-build` and `kafka-static` are REMOVED from camel-cli
- `release.yml` kafka legs build with `--features kafka`.
- New release-CI capability assert on kafka legs whose binaries are
  natively executable (x86_64-gnu, macOS ×2, Windows): `camel lint` a
  fixture route with a syntactically valid `kafka:` source endpoint
  (`kafka:orders?brokers=localhost:9092`). The normative gate is
  two-part, both load-bearing: **exit 0 AND no `unverified-scheme`
  diagnostic in the output** (the matcher keys on the diagnostic
  code — the fixture's only capability-gated scheme is kafka;
  empirically calibrated: an unregistered scheme is an
  Info-severity diagnostic that also exits 0; a registered component
  over an endpoint missing `brokers=` errors into exit 1). The cross-compiled aarch64-gnu leg skips it by
  explicit criterion (registration is compile-time and
  target-independent; the identical feature string is proven on
  x86_64-gnu).
- Feature-graph tests in `tests/feature_profiles.rs`: the
  `dynamic-linking` feature list includes `kafka` (feature-table
  assertion — forwarding edges do not render in cargo tree); removed
  names fail to resolve.

Excluded: musl kafka support, Docker changes, the flavor split
(rc-5t5fo.5), the crates.io default flip (rc-5t5fo.8). Default and
`full` closures are untouched — the golden fixture must stay green
without regeneration.

## Acceptance criteria

- Release CI kafka legs build with a capability feature and the built
  binary resolves `kafka:` endpoints (lint probe green in CI)
- camel-cli exposes exactly `kafka` and `dynamic-linking` for kafka;
  `cmake-build`/`kafka-static` no longer resolve
- Default and `full` closures unchanged
  (`default_closure_matches_golden` green, no fixture regen)
- camel-kafka component crate unchanged (its internal rdkafka forwards
  stay)
- The camel-cli Cargo.toml kafka comment block no longer references
  `cmake-build`/`kafka-static` (verified to be the only remaining
  in-referencing doc site; no README/docs consumers exist)
- The probe's normative gate is two-part (exit 0 AND no
  `unverified-scheme` diagnostic); each part is individually
  load-bearing
- The probe step is explicitly absent from the cross-compiled
  aarch64-gnu leg — an implementer cannot silently add it there
- `dynamic-linking` cannot be enabled without the kafka capability
  (feature-table assertion: its list includes `kafka`)

## Risk budget

Acceptable: breaking the two broken-by-design feature names
(`cmake-build`, `kafka-static`) for pre-1.0 users — release-notes
callout; neither name ever delivered capability. Out of bounds: any
default-closure change, any behavior change to non-kafka builds, CI time
growth beyond the probe step.
