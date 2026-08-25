# Design: redis-response-timeout

## Approach

The redis 1.6.0 driver enforces a 500 ms `DEFAULT_RESPONSE_TIMEOUT` on every
command pipelined over a multiplexed connection built through the config-less
`Client::get_multiplexed_async_connection()`. That is exactly how
`MultiplexedExecutor::get_conn` builds connections, so the service crate's
30 s local backstop (ADR-0063 Decision 13, commit 1945babc) can never govern.
ADR-0063 Decision 13 explicitly tracks this plumbing as a follow-up; this
change lands it and amends that paragraph.

Fix at the single connection-build point:

1. `MultiplexedExecutor` gains `response_timeout: Option<Duration>` and a
   builder `pub fn with_response_timeout(mut self, timeout: Duration) -> Self`.
   `new(...)` is unchanged; `None` keeps the config-less build call, so the
   default path is bit-identical and zero call sites change.
2. When `Some(t)` is set, `get_conn` builds through
   `Client::get_multiplexed_async_connection_with_config` with
   `AsyncConnectionConfig::new().set_response_timeout(Some(t)).set_connection_timeout(None)`.
   The response timeout replaces the driver's 500 ms per-command default;
   the connection timeout is disabled on the driver side because the
   component's own `tokio::time::timeout(connection_timeout_secs)` wrapper
   already bounds the connect phase — keeping the driver's parallel 1 s
   default would let the inner driver error eclipse the component's
   connect-timeout message at the same instant (tokio's `Timeout` polls the
   wrapped future before its own delay) and could fail slow-but-healthy
   handshakes before the configured bound. The component wrapper becomes
   the sole connect bound on this branch. Because `refresh()` and
   `RedisCommandExecutor::reconnect` both rebuild via `get_conn`, every
   rebuilt connection (failover path included) carries the same deadline
   automatically.
3. `camel-redis-repo`'s (repository service crate)
   `connect_executor_with_topology` constructs the executor with a driver
   response timeout strictly above its crate-local 30 s backstop —
   implementation constant 35 s (30 s + 5 s margin); the binding contract is
   the strict ordering. The local backstop therefore always fires first,
   preserving the tested error message and transient-Io classification; the
   driver deadline remains defense-in-depth if the local backstop is ever
   bypassed.

The setting lives on the executor, not on `RedisEndpointConfig`:
endpoint config is URI/defaults-driven (camel-config plumbing, schema, URI
parsing) and this need is service-crate-to-executor, not
user-config-to-endpoint. Keeping it off the config surface keeps the
component's default behavior and configuration contract untouched.

## Affected crates

- `camel-redis` (component): `executor.rs` — new optional field + builder,
  config-carrying build in `get_conn`; `tests/response_timeout.rs` (new
  integration target) with the handshake-completing silent stub and the
  paused-clock deadline tests; `tests/pub_surface.rs` extended for the new
  builder.
- `camel-redis-repo` (repository service crate): `connection.rs` — pass the
  margin value at construction; production-path test proving the local
  backstop governs over the driver deadline.
- `docs/adr/0063-redis-repository-service.md` — amend Decision 13's
  "Effective bound today" paragraph: the tracked follow-up has landed
  (component accepts `with_response_timeout`; the repository service crate
  passes 35 s so its own 30 s contract governs end to end).

## Architecture boundaries

Data-plane only. No DSL, URI, or config-schema surface changes; no
control-plane involvement. The component seam widens additively
(`with_response_timeout` joins `get_conn`/`refresh` as public seams the
service crate may use, per the redis-failover spec's seam-reuse pattern).
ADR-0063 Decision 13 (repository service) governs the backstop contract and
explicitly anticipates this plumbing as its tracked follow-up; landing it
amends that paragraph (the "component untouched" clause is superseded
precisely to the extent Decision 13 mandated). The ADR amendment replaces
the "Effective bound today" paragraph with the landed state. Consumers that
do not call the builder see zero behavior delta.

## Alternatives considered

- **Set the response timeout to `None` in the repo path** (disable driver
  deadline, rely solely on the local backstop): rejected — removes
  defense-in-depth for any future code path that bypasses the local
  backstop; the margin approach keeps both layers with deterministic
  ordering.
- **Pass exactly 30 s and let the driver enforce**: rejected — makes the
  winner a race between two equal deadlines, moving classification into the
  driver's error type instead of the tested local message.
- **New `RedisEndpointConfig` field (URI `?responseTimeout=`)**: rejected —
  drags camel-config defaults/schema/URI plumbing for a need that is
  internal to the service-crate seam; widens blast radius (proposal
  excludes config-surface growth).

## Testing strategy

Deterministic tests against silent/stubbing TCP peers, all timing under
tokio's paused clock (virtual-time boundaries, no wall-clock margins — poll
the command future before advancing virtual time):

- Component: configured large timeout (10 s) keeps a command still pending
  at a virtual-time boundary past the 500 ms driver default (asserts the
  configured value replaced the default); configured small timeout (100 ms)
  fails by its virtual boundary; `refresh()` rebuild still carries the
  deadline (probe after refresh); a peer that never completes the RESP
  handshake fails by `connection_timeout_secs`, proving the response-timeout
  setting does not alter the connect timeout. The command-deadline tests run
  against a handshake-completing silent stub (port of the repository
  service crate's `FakeRedisServer::start_silent` pattern: RESP-frame-aware,
  answers `CLIENT SETINFO`/`SELECT`/`AUTH`, then consumes application
  commands without reply) — a zero-byte peer cannot work because the driver
  completes `setup_connection` before returning the connection. The
  connect-timeout test uses a raw never-responding peer. Tests live in a
  new integration-test target (`tests/response_timeout.rs`) exercising the
  public `new`/`with_response_timeout`/`get_conn`/`refresh` surface through
  the real `topology_from_config` path.
- Repo: production-path connect against the silent peer with an injectable
  short local backstop, under the paused clock where both deadlines are
  deterministic: the local backstop error ("redis command response timed
  out after …") must win, proving the driver deadline sits strictly above
  the backstop. Pre-fix, the driver's 500 ms timer would fire first under
  the same clock. The exact margin (35 s) is an implementation constant
  asserted only through this ordering behavior.
