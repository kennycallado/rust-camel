# cxf-timestamp-window Specification

## Purpose
TBD - created by archiving change cxf-timestamp-ttl. Update Purpose after archive.
## Requirements
### Requirement: Per-profile timestamp freshness window

The system SHALL let each CXF security profile configure `timestamp_ttl_seconds` and `timestamp_future_tolerance_seconds` in `[profiles.security]`, carried to the bridge as `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` and `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS`. When a value is not set, the bridge SHALL use 300 s and 60 s. Values SHALL be accepted from 1 to 2147483647 inclusive (the WSS4J `int` limit, about 68 years). A value above 2147483647 SHALL fail startup with an error that names the field and the maximum; the system SHALL NOT clamp it silently.

#### Scenario: TOML values reach the bridge environment

- **GIVEN** a profile with `timestamp_ttl_seconds = 900` and `timestamp_future_tolerance_seconds = 120` and a truststore
- **WHEN** the pool config is parsed, validated, and mapped to bridge env vars
- **THEN** validation passes and the env carries `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS=900` and `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS=120`

#### Scenario: Unset values keep the defaults

- **GIVEN** a profile that sets neither option
- **WHEN** the env vars are built and the bridge loads the profile
- **THEN** neither variable is emitted and the profile's effective values are 300 and 60

#### Scenario: Maximum value accepted

- **GIVEN** `timestamp_ttl_seconds` or `timestamp_future_tolerance_seconds` set to `2147483647` in TOML, or `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` / `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` set to `2147483647` in the bridge env
- **WHEN** the config is validated and mapped to env vars, and the bridge loads the profile
- **THEN** validation passes, the env carries `2147483647`, and the effective value is 2147483647 s

#### Scenario: Value above the maximum rejected

- **GIVEN** either option set to `2147483648` or `18446744073709551615` (`u64::MAX`), in TOML or in the bridge env
- **WHEN** `CxfPoolConfig::validate` runs, or the bridge loads security profiles
- **THEN** Rust returns `CamelError::Config` naming the option and `2147483647`, and the bridge fails startup with an error naming the variable and `2147483647`; no value is clamped

### Requirement: Invalid timestamp window values fail startup

The system SHALL reject a zero value with a typed config error that names the profile and the option. The bridge SHALL reject a zero, negative, or non-integer env value with a startup error that names the variable. The system SHALL reject a set value on a profile that has neither a truststore nor a keystore, because the value would be inert.

#### Scenario: Zero rejected in Rust

- **GIVEN** a profile with `timestamp_ttl_seconds = 0` or `timestamp_future_tolerance_seconds = 0`
- **WHEN** `CxfPoolConfig::validate` runs
- **THEN** it returns `CamelError::Config` naming the profile and the option

#### Scenario: Zero or garbage rejected in the bridge

- **GIVEN** `CXF_PROFILE_<NAME>_TIMESTAMP_TTL_SECONDS` or `CXF_PROFILE_<NAME>_TIMESTAMP_FUTURE_TOLERANCE_SECONDS` set to `0`, `-5`, or `abc`
- **WHEN** the bridge loads security profiles
- **THEN** startup fails with an error that names the variable

#### Scenario: Inert value rejected

- **GIVEN** a profile without truststore and keystore that sets `timestamp_ttl_seconds`
- **WHEN** the config is validated (Rust) or the profile is built (Java)
- **THEN** both reject it

### Requirement: Startup WARN above the default window

The bridge SHALL log one startup WARN naming the profile when its effective TTL is above 300 s or its effective future tolerance is above 60 s. The bridge SHALL NOT log it when both values are at or below their defaults.

#### Scenario: WARN above default

- **GIVEN** a profile with an effective TTL of 900 s, or a profile with only an effective future tolerance of 120 s
- **WHEN** the bridge loads the profile
- **THEN** exactly one WARN is logged that names the profile and ADR-0088

#### Scenario: No WARN at or below default

- **GIVEN** a profile with no window settings, or with a TTL of 120 s
- **WHEN** the bridge loads the profile
- **THEN** no timestamp-window WARN is logged

### Requirement: Consumer inbound verification honours the profile window

The consumer inbound path SHALL validate the WS-Security `Timestamp` with the profile's effective TTL and future tolerance. The producer in-interceptor path SHALL NOT change. The replay caches and the Body+Timestamp signature-coverage check SHALL NOT change.

#### Scenario: Old Created accepted only with a raised TTL

- **GIVEN** a signed message whose `Timestamp` `Created` is 600 s in the past and whose `Expires` is in the future
- **WHEN** a consumer profile with the default TTL verifies it
- **THEN** verification fails with `The message timestamp is out of range`
- **AND WHEN** a consumer profile with `timestamp_ttl_seconds = 900` verifies it
- **THEN** verification succeeds

#### Scenario: Future Created accepted only with a raised tolerance

- **GIVEN** a signed message whose `Created` is 120 s in the future
- **WHEN** a consumer profile with the default future tolerance verifies it
- **THEN** verification fails with `The message timestamp is out of range`
- **AND WHEN** a profile with `timestamp_future_tolerance_seconds = 300` verifies it
- **THEN** verification succeeds

#### Scenario: Fixture with a multi-year window

- **GIVEN** a signed message whose `Created` is 9 years in the past and whose `Expires` is 10 years after `Created`
- **WHEN** a consumer profile with a TTL of 10 years verifies it
- **THEN** verification succeeds

#### Scenario: Expired Expires still rejected with a raised TTL

- **GIVEN** a signed message whose `Created` is 600 s in the past and whose `Expires` is 300 s in the past
- **WHEN** a consumer profile with `timestamp_ttl_seconds = 900` verifies it
- **THEN** verification fails with `The message timestamp has expired`

#### Scenario: Replay still rejected with a raised TTL

- **GIVEN** a consumer profile with `timestamp_ttl_seconds = 900` that has accepted a message with a 600 s old `Created`
- **WHEN** the byte-identical message is verified again by the same profile
- **THEN** verification fails as a replay

#### Scenario: Producer path unchanged

- **GIVEN** a profile with a raised TTL and future tolerance
- **WHEN** the producer in-interceptor is created
- **THEN** its properties carry no timestamp TTL keys

