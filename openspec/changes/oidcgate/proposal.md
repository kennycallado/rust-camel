# Proposal: oidcgate

## Why

The release workflow uses crates.io OIDC trusted publishing, but publish order and OIDC registration can drift as new publishable crates are added. A crate that already exists on crates.io but is not registered fails later with an opaque trusted-publishing error. A new crate cannot be registered before its first publication, so its first publication needs an explicit owner-controlled classic-token path. bd rc-qvjcp requests a pre-tag gate that makes both cases visible without requiring network access.

## What Changes

- Add `cargo xtask lint-publish-registration`, reusing the existing publish-order resolver.
- Add a committed, reviewable OIDC registration manifest with the current registered crate set and lifecycle status.
- Make the default check offline and deterministic; provide optional online crates.io existence checks without storing secrets.
- Add deterministic diagnostics for published-unregistered and new-unpublished crates.
- Add the pre-tag and first-publish procedure to `docs/src/operations/oidc-publish-fallback.md`, register the check in project quality gates, and run it in CI.

Affected area: `scripts/xtask`, release operations documentation, `AGENTS.md`, and `.github/workflows/ci.yml`. No runtime crate behavior changes.

## Acceptance criteria

- Current workspace publish order passes against the committed 66-crate registration manifest.
- Drift names each crate and identifies Case A (`register first`) or Case B (manual owner first publish, then registration).
- Default execution performs no network I/O; online mode is explicit and never handles credentials.
- `publish = false` members are excluded and diagnostics are stable in publish order.
- Tests cover clean, mixed, malformed, duplicate, stale, and both chicken-and-egg cases.

## Risk budget

Acceptable risk: maintainers must update the manifest after adding or registering a crate. Out of bounds: network-dependent default checks, credentials in repository files, changes to publish mechanics, or silently treating an unregistered crate as safe.
