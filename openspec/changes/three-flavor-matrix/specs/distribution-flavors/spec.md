# distribution-flavors Specification

## ADDED Requirements

### Requirement: Three distribution flavors with principled bodies

The camel CLI SHALL ship in exactly three flavors, selected only via the
marker features `flavor-slim`, `flavor-regular`, `flavor-full` in
`crates/camel-cli/Cargo.toml`. Bodies SHALL be CHAINED (each flavor's list
includes the marker of the flavor below it, making slim ⊆ regular ⊆ full
structural). Exclusion from regular SHALL be justified ONLY by a named
principle: a C dependency unportable to musl (kafka), a non-OSI license
(surrealdb, BUSL-1.1), arbitrary host-binary execution (exec, ADR-0037), or
an infrastructure-daemon client (containers: camel-function +
camel-component-container as ONE feature). `flavor-full` SHALL cover the
complete feature universe of camel-cli except non-flavor axes — a
`full_covers_universe` closure test SHALL fail CI when any feature is
placed in no flavor. Enabling no marker SHALL report flavor `custom`.

#### Scenario: slim body is the edge pack

- **WHEN** `cargo build --no-default-features --features flavor-slim` resolves
- **THEN** the closure contains the base components (core, direct, seda,
  log, file, timer, http with REST DSL, stream in/out/err) plus mqtt
  (+tls), http-static, sql
  (sqlite), lang-jsonpath and lang-rhai, and contains no kafka, no exec,
  no surrealdb, no containers (camel-function/camel-component-container),
  no lang-js, no lang-xpath, and no security features

#### Scenario: regular body excludes only by principle

- **WHEN** `cargo build --features flavor-regular` resolves (default)
- **THEN** the closure is slim plus otel, grpc, wasm, llm, mcp, security,
  redis (+tls), jms, cxf, xj, xslt, opensearch, ws, lang-xpath, lang-js,
  lang-minijinja, lsp, kubernetes, integration-http and integration-sql,
  and contains none of the four principled exclusions (kafka, surrealdb,
  exec, containers)

#### Scenario: full is regular plus exactly the principled deltas

- **WHEN** `cargo build --features flavor-full` resolves
- **THEN** the closure is exactly the regular closure plus `exec`, `kafka`,
  `surrealdb` and `containers`, the kafka component is reachable (registry
  resolves `kafka` — the historical kafka-less-full defect stays fixed),
  and the container/function services are registered

#### Scenario: full covers the feature universe

- **WHEN** the feature-closure suite runs the full_covers_universe test
- **THEN** every camel-cli feature except the non-flavor axes (jemalloc,
  dynamic-linking, itest-e2e, `default`, the legacy `full` closure list,
  and the flavor markers themselves) is reachable from `flavor-full`, and
  an unplaced feature fails the test

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
  absence asserts on slim/regular) before the release is created —
  cross-compiled legs (aarch64 musl/gnu) are filename-asserted at release
  collection instead, since their binaries do not run on the build runner

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
