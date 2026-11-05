# Proposal: release-bridge-asset-verify

## Why

bd rc-6ep0i: v0.65.0 shipped 21 of 22 core assets because `softprops/action-gh-release`
treated an eventual-consistency 404 as success and never retried. The fix
(`scripts/verify-release-assets.sh`, change `release-asset-verify`) guards only the core
release job. The three bridge release workflows (`.github/workflows/{cxf,jms,xml}-bridge-release.yml`)
use the same upload action with the same blind spot and have no post-upload verification —
the identical incident class can silently ship a partial bridge release (bd rc-qd3ug).
Two factual gaps make the naive port useless: the script's directory scan filters on
`camel-*` (bridge archives and SHA256SUMS would be invisible, and the gate would pass
vacuously on an empty expected set), and the bridge `release` jobs never check out the
repository (the script would not exist at runtime). Separately, the offline stub battery
never exercises the script's repair-upload-failure WARN branch
(`scripts/verify-release-assets.sh:163-165`): every scenario's stub upload exits 0, so
the "repair failed but assets verify anyway → exit 0 + WARN" path is uncovered
(bd rc-9hgfy).

## What Changes

- Widen the script's expected-set scan to every regular file in the dist directory
  (drop the `-name 'camel-*'` filter and update the header wording). Core behavior is
  unchanged: `release-matrix.yml:410-436` proves `dist/` holds only `camel-*` files.
- Add a checkout step (the files' existing SHA-pinned `actions/checkout` `# v4`) as the
  first step of each bridge `release` job so the verification script exists.
- Add a **"Verify release assets"** step immediately after "Create GitHub Release" in
  `cxf-bridge-release.yml`, `jms-bridge-release.yml`, and `xml-bridge-release.yml`,
  mirroring the core job (`release-matrix.yml:448-462`): `GH_TOKEN` bound from
  `github.token`, `shell: bash`, `scripts/verify-release-assets.sh "$GITHUB_REF_NAME" artifacts`,
  bd comment retained.
- Extend the battery (`scripts/tests/verify-release-assets-test.sh`) with a 9th scenario
  (stub `gh release upload --clobber` exits 1, post-repair listing verifies, assert exit
  0 plus the exact WARN line on stderr and no `MISSING:` lines) and a 10th regression
  scenario (bridge-named dist set with one asset missing from the listing → exit 1
  naming it; guards the vacuous-pass class). The existing 8 scenarios stay
  behavior-identical.

Excluded: no further `verify-release-assets.sh` behavior change, no core-release
workflow changes, no new workflow files, no permission changes.

## Acceptance criteria

- All three bridge workflows: `release` job checks out the repo before verification and
  runs the verify step after Create GitHub Release on every release cut (no `if:`
  guards); each job keeps `permissions: contents: write`.
- The script's expected set is prefix-agnostic; a missing bridge-named asset fails the
  gate (battery regression scenario proves it).
- `scripts/tests/verify-release-assets-test.sh` passes 10/10 offline.
- All existing scenario names of the `release-asset-verify` capability remain in the
  MODIFIED delta block (archive-safe).
- `shellcheck` clean on both scripts (when installed); touched workflows parse as YAML.

## Risk budget

Low: CI-only plus test-only shell changes and a one-line scan widening. Affected crates:
none (CI workflows + shell scripts only). Risks: the scan widening is the only script
behavior change — bounded by the release-matrix classification check showing `dist/`
holds only `camel-*` files, so the core gate's expected set is identical. A wrong
artifacts path or missing checkout/GH_TOKEN would fail bridge releases at cut time —
mitigated by mirroring the proven core-job invocation and YAML/order assertions in the
gate. Out of bounds: widening permissions, changing upload pins, further script logic
changes.
