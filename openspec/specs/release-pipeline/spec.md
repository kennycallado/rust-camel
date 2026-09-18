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
  `publish: false`, references no secrets, and carries
  `concurrency` with `cancel-in-progress: true`

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
carry a publish-input conditional.

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

### Requirement: Dev harness proves the feature closures

The reusable workflow's dev path (`publish: false`) SHALL run the
camel-cli feature-closure test suite
(`cargo test -p camel-cli --test feature_profiles`) so every dev
run re-validates the feature composition the matrix legs build
(resolver-2 unification tripwire).

#### Scenario: dev run executes the closure tests

- **GIVEN** a dev run of the reusable workflow
- **WHEN** the checks phase completes
- **THEN** the feature-closure test suite ran as a workflow step,
  and a failure fails the run

