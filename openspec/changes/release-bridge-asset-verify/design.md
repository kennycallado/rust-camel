# Design: release-bridge-asset-verify

## Approach

Four edits, one goal: the bridge release gate must actually run and actually cover
bridge assets.

1. Script scan widening (`scripts/verify-release-assets.sh`, one line). The
   expected-set scan in `verify_pass` currently filters `-name 'camel-*'`
   (line ~93); bridge archives (`cxf-bridge-*.tar.gz`, `*.zip`) and
   `*-SHA256SUMS` sidecars would be invisible and the gate would pass vacuously
   on an empty expected set. Drop the name filter so the expected set is every
   regular file in the dist directory, and update the header wording
   ("every local `camel-*` file" → "every regular file in the dist directory").
   Core behavior is provably identical: `release-matrix.yml:410-436` iterates
   all of `dist/*` and asserts every file classifies as a `camel-*` slim/full/
   regular/sidecar artifact, so the widened set equals the old set for the core
   job. Nothing else in the script changes (contract stays
   `verify-release-assets.sh <tag> <dist-dir>`, `gh release view --json assets`,
   exact name+size equality, one `--clobber` repair, WARN branch untouched).
2. Checkout in each bridge `release` job. All three jobs start at
   `download-artifact` and never check out the repository, so
   `scripts/verify-release-assets.sh` would not exist in the workspace. Add as
   the first step of each job:
   `uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v4`
   (the identical pin already used at :16 and :42 of the same files).
3. The gate step, inserted directly after "Create GitHub Release" in each of the
   three files (mirroring the core job, `release-matrix.yml:448-462`):

```yaml
      - name: Verify release assets
        # bd rc-qd3ug: independent post-upload gate for the rc-6ep0i class of
        # silent asset loss (eventual-consistency 404 treated as success).
        env:
          GH_TOKEN: ${{ github.token }}
        shell: bash
        run: scripts/verify-release-assets.sh "$GITHUB_REF_NAME" artifacts
```

   Tag source: `$GITHUB_REF_NAME` is exactly the release tag
   (`cxf-bridge-v*`/`jms-bridge-v*`/`xml-bridge-v*`). Dist dir: `artifacts`,
   where the job's `download-artifact` merge lands. `gh` and `jq` are
   preinstalled on `ubuntu-latest`. No `if:` guard: the gate runs on every
   release cut. All existing steps and pins stay untouched; the script's repair
   path needs `contents: write`, already held by all three `release` jobs.

4. Battery (`scripts/tests/verify-release-assets-test.sh`) gains two scenarios,
   existing 8 behavior-identical:
   - 9th, `repair_upload_failure_then_verify` + `t_repair_upload_failure_warns_and_reverifies`
     (bd rc-9hgfy): stub view serves the missing-a listing until an upload has
     been attempted (`upload_log_has_a`), then the full listing; stub upload
     records the `--clobber` call then exits 1. Runner: `run_verify 2 2`; assert
     exit 0, stdout `Verified release assets for tag '$TAG' after repair`,
     stderr exact `WARN: repair upload failed; re-verifying from a fresh listing anyway`,
     no `MISSING:` lines, uploads exactly `camel-a.tar.gz`, view count > 2.
   - 10th, regression for the widened scan: `make_bridge_dist` writes
     `cxf-bridge-9.9.9.tar.gz` (100 B), `cxf-bridge-9.9.9.zip` (200 B),
     `cxf-bridge-9.9.9-SHA256SUMS` (64 B); fixture listing holds only the
     tarball and SUMS. Scenario `bridge_missing` + `t_bridge_named_missing_asset_fails`:
     `run_verify 2 1`; assert exit 1, exactly one `MISSING: cxf-bridge-9.9.9.zip`
     line, uploads exactly `cxf-bridge-9.9.9.zip` (repair targets bridge names
     too — a name-filtered scan would have exited 0 vacuously here).
   The battery prints computed counts (`%d passed, %d failed`); no count text
   needs editing.

## Affected crates

- None: CI workflows and shell scripts only. No Rust code, no `Cargo.toml`.

## Architecture boundaries

No Runtime/DSL/Components/Services/Languages/Functions surface is touched. The change
extends the existing release-engineering capability (`release-asset-verify`) to the three
bridge release surfaces, makes its expected set prefix-agnostic, and closes two
test-coverage gaps (WARN branch, name-agnostic scan) in that capability's offline battery.

## Non-goal reconciliation

The mission order's non-goal ("no change to `verify-release-assets.sh` behavior beyond
what rc-9hgfy needs") predates the spec-bless finding that the `camel-*` name filter
makes the bridge gate vacuous. The scan widening is the minimal script change rc-qd3ug
requires for its gate to cover bridge assets at all; it is bounded (core expected set
provably unchanged) and is recorded in the park as a deliberate non-goal adjustment.

## Alternatives considered

- Port the verify step without touching the script: rejected — with the `camel-*` filter
  the step verifies an empty set and always passes (spec-bless blocker, demonstrated
  offline).
- Widen selection by adding bridge prefixes alongside `camel-*`: rejected — prefix
  enumeration rots as new surfaces appear; "every regular file in the dir" is the
  invariant the core job already relies on.
- Retry inside `action-gh-release` (fork/pin bump): rejected — new third-party surface;
  the core job standardized on the script gate.
- Verify step inside the build matrix jobs: rejected — assets are uploaded by the single
  `release` job after artifact merge; verification belongs where the release is cut.
- Drive the WARN branch by a stub upload that exits 1 forever: rejected — today's stub
  upload always exits 0, and the existing
  `t_failed_repair_uploads_unverified_set_and_reverifies` failure comes from the
  listing still missing the asset, not from upload-command failure. The new branch
  needs upload exit 1 AND eventual listing success — a distinct stub scenario.
- Widen job permissions: rejected — all three `release` jobs already hold
  `contents: write`; no permission change is needed or allowed.
