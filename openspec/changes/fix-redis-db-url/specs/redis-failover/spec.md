# Delta Spec: fix-redis-db-url

## ADDED Requirements

### Requirement: standalone redis database selection is honored by the driver

The redis component's standalone topology SHALL resolve a `redis::Client`
whose `ConnectionInfo` carries the configured database number (from the
`?db=N` URI parameter), instead of deriving the driver connection from a URL
string that drops it. Sentinel resolution already sets db explicitly and is
unchanged.

#### Scenario: standalone URI with db N resolves client with db N

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `redis://localhost:6379?command=GET&db=2`
- **WHEN** the topology resolves a client for `ServerKind::Master`
- **THEN** `redis_settings().db()` equals 2

#### Scenario: standalone URI without db resolves db 0 (unchanged default)

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `redis://localhost:6379?command=GET` (no `db` parameter)
- **WHEN** the topology resolves a client
- **THEN** `redis_settings().db()` equals 0

#### Scenario: TLS standalone URI keeps TcpTls addr and db

- **GIVEN** a standalone `RedisEndpointConfig` parsed from
  `rediss://localhost:6380?command=GET&db=3` (ssl enabled)
- **WHEN** the topology resolves a client
- **THEN** the connection address is `TcpTls` (insecure false,
  tls_params none) AND `redis_settings().db()` equals 3

#### Scenario: credentials ride ConnectionInfo without re-encoding

- **GIVEN** a standalone config with a password containing URI-reserved
  characters (e.g. `p@ss:word`)
- **WHEN** the topology resolves a client
- **THEN** `redis_settings().password()` equals the raw configured password
  (no percent-decode/encode on the driver path); username propagation is
  out of scope and unchanged

#### Scenario: repository service standalone backends inherit db selection

- **GIVEN** a camel-redis-repo cache or idempotent backend configured for a
  standalone endpoint with `?db=2`
- **WHEN** `connect_executor` establishes the connection
- **THEN** the connection issues `SELECT 2` (the configured db takes effect
  on the repo driver path)

#### Scenario: display strings unchanged

- **GIVEN** a standalone config with `db=2`
- **WHEN** `redis_url()`, `redis_url_safe()`, and `safe_endpoint()` render
- **THEN** the rendered strings are byte-identical to before this change
  (`?db=N` query form), and `from_uri(redis_url())` still round-trips
