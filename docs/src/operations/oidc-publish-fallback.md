# OIDC publish fallback runbook

Audience: the maintainer on deck when a `v0.45.x` crates.io publish fails
under OIDC trusted publishing. The tag wrapper `.github/workflows/release.yml`
homes the crates.io publish job; the reusable workflow
`.github/workflows/release-matrix.yml` builds the 7-target matrix, artifacts,
images, and the GitHub Release. Publishing uses a cargo credential
provider that exchanges the
GitHub Actions OIDC token for short-lived crates.io tokens
(`scripts/trustpub-credential-helper.sh`). The `CARGO_REGISTRY_TOKEN`
repository secret is kept ONLY as this documented one-shot fallback.

## Before tagging: registration gate

Run the offline gate before creating any release tag:

```bash
cargo xtask lint-publish-registration
```

The gate compares the workspace publish order against the committed
registration manifest (`scripts/xtask/trustpub-registrations.toml`) and
exits non-zero on drift. Resolve every finding before tagging:

- **Case A** (`published-unregistered`): the crate exists on crates.io but
  trustpub is not registered for it. Remedy: `register first` — register
  the crate on crates.io (owner action, ~1 minute), then re-run the gate.
- **Case B** (`new-unpublished`): the crate has never been published.
  Remedy: owner manual first publish through the classic
  `CARGO_REGISTRY_TOKEN` path, then register trustpub, then update the
  manifest state to `registered` (see the first-publication lifecycle
  below).

`--online` is optional diagnostic evidence only: it observes crate-level
existence on crates.io for every publishable crate the manifest does not
assert as `registered` — including names with no manifest entry (HTTP 200
classifies the name as Case A, HTTP 404 as Case B) — and may correct a
stale or missing manifest assertion, but it does NOT prove trustpub
registration. The offline gate is the release gate; online mode never
replaces it.

## First publication lifecycle

Each new crate's first publication is owner-controlled through the classic
`CARGO_REGISTRY_TOKEN` path, followed immediately by trustpub registration.
OIDC trusted publishing is the steady-state path for subsequent releases.

1. Owner publishes the crate once with the classic token (the
   `CARGO_REGISTRY_TOKEN` secret in the release workflow).
2. Owner registers trustpub for the crate on crates.io (registration
   tuple: owner `kennycallado`, repository `rust-camel`, workflow
   `release.yml`, environment `crates-io`).
3. Maintainer updates the manifest state
   (`scripts/xtask/trustpub-registrations.toml`) to `registered` after
   first publication or any registration change, preserving publish order.

The manifest is a maintainer assertion: crates.io does not expose
trusted-publisher configuration to the gate, so the committed state must
be kept current by hand.

## First: diagnose, do not fall back

1. Open the failed `publish` job log. Find the crate that failed and the
   error class:
   - `No Trusted Publishing config found for repository ...` — a crate is
     NOT registered on crates.io (registration tuple: owner
     `kennycallado`, repository `rust-camel`, workflow `release.yml`,
     environment `crates-io`). Register it (owner action, ~1 minute on
     crates.io), then re-run — no fallback needed.
   - OIDC claim/exchange errors (`invalid_claims`, audience mismatch,
     token mint failures) — check the `crates-io` environment exists and
     the job ran with `permissions: id-token: write` (it does in the
     workflow; a manual edit may have broken it).
   - Network/timeout or crates.io 5xx — re-run first; transient failures
     do not need the fallback.
2. Re-running is ALWAYS safe: `cargo xtask publish` (the publish job's
   engine) skips crates whose version already exists on crates.io
   (`scripts/xtask/src/main.rs`, `crate_exists_on_crates_io`). A
   half-published release resumes where it stopped. First re-run
   mechanism: the Actions UI "Re-run failed jobs" button on the publish
   job (transient failures — no tag moves). If the failure is persistent
   or the tree must change, delete the remote tag and push it again (see
   the re-tag recipe below) — the workflow re-triggers on the tag push.

## The fallback: revert-commit path (token publish)

`release.yml` (the tag wrapper) triggers ONLY on `v*` tag pushes and
homes the crates.io publish job; the reusable `release-matrix.yml` builds
artifacts, images, and the GitHub Release — there is no
`workflow_dispatch` input. The fallback is therefore a temporary revert of
the OIDC commit on the release line, a moved tag, and a re-run; the pre-OIDC
workflow publishes with the `CARGO_REGISTRY_TOKEN` secret.

```bash
# 0. On the tagged release commit (vX.Y.Z), verify the tree carries OIDC:
git show vX.Y.Z:.github/workflows/release.yml | grep -c credential-provider  # > 0

# 1. Create the fallback commit ON TOP of the tagged commit
#    (version files unchanged — only the workflow swaps back to the token):
git checkout -b fallback/vX.Y.Z vX.Y.Z
git revert <oidc-commit-sha>            # restores: env CARGO_REGISTRY_TOKEN,
                                        # removes: credential-provider install
git push -u origin fallback/vX.Y.Z      # keep the audit trail on the remote

# 2. Move the tag to the fallback commit and re-trigger:
git tag -f vX.Y.Z fallback/vX.Y.Z
git push origin :refs/tags/vX.Y.Z       # delete remote tag
git push origin vX.Y.Z                  # re-push -> workflow re-runs

# 3. Watch the publish job: it runs the token workflow, and
#    xtask publish SKIPS every crate already published under OIDC —
#    only the failed remainder publishes with the token.

# 4. After the release is fully green, RESTORE the OIDC path:
git checkout main
git revert <revert-commit-sha>          # re-applies the OIDC workflow
git push origin main                    # human pushes; never automate
# The tag may stay on the fallback commit: the release is published and
# immutable; the NEXT release tag cuts from main and carries OIDC again.
```

## Rules

- The `CARGO_REGISTRY_TOKEN` secret is deleted ONLY after the first fully
  green OIDC release, and ONLY by the owner (human action). A partially
  registered set half-publishes a version — unrecoverable without yanking.
- One fallback per release at most: if the token publish also fails, stop
  and debug; do not iterate blind re-tags.
- Never publish with a personal `cargo login` token from a workstation as
  a shortcut — the audit trail must stay inside the workflow.

## Reference

- Registration tuple (identical for every crate): owner `kennycallado`,
  repository `rust-camel`, workflow `release.yml`, environment `crates-io`.
  The crates.io publish job MUST remain homed in `release.yml` because
  trusted publishing matches `job_workflow_ref` (the workflow file
  containing the publish job) against each crate's registration, and all
  60+ crates are registered against `release.yml`. The machine-generated
  per-crate list regenerates with `cargo xtask publish-order`.
- Wire format and protocol evidence: mission `oidc-prep` (bd rc-bpyz),
  CI runs 34630411072 / 34631907359.
