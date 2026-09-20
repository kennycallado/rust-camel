# docker-tag-remap — delta spec

## MODIFIED Requirements

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

## ADDED Requirements

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
