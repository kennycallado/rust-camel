# distribution-flavors Specification

## Purpose
TBD - created by archiving change three-flavor-matrix. Update Purpose after archive.
## Requirements
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
path SHALL run per-flavor capability asserts against the built binary
before packaging, extending the existing symbol-assert pattern.

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
- **THEN** each leg's binary is smoke-probed for its flavor suffix in
  `--version` and its flavor capability set (kafka registered on full;
  absence asserts on slim/regular) before packaging into its tarball —
  cross-compiled legs (aarch64 musl/gnu) are tarball-name-asserted at
  release collection instead, since their binaries do not run on the
  build runner

### Requirement: Shipped artifact names are flavor-prefixed

Release artifacts SHALL be named `camel-slim-<target>.tar.gz`,
`camel-<target>.tar.gz` (regular), and `camel-full-<target>.tar.gz`,
each accompanied by its `.tar.gz.sha256` sidecar (format per the
release-pipeline checksummed-tarballs requirement). The clean legacy
name `camel-<target>` SHALL re-alias from the historical full-ish
closure to regular at the first flavored release, announced in the
release notes (one-time semantic break, bd ruling superseding the
verdict's transition-release alternative). All three names SHALL exist
from that release onward; the raw extensionless names shipped by
v0.50.0 are retired at the first tarball release (documented one-time
break).

#### Scenario: all three names ship from the first flavored release

- **WHEN** the first flavored tag release completes
- **THEN** the GitHub release carries `camel-slim-*` (musl ×2),
  `camel-*` (regular, the 4 Linux targets), and `camel-full-*` (gnu ×2,
  macOS ×2, Windows) — 11 tarballs with 11 matching `.sha256` sidecars
  (22 files); on desktop targets the clean regular name is retired
  (documented breaking change, scripts must switch to `camel-full-*`),
  and no asset name 404s relative to the previous release's names
  except by documented retirement or re-alias

### Requirement: Docker variants map to flavors with explicit semantic tags

Docker images SHALL be built from flavor-matched binaries: production
(scratch, musl) from regular musl binaries, slim (scratch, musl) from slim
musl binaries, full (distroless, gnu) from full gnu binaries. Each variant
SHALL publish its flavor-named suffix tag plus an explicit semantic tag:
`{VERSION}` and `latest` (unsuffixed family, = regular) and `:regular` for
production, `{VERSION}-slim`/`latest-slim` and `:slim` for slim,
`{VERSION}-full`/`latest-full` and `:full` for full. The `latest` tag
SHALL carry regular from 0.51.0 onward. The `-alpine` and `-gnu` legacy
suffix families SHALL NOT be pushed anymore (discontinued; existing tags
freeze). musl-based images SHALL never link rdkafka.

#### Scenario: variant binaries match flavors

- **WHEN** each docker variant image is built and smoke-run
- **THEN** production reports `(regular)`, slim reports `(slim)`, full
  reports `(full)`, and each downloads its binaries from the matching
  flavor-prefixed workflow artifacts

#### Scenario: semantic tags publish alongside legacy suffixes

- **WHEN** a stable tag run publishes images
- **THEN** `{VERSION}`, `latest`, `:regular`, `{VERSION}-slim`,
  `latest-slim`, `:slim`, `{VERSION}-full`, `latest-full`, and `:full`
  exist for the same version, no `-alpine` or `-gnu` tag is pushed, and
  the discontinued legacy suffix tags remain pullable in the registries
  at their last frozen content

### Requirement: Prebuilt assets are binstall-addressable

The `camel-cli` crate MUST carry `[package.metadata.binstall]` so that
`cargo binstall camel-cli` installs a prebuilt binary from the project's
GitHub releases instead of falling back to a long compile. Flavor selection
MUST use only `cfg(target)` overrides — binstall's template language has no
feature variable — and MUST follow the platform-ceiling rule. Flavor selection
uses only `cfg(target)` overrides — binstall's template language has no
feature variable — and follows the platform-ceiling rule: the default is
`full` everywhere full exists; targets where full cannot exist get `regular`
via a single override.

#### Scenario: non-musl target resolves to the full tarball

- **Given** the release pipeline has published flat tarballs named
  `camel-full-<target>.tar.gz` for the non-musl targets
  (`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `x86_64-apple-darwin`, `aarch64-apple-darwin`,
  `x86_64-pc-windows-msvc`)
- **When** `cargo binstall --dry-run --target <non-musl-target> camel-cli`
  resolves its download URL
- **Then** the URL is
  `{repo}/releases/download/v{version}/camel-full-<target>.tar.gz`
  with the literal `.tar.gz` suffix (never `{archive-suffix}`, which
  resolves to `.tgz`), and the URL points at an asset that exists in the
  release

#### Scenario: musl target resolves to regular through the override

- **Given** full tarballs do not exist for musl targets (librdkafka does
  not build on musl) and `camel-<target>.tar.gz` regular tarballs exist for
  `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`
- **When** `cargo binstall --dry-run --target <musl-target> camel-cli`
  resolves its download URL
- **Then** the `cfg(all(target_os = "linux", target_env = "musl"))`
  override selects
  `{repo}/releases/download/v{version}/camel-<target>.tar.gz`, and no musl
  resolution ever names a `camel-full-` asset

#### Scenario: the installed binary is named camel

- **Given** tarballs are flat — the binary sits at the archive root as
  `camel` (or `camel.exe` on Windows) while the asset filename carries the
  flavor and target prefix
- **When** binstall extracts a fetched tarball during install
- **Then** `bin-dir = "{bin}{binary-ext}"` places the binary as `camel`
  (`camel.exe` on Windows) in the cargo bin directory — never under the
  asset stem name

#### Scenario: quickinstall is disabled and compile stays enabled

- **Given** third-party quickinstall serves default-features builds that
  would masquerade as official prebuilts
- **When** the metadata's `disabled-strategies` is read
- **Then** it contains exactly `["quick-install"]` — `crate-meta-data`
  (the project's own releases) and `compile` (which yields the
  `flavor-regular` default) remain enabled

#### Scenario: smoke check maps every supported target to a real asset

- **Given** the current release's published asset list
- **When** the repeatable smoke check resolves, for every supported
  target, the URL binstall would fetch
- **Then** every resolved URL matches a published asset name exactly, and
  the check fails if any target resolves to a missing asset or falls
  through to a URL miss

### Requirement: Docker tags name flavors, not libc

The docker publish pipeline MUST name image tag families after the flavor
they carry (`-slim`, `-full`; the unsuffixed family = regular) and MUST NOT
introduce libc-named families for new releases. The `-alpine` and `-gnu`
families MUST NOT be pushed anymore: both variants are discontinued (zero
installed base — no legacy aliases). The slim flavor ships on a scratch
base (static musl binary + CA bundle, same pattern as the regular image).
Image base MUST follow the toolchain: musl flavors on scratch, gnu
flavors on distroless.

#### Scenario: stable release tag set

- **WHEN** a stable (non `-rc.`) release publishes
- **THEN** the registries (GHCR + Docker Hub) carry, for the new version:
  `{VERSION}`, `latest`, `regular`, `{VERSION}-slim`, `latest-slim`,
  `slim`, `{VERSION}-full`, `latest-full`, `full`
- **AND** no `-alpine` or `-gnu` tag is pushed (both families
  discontinued; existing tags remain in the registry, untouched)

#### Scenario: slim ships on scratch

- **WHEN** the slim variant image is built
- **THEN** its base is scratch (no OS packages, no shell) and it carries
  the slim static musl binary plus the CA bundle — the same stage pattern
  as the regular image

### Requirement: Prereleases never move floating tags

A `-rc.` tag push MUST NOT create or move any floating tag (`latest` and
its arch/suffix derivatives, and the plain semantic tags `regular`,
`slim`, `full`) in any registry. Rehearsal runs MUST still push immutable
`{VERSION}`-derived tags so the full tag logic is exercised.

#### Scenario: rehearsal run floating-tag quarantine

- **WHEN** a `v*-rc.*` tag triggers the release workflow
- **THEN** immutable tags `{VERSION}`, `{VERSION}-slim`, `{VERSION}-full`
  (and their arch intermediates) are pushed
- **AND** `latest`, `latest-slim`, `latest-full`, `regular`, `slim`,
  `full` are not touched

### Requirement: Pushed tag set is asserted

The docker publish job MUST verify, per variant, the exact set of tags it
pushed for the current run and MUST fail the run on any drift from the
expected set (stable and prerelease each have a fixed expected set).

#### Scenario: tag-set drift fails the release

- **WHEN** the manifest step computes a tag list that differs from the
  expected set for the run type (stable vs `-rc.`)
- **THEN** the job fails with a diff of expected vs actual tags

### Requirement: Unsuffixed tag family stays regular

The unsuffixed image tag family (`{VERSION}`, `latest`) MUST carry the
regular flavor permanently, consistent with the crates.io default and the
binstall metadata. The per-variant flavor smoke MUST keep asserting it.

#### Scenario: unsuffixed flavor assert

- **WHEN** the production variant image is smoke-tested with `--version`
- **THEN** the output ends with `(regular)`

