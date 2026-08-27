# Proposal: sentinel-live-coverage

## Why

The e_opus review of the sentinel data-auth work (bd rc-q3mb) found three live-coverage
gaps in the Redis repository path. Everything below runs through `cache_repo` config —
the path production code takes — not raw clients:

1. `username` (Redis 6+ ACL) has zero live coverage: the authenticated-sentinel live
   test proves `password` only (`requirepass`), so a regression that drops the ACL
   username from the connection handshake would pass CI.
2. Non-zero db through the sentinel repo path: standalone `?db=2` gained live coverage
   in a7bff938, but a sentinel-selected repository with `db = 2` has no live test —
   the SELECT must ride the sentinel-resolved master connection, a different code path.
3. Failover with authentication on: no live test proves that a promoted master
   receives re-auth (and re-SELECT when `db != 0`) on the refreshed repository
   connection. Today failover coverage is unauthenticated, and auth coverage is
   failover-free.

Redis is one failover regression away from a production incident; these three gaps are
the uncovered surfaces of exactly that scenario.

## What Changes

- Extend `crates/camel-test/tests/redis_repositories_test.rs` (integration-tests
  feature, testcontainers, redis:7-alpine — ACL-capable):
  - an ACL sentinel topology (named ACL user, not `default`) proving `username` +
    `password` round-trip through `cache_repo`, with no-username (WRONGPASS) and
    bare-client (NOAUTH/WRONGPASS) rejection proofs;
  - a sentinel-selected `cache_repo` with `db = 2` proving keys land in db 2 of the
    sentinel-resolved master, with the key absent from its db 0;
  - a failover-under-auth test on the authenticated topology: gate on
    replica-connected, issue `SENTINEL FAILOVER`, then prove post-promote writes
    re-authenticate (and re-SELECT when combined with `db = 2`).
- Excluded: production-code changes (this is a coverage change; any bug the new tests
  surface becomes its own bd issue), CI workflow edits (suite already runs in CI),
  component-level sentinel tests (`redis_sentinel_test.rs` owns component failover).

## Acceptance criteria

- ACL test fails if `username` is dropped from the data-node handshake.
- Sentinel db test fails if SELECT is skipped on the sentinel-resolved connection
  (key must be present in db 2, absent from db 0).
- Failover-under-auth test fails if the refreshed connection does not re-authenticate
  (and re-SELECT when db != 0).
- All three run non-ignored under the existing `integration-tests` feature; CI needs
  no workflow change.

## Risk budget

Test-only, camel-test crate, no production surface touched. New fixed host ports must
not collide with existing suites (17389/17390/27389 pattern extends). Acceptable:
longer suite runtime (each new topology adds ~10-15 s container startup; one
failover adds bounded wait loops). Out of bounds: weakening existing gates, touching
component or repo production code, adding `#[ignore]` tests (ADR-0054).

Bd: rc-q3mb
