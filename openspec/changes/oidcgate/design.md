# Design: oidcgate

## Approach

Add a dedicated `LintPublishRegistration` xtask command. It calls the existing publish-order resolver, then compares its ordered publishable crate names with a checked-in TOML manifest. The manifest schema is an ordered array of unique entries, each containing a crate name and one lifecycle state: `registered`, `published-unregistered`, or `new-unpublished`. The workspace publish order is authoritative for expected names and positions; manifest order must match it exactly. Missing entries are reported as unknown lifecycle status and fail closed. `registered` is a maintainer assertion: crates.io does not expose trusted-publisher configuration for this check.

The default command reads only workspace metadata and the manifest. It reports set drift and order drift deterministically and exits non-zero for Case A and Case B. Case B is operationally actionable rather than ignorable: the gate says that the owner must perform one classic-token first publish, then register trustpub before the next OIDC release. An explicit `--online` mode queries crate-level existence, not the workspace version: HTTP 200 classifies an unregistered name as Case A, HTTP 404 as Case B, and every other status or transport/timeout error fails closed. Online observations may correct a stale nonregistered classification but never verify trustpub configuration.

Keep parsing and comparison in a focused module with pure functions and unit tests. Reuse `resolve_publish_order` instead of duplicating dependency discovery. Add the committed manifest under `scripts/xtask/`, document its update lifecycle, and invoke the check from the release quality-gate path.

## Affected crates

- `scripts/xtask`: command, manifest parsing, comparison, diagnostics, and tests.
- No runtime, DSL, component, service, language, or function crate changes.

## Architecture boundaries

This is release tooling and control-plane validation only. It does not enter Camel runtime route execution, exchange data, or component registration. It uses Cargo workspace metadata already owned by xtask and treats crates.io as an optional external observation, never as a required build input. No secrets are read or serialized.

## Phases

### Phase 1: Offline and online registration gate
- **Goal:** Add and test the deterministic xtask registration comparison, manifest, and explicit online observation seam.
- **Dependencies:** Existing `resolve_publish_order`, `toml`, and `ureq` dependencies.
- **Externally-visible types/interfaces:** `cargo xtask lint-publish-registration [--online]`; committed registration manifest.
- **Deliverable:** Working xtask command with unit and command-level tests.
- **Exit-criteria:** Offline current manifest passes; all Case A/B, malformed, order, and online failure tests pass.

### Phase 2: Release enforcement documentation and CI
- **Goal:** Make pre-tag use and continuous CI enforcement explicit.
- **Dependencies:** Phase 1 command and manifest.
- **Externally-visible types/interfaces:** AGENTS.md quality gate and CI quality-job step; release runbook procedure.
- **Deliverable:** Updated CI workflow and operator documentation.
- **Exit-criteria:** CI invokes the offline command and runbook names both chicken-and-egg remedies.

## Alternatives considered

- Parsing the fleet Markdown checklist: rejected because it is operational prose, not a stable repository contract.
- Making crates.io queries mandatory: rejected because cold or unavailable networks must not block local pre-tag validation.
- Adding the check as a flag to `publish-order`: rejected because ordering and trustpub lifecycle are separate concerns and need distinct diagnostics.
- Warning only for new crates: rejected because a passing gate would permit a release path known to fail; the diagnostic is distinct and prescribes the manual first-publish path.
