### Workspace version bump prompt

Bump the rust-camel workspace version. Released via tag `vX.Y.Z` →
`.github/workflows/release.yml` (7-target build matrix → GitHub Release →
`scripts/publish-crates.sh` to crates.io → Docker images ghcr + dockerhub, amd64+arm64).

The versioning mechanism is documented as a comment in `Cargo.toml` above
the exact-pinned internal deps (~lines 70-75): exact pinning (`=X.Y.Z`) of
all internal deps ensures `cargo publish` verifies against the version
being released.

**Steps (OLD=A.B.C → NEW=X.Y.Z):**

1. Bump `[workspace.package].version` in `Cargo.toml` (~line 58).
2. Update all exact-pinned internal deps (the `version = "=OLD"` block).
   Canonical command from the Cargo.toml comment:
   `sed -i 's/version = "=OLD"/version = "=NEW"/g' Cargo.toml`
3. Regenerate the lockfile: `cargo generate-lockfile`. No build needed —
   pin consistency is what matters.
4. Commit: `chore(release): bump version to X.Y.Z` — SUBJECT ONLY, no body,
   no bullets, no Bd reference (matches v0.37.0/v0.38.0).
5. Local tag: `git tag vX.Y.Z`

Note: README carries no version reference — nothing to update there.

**Do NOT `git push`.** The human pushes the tag to trigger `release.yml`.
