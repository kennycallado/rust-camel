# camel-redis-repo

Redis-backed `CacheRepository` and `IdempotentRepository` implementations for
rust-camel, built on the connection management of `camel-component-redis`.
Sentinel deployments are always supported; TLS is opt-in via the `tls` feature.

## Configuration

Standalone Redis server:

```toml
[default.cache_repo]
backend = "redis"
url = "redis://127.0.0.1:6379"
```

The database is selected with the `?db=N` query parameter (default 0); a
`/N` path suffix is rejected by the URI grammar.

Redis Sentinel:

```toml
[default.cache_repo]
backend = "redis"
sentinel_nodes = ["redis://sentinel-1:26379", "redis://sentinel-2:26379"]
master_name = "mymaster"
```
