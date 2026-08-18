## ADDED Requirements

### Requirement: cache_repo cross-backend field rejection

The `cache_repo` configuration section SHALL fail validation, per the dead-config-policy
fail-closed principle, when fields that do not apply to the configured `backend` are set
to non-default values: with `backend = "memory"`, any of `path`, `stale_retention`,
`max_entries`, `cache_size`, or `sweep_interval` set SHALL be rejected; with
`backend = "redb"`, `max_capacity` set SHALL be rejected. An omitted `stale_retention`
SHALL deserialize as `None` (the serde default SHALL NOT materialize a value for an
absent field), and the 7-day fallback SHALL apply in wiring only after validation
passes for the redb backend — so memory-backend configs that omit the field validate
unchanged.

#### Scenario: cache_size on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.cache_size = "512MiB"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.cache_size` as not applicable
  to the `memory` backend

#### Scenario: path on memory backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` and
  `cache_repo.path = "data/cache.redb"`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.path` as not applicable to the
  `memory` backend

#### Scenario: max_capacity on redb backend rejected

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with valid redb fields
  and `cache_repo.max_capacity = 5000`
- **WHEN** the config is validated
- **THEN** validation returns an error naming `cache_repo.max_capacity` as not
  applicable to the `redb` backend

#### Scenario: omitted stale_retention stays None on memory backend

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "memory"` with only
  `max_capacity` set (stale_retention omitted)
- **WHEN** the config is deserialized and validated
- **THEN** `stale_retention` deserializes as `None` and validation succeeds

#### Scenario: omitted stale_retention falls back in wiring for redb

- **GIVEN** a `CamelConfig` whose `cache_repo.backend = "redb"` with a path and
  `cache_size`, stale_retention omitted
- **WHEN** the context is built from that config
- **THEN** the `"persistent"` repository is constructed with a 7-day stale retention
  applied by wiring, and validation did not treat the field as set
