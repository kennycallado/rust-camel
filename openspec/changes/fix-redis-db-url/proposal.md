# Proposal: fix-redis-db-url

## Why

In Redis standalone mode, `db=N` in the endpoint URI (`redis://host:6379?db=2`)
is validated by our config parser but silently ignored at runtime: the client
always connects to db 0. Confirmed empirically with `CLIENT LIST` on a real
valkey instance (bd rc-c5l7).

Root cause: `RedisEndpointConfig::redis_url()` renders db as a `?db=N` query
parameter, and `StandaloneTopology` feeds that URL string to
`redis::Client::open`. redis-rs (1.6) parses db for TCP URLs **only** from the
path segment (`/N`); the `?db=` query form is honored only for unix-socket
URLs. The query parameter is therefore dropped.

Blast radius: the standalone redis component AND the standalone
`camel-redis-repo` backends (cache, idempotent) — both resolve connections
through `StandaloneTopology::new(config.redis_url())`. Sentinel mode is NOT
affected (it sets db explicitly via `set_db`).

## What Changes

- `StandaloneTopology` stops feeding a URL string to `Client::open`. It builds
  a `redis::ConnectionInfo` directly (addr from host/port/ssl, credentials,
  `set_db(config.db)`), mirroring what sentinel already does.
- `redis_url()` / `redis_url_safe()` / `safe_endpoint()` remain UNCHANGED as
  display/log strings (never fed to the driver). The
  `from_uri(redis_url())` round-trip invariant is preserved.
- Operator syntax unchanged: `?db=N` stays the single accepted dialect; `/N`
  stays rejected.
- New regression tests: resolved standalone client carries the configured db
  (incl. `rediss://` TLS + db), plus a repo-level test so camel-redis-repo
  cannot regress independently.
- Out of scope: any change to the URI parser (`camel-endpoint/src/uri.rs`),
  `from_uri()` acceptance rules, docs rewording beyond what tests require.

## Acceptance criteria

- A standalone `redis://host:6379?db=2` endpoint produces a client whose
  resolved db (observed via `redis_settings().db()`) equals 2 (0 would fail).
- `rediss://…?db=2` resolves with a `TcpTls` addr (insecure false) AND db 2.
- db 0 (no `db` param) behaves exactly as before.
- camel-redis-repo standalone backends inherit the fix: a regression test
  drives `connect_executor` and observes the `SELECT 2` the connection issues.
- Sentinel behavior unchanged.
- All existing round-trip/render tests still pass (display strings unchanged).

## Risk budget

Acceptable: internal refactor of StandaloneTopology connection construction;
new tests. Out of bounds: changing operator-facing URI syntax, touching the
uri.rs parser, password handling regressions (must set password on
RedisConnectionInfo directly with no double percent-decode), TLS regressions.
Security-adjacent surface: credentials in ConnectionInfo must mirror the
current userinfo-encoding semantics exactly.
