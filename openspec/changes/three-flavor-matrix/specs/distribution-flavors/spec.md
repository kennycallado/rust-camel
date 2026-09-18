# distribution-flavors Specification

## ADDED Requirements

### Requirement: Three distribution flavors with hand-curated bodies

The camel CLI SHALL ship in exactly three flavors, selected only via the
marker features `flavor-slim`, `flavor-regular`, `flavor-full` in
`crates/camel-cli/Cargo.toml`. Curation SHALL be hand-curated capability
contracts, not size-budget thresholds. `flavor-regular` SHALL be a pure-Rust
closure across all 7 release targets; `flavor-slim` SHALL be a pure-Rust
edge/IoT closure restricted to musl targets in CI; `flavor-full` SHALL be the
regular closure plus `exec`, `lang-js`, `lang-rhai`, `lang-xpath`, and
`kafka`. Enabling no marker SHALL report flavor `custom` in `--version`
(unchanged from rc-5t5fo.3).

#### Scenario: slim body is the edge contract

- **WHEN** `cargo build --no-default-features --features flavor-slim` resolves
- **THEN** the closure contains the base CLI components (core, direct, seda,
  log, file, timer) plus http-static serving and mqtt, and contains no
  `camel-component-kafka`, no `exec`, no `lang-js`, no `lang-rhai`, no
  `lang-xpath`, no security features, and no non-pure-Rust dependency

#### Scenario: regular body is the most-used contract

- **WHEN** `cargo build --features flavor-regular` resolves (default)
- **THEN** the closure resolves the schemes http, file, timer, log, direct,
  seda, mqtt, redis (incl. tls), sql, jms, otel, jsonpath, minijinja with the
  security guard active, and contains no `kafka`, no `exec`, no `lang-js`,
  no `lang-rhai`, no `lang-xpath`

#### Scenario: full is regular plus the explicit deltas

- **WHEN** `cargo build --features flavor-full` resolves
- **THEN** the closure is exactly the regular closure plus `exec`,
  `lang-js`, `lang-rhai`, `lang-xpath`, and `kafka`, and the kafka component
  is reachable (registry resolves `kafka` — the historical kafka-less-full
  defect stays fixed)

### Requirement: Flavor closure contract enforced in tests and release

The feature-closure test suite (`crates/camel-cli/tests/feature_profiles.rs`)
SHALL encode the flavor contracts as prefix sets alongside the existing
`SLIM_FORBIDDEN_PREFIXES`: `REGULAR_REQUIRED`, `REGULAR_FORBIDDEN`, and
`FULL_REQUIRED` (containing kafka). The golden closure fixture SHALL be
regenerated in the same change that re-curates any flavor body. The release
path SHALL run per-flavor capability asserts against the downloaded release
artifact before attaching it, extending the existing symbol-assert pattern.

#### Scenario: contract sets gate the closures

- **WHEN** the feature-closure suite runs
- **THEN** regular satisfies every `REGULAR_REQUIRED` prefix and contains no
  `REGULAR_FORBIDDEN` prefix, full satisfies `FULL_REQUIRED` (kafka), and slim
  contains no `SLIM_FORBIDDEN_PREFIXES` entry

#### Scenario: slim security omission fails closed

- **WHEN** a slim build encounters a configuration requiring security
  features it does not embed
- **THEN** startup rejects the configuration with an explicit error, and a
  test proves the rejection (no silent degradation to an insecure mode)

#### Scenario: release artifacts carry per-flavor capability asserts

- **WHEN** a tag run builds and collects artifacts
- **THEN** each artifact is smoke-probed for its flavor suffix in
  `--version` and its flavor capability set (kafka registered on full;
  absence asserts on slim/regular) before the release is created

### Requirement: Shipped artifact names are flavor-prefixed

Release artifacts SHALL be named `camel-slim-<target>`, `camel-<target>`
(regular), and `camel-full-<target>`. The clean legacy name `camel-<target>`
SHALL re-alias from the historical full-ish closure to regular at the first
flavored release, announced in the release notes (one-time semantic break,
bd ruling superseding the verdict's transition-release alternative). All
three names SHALL exist from that release onward.

#### Scenario: all three names ship from the first flavored release

- **WHEN** the first flavored tag release completes
- **THEN** the GitHub release carries `camel-slim-*` (musl ×2),
  `camel-*` (regular, all 7 targets), and `camel-full-*` (gnu ×2, macOS ×2,
  Windows), and no asset name 404s relative to the previous release's names
  except by documented re-alias

### Requirement: Docker variants map to flavors with explicit semantic tags

Docker images SHALL be built from flavor-matched binaries: production
(scratch, musl) from regular musl binaries, alpine from slim musl binaries,
gnu (distroless) from full gnu binaries. Each variant SHALL publish its
legacy suffix tag plus an explicit semantic tag: `:regular` (and `latest`)
for production, `:slim` for alpine, `:full` for gnu. The `latest` re-alias to
regular SHALL land in the same release as the artifact re-alias (one
announcement). musl-based images SHALL never link rdkafka.

#### Scenario: variant binaries match flavors

- **WHEN** each docker variant image is built and smoke-run
- **THEN** production reports `(regular)`, alpine reports `(slim)`, gnu
  reports `(full)`, and each downloads its binaries from the matching
  flavor-prefixed workflow artifacts

#### Scenario: semantic tags publish alongside legacy suffixes

- **WHEN** a tag run publishes images
- **THEN** `:slim`, `:regular`, `:full`, and `latest` exist alongside the
  legacy `-alpine`/`-gnu`/no-suffix tags for the same version
