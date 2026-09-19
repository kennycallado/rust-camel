# release-pipeline Specification

## Purpose
TBD - created by archiving change release-restructure. Update Purpose after archive.
## Requirements
### Requirement: Release build matrix has a single reusable source of truth

The release build matrix (legs, targets, runners, feature
composition, probes) SHALL live in exactly one reusable workflow
(`release-matrix.yml`, `on: workflow_call`) invoked by wrappers.
The tag wrapper (`release.yml`) SHALL contain no inline matrix: it
declares the tag trigger, the publish permissions
(`contents: write`, `packages: write`, `id-token: write`), the
secrets handoff (`secrets: inherit`), and a single call with
`publish: true`. The tag wrapper SHALL also home the crates.io
publish job (see Requirement: Publish side effects are gated on the
publish input) — this homing is a hard constraint: crates.io
trusted publishing matches the OIDC `job_workflow_ref` (the
workflow file containing the publish job) against each crate's
registered entry, and the 60+ published crates are registered
against `.github/workflows/release.yml`.

The tag matrix SHALL build the three-flavor topology (12 entries,
11 uploading): slim on x86_64/aarch64 musl; regular on the 4 Linux
targets plus a macOS compile-guard entry (upload: false, protects
cargo-install source builds); full on gnu ×2 (aarch64 on the native
`ubuntu-24.04-arm` runner), macOS ×2, and Windows. Desktop platforms
ship full-only (owner ruling post-rc.2 CI rehearsal). Each leg SHALL select its closure exclusively via the
`flavor-<x>` marker feature (`--features flavor-<x>`); no leg SHALL
compose feature lists by string interpolation. The reusable
workflow SHALL accept a `dev-profile` boolean input (default
`false`); when true, the build matrix reduces to the 3-leg
representative dev subset.

#### Scenario: tag wrapper is thin

- **GIVEN** `.github/workflows/release.yml` and the reusable
  `.github/workflows/release-matrix.yml`
- **WHEN** the tag wrapper file is inspected
- **THEN** it declares the `v*` tag trigger, the three publish
  permissions, `secrets: inherit`, exactly one job that calls the
  reusable workflow with `publish: true`, and the crates.io
  publish job (with `environment: crates-io` and
  `needs: call-release-matrix`), and contains no build matrix and
  no probe steps

#### Scenario: dev wrapper calls the same matrix

- **GIVEN** `.github/workflows/release-dev.yml`
- **WHEN** a push to a tracked branch or a `workflow_dispatch` runs
  it
- **THEN** it invokes the identical reusable workflow with
  `publish: false` and `dev-profile: true`, references no secrets,
  and carries `concurrency` with `cancel-in-progress: true`

#### Scenario: dev profile trims the build matrix

- **GIVEN** the reusable workflow invoked with `publish: false` and
  `dev-profile: true`
- **WHEN** the build matrix is expanded
- **THEN** exactly 3 build legs run — x86_64-unknown-linux-gnu
  (full), x86_64-unknown-linux-musl (regular), x86_64-apple-darwin
  (regular) — and the closure-check job runs unchanged; with
  `dev-profile: false` (tag path) all 12 entries run

### Requirement: Publish side effects are gated on the publish input

Every step or job in the reusable workflow whose execution has
external side effects — registry
logins, docker push or `imagetools` manifest creation, GitHub
release creation, or any future credential-provider install — SHALL
carry `if: inputs.publish` (directly or
via its parent job). With `publish: false`, none of these steps
SHALL execute, and no credential material SHALL be requested or
installed. crates.io publishing is NOT gated by the input: it is
gated structurally — the publish job exists only in the tag
wrapper, whose sole trigger is the `v*` tag push, and it SHALL NOT
carry a publish-input conditional. The publish job SHALL
additionally skip prerelease tags: it SHALL NOT execute when the
pushed tag matches `v*-rc.*`, so a throwaway smoke tag exercises
the matrix, release assets, and docker logins without publishing
to crates.io.

#### Scenario: dev run stays side-effect-free

- **GIVEN** the reusable workflow invoked with `publish: false`
- **WHEN** the run completes
- **THEN** no login, push, release, or publish step executed; the
  docker image was built with `buildx --load` into the local
  daemon and smoke-run locally (`--version` flavor assert), and no
  `push` verb was reachable

#### Scenario: tag run publishes as before

- **GIVEN** the reusable workflow invoked with `publish: true` from
  the tag wrapper
- **WHEN** the run completes
- **THEN** the publish path behaves equivalently to the
  pre-refactor tag pipeline: same legs built, same probes executed,
  same artifacts, images, and release outputs produced, and the
  crates.io publish job (homed in the wrapper) publishes under
  `job_workflow_ref = .github/workflows/release.yml` — with one
  accepted difference: the publish job waits for ALL matrix jobs
  (`needs: call-release-matrix`), so a matrix failure (e.g. docker)
  blocks crates.io publishing

#### Scenario: prerelease tag skips crates.io publish

- **GIVEN** a tag matching `v*-rc.*` pushed to the repository
- **WHEN** the tag pipeline completes
- **THEN** the matrix, release assets, and docker publish steps
  ran, and the crates.io publish job was skipped without failing
  the run

### Requirement: Dev harness proves the feature closures

The reusable workflow's dev path (`publish: false`) SHALL run the
camel-cli feature-closure test suite
(`cargo test -p camel-cli --test feature_profiles`) so every dev
run re-validates the feature composition the matrix legs build
(resolver-2 unification tripwire). The suite SHALL include the
flavor contract sets (`SLIM_FORBIDDEN_PREFIXES`,
`REGULAR_REQUIRED`, `REGULAR_FORBIDDEN`, `FULL_REQUIRED`) proving
the three flavor bodies and the slim fail-closed security
rejection.

#### Scenario: dev run executes the closure tests

- **GIVEN** a dev run of the reusable workflow
- **WHEN** the checks phase completes
- **THEN** the feature-closure test suite ran as a workflow step,
  and a failure fails the run

