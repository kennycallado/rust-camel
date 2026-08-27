# sentinel-live-coverage delta

## MODIFIED Requirements

### Requirement: Redis repository live coverage runs in camel-test, never ignored

The system SHALL cover the Redis repository backends with a non-ignored integration
suite in `crates/camel-test/tests/redis_repositories_test.rs` under the existing
`integration-tests` feature, using testcontainers to self-provision Redis and a Sentinel
topology (the `redis_sentinel_test.rs` pattern, per ADR-0054 which forbids
external-service `#[ignore]` tests). The suite SHALL exercise cache and idempotent
trait behavior against real Redis (round-trips, prefix-scoped `clear`, registration via
`CamelConfig`) and the sentinel path (construction through sentinels). The suite SHALL
also cover, through the repository config path (`cache_repo`), the three live-coverage
gaps identified by the sentinel data-auth review (bd rc-q3mb): named-user ACL
authentication (Redis 6+ `username` + `password` reaching the data nodes), non-zero
database selection on a sentinel-selected repository, and failover recovery while
authentication and non-zero db are active (post-promote writes re-authenticate and
re-SELECT on the refreshed connection). The
`.github/workflows/ci.yml` workflow SHALL run the suite alongside the existing
`redis_test` target. `camel-redis-repo` SHALL be an optional dependency of `camel-test`
under `integration-tests` (leaf direction only); the service crate SHALL NOT depend on
`camel-test`.

#### Scenario: integration suite runs in CI without ignore markers

- **GIVEN** the `redis_repositories_test` suite in `camel-test` with the
  `integration-tests` feature enabled
- **WHEN** `cargo test -p camel-test --features integration-tests --test
  redis_repositories_test` runs in CI
- **THEN** the suite provisions its containers, exercises cache and idempotent
  behavior, and contains no `#[ignore]` attributes

#### Scenario: named ACL user authenticates through the sentinel repo path

- **GIVEN** a sentinel topology whose master requires a named ACL user (default user
  holds a different password; sentinel and replica authenticate as the named user)
- **WHEN** a cache repository is configured with `sentinel_nodes` + `master_name` +
  `username` (the ACL name) + `password` (the ACL password) and performs a set/get
  round-trip
- **THEN** the round-trip succeeds against the sentinel-resolved master, a client
  presenting the ACL password without the ACL username is rejected (WRONGPASS), and a
  client with no credentials is rejected (NOAUTH or WRONGPASS)

#### Scenario: sentinel-selected repository writes to the configured database

- **GIVEN** the unauthenticated sentinel topology with a ready master
- **WHEN** a cache repository is configured with `sentinel_nodes` + `master_name` +
  `db = 2` and performs a set/get round-trip
- **THEN** the written key is visible in db 2 of the sentinel-resolved master (via a
  raw client on db 2) and is absent from db 0 of that master

#### Scenario: repository recovers from sentinel failover with auth and db active

- **GIVEN** the authenticated (named ACL user) sentinel topology with a
  replica-connected master, a cache repository configured with `username` + `password`
  + `db = 2` that has completed a round-trip before failover
- **WHEN** `SENTINEL FAILOVER` promotes the replica and the old master is demoted
- **THEN** subsequent repository set/get succeed within a bounded recovery window,
  re-authenticating as the ACL user and re-selecting db 2 on the refreshed connection,
  and the post-failover key is visible in db 2 of the new master and absent from its
  db 0
