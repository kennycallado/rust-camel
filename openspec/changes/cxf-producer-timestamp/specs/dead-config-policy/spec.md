## ADDED Requirements

### Requirement: cxf security out-actions fail loud without their crypto material

The bridge-side producer security profile SHALL reject, at build time,
explicitly configured outbound actions whose security effect would
otherwise be a silent no-op: a `Timestamp` action without `Signature`
(strippable decorative security), and any of `Signature`/`Encrypt`/
`Timestamp` without a configured keystore. Profiles that did not
configure outbound actions (falling back to the default action set) are
outside this rule. Checks are ordered deterministically: action
composition first, crypto material second.

#### Scenario: Timestamp without Signature is rejected

- **GIVEN** a security profile builder with out-actions "Timestamp" and a keystore
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the Timestamp action and the missing Signature action

#### Scenario: security actions without keystore are rejected

- **GIVEN** a security profile builder with out-actions "Signature" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the missing keystore

#### Scenario: Encrypt without keystore is rejected

- **GIVEN** a security profile builder with out-actions "Encrypt" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown whose message names the missing keystore

#### Scenario: composed actions without keystore follow composition-first precedence

- **GIVEN** a security profile builder with out-actions "Signature Timestamp" and no keystore configured
- **WHEN** build() runs
- **THEN** an IllegalArgumentException is thrown naming the missing keystore (the action composition is valid, so the material check fires)

#### Scenario: explicitly blank actions are unaffected

- **GIVEN** a security profile with no out-actions configured and no keystore
- **WHEN** build() runs
- **THEN** the profile builds successfully (no security interceptors; default action resolution is not subject to the material check)
