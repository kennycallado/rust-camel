# Design: fix-redis-db-url

## Approach

Fix Direction B (blessed by expert review): stop round-tripping the database
number through the URL string on the driver path. `StandaloneTopology` gains
an explicit `redis::ConnectionInfo` construction instead of
`Client::open(url_string)`:

- `ConnectionAddr`: `Tcp { host, port }`, or `TcpTls { insecure: false,
  tls_params: None }` when `is_ssl_enabled()` (mirrors the `redis://`/
  `rediss://` scheme selection in `redis_url()`).
- `RedisConnectionInfo` (via builder — direct struct construction is not
  possible, fields are private): start from the addr's
  `into_connection_info()`, then `set_redis_settings(...)` with a
  `RedisConnectionInfo` built through its public builder
  (`RedisConnectionInfo::default().set_db(config.db as i64)` plus password
  setter); `redis_settings().db()` / `redis_settings().password()` getters are
  the test observation points.
- Username propagation on the standalone driver path is OUT OF SCOPE and
  unchanged from current behavior (no username is rendered into
  `redis_url()` today either).
- `redis::Client::open(connection_info)` — the driver accepts
  `IntoConnectionInfo`.

This unifies standalone and sentinel on the same db-setting mechanism
(`set_db`, as `node_redis_connection_info()` already does at topology.rs:337).

Display strings `redis_url()` / `redis_url_safe()` / `safe_endpoint()` are
unchanged: their only remaining consumers are error messages and tracing.
`from_uri()` acceptance rules unchanged (`?db=N` in, `/N` rejected).
New tests inspect the resolved client's getters (`addr()` kind,
`redis_settings().db()`) — closing the gap that no existing test checks the
*effective* db, only the rendered string.
The repo-level regression drives `connect_executor` and observes the `SELECT 2`
the connection issues.

## Affected crates

- `camel-component-redis`: `topology.rs` — `StandaloneTopology` stores a
  `RedisEndpointConfig`-derived `ConnectionInfo` (or builds it in `resolve`);
  new unit tests for db/TLS/credentials. Public surface adjustment:
  `StandaloneTopology::new` takes `&RedisEndpointConfig` instead of a URL
  string (all in-repo callers are topology_from_config and test modules;
  the operator-facing URI syntax and config struct are untouched).
- `camel-redis-repo` (services): NO code change — it inherits the fix via
  `topology_from_config`. The regression test asserting the effective db on
  the repository path lives in `crates/camel-test/tests/redis_repositories_test.rs`
  (testcontainers, end-to-end through config → connect_executor → topology).

## Architecture boundaries

Components layer only (`camel-redis`) plus an end-to-end regression test in
the test workspace (`camel-test`). No Runtime/DSL/Language/Function surface
changes. The component's public contract (URI syntax, config struct) is
untouched — the fix is internal connection construction, preserving the
hexagonal boundary the topology abstraction already provides.

## Phases

Omitted — single coherent slice, one phase.

## Alternatives considered

- **Direction A: render db as path segment (`redis://host:port/N`) and accept
  both syntaxes in `from_uri`.** Rejected by expert review: forces churn in
  the uri.rs parser, from_uri, port-split logic, and every doc/test that pins
  `?db=`, and re-introduces the fragile string round-trip that caused the bug.
- **Do nothing / document db 0 only.** Rejected: silent wrong-database
  connections are a data-integrity hazard (keys written to db 0 alongside
  other tenants).
