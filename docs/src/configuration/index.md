# Configuration

`Camel.toml` is the operator surface for rust-camel. `CamelConfig` deserializes the file into a profile-aware tree. Fields live under `[default]` and `[<profile>]` sections, deep-merged with includes and `CAMEL_*` overrides.

Top-level sections: `[default.routes]` (discovery globs), `[components.*]` (per-component defaults, untyped TOML), `[supervision]` (retry and backoff), `[observability]` (tracing and metrics), `[idempotent_repo]` (persistent idempotent backend).

Set `CAMEL_PROFILE` to select a profile. The `[default]` section always applies. The named profile merges on top. Use `include = ["path/to/file.toml"]` to pull shared sections from other files. An `include` list can also live inside `[default]` or a named profile section. Profile-scoped lists override top-level lists on key conflicts.

> **Note:** `include` entries are literal paths. A relative entry resolves against the directory that holds the main config file. `${env:}` expansion runs after the merge, over values only. Placeholders inside an `include` list stay literal, so `include = ["${env:CAMEL_INCLUDE_CONF}"]` fails with `included file not found`.

Switching the cache backend between profiles needs one extra step. Profile merges are additive: keys omitted by a profile survive from `[default]`, and validation rejects any cross-backend `cache_repo` key. Do not set `[default.cache_repo]`; define the complete table inside each profile instead:

```toml
[dev.cache_repo]
backend = "redb"
path = "/var/lib/rust-camel/dev-cache.redb"
cache_size = "64MiB"

[prod.cache_repo]
backend = "redis"
url = "redis://prod-redis.internal:6379"
```

A `[<profile>.cache_repo]` table whose counterpart is absent from `[default]` inserts whole at merge time, so no redb key survives under the redis profile.

## Environment overrides

After includes and profile merges, the loader overlays a fixed allowlist of `CAMEL_*` environment variables onto the merged tree. The loader ignores a `CAMEL_*` variable outside the allowlist and logs a warning. Two exceptions, `CAMEL_PROFILE` and `CAMEL_CONFIG_FILE`, select the profile and the config file itself. They do not override config fields and do not warn.

`camel run` and `CamelConfig::from_env_or_default()` apply these overrides to the loaded file; `CamelConfig::from_file()` does not.

Allowlisted variables, by group:

- General: `CAMEL_TIMEOUT_MS`, `CAMEL_DRAIN_TIMEOUT_MS`, `CAMEL_WATCH`, `CAMEL_WATCH_DEBOUNCE_MS`, `CAMEL_LOG_LEVEL`
- Runtime journal: `CAMEL_RUNTIME_JOURNAL_PATH`, `CAMEL_RUNTIME_JOURNAL_DURABILITY`, `CAMEL_RUNTIME_JOURNAL_COMPACTION_THRESHOLD_EVENTS`
- Idempotent repo: `CAMEL_IDEMPOTENT_REPO_PATH`, `CAMEL_IDEMPOTENT_REPO_DURABILITY`
- Cache repo: `CAMEL_CACHE_REPO_BACKEND`, `CAMEL_CACHE_REPO_PATH`, `CAMEL_CACHE_REPO_MAX_CAPACITY`, `CAMEL_CACHE_REPO_STALE_RETENTION`, `CAMEL_CACHE_REPO_MAX_ENTRIES`, `CAMEL_CACHE_REPO_PAYLOAD`, `CAMEL_CACHE_REPO_PAYLOAD_DIR`, `CAMEL_CACHE_REPO_CACHE_SIZE`, `CAMEL_CACHE_REPO_SWEEP_INTERVAL`, `CAMEL_CACHE_REPO_MASTER_NAME`, `CAMEL_CACHE_REPO_KEY_PREFIX`, `CAMEL_CACHE_REPO_DB`, `CAMEL_CACHE_REPO_SENTINEL_NODES`
- Supervision: `CAMEL_SUPERVISION_INITIAL_DELAY_MS`, `CAMEL_SUPERVISION_MAX_ATTEMPTS`

`CAMEL_CACHE_REPO_SENTINEL_NODES` is the only override whose value is a list. Its value is a comma-separated list of `host:port` entries, for example `CAMEL_CACHE_REPO_SENTINEL_NODES=sentinel-1:26379,sentinel-2:26379`. The loader splits the value on commas, trims each entry, and drops blank entries. An empty value yields an empty list. The list replaces the file value, and, on the redis backend, an empty node list normalizes to absent. The override clears the field to unset.

The empty-means-unset rule composes with the complete per-profile `[<profile>.cache_repo]` tables shown above. One image ships one file with a complete table per environment, and env overrides adjust values that differ per deployment. Flipping `CAMEL_CACHE_REPO_BACKEND` fails validation when the merged table carries keys of the other backend; switch backends through the profile tables instead.

An empty value preserves the file or profile value for these scalar cache repo variables: `CAMEL_CACHE_REPO_PAYLOAD`, `CAMEL_CACHE_REPO_PAYLOAD_DIR`, `CAMEL_CACHE_REPO_CACHE_SIZE`, `CAMEL_CACHE_REPO_SWEEP_INTERVAL`, `CAMEL_CACHE_REPO_MASTER_NAME`, `CAMEL_CACHE_REPO_KEY_PREFIX`, and `CAMEL_CACHE_REPO_DB`. The loader skips the empty override instead of passing an empty string to typed deserialization.

> **Note:** Connection strings and credentials are outside the allowlist. Set them with `${env:VAR}` placeholders in `Camel.toml` values, never through env overrides. The loader ignores a variable such as `CAMEL_CACHE_REPO_URL` and logs a warning.

- [Environment variable interpolation](env-interpolation.md): substitute `${env:VAR}` tokens in route files before parse
- [Hot reload](hot-reload.md): swap pipelines at runtime without downtime

**Reference**: [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
