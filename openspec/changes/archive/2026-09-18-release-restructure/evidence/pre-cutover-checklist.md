> OBSOLETE (2026-09-18): superseded by change release-publisher-homing — the publish job returned to release.yml; NO crates.io re-registration is needed or wanted. Do not follow the steps below.

# Pre-cutover checklist: release-restructure

## Human action (owner) — BEFORE the next tag push

Re-register every crate's trusted-publisher entry on crates.io:

- owner: `kennycallado`
- repository: `rust-camel`
- workflow: `release-matrix.yml`
- environment: `crates-io`

Why: crates.io trusted publishing matches the workflow file containing the
publish job (`job_workflow_ref`). After the restructure, the publish job
lives in the reusable workflow `release-matrix.yml`, not in the
`release.yml` tag wrapper. Entries still pointing at `release.yml` fail
with `No Trusted Publishing config found for repository ...`.

The conductor surfaces this action in the merge report.
