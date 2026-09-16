# Configuration

`Camel.toml` is the operator surface for rust-camel. `CamelConfig` deserializes the file into a profile-aware tree. Fields live under `[default]` and `[<profile>]` sections, deep-merged with includes and `CAMEL_*` overrides.

Sections under `[default]` and `[<profile>]`: `routes` (discovery globs), `jobs` (camel job discovery roots), `components.*` (per-component defaults, untyped TOML), `supervision` (retry and backoff), `observability` (tracing and metrics), `idempotent_repo` (persistent idempotent backend).

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

## Repository registration names

Both `[idempotent_repo]` and `[cache_repo]` accept an optional `name` string. The name is the registry key that EIP steps (idempotent consumer, cache) resolve repositories by, and for the redis backends it is also a keyspace segment (`camel:idem:<name>:*`, `camel:cache:<name>:*`), so two differently-named repositories never share keys. Allowed characters are `[A-Za-z0-9:_-]`; glob metacharacters are rejected because `clear` scans by prefix.

When `name` is omitted, the backend convention applies. The defaults are historical and pinned by existing scenarios; the table shows the full mapping:

| Section             | Backend   | Default name   |
| ------------------- | --------- | -------------- |
| `[idempotent_repo]` | `redb`    | `redb`         |
| `[idempotent_repo]` | `redis`   | `redis`        |
| `[cache_repo]`      | `memory`  | `memory`       |
| `[cache_repo]`      | `redb`    | `persistent`   |
| `[cache_repo]`      | `redis`   | `redis`        |

The cache `redb` default is `persistent`, not `redb` — an asymmetry kept for backward compatibility. Set `name` explicitly when the distinction matters to your routes.

One caveat on the cache `memory` backend: the config only replaces the default memory repository when `max_capacity` is set. A `name` (or any other `cache_repo` key) on a memory table without `max_capacity` registers nothing — the built-in default stays.

One boundary of the cross-repository prefix-collision rule: validation compares declared endpoints, not resolved addresses. A standalone `url` and a `sentinel_nodes` topology that both point at the same physical Redis instance and database are treated as distinct databases, because resolving the sentinel topology requires network I/O that `validate()` does not perform. The repositories stay separated by their distinct default key prefixes and name segments; set explicit `key_prefix` values when mixing both shapes against one instance.

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
