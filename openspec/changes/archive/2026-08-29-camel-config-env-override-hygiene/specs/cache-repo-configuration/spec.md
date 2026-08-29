## MODIFIED Requirements

### Requirement: Non-credential cache_repo environment overrides

The system SHALL allow per-deployment overrides of non-credential
`cache_repo` fields via `CAMEL_CACHE_REPO_*` environment variables with a
typed contract per field class: `BACKEND`, `PATH`, `STALE_RETENTION`,
`MAX_CAPACITY`, `MAX_ENTRIES`, `PAYLOAD`, `PAYLOAD_DIR`, `CACHE_SIZE`,
`SWEEP_INTERVAL`, `MASTER_NAME`, `KEY_PREFIX`, `DB`, and `SENTINEL_NODES`
(comma-separated list).

String-typed scalar fields (`BACKEND`, `PATH`, `STALE_RETENTION`,
`PAYLOAD`, `PAYLOAD_DIR`, `CACHE_SIZE`, `SWEEP_INTERVAL`, `MASTER_NAME`,
`KEY_PREFIX`) SHALL receive their raw value verbatim as a string — no
numeric or boolean coercion. Numeric-typed fields (`MAX_CAPACITY`,
`MAX_ENTRIES`, `DB`) SHALL parse strictly typed values. Duration string
fields (`STALE_RETENTION`, `SWEEP_INTERVAL`) accept humantime forms with
explicit units; a unitless numeric value SHALL be rejected at validation
with an error naming the required format, and SHALL NOT be interpreted as
seconds.

An EMPTY scalar override variable among the newer set (`PAYLOAD`,
`PAYLOAD_DIR`, `CACHE_SIZE`, `SWEEP_INTERVAL`, `MASTER_NAME`, `KEY_PREFIX`,
`DB`) is a no-op (the file/profile value remains effective); an EMPTY
`SENTINEL_NODES` override replaces the field with an empty list, which the
first requirement then normalizes to absent. Legacy vars (`BACKEND`,
`PATH`, `STALE_RETENTION`, `MAX_CAPACITY`, `MAX_ENTRIES`) keep their
established empty-value behavior — the empty-scalar skip does not apply to
them.

#### Scenario: scalar override applied

- **GIVEN** a loaded config with `cache_repo.db = 0` and the environment
  variable `CAMEL_CACHE_REPO_DB=3`
- **WHEN** env overrides are merged
- **THEN** the effective config carries `db = 3`

#### Scenario: empty scalar override preserves the file value

- **GIVEN** a loaded config with `cache_repo.db = 5` and the environment
  variable `CAMEL_CACHE_REPO_DB=` set to an empty string
- **WHEN** env overrides are merged and the config deserializes
- **THEN** the effective config still carries `db = 5` (the empty override is
  skipped before type coercion, so `Option<u16>` deserialization never sees
  an empty string)

#### Scenario: CSV override builds a node list

- **GIVEN** the environment variable
  `CAMEL_CACHE_REPO_SENTINEL_NODES="node-a:26379, node-b:26379"`
- **WHEN** env overrides are merged
- **THEN** the effective config carries both trimmed node entries

#### Scenario: CSV override composes with master_name into a sentinel topology

- **GIVEN** a config with `cache_repo.url` absent,
  `master_name = "${env:SENTINEL_MASTER:-}"`, the environment variables
  `CAMEL_CACHE_REPO_SENTINEL_NODES="node-a:26379,node-b:26379"` and
  `SENTINEL_MASTER=mymaster` set
- **WHEN** env overrides are merged, placeholders expand, and validation runs
- **THEN** the config validates as a sentinel topology with both nodes and
  `master_name = "mymaster"`

#### Scenario: empty CSV override clears a populated file value

- **GIVEN** a config with `cache_repo.sentinel_nodes = ["file-node:26379"]`,
  a valid standalone `url`, and the environment variable
  `CAMEL_CACHE_REPO_SENTINEL_NODES=` set to an empty string
- **WHEN** env overrides are merged and normalization runs
- **THEN** the empty override replaces the populated file list with an empty
  list, normalization resolves `sentinel_nodes` to absent, and the standalone
  topology validates

#### Scenario: credential vars stay denied

- **GIVEN** the environment variable `CAMEL_CACHE_REPO_URL` (or
  `_USERNAME`, `_PASSWORD`, `_SENTINEL_USERNAME`, `_SENTINEL_PASSWORD`) set
  to any value
- **WHEN** env overrides are merged
- **THEN** the variable is ignored with the existing
  "not in override allowlist" warning and no security-sensitive field of the
  config changes

#### Scenario: unitless numeric stale retention fails validation with a unit-naming error

- **GIVEN** a config with a valid standalone `url` topology (persistent
  backend) and the environment variable
  `CAMEL_CACHE_REPO_STALE_RETENTION=604800` set to a bare numeric value
- **WHEN** env overrides are merged, the config deserializes, and validation
  runs
- **THEN** load fails with the stale-retention duration validation error
  naming the accepted unit-bearing format — not with a deserialization
  "invalid type: integer" error

#### Scenario: human-readable duration override applies

- **GIVEN** a config with a valid standalone `url` topology (persistent
  backend), `cache_repo.stale_retention` absent, and the environment
  variable `CAMEL_CACHE_REPO_STALE_RETENTION=7d`
- **WHEN** env overrides are merged
- **THEN** the effective config carries `stale_retention = Some("7d")`,
  which resolves to a seven-day retention

#### Scenario: legacy string scalars pass through verbatim

- **GIVEN** the environment variable `CAMEL_CACHE_REPO_PATH=007`
- **WHEN** env overrides are merged and the config deserializes
- **THEN** the effective config carries `path = Some("007")` — the value is
  never coerced to an integer

#### Scenario: bare numeric backend reaches backend validation

- **GIVEN** the environment variable `CAMEL_CACHE_REPO_BACKEND=123`
- **WHEN** env overrides are merged and validation runs
- **THEN** load fails with the unknown-backend validation error, the value
  having passed through verbatim as the string `"123"`

#### Scenario: numeric fields stay strictly typed

- **GIVEN** the environment variable `CAMEL_CACHE_REPO_MAX_ENTRIES=notanumber`
- **WHEN** env overrides are merged and the config deserializes
- **THEN** load fails with a typed deserialization error on
  `Option<usize>`

#### Scenario: legacy empty scalar overrides the file value and fails validation

- **GIVEN** a config with a valid standalone `url` topology and
  `cache_repo.stale_retention = "7d"` in the file, and the environment
  variable `CAMEL_CACHE_REPO_STALE_RETENTION=` set to an empty string
- **WHEN** env overrides are merged, the config deserializes, and validation
  runs
- **THEN** no empty-scalar skip applies to the legacy var: the empty
  override replaces the file value, and load fails with the same
  stale-retention duration-format error as an invalid non-empty value
