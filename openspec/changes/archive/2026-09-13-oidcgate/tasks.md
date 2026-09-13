# Tasks: oidcgate

## Phase 1: Offline and online registration gate

### scripts/xtask

### Task 1.1: Add manifest model and deterministic drift diagnostics

**Files:**
- `scripts/xtask/src/main.rs` (modified)
- `scripts/xtask/src/lint_publish_registration.rs` (new)

**Steps:**
1. Define `RegistrationState` with only `Registered`, `PublishedUnregistered`, and `NewUnpublished` values and deserialize the ordered TOML manifest while rejecting unknown states, duplicate names, and malformed entries.
2. Add `mod lint_publish_registration;` to `main.rs` so the new module is compiled by the xtask binary.
3. Define pure comparison functions that accept ordered publishable crate names and manifest entries, report missing/stale names, exact position drift, and lifecycle-specific Case A/Case B diagnostics in input order.
4. Implement exact remediation text: Case A contains `register first`; Case B instructs the owner to perform a manual `CARGO_REGISTRY_TOKEN` first publish, then register trustpub.
5. Add unit tests for clean 66-style reconciliation, `publish = false` exclusion at the comparison boundary, mixed deterministic Case A/B findings, missing/stale names, duplicate names, unknown states, malformed TOML, and order-only drift.

**Tests:**
- `registered_manifest_matches_publish_order`: arrange unique ordered publishable names and all `registered` entries; act by comparing; assert no findings.
- `mixed_case_findings_are_publish_ordered`: arrange two Case A and Case B entries in interleaved publish order; act by comparing; assert output order matches publish order and contains exact `register first` and manual first-publish text.
- `manifest_validation_rejects_invalid_input`: arrange malformed TOML, duplicate names, and unknown state fixtures; act by parsing; assert each returns a named validation error.
- `order_drift_is_reported`: arrange equal name sets with different positions; act by comparing; assert non-zero findings identify expected and actual positions.
- Command: `cargo test -p xtask lint_publish_registration`.
- Expected before implementation: tests fail to compile or fail assertions; after implementation: all pass.

**Acceptance:**
- Parser accepts only the documented three states and ordered unique entries.
- Pure comparison has deterministic diagnostics for all Case A/B and set/order scenarios.
- `cargo test -p xtask lint_publish_registration` exits 0 after implementation.

- [x] 1.1

### Task 1.2: Wire offline and explicit online xtask command

**Files:**
- `scripts/xtask/src/main.rs` (modified)
- `scripts/xtask/src/lint_publish_registration.rs` (modified)
- `scripts/xtask/trustpub-registrations.toml` (new)

**Steps:**
1. Register `LintPublishRegistration { online: bool }` in the Clap command enum and dispatch it through `main.rs` using the existing `resolve_publish_order` result.
2. Load the committed manifest from a workspace-root-relative path, filter out workspace members with `publish = false` through the existing publish-order resolver, and print deterministic diagnostics with non-zero exit for findings.
3. Implement `--online` as an explicit crate-level crates.io existence observation: HTTP 200 means Case A for an unregistered name, HTTP 404 means Case B, and every other status or transport/timeout error fails closed; default mode must not construct a network client or request.
4. Add a `PUBLISH_REGISTRATION_BASE_URL` environment override for tests, defaulting to the fixed crates.io API base URL; never read or serialize credentials.
5. Populate the manifest with the current registered publishable crate names and `registered` state, preserving publish order; document its maintainer assertion in comments.
6. Add command-level tests proving default offline execution, online 200/404 classification, unexpected status/timeout failure, and stale `new-unpublished` correction without exposing credentials.

**Tests:**
- `offline_command_does_not_query_network`: arrange a valid manifest and a test HTTP server that would fail if contacted; act by running command without `--online`; assert comparison completes without a request.
- `online_statuses_classify_cases`: arrange mocked 200 and 404 responses; act with `--online`; assert 200 reports `register first`, 404 reports manual first-publish guidance, and both exit non-zero.
- `online_unexpected_status_fails_closed`: arrange mocked 500 and timeout responses; act with `--online`; assert error output names observation failure and no success result is returned.
- `online_corrects_stale_new_unpublished`: arrange `new-unpublished` plus mocked 200; act with `--online`; assert Case A output supersedes the stale state.
- Command: `cargo test -p xtask lint_publish_registration`.
- Expected before implementation: command and tests fail; after implementation: all pass without repository secrets.

**Acceptance:**
- `cargo xtask lint-publish-registration` is offline by default and exits 0 for the committed current manifest.
- `cargo xtask lint-publish-registration --online` uses only explicit crate-existence observation and fails closed on non-200/404 or transport errors.
- `publish = false` crates are absent from expected names and manifest reconciliation.
- `cargo fmt --check` and `cargo clippy -p xtask -- -D warnings` exit 0.

- [x] 1.2

## Phase 2: Release enforcement documentation and CI

### Task 2.1: Document pre-tag gate and first-publish lifecycle

**Files:**
- `docs/src/operations/oidc-publish-fallback.md` (modified)
- `AGENTS.md` (modified)
- `.github/workflows/ci.yml` (modified)

**Steps:**
1. Add a pre-tag checklist requiring `cargo xtask lint-publish-registration` before release tagging and define how to resolve Case A and Case B output.
2. Document that each new crate's first publication is owner-controlled through the classic `CARGO_REGISTRY_TOKEN` path, followed immediately by trustpub registration; OIDC is the steady-state path. Require maintainers to update the manifest state after first publication or registration changes.
3. Add the offline xtask command to the canonical quality-gates list without adding secrets or making online access mandatory.
4. Add a CI quality-job step that runs the offline command.
5. State that `--online` is optional diagnostic evidence and does not prove trustpub registration.

**Tests:**
- `runbook_contains_pre_tag_gate`: arrange edited runbook and AGENTS.md; act by searching prose; assert both name `cargo xtask lint-publish-registration` and distinguish Case A/B remedies.
- `runbook_states_manual_first_publish`: arrange runbook text; act by searching lifecycle section; assert it names owner manual `CARGO_REGISTRY_TOKEN` first publish followed by trustpub registration.
- Command: `python3 -c "from pathlib import Path; p=Path('docs/src/operations/oidc-publish-fallback.md').read_text(); a=Path('AGENTS.md').read_text(); c=Path('.github/workflows/ci.yml').read_text(); assert 'cargo xtask lint-publish-registration' in p and 'cargo xtask lint-publish-registration' in a and 'CARGO_REGISTRY_TOKEN' in p and 'lint-publish-registration' in c"`.
- Expected before implementation: assertion fails; after implementation: command exits 0.

**Acceptance:**
- Runbook has executable pre-tag instructions for both chicken-and-egg cases.
- AGENTS.md lists the offline gate as a canonical quality gate.
- Documentation contains no credentials or fabricated online registration guarantees.

- [x] 2.1
