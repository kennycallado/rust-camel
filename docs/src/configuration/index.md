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

- [Environment variable interpolation](env-interpolation.md): substitute `${env:VAR}` tokens in route files before parse
- [Hot reload](hot-reload.md): swap pipelines at runtime without downtime

**Reference**: [Config crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-config/CONTEXT.md)
