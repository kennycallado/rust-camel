## ADDED Requirements

### Requirement: Offline publish registration drift gate

The release tooling SHALL provide `cargo xtask lint-publish-registration` that compares the existing workspace publish order with a committed OIDC registration manifest without network access by default.

#### Scenario: current registered publish set passes

- **GIVEN** every publishable workspace crate appears once in the manifest with state `registered` and the manifest order matches the publish-order resolver
- **WHEN** the command runs without `--online`
- **THEN** it performs no network request and exits zero

#### Scenario: publish=false members are ignored

- **GIVEN** a workspace member has `publish = false` and is absent from the registration manifest
- **WHEN** the command runs
- **THEN** the member does not create drift and the command continues to evaluate publishable members

#### Scenario: new crate is not yet published

- **GIVEN** a publishable crate is absent from the registered set and the manifest records it as `new-unpublished`
- **WHEN** the command runs
- **THEN** it reports Case B with the crate name and instructs the owner to perform a manual classic-token first publish, then register trustpub, and exits non-zero

#### Scenario: existing crate lacks registration

- **GIVEN** a publishable crate is recorded as `published-unregistered`
- **WHEN** the command runs
- **THEN** it reports Case A with the crate name and the exact remedy `register first` before tagging, and exits non-zero

#### Scenario: order drift is explicit

- **GIVEN** manifest and workspace contain the same crate set but in different publish order
- **WHEN** the command runs
- **THEN** it reports order drift with stable positions and exits non-zero

### Requirement: Manifest validation and online classification

The gate SHALL validate an ordered manifest of unique publishable crate names whose state is exactly one of `registered`, `published-unregistered`, or `new-unpublished`; `registered` SHALL be treated as a maintainer assertion because crates.io does not expose trusted-publisher configuration to this check. It SHALL reject malformed, duplicate, unknown-state, missing, or stale entries with actionable diagnostics, and SHALL support an explicit `--online` mode that uses crates.io only to observe crate-level existence.

#### Scenario: malformed manifest fails closed

- **GIVEN** the manifest has invalid TOML, duplicate crate names, or an unsupported lifecycle state
- **WHEN** the command runs
- **THEN** it reports the manifest error and exits non-zero without attempting network access

#### Scenario: missing entry fails closed

- **GIVEN** a publishable workspace crate has no manifest entry
- **WHEN** the command runs offline
- **THEN** it reports unknown registration status for that crate and exits non-zero

#### Scenario: online mode classifies missing registration entries

- **GIVEN** `--online` is supplied and a publishable crate is not in the registered manifest
- **WHEN** crates.io returns existence for the crate
- **THEN** the diagnostic classifies it as Case A and says `register first`

#### Scenario: online 404 identifies a new crate

- **GIVEN** `--online` is supplied and crates.io returns HTTP 404 for an unregistered crate name
- **WHEN** the command evaluates the response
- **THEN** it classifies the crate as Case B and reports the manual owner first-publish instruction

#### Scenario: unexpected online response fails closed

- **GIVEN** `--online` is supplied and crates.io returns a status other than HTTP 200 or 404, or the request times out
- **WHEN** the command evaluates the response
- **THEN** it reports the observation failure and exits non-zero without classifying the crate as safe

#### Scenario: online mode corrects a stale lifecycle assertion

- **GIVEN** `--online` is supplied and a missing manifest entry or `new-unpublished` assertion receives HTTP 200 from crates.io
- **WHEN** the command evaluates the response
- **THEN** it reports Case A and says `register first` rather than trusting the stale offline assertion

#### Scenario: online network failure fails closed

- **GIVEN** `--online` is supplied and crates.io cannot be reached
- **WHEN** the query fails
- **THEN** the command reports the network failure and exits non-zero without treating the crate as safe

### Requirement: Release runbook

The release runbook SHALL require the offline gate before creating a release tag and SHALL document that every new crate's first publication is manual by the owner before OIDC registration.

#### Scenario: maintainer follows pre-tag procedure

- **GIVEN** a maintainer prepares a release tag
- **WHEN** the maintainer follows the runbook
- **THEN** they run the offline gate, resolve every Case A finding by registering first, and resolve every Case B finding through owner manual first publish followed by trustpub registration
