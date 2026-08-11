# publish-topology Specification

## Purpose
TBD - created by archiving change eliminate-devdep-cycles. Update Purpose after archive.
## Requirements
### Requirement: Cycle detection is SCC-accurate

`resolve_publish_order` SHALL detect publish-order cycles by SCC
analysis (Tarjan or equivalent) on the combined normal + weak
(dev/build) dependency graph. A weak edge SHALL be broken only if both
its endpoints lie inside the same non-trivial SCC of the remaining
(unscheduled) graph. After each break the SCC decomposition SHALL be
recomputed. When every remaining unscheduled node is in a trivial
singleton SCC, the drain completes; if unscheduled nodes remain but no
non-trivial SCC exists, a hard normal-only cycle is reported. Weak-edge
selection among intra-SCC candidates SHALL be deterministic (e.g.
lexicographic by `(holder, target)`). This guarantees cycle-only
accuracy (no phantom edges) — not globally-minimal feedback-edge count.
The function SHALL return the broken weak edges (not print them as a
private side-effect), so the diagnostic and lint report the true
cycle-participant set.

#### Scenario: acyclic-but-weak graph reports zero broken edges

- **GIVEN** a publish graph whose weak edges do not close any cycle (an SCC analysis yields only trivial singletons)
- **WHEN** `resolve_publish_order` runs
- **THEN** it SHALL return an empty broken-edge set and an empty `no_verify` set
- **AND** pure Kahn's algorithm SHALL sort every publishable crate
- **AND** no weak edge outside a non-trivial SCC SHALL be broken

#### Scenario: a real cycle breaks only an intra-SCC weak edge

- **GIVEN** a graph with a non-trivial SCC closed by weak edges
- **WHEN** `resolve_publish_order` runs
- **THEN** it SHALL break only weak edges whose both endpoints are inside that SCC
- **AND** SHALL recompute the SCC decomposition after each break
- **AND** SHALL NOT break weak edges of crates outside any non-trivial SCC

#### Scenario: hard normal-only cycle is reported, not papered over

- **GIVEN** a cycle closed only by normal edges (no weak edge to break)
- **WHEN** `resolve_publish_order` runs
- **THEN** it SHALL return an error indicating a non-breakable dependency cycle (not silently break a normal edge or publish with `--no-verify`)

### Requirement: No publishable crate closes a publish-order cycle

A crate published to crates.io SHALL NOT declare a `camel-*`
dev-dependency or build-dependency that closes a publish-order cycle
(i.e. the crate appears in the `no_verify` set returned by the
SCC-accurate `resolve_publish_order`). Integration tests that require
such an edge SHALL be remediated by StubComponent substitution,
relocation to the publish-order leaf `camel-test`, or relocation to the
consumer crate (for proc-macro derive tests).

#### Scenario: lint fails on a real cyclic dev-dependency

- **GIVEN** a publishable crate whose `[dev-dependencies]` contains a `camel-*` edge placing it in the `no_verify` set
- **WHEN** `cargo xtask lint-publish-cycles` is run
- **THEN** it SHALL exit non-zero and name the offending crate and edge

#### Scenario: show-cycles and lint report the same set

- **GIVEN** the publish graph
- **WHEN** both `cargo xtask publish --show-cycles` and `cargo xtask lint-publish-cycles` run
- **THEN** they SHALL compute the same `no_verify` set from the same `resolve_publish_order` predicate

#### Scenario: StubComponent substitutes a cyclic real-component dev-dep

- **GIVEN** a `#[cfg(test)]` function in a publishable crate that registers a real component purely as scaffolding to assert scheme registration / `all_metadata().len()` / `query_capabilities` (not the component's real option shape)
- **WHEN** the cycle is remediated
- **THEN** the function SHALL register a local `StubComponent` (implementing the `Component` trait with a configurable scheme + synthetic metadata) instead of the real component
- **AND** the real component's crate SHALL be removed from the holder's `[dev-dependencies]`
- **AND** `cargo test -p <holder-crate> --lib` SHALL pass

#### Scenario: real-option catalog tests relocate to the leaf

- **GIVEN** a `#[cfg(test)]` function that asserts a real component's option-catalog shape (and the component's dev-dep closes a cycle)
- **WHEN** the cycle is remediated
- **THEN** the function SHALL relocate to `camel-test` (which already depends on the component normally)
- **AND** `cargo test -p camel-test --test <relocated-file>` SHALL pass

#### Scenario: proc-macro derive tests relocate to the consumer crate

- **GIVEN** a proc-macro crate whose derive-integration/trybuild tests reference the consumer crate's types (an inherent proc-macro-testing cycle), with the cycle closed by a dev-dep on the consumer crate AND a companion dev-dep on the api crate used only by those tests
- **WHEN** the cycle is remediated
- **THEN** the derive + trybuild UI tests SHALL relocate to the consumer crate (which already normal-depends on the proc-macro crate and the api crate)
- **AND** BOTH the consumer-crate and the api-crate SHALL be removed from the proc-macro crate's `[dev-dependencies]`
- **AND** `cargo test -p <consumer-crate> --test <relocated-derive-test>` SHALL pass
- **AND** the proc-macro crate's pure unit tests SHALL remain and pass under `cargo test -p <proc-macro-crate>`

