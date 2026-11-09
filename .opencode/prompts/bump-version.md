### Workspace version bump prompt

Bump the rust-camel workspace version. Released via tag `vX.Y.Z` →
`.github/workflows/release-matrix.yml` (7-target build matrix → GitHub Release →
Docker images ghcr + dockerhub, amd64+arm64); the `release.yml` tag wrapper
homes the crates.io publish job (`scripts/publish-crates.sh`).

The versioning mechanism is documented as a comment in `Cargo.toml` at the
top of `[workspace.dependencies]` (search `IMPORTANT: when bumping`): exact pinning (`=X.Y.Z`) of
all internal deps ensures `cargo publish` verifies against the version
being released.

**Steps (OLD=A.B.C → NEW=X.Y.Z):**

1. Bump `[workspace.package].version` in `Cargo.toml` (under
   `[workspace.package]`).
2. Update all exact-pinned internal deps (the `version = "=OLD"` block).
   Canonical command from the Cargo.toml comment:
   `sed -i 's/version = "=OLD"/version = "=NEW"/g' Cargo.toml`
3. Refresh lockfile workspace entries: `cargo update --workspace`. No build
   needed — pin consistency is what matters. Do NOT use
   `cargo generate-lockfile`: it re-resolves the entire lockfile (a past
   release churned 662 lines of transitive deps inside the release commit).
4. Commit: `chore(release): bump version to X.Y.Z` — SUBJECT ONLY, no body,
   no bullets, no Bd reference (matches all previous release commits).
5. Verify the commit is tested — a skipped or cancelled Full Tests
   run is **not tested**. Check with
   `gh run list --workflow ci.yml --commit <SHA>`: it must show a
   completed **successful push run on main** (PR-triggered rows do
   not qualify — they test a synthetic merge commit). Then confirm
   the evidence with
   `gh run view <run-id> --json jobs --jq '.jobs[] | select(.name == "Full Tests (ubuntu-latest)") | {conclusion, steps: [.steps[] | select(.name == "Test (full workspace)" or .name == "Test camel-cli (exec flavor)") | {name, conclusion}]}'`:
   the job and both test steps must show `"conclusion": "success"`.
   A skipped or cancelled Full Tests means the release workflow's
   preflight (release.yml `ci-preflight`, built-in GITHUB_TOKEN with
   the `actions: read` scope — no secret setup) will fail the tag
   run before the matrix or publish starts. Do not tag until the
   check passes.
6. Local tag: `git tag vX.Y.Z`

Note: README carries no version reference — nothing to update there.

**Do NOT `git push`.** The human pushes the tag to trigger `release.yml`.
