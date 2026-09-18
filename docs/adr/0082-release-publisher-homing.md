# ADR-0082: Release publisher homing (crates.io publish job pinned to release.yml)

- Status: Accepted (decided 2026-09-18; bd rc-s926l)
- Source: bd rc-s926l (e_glm adjudication 2026-09-18)

## Context

The release-restructure change (bd rc-5t5fo.4) moved the crates.io
publish job from `release.yml` into the reusable workflow
`release-matrix.yml`. That move changed `job_workflow_ref`.

Crates.io trusted publishing matches `job_workflow_ref` against each
crate's registration. `job_workflow_ref` is the workflow FILE that
contains the publish job. For a job inside a called reusable workflow,
it is the reusable file's path. For a job defined directly in a
workflow file, it is that file's path.

All 60+ crates are registered against `release.yml`. Crates.io exposes
no API for trusted-publisher configuration. Manual UI re-registration
of 60+ crates is the only alternative. The owner verified on
2026-09-18 that zero crates had been re-registered. The next tag push
would therefore fail trusted publishing for every crate.

## Decision

The crates.io publish job is permanently homed in `release.yml`, the
tag wrapper. `job_workflow_ref` stays `.github/workflows/release.yml`.

The matrix `publish` input gates image and registry effects only. It
no longer gates crates.io publishing. The tag-only trigger on the
wrapper is the structural gate for the publish job.

## Accepted cost

The wrapper grows from 18 to about 75 lines. The publish job waits for
ALL matrix jobs through `needs: call-release-matrix`. A docker failure
now blocks crates publishing. This is accepted. Crates.io publishing
is the least-reversible side effect. Failing late but together is
safer than publishing while images failed. GitHub Actions has no
partial-dependency mechanism.

## Consequences

Any future refactor that moves the publish job to another file re-opens
the 60+ re-registration problem. This ADR forbids that move without an
explicit superseding ADR.

The load-time permission-validation lesson from bd rc-42pkx stands.
GitHub validates nested job permission requests at workflow load, even
for jobs that never run. Callers must grant a ceiling that covers
nested requests. The dev wrapper keeps the permissions trio even after
the publish job left the matrix, because the docker attest step still
requests `id-token: write`.

## References

- bd rc-s926l (this decision)
- bd rc-5t5fo.4 (release-restructure; moved the publish job into the matrix)
- bd rc-42pkx (load-time permission validation; dev wrapper ceiling)
- openspec change `release-publisher-homing` (spec `release-pipeline`)