### Requirement: Publish-order leaf sink

`camel-test` is a publish-order leaf: no publishable workspace crate
declares `camel-test` as a normal, dev, or build dependency. `camel-test`
SHALL remain published as the downstream-facing test utility crate. The
lint SHALL fail if any publishable crate declares `camel-test` as any
kind of dependency. (This is a guard; `camel-test` is already a true leaf
once the cycle detector stops fabricating phantom edges.)

#### Scenario: camel-test stays published and remains a leaf

- **GIVEN** the workspace manifests
- **WHEN** the publish graph is computed
- **THEN** `camel-test` SHALL NOT have `publish = false`
- **AND** no publishable workspace crate SHALL declare `camel-test` in `[dependencies]`, `[dev-dependencies]`, or `[build-dependencies]`
- **AND** `camel-test` SHALL NOT appear in `no_verify`

### Requirement: Publish-cycle diagnostic

The xtask SHALL provide `cargo xtask publish --show-cycles` that prints
the `no_verify` set and the broken weak edges from the SCC-accurate
`resolve_publish_order`, WITHOUT publishing anything and WITHOUT
mutating any `Cargo.toml`.

#### Scenario: show-cycles on a clean graph

- **GIVEN** the `no_verify` set is empty
- **WHEN** `cargo xtask publish --show-cycles` runs
- **THEN** it SHALL exit 0, print an empty `no_verify` set and zero broken edges, invoke no `cargo publish`, and write no `Cargo.toml`

#### Scenario: show-cycles on a graph with cycles

- **GIVEN** the `no_verify` set is non-empty
- **WHEN** `cargo xtask publish --show-cycles` runs
- **THEN** it SHALL list each cycle-participant crate and each broken weak edge, invoke no `cargo publish`, and write no `Cargo.toml`

### Requirement: Publish-cycle lint gate

The xtask SHALL provide `cargo xtask lint-publish-cycles` that exits
non-zero when the `no_verify` set is non-empty OR when any publishable
crate declares `camel-test` in any dependency kind. It SHALL be wired
into `AGENTS.md ## QUALITY GATES`. It SHALL use the same SCC-accurate
predicate as `publish --show-cycles`.

#### Scenario: lint passes on a clean graph

- **GIVEN** the `no_verify` set is empty and no publishable crate depends on `camel-test`
- **WHEN** `cargo xtask lint-publish-cycles` runs
- **THEN** it SHALL exit 0

#### Scenario: lint fails on a reintroduced cycle

- **GIVEN** a publishable crate declares a `camel-*` dev-dependency that closes a cycle
- **WHEN** `cargo xtask lint-publish-cycles` runs
- **THEN** it SHALL exit non-zero and name the offending crate and edge

### Requirement: Publish pipeline has no manifest mutation

The xtask publish pipeline SHALL NOT mutate any `Cargo.toml` on disk in
order to publish. Cycles are prevented by topology (remediation + lint),
not repaired at publish time. `comment_out_camel_dev_deps` and the
strip/restore loop SHALL be removed; `publish_crates` SHALL be a plain
linear topological sort. (`is_weak_dependency_section` is retained as
benign shared edge-classification logic, not part of the hack.)

#### Scenario: publish with a clean graph

- **GIVEN** the `no_verify` set is empty
- **WHEN** `cargo xtask publish` runs
- **THEN** no `Cargo.toml` SHALL be written to during the publish loop
- **AND** every publishable crate SHALL publish without `--no-verify` for dev-dep cycles

### Requirement: Topology invariant recorded as ADR

The topology invariant SHALL be recorded as ADR-0055 in `docs/adr/`,
covering: the publish-cycle constraint, the SCC-accurate detection
requirement, the leaf-sink role of `camel-test`, the two remediation
patterns (StubComponent substitution; proc-macro test relocation to the
consumer), and the rejected manifest-mutation hack with rationale.
ADR-0055 SHALL be cited from `CONTEXT-MAP.md`.

#### Scenario: ADR-0055 exists and is cited

- **GIVEN** the change is complete
- **WHEN** `docs/adr/` is inspected
- **THEN** ADR-0055 SHALL exist with the decision statement and rejected-alternative rationale
- **AND** `CONTEXT-MAP.md` SHALL cite ADR-0055

### Requirement: Relocation manifest

A relocation manifest SHALL be committed recording, for every test file
moved out of a publishable crate (to `camel-test` or to a consumer
crate), the origin path and destination path.

#### Scenario: manifest accounts for every moved file

- **GIVEN** test files were relocated during the cycle-break
- **WHEN** the relocation manifest is inspected
- **THEN** it SHALL contain one entry per moved file with origin and destination paths
- **AND** each destination path SHALL exist
- **AND** each relocated test SHALL pass under its new crate

