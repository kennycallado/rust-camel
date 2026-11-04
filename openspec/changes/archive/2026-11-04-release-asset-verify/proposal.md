# Proposal: release-asset-verify

## Why

v0.65.0 (Release run 41810273383) published **21 of 22** assets: `camel-full-aarch64-unknown-linux-gnu.tar.gz` never landed while its `.sha256` sidecar did. The `softprops/action-gh-release@v3.0.3` log shows the root cause: `error updating release asset metadata ... HttpError: Not Found` followed by `Matching asset is present after refresh; continuing...`. The action treats a GitHub eventual-consistency 404 as success and does not retry the upload. Nothing after the upload step verifies the release, so a truncated release ships silently (bd rc-6ep0i).

## What Changes

- **New verification script** `scripts/verify-release-assets.sh`, called from the `release` job of `.github/workflows/release-matrix.yml` immediately after `Create release` (the job already grants `contents: write`; the step binds `GH_TOKEN: ${{ github.token }}` for the `gh` CLI):
  - Derives the expected asset set from the local `dist/camel-*` files (not a hardcoded list).
  - Reads the release's actual asset list via `gh release view --json assets` and asserts every expected file is present with a remote size exactly equal to the local file (zero-byte or truncated assets are treated as unverified and repaired).
  - Bounded retry with backoff for the transient 404 (6 attempts, ~60s, sleep overridable for tests).
  - Idempotent repair: if assets are still missing, re-uploads them with `gh release upload --clobber` and verifies again.
  - After the last attempt, exits non-zero and prints exactly which assets are missing.
- **Stub tests** `scripts/tests/verify-release-assets-test.sh`: scenarios (success, missing-asset, 404-then-success, persistent-404, successful repair, failed repair) using a stubbed `gh` on `PATH` — no GitHub access. Wired into the ci.yml `quality` job (Quality Gates) so they run in the normal battery.
- **Workflow comment** linking bd rc-6ep0i explaining why the verification step exists (so it is not deleted as redundant).

Explicitly excluded: no change to the asset naming scheme, the build matrix, the flavor-suffix contract, or packaging. This is verification and repair only. `gh` is already authenticated in the release job.

## Acceptance criteria

- The release job fails when the published asset set is incomplete, and its output names every missing asset.
- A transient 404 during asset listing is absorbed by retry; the job succeeds when assets verify.
- Stub tests for all scenarios pass without network access and run in CI.
- `bash -n` passes on every touched script.

## Risk budget

Verification runs only in the release job and adds at most ~90s (initial retry window ~60s + post-repair verification ~30s). Repair re-uploads only unverified assets — absent or size-mismatched files — and never touches assets already verified present (repair targets exactly the unverified set). Out of bounds: replacing the release action, renaming assets, changing the matrix.

Affected crates: none (CI/scripts only). Bd: rc-6ep0i.
