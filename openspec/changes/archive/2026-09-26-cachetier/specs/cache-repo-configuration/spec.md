## ADDED Requirements

### Requirement: Payload location tier selection

The system SHALL support selecting the cache payload location per
`[cache_repo]` via `payload = "inline" | "disk" | "redis"`. The `redis`
tier SHALL store payload bytes as Redis entries under the repository's
own keyspace (`{prefix}:{repo}:payload:{blob}`) with an EXAT deadline at
the blob death epoch, while the index row stays bytes-empty with a
`payload_path`. The `redis` tier SHALL be valid only with
`backend = "redis"`; configuration combining `payload = "redis"` with
any other backend, or with `payload_dir`, SHALL fail validation with an
error naming the offending field. The duration knobs
`payload_sweep_interval` and `payload_max_ttl` SHALL apply to both
offload tiers and stay rejected under inline mode. The `disk` tier and
inline mode SHALL keep their established validation matrix and behavior
unchanged.

#### Scenario: redis tier accepted on the redis backend

- **GIVEN** a `cache_repo` with `backend = "redis"` and
  `payload = "redis"`
- **WHEN** the configuration is validated and the context boots
- **THEN** validation passes and the cache repository registers a
  payload-offload decorator whose payload store writes to Redis

#### Scenario: redis tier rejected on the redb backend

- **GIVEN** a `cache_repo` with `backend = "redb"` and
  `payload = "redis"`
- **WHEN** the configuration is validated
- **THEN** validation fails with an error naming `backend` and stating
  that `payload = "redis"` requires `backend = "redis"`

#### Scenario: redis tier rejected on the memory backend

- **GIVEN** a `cache_repo` with `backend = "memory"` and
  `payload = "redis"`
- **WHEN** the configuration is validated
- **THEN** validation fails with the memory backend's payload-offload
  rejection; the memory backend accepts no offload tier

#### Scenario: redis tier with payload_dir is rejected

- **GIVEN** a `cache_repo` with `backend = "redis"`,
  `payload = "redis"`, and `payload_dir` set
- **WHEN** the configuration is validated
- **THEN** validation fails with an error naming `payload_dir` as
  inconsistent with the redis tier

#### Scenario: disk tier matrix unchanged

- **GIVEN** a `cache_repo` with `payload = "disk"`
- **WHEN** the configuration is validated
- **THEN** the established disk rules apply unchanged: `payload_dir` is
  required, the memory backend is rejected, and payloads land as blob
  files reclaimed by the death-epoch sweeper
