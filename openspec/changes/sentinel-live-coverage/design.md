# Design: sentinel-live-coverage

## Approach

Test-only extension of `crates/camel-test/tests/redis_repositories_test.rs`. One new
container topology serves two of the three gaps; the third reuses an existing one:

1. **ACL topology** (`SENT_ACL_*` ports 17489/17490/27489, distinct from `SENT_*`
   17379-prefixed and `SENT_AUTH_*` 17389-prefixed ports). BOTH data nodes start with
   an identical ACL surface via `redis-server --user` startup flags — no runtime
   `ACL SETUSER`, so the replica carries the users from its first start (a runtime
   SETUSER on an attached replica is rejected; startup flags apply before the
   replica role begins). The topology script runs under `sh -c`, so every ACL rule
   string is single-quoted to keep `>password` and `~*` away from the shell:
   `--user 'default on >default-secret ~* +@all' --user 'camel on >camel-secret
   ~* +@all'`. The replica runs `--masteruser camel --masterauth camel-secret`;
   the promoted replica therefore accepts `camel` immediately.
   Sentinel config adds `sentinel auth-user` + `sentinel auth-pass` so monitoring
   authenticates as the named user against whichever node holds the master role.
   The repo config sets `username = "camel"` + `password = "camel-secret"`.
   Discriminating power: if the handshake drops `username`, the client authenticates
   as `default` with camel-secret → WRONGPASS → round-trip fails. Rejection proofs:
   a client presenting camel-secret with no username (WRONGPASS) and a bare client
   (NOAUTH/WRONGPASS) both fail against the master.
2. **Sentinel db test** reuses the unauthenticated `SENT_*` topology: `db = 2` in the
   cache_repo config, round-trip on a unique key, then assert `EXISTS <key>` is
   false on db 0 and true on db 2 of the sentinel-resolved master via a raw client.
   Key-existence assertions (not `DBSIZE`) keep the test immune to keys written by
   other tests against the same topology. Mirrors a7bff938's standalone assertions
   on the sentinel path.
3. **Failover-under-auth** reuses the ACL topology with the exact ordering of
   `redis_sentinel_test.rs`: capture the current master port → require that master
   to report a connected replica (auth-aware probe: `redis-cli --user camel -a
   camel-secret`) → `SENTINEL FAILOVER` retried only while sentinel reports no
   promotable replica (bounded) → wait until sentinel reports a DIFFERENT master
   port → wait until the old master reports `role:slave` → only then issue repo
   set/get with `db = 2` config. Post-promote the cached connection targets the
   demoted node; the write fails (READONLY or connection error), the repository
   refresh (redis-failover spec: failover surfaces through explicit refresh after a
   failed command) re-resolves, re-authenticates as `camel`, and re-SELECTs db 2.
   Final proof: the post-failover key exists on the NEW master's db 2 and is absent
   from the new master's db 0. Combining db with failover proves re-SELECT on the
   refreshed connection at no extra cost.

**Test isolation.** Every topology in this suite binds fixed host ports and
force-removes same-label containers, so concurrent tests sharing a topology would
kill each other's containers and collide on ports. All sentinel-topology tests in
the file (existing and new) serialize through one suite-wide async mutex held for
the whole test body (the `redis_sentinel_test.rs` serial-failover approach),
provisioning a fresh container per test under the lock. Labels stay per-topology
(`SENT_ACL_LABEL_KEY` distinct from `SENT_*`/`SENT_AUTH_*`).

No production code changes. Structural wiring already exists: `RedisRepositoryConfig`
exposes `username`, `db`, `sentinel_username` (camel-config config.rs), and
camel-redis threads credentials onto sentinel endpoints.

## Affected crates

- `camel-test`: three new `#[tokio::test]`s + topology/probe helpers in
  `tests/redis_repositories_test.rs` (integration-tests feature).

## Architecture boundaries

Tests live in the camel-test integration layer (ADR-0054: testcontainers, never
`#[ignore]`); the service crate (`camel-redis-repo`) and component (`camel-redis`)
are exercised through their public config surface (`CamelConfig::configure_context`
→ `cache_repository`), preserving the leaf direction camel-test → camel-redis-repo.
No boundary crossings; CI runs the existing `redis_repositories_test` target
unchanged.

## Open questions

None — the structural config surface was verified against main
(username/db/sentinel_username present in camel-config; sentinel auth-user/auth-pass
and masteruser/masterauth are Redis 6+ features, image is redis:7-alpine).

## Implementation deviations (recorded at task 1.2)

- **ACL rule quoting.** The blessed form `--user 'default on >default-secret ~* +@all'`
  (whole rule single-quoted) cannot start redis 7.4: the server joins every argv
  token after `--user` into one config line, and a single spaced argv is wrapped
  whole by `sdscatrepr`, failing with "Spaces not allowed in ACL usernames". The
  landed topology quotes per token (`--user default on '>default-secret' '~*' +@all`),
  preserving the blessed users, rules, and roles; `>` still never reaches the shell.
  Proven against the running container during task 1.2; the topology function's doc
  comment records the trap.
- **WRONGPASS surface.** redis-rs folds the handshake WRONGPASS reply into
  `ErrorKind::AuthenticationFailed` (message "Password authentication failed"),
  so the password-only rejection proof matches the error kind with a WRONGPASS
  string fallback; the bare-client proof keeps the NOAUTH/WRONGPASS string match.
- **ACL channel patterns (`&*`), recorded at task 1.4.** Command-line `--user` rules
  load with reset semantics: a user declared without a `&` channel pattern gets an
  empty channel list, and Sentinel's pub/sub hello link (authenticated as the named
  user) is rejected with `-NOPERM` every second. The replica link is then flagged
  disconnected and `SENTINEL FAILOVER` returns NOGOODSLAVE forever. All four
  `--user` rules therefore include `'&*'`; the topology function documents the trap.
- **Recovery-loop timing.** The repository connection refreshed during the ~10 s
  demotion wait, so the recovery loop succeeded on its first attempt (the plan
  tolerates immediate success). The chain's demotion gate plus the final db-2 EXISTS
  proof on the new master carry the re-auth and re-SELECT assertions. The test keeps
  timing `println!` lines: cargo test captures stdout, so they surface only on
  failure, where they attribute the failing stage of a 12 s live-Docker test.
