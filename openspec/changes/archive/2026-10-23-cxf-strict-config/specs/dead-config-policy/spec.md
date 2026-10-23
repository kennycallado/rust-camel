## ADDED Requirements

### Requirement: cxf profile TOML surface fails closed on unknown or misplaced keys

The system SHALL reject, at config load, any key on the cxf TOML surface
(`[components.cxf]`, `[[components.cxf.profiles]]`,
`profiles.<name>.security`, `profiles.<name>.security.tls`) that is either not
a known key at that level or is a known cxf key placed in the wrong table.
For a profile-scoped violation the error SHALL name the profile (its `name`
when present, else `profiles[<index>]`); a pool-level violation SHALL identify
`[components.cxf]`. A misplaced known key SHALL name the location(s) where it
is accepted; an unknown key SHALL list the accepted keys at the offending
level. All walker violations in one load SHALL be reported in a single
deterministic (sorted) error. Walker diagnostics SHALL contain key names,
accepted locations, and (only) the profile `name` as identity — no other
config values. Wrong-shape/type failures keep the typed decoder's existing
error behavior. A literal `listener_tls` pool table SHALL be rejected with a
hint naming the five flat `soap_tls_*` keys.

#### Scenario: Unknown key in the profile table is rejected

- **GIVEN** a profile `[[components.cxf.profiles]]` entry with `name = "p"`
  and an extra key `exposd_soap_headers`
- **WHEN** the config loads
- **THEN** loading fails with an error naming profile `p`, the key
  `exposd_soap_headers`, that it is unknown, and the accepted keys at profile
  level

#### Scenario: Unknown key in the pool table is rejected

- **GIVEN** `[components.cxf]` containing a key `max_bridge`
- **WHEN** the config loads
- **THEN** loading fails identifying `[components.cxf]`, the key
  `max_bridge`, that it is unknown, and the accepted keys at pool level

#### Scenario: Unknown key in the security table is rejected

- **GIVEN** `profiles.p.security` containing a key `sig_usrname`
- **WHEN** the config loads
- **THEN** loading fails naming profile `p`, the key, and the accepted keys
  at security level

#### Scenario: Unknown key in the security.tls table is rejected

- **GIVEN** `profiles.p.security.tls` containing a key `keystore_paht`
- **WHEN** the config loads
- **THEN** loading fails naming profile `p`, the key, and the accepted keys
  at security.tls level

#### Scenario: Profile without a usable name falls back to index identity

- **GIVEN** a profile entry with no `name`, or a non-string `name`, carrying
  a misplaced key
- **WHEN** the config loads
- **THEN** loading fails and the diagnostic identifies the entry as
  `profiles[<index>]`

#### Scenario: Misplaced exposed_soap_headers names the accepted path

- **GIVEN** `exposed_soap_headers` placed directly under a profile entry
  instead of `profiles.<name>.security`
- **WHEN** the config loads
- **THEN** loading fails with an error stating `exposed_soap_headers` is
  accepted under the profile's `security` table, not the profile table,
  naming the profile

#### Scenario: Multi-location key lists every accepted location

- **GIVEN** `keystore_path` placed at profile level
- **WHEN** the config loads
- **THEN** loading fails stating `keystore_path` is accepted under BOTH
  `profiles.<name>.security` and `profiles.<name>.security.tls`

#### Scenario: Single-location misplaced key names its one location

- **GIVEN** `keystore_type` placed at security level
- **WHEN** the config loads
- **THEN** loading fails stating `keystore_type` is accepted under
  `profiles.<name>.security.tls`

#### Scenario: Correct placement still loads and works

- **GIVEN** a profile with `exposed_soap_headers` under
  `profiles.<name>.security` and otherwise valid keys at every level
- **WHEN** the config loads
- **THEN** loading succeeds and the profile is usable (feature value reaches
  the bridge env-var assembly unchanged)

#### Scenario: Pool-level misplaced profile key is rejected

- **GIVEN** `exposed_soap_headers` placed directly under
  `[components.cxf]`
- **WHEN** the config loads
- **THEN** loading fails stating the key is accepted under
  `profiles.*.security`, not at pool level

#### Scenario: listener_tls table is rejected in favor of flat keys

- **GIVEN** a `listener_tls = { ... }` table under `[components.cxf]`
- **WHEN** the config loads
- **THEN** loading fails with a hint naming the five flat `soap_tls_*` keys

#### Scenario: Violations aggregate into one deterministic error

- **GIVEN** a config with several unknown/misplaced keys at different levels
- **WHEN** the config loads
- **THEN** a single error reports all violations, in a stable sorted order

#### Scenario: Error messages never echo values

- **GIVEN** a config with an unknown key whose value is a secret-looking
  string
- **WHEN** the config loads
- **THEN** the error message contains the key name and no part of the value

#### Scenario: Env-var assembly path is unaffected

- **GIVEN** a valid profile config that loads successfully
- **WHEN** the pool emits `CXF_PROFILE_*` environment variables for the
  bridge
- **THEN** the emitted set is identical to the pre-change assembly (unknown
  env vars are not config keys and are not validated)
