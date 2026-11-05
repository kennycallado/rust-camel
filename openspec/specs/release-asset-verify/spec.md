# release-asset-verify Specification

## Purpose
TBD - created by archiving change release-asset-verify. Update Purpose after archive.
## Requirements
### Requirement: Release job verifies the published asset set

Every release job that publishes assets via `softprops/action-gh-release` — the core
release matrix job and the `cxf`, `jms`, and `xml` bridge release jobs — SHALL,
immediately after its `Create release` / `Create GitHub Release` step, verify that every
expected release asset is actually present on the GitHub release before the job can
succeed. The expected set SHALL be every regular file in the job's local distribution
directory (`dist/` for the core job, the `artifacts/` directory for the bridge jobs),
selected by directory scan WITHOUT filtering on file-name prefix, so that core
(`camel-*`) assets, bridge archives (`*-bridge-*.tar.gz`, `*.zip`), and SHA256SUMS
sidecars are all verified; a scan that silently excludes part of the distribution
directory would pass vacuously on an empty expected set and SHALL NOT be used. An asset
is verified only when it is present on the release with a remote size exactly equal to
the local file's size; a present-but-truncated (size-mismatched) asset, including any
zero-byte asset, SHALL be treated as not verified and SHALL be eligible for repair like
a missing asset. Verified assets SHALL never be re-uploaded or deleted. The verification
gate SHALL run on every release cut without an `if:` guard. The bridge `release` jobs
SHALL check out the repository (SHA-pinned `actions/checkout`) before the verification
step so the verification script exists in the job workspace.

#### Scenario: All expected assets present

- **GIVEN** a release whose published assets match the `dist/camel-*` file set, each with a size equal to its local file
- **WHEN** the verification script runs
- **THEN** it exits 0 and the release job proceeds

#### Scenario: Asset missing or size-mismatched on the published release

- **GIVEN** the release is missing one or more files present in `dist/`, or holds one with a size differing from the local file (for example a truncated `camel-full-aarch64-unknown-linux-gnu.tar.gz`)
- **WHEN** the verification script has exhausted its bounded retry and repair attempts
- **THEN** the script exits non-zero and its output names every failing asset file
- **AND** the release job fails

#### Scenario: Bridge release verifies its published assets

- **GIVEN** a bridge release is cut by one of the `cxf-bridge-release.yml`, `jms-bridge-release.yml`, or `xml-bridge-release.yml` workflows, whose `release` job checks out the repository, uploads `artifacts/*` via `softprops/action-gh-release`, and holds `permissions: contents: write`
- **WHEN** the `Create GitHub Release` step finishes
- **THEN** a `Verify release assets` step runs immediately after it with `GH_TOKEN` bound to `github.token`, invoking `scripts/verify-release-assets.sh "$GITHUB_REF_NAME" artifacts` with `shell: bash`, and its failure fails the release job

#### Scenario: Non-core-prefixed assets are verified

- **GIVEN** a distribution directory containing bridge-named files (for example `cxf-bridge-9.9.9.tar.gz`, `cxf-bridge-9.9.9.zip`, and `cxf-bridge-9.9.9-SHA256SUMS`)
- **WHEN** the published listing omits or size-mismatches one of those bridge-named files and the script exhausts its retry and repair attempts
- **THEN** the script exits non-zero and names that file
- **AND** it never exits 0 on an expected set that silently excluded the bridge-named files

### Requirement: Bounded retry absorbs transient asset-listing 404s

The verification script SHALL retry asset-listing queries a bounded number of times with
backoff (6 attempts over ~60 seconds by default) so that GitHub eventual-consistency 404s
do not cause false failures. The retry window SHALL be overridable via environment
variables for testing. Every verification decision SHALL be based on a fresh listing: a
listing obtained in an earlier pass SHALL never be reused to declare success. If the
listing keeps failing so completeness cannot be verified, the script SHALL exit non-zero
with a diagnostic that distinguishes the unverifiable case from confirmed missing assets.

#### Scenario: Transient 404 followed by success

- **GIVEN** the asset listing fails with a 404 on the first attempt and a fresh listing succeeds on a later attempt, with all expected assets verified
- **WHEN** the verification script runs
- **THEN** it exits 0 without re-uploading

#### Scenario: Persistent listing failure

- **GIVEN** every asset-listing attempt fails (for example persistent 404) within the bounded retry window
- **WHEN** the verification script finishes
- **THEN** it exits non-zero and its output reports that the asset set could not be verified, distinct from a `MISSING:` confirmation

### Requirement: Idempotent repair re-uploads missing assets

The verification script SHALL, when assets are still unverified after the retry window
(absent or size-mismatched), re-upload exactly the unverified files using
`gh release upload --clobber` and verify again with a fresh listing. It SHALL fail only
if assets are still unverified after repair. It SHALL NOT upload, replace, or delete
assets that are already present and verified.

#### Scenario: Repair recovers a missing asset

- **GIVEN** an asset is absent on the first verification pass
- **WHEN** the re-upload succeeds on the repair pass and the asset is then verified present
- **THEN** the script exits 0

#### Scenario: Repair fails and assets remain missing

- **GIVEN** assets remain absent after repair re-upload attempts
- **WHEN** the script finishes
- **THEN** it exits non-zero and names every still-unverified asset

### Requirement: Verification is testable without GitHub

The verification script SHALL be testable offline by running with a stubbed `gh`
executable placed on `PATH`. The stub tests SHALL cover: success, missing-asset,
404-then-success, and persistent-404 scenarios; successful repair; failed repair; repair
upload followed by persistent listing failure (which SHALL exit non-zero with the
unverifiable diagnostic, never a `MISSING:` confirmation derived from stale
observations); repair-upload failure followed by successful post-repair verification
(which SHALL exit 0, SHALL emit a WARN line on stderr about
the failed repair upload,
and SHALL NOT emit `MISSING:` lines); and a name-agnostic selection regression in which
a distribution set of bridge-named files misses one asset from the listing (which SHALL
exit non-zero naming it, proving the expected-set scan does not filter by file-name
prefix). The tests SHALL require no network access, SHALL assert that repair uploads
exactly the unverified set with `--clobber` (never already-verified assets) and that
verification runs again afterwards, and SHALL run in the normal CI test battery.

#### Scenario: Stub test battery runs offline

- **GIVEN** the stub test script is executed with only a stubbed `gh` and no network
- **WHEN** all scenarios run
- **THEN** each scenario asserts the expected script exit status, the `MISSING:` output for failing assets, and the exact set of files the stub `gh` was asked to upload with `--clobber`

#### Scenario: Repair upload failure warns and re-verifies

- **GIVEN** the stubbed listing reports an asset missing until after the repair upload is attempted, and the stubbed `gh release upload --clobber` exits 1
- **WHEN** the verification script exhausts its initial attempts, issues the failed repair upload, and re-verifies from a fresh listing that now reports every asset present with matching sizes
- **THEN** the script exits 0 and reports verification after repair on stdout
- **AND** stderr contains the exact WARN line about the failed repair upload and no `MISSING:` lines

#### Scenario: Bridge-named missing asset fails the gate

- **GIVEN** a stubbed distribution set of bridge-named files (`cxf-bridge-9.9.9.tar.gz`, `cxf-bridge-9.9.9.zip`, `cxf-bridge-9.9.9-SHA256SUMS`) whose listing omits the `.zip`
- **WHEN** the verification script exhausts its initial attempts and its repair
- **THEN** it exits non-zero with exactly one `MISSING: cxf-bridge-9.9.9.zip` line
- **AND** the stub was asked to upload exactly `cxf-bridge-9.9.9.zip` with `--clobber`

