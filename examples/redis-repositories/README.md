# redis-repositories

A minimal example that points the EIP cache and idempotent-consumer
repositories at Redis. Both repositories come from `Camel.toml`
(`[default.cache_repo]` and `[default.idempotent_repo]`) and register under
the name `redis`:

- a `cache` step with `repository = "redis"` stores computed bodies in Redis
- an `idempotent_consumer` with `repository = "redis"` records
  deduplication keys in Redis

Entries and keys survive a process restart and are shared with every process
that connects to the same Redis.

Routes are declared in `routes.yaml` because `RouteBuilder` (camel-builder)
exposes no `cache`/`idempotent_consumer` step methods — the same pattern as
`examples/cache-example` and `examples/idempotent-consumer`.

Prerequisites: a running Redis at `127.0.0.1:6379`. This example does not
start one itself.

Run it from this directory, so `Camel.toml` is found:

```text
cargo run
```

Point it at another Redis (and database, via the `?db=N` query parameter)
with `REDIS_URL`:

```text
REDIS_URL="redis://redis.internal:6379?db=2" cargo run
```
