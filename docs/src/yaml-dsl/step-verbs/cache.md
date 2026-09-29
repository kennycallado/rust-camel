# Cache verbs

Cache message bodies and manage cache entries.

## `cache`
Cache a computed body by key with TTL. On hit, serves the cached body. On miss, runs the `on_miss` sub-pipeline and stores the result.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | no | `"memory"` | Repository name |
| `key` | string | yes | — | Cache key expression (None = bypass cache) |
| `ttl` | duration | no | — | Time-to-live for the cached entry |
| `max_entry_bytes` | integer | no | 10 MiB | Maximum body size to cache |
| `coalesce_misses` | bool | no | `false` | Run one `on_miss` per concurrent miss wave on the same key |
| `on_miss` | list | no | — | Sub-pipeline to run on cache miss. Omit it to let the exchange pass through unchanged on a miss. |

```yaml
- cache:
    key: "${header.cacheKey}"
    ttl: "5s"
    on_miss:
      - set_body: "computed"
```

With `coalesce_misses: true`, concurrent misses on the same key run the `on_miss` sub-pipeline once. The first miss leads. The rest wait and share the leader's body and error.

## `cache_invalidate`
Remove a single entry or a namespace from the cache repository. Set `key` for an exact-key removal or `key_prefix` for a namespace purge. Exactly one of the two is required.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | no | `"memory"` | Repository name |
| `key` | string | no | — | Exact cache key expression (mutually exclusive with `key_prefix`) |
| `key_prefix` | string | no | — | Namespace prefix expression (mutually exclusive with `key`) |

On success the step sets the `CamelCacheInvalidatedCount` exchange property: `1` for an exact key, the removed count for a prefix. A backend without key iteration (memory) fails closed on `key_prefix`.

```yaml
- cache_invalidate:
    key: "${header.cacheKey}"
- cache_invalidate:
    key_prefix: "user-profile-"
```

## `cache_clear`
Remove every entry from the cache repository.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | no | `"memory"` | Repository name |

```yaml
- cache_clear: {}
- cache_clear:
    repository: "persistent"
```

## `cache_stats`
Replace the body with a JSON snapshot of the cache repository statistics.

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `repository` | string | no | `"memory"` | Repository name |

```yaml
- cache_stats: {}
```

The snapshot holds `repository`, `hits`, `misses`, `evictions`, `entries`, `peek_stale_served`, `invalidations`, and `bytes`. The `bytes` field is the stored payload size when the backend reports it (`null` for the memory backend).

## `cache_peek_stale`
Serve a cached entry, ignoring its in-band expiry. Used as a stale-read fallback.

| Field | Type | Required | Description |
|---|---|---|---|
| `repository` | string | no | Repository name (default: the default cache repository) |
| `key` | string | yes | Cache key expression |
| `on_miss` | string | no | On-miss policy: `"stop"` (default) or `"continue"` |

`on_miss` does not have the same meaning as the `on_miss` field of `cache`. In `cache`, the field holds a sub-pipeline. In `cache_peek_stale`, the field holds a policy word. Do not write a step list under `cache_peek_stale.on_miss`.

```yaml
- cache_peek_stale:
    key: "${header.cacheKey}"
    on_miss: continue
```
