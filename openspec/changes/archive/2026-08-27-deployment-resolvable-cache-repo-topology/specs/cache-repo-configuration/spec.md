# cache-repo-configuration spec delta

## ADDED Requirements

### Requirement: Empty topology values resolve to absent

When a configuration reaches `CamelConfig` through a file-loader entry point
(sync loader, async loaders, hot-reload wiring — all converge on
`build_from_toml_value_inner`), the system SHALL normalize empty redis
topology field values to absent (`None`) after `${env:}` placeholder
expansion and deserialization, before validation runs.

Normalized fields, in both `cache_repo` and `idempotent_repo` redis sections:
`url`, `sentinel_nodes`, `master_name`, `username`, `sentinel_username`,
`key_prefix`. Rules: whitespace-only strings become `None` — including `password` and
`sentinel_password` (a blank value means unset; a non-blank credential is
never dropped); `sentinel_nodes` arrays that are empty or whose every entry
is whitespace-only become `None`; mixed blank/non-blank arrays remain
unchanged and fail validation; `db` is untouched.
Programmatic construction and direct deserialization that bypass the loader
pipeline are out of scope.

#### Scenario: sentinel topology selected by empty url

- **GIVEN** a `[cache_repo]` section with `backend = "redis"`,
  `url = "${env:REDIS_URL:-}"`, `sentinel_nodes = ["node-a:26379"]` and
  `master_name = "m"` set, and `REDIS_URL` unset in the environment
- **WHEN** the configuration is loaded and validated
- **THEN** the config validates as a sentinel topology and no
  mutual-exclusion error is raised

#### Scenario: standalone topology selected by populated url

- **GIVEN** the same section with `REDIS_URL` set to a valid standalone URL
  and `sentinel_nodes = ["${env:SENTINEL_NODES_0:-}"]` and
  `master_name = "${env:SENTINEL_MASTER:-}"` both expanding empty
- **WHEN** the configuration is loaded and validated
- **THEN** the sentinel-only fields normalize to absent and the standalone
  topology validates

#### Scenario: all-blank sentinel array means unset

- **GIVEN** `sentinel_nodes = ["${env:NODES:-}"]` with `NODES` unset and
  `url` set to a valid standalone URL
- **WHEN** the configuration is loaded and validated
- **THEN** the blank entry resolves to absent and the standalone topology
  validates

#### Scenario: mixed blank entries still fail loudly

- **GIVEN** `sentinel_nodes = ["redis-a:26379", " "]`
- **WHEN** the configuration is loaded and validated
- **THEN** validation fails with the existing non-empty-entry error

#### Scenario: literal empty string in file

- **GIVEN** a config file with `url = ""` and `sentinel_nodes` set
- **WHEN** the configuration is loaded and validated
- **THEN** the empty literal is treated the same as an unset key and the
  sentinel topology validates

#### Scenario: both topologies genuinely absent still fail

- **GIVEN** a redis section where `url` and `sentinel_nodes` are both unset
  or expand empty
- **WHEN** the configuration is loaded and validated
- **THEN** validation fails with the existing "requires a topology" error

#### Scenario: empty key_prefix selects the default prefix

- **GIVEN** a valid redis topology section (standalone `url` set) with
  `key_prefix = "${env:KEY_PREFIX:-}"` and `KEY_PREFIX` unset
- **WHEN** the configuration is loaded and validated
- **THEN** `key_prefix` is absent and the repository uses its default prefix;
  a non-blank invalid prefix (e.g. `bad*prefix`) is not normalized and stays
  rejected by keyspace validation

#### Scenario: idempotent_repo parity

- **GIVEN** an `[idempotent_repo]` redis section with
  `url = "${env:IDEM_URL:-}"` expanding empty, `sentinel_nodes` and
  `master_name` set, and `IDEM_URL` unset
- **WHEN** the configuration is loaded and validated
- **THEN** the idempotent repository validates as a sentinel topology, same
  as `cache_repo`

### Requirement: Non-credential cache_repo environment overrides

The system SHALL allow per-deployment overrides of non-credential
`cache_repo` fields via `CAMEL_CACHE_REPO_*` environment variables:
`PAYLOAD`, `PAYLOAD_DIR`, `CACHE_SIZE`, `SWEEP_INTERVAL`, `MASTER_NAME`,
`KEY_PREFIX`, `DB`, and `SENTINEL_NODES` (comma-separated list).

An EMPTY scalar override variable is a no-op (the file/profile value remains
effective); an EMPTY `SENTINEL_NODES` override replaces the field with an
empty list, which the first requirement then normalizes to absent.

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
