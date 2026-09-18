# Proposal: release-publisher-homing

## Why

`release-restructure` (rc-5t5fo.4) moved the crates.io publish job
into `release-matrix.yml`. crates.io trusted publishing verifies the
OIDC `job_workflow_ref` — the workflow FILE containing the publish
job — against each crate's registered entry. All 60+ published crates
are registered with workflow `release.yml`. The next `v*` tag push
would therefore FAIL crates.io publishing for every crate (fails at
crate 1; recoverable but blocking). crates.io exposes no API for
trusted-publisher configuration: the only alternative is manual
re-registration of 60+ crates in the UI.

## What Changes

- Move the `publish` job (trustpub credential-provider install +
  `scripts/publish-crates.sh`, `environment: crates-io`) from
  `release-matrix.yml` back into the `release.yml` tag wrapper,
  verbatim, with `needs: call-release-matrix`, dropping the
  `if: inputs.publish` gate (the tag-only trigger is the structural
  gate). `job_workflow_ref` returns to `release.yml`; all 60+
  registrations stay valid. Owner confirmed: zero crates were
  re-registered to `release-matrix.yml`.
- Shrink the matrix `publish` input semantics to the image/registry
  effects it still gates (docker logins/pushes/attest/imagetools,
  gh-release); update its description text and move the in-file
  trusted-publisher comment block with the job.
- Revert `docs/src/operations/oidc-publish-fallback.md` tuples and
  verify command to `release.yml`, KEEPING an explicit WHY-constraint
  (the publish job must remain homed in `release.yml`); same for
  bump-version instruction and prompt.
- Small ADR recording the homing constraint + CONTEXT-MAP decisions
  line; OBSOLETE banner on the archived pre-cutover checklist.

## Capabilities

### Modified
- `release-pipeline`: wrapper shape (Req 1) and publish gating scope
  (Req 2) — crates.io gating becomes structural (job homed in the
  tag wrapper) instead of input-gated inside the matrix.

## Impact

- Files: `.github/workflows/release.yml`, `.github/workflows/
  release-matrix.yml`, `docs/src/operations/oidc-publish-fallback.md`,
  `.opencode/instructions/bump-version.md`,
  `.opencode/prompts/bump-version.md`, new ADR, `CONTEXT-MAP.md`,
  archived checklist banner.
- Accepted behavioral difference: the wrapper publish job waits for
  ALL matrix jobs (`needs: call-release-matrix`); a docker failure
  now blocks crates.io publishing. Accepted: crates is the least
  reversible side effect; no partial-dependency mechanism exists.
- No Rust code changes. bd rc-s926l (p1).
