# Release Asset Verification

## ADDED Requirements

### Requirement: Release job verifies the published asset set

The release job SHALL, immediately after the `Create release` step, verify that every expected release asset is actually present on the GitHub release before the job can succeed. The expected set SHALL be derived from the local `dist/camel-*` files produced by the build, not from a hardcoded asset list. An asset is verified only when it is present on the release with a remote size exactly equal to the local `dist/` file's size; a present-but-truncated (size-mismatched) asset, including any zero-byte asset, SHALL be treated as not verified and SHALL be eligible for repair like a missing asset. Verified assets SHALL never be re-uploaded or deleted.

#### Scenario: All expected assets present

- **GIVEN** a release whose published assets match the `dist/camel-*` file set, each with a size equal to its local file
- **WHEN** the verification script runs
- **THEN** it exits 0 and the release job proceeds

#### Scenario: Asset missing or size-mismatched on the published release

- **GIVEN** the release is missing one or more files present in `dist/`, or holds one with a size differing from the local file (for example a truncated `camel-full-aarch64-unknown-linux-gnu.tar.gz`)
- **WHEN** the verification script has exhausted its bounded retry and repair attempts
- **THEN** the script exits non-zero and its output names every failing asset file
- **AND** the release job fails

### Requirement: Bounded retry absorbs transient asset-listing 404s

The verification script SHALL retry asset-listing queries a bounded number of times with backoff (6 attempts over ~60 seconds by default) so that GitHub eventual-consistency 404s do not cause false failures. The retry window SHALL be overridable via environment variables for testing. Every verification decision SHALL be based on a fresh listing: a listing obtained in an earlier pass SHALL never be reused to declare success. If the listing keeps failing so completeness cannot be verified, the script SHALL exit non-zero with a diagnostic that distinguishes the unverifiable case from confirmed missing assets.

#### Scenario: Transient 404 followed by success

- **GIVEN** the asset listing fails with a 404 on the first attempt and a fresh listing succeeds on a later attempt, with all expected assets verified
- **WHEN** the verification script runs
- **THEN** it exits 0 without re-uploading

#### Scenario: Persistent listing failure

- **GIVEN** every asset-listing attempt fails (for example persistent 404) within the bounded retry window
- **WHEN** the verification script finishes
- **THEN** it exits non-zero and its output reports that the asset set could not be verified, distinct from a `MISSING:` confirmation

### Requirement: Idempotent repair re-uploads missing assets

The verification script SHALL, when assets are still unverified after the retry window (absent or size-mismatched), re-upload exactly the unverified files using `gh release upload --clobber` and verify again with a fresh listing. It SHALL fail only if assets are still unverified after repair. It SHALL NOT upload, replace, or delete assets that are already present and verified.

#### Scenario: Repair recovers a missing asset

- **GIVEN** an asset is absent on the first verification pass
- **WHEN** the re-upload succeeds on the repair pass and the asset is then verified present
- **THEN** the script exits 0

#### Scenario: Repair fails and assets remain missing

- **GIVEN** assets remain absent after repair re-upload attempts
- **WHEN** the script finishes
- **THEN** it exits non-zero and names every still-unverified asset

### Requirement: Verification is testable without GitHub

The verification script SHALL be testable offline by running with a stubbed `gh` executable placed on `PATH`. The stub tests SHALL cover: success, missing-asset, 404-then-success, and persistent-404 scenarios; successful repair; failed repair; and repair-upload followed by persistent listing failure (which SHALL exit non-zero with the unverifiable diagnostic, never a `MISSING:` confirmation derived from stale observations). The tests SHALL require no network access, SHALL assert that repair uploads exactly the unverified set with `--clobber` (never already-verified assets) and that verification runs again afterwards, and SHALL run in the normal CI test battery.

#### Scenario: Stub test battery runs offline

- **GIVEN** the stub test script is executed with only a stubbed `gh` and no network
- **WHEN** all scenarios run
- **THEN** each scenario asserts the expected script exit status, the `MISSING:` output for failing assets, and the exact set of files the stub `gh` was asked to upload with `--clobber`
