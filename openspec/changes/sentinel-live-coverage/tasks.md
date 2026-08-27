# Tasks: sentinel-live-coverage

## 1. Suite serialization

### Task 1.1 — serialize sentinel-topology tests on a suite-wide lock

- **Files**:
  - `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- **Steps**:
  1. Add a suite-wide serialization lock near the sentinel section:
     `static SENTINEL_TOPOLOGY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());`
     with a doc comment: fixed host ports + same-label stale removal make concurrent
     sentinel-topology tests kill each other's containers; every sentinel-topology
     test in this file holds this guard for its whole body.
  2. Retrofit the two existing sentinel tests (`cache_sentinel_selected_by_config_live`,
     `cache_sentinel_data_auth_live`) to acquire
     `let _guard = SENTINEL_TOPOLOGY_LOCK.lock().await;` as their first statement.
- **Tests**:
  - name: `cache_sentinel_selected_by_config_live` (existing, unmodified behavior)
    - setup: Docker available; existing suite code plus the lock retrofit.
    - action: run the existing test.
    - assert: still passes with the lock held (container provisions under the guard).
    - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_sentinel_selected_by_config_live`
    - expected: pass.
  - name: `cache_sentinel_data_auth_live` (existing, unmodified behavior)
    - setup: Docker available; SENT_AUTH_* topology provisions under the lock.
    - action: run the existing password-auth round-trip test.
    - assert: passes with the lock held.
    - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_sentinel_data_auth_live`
    - expected: pass.
- **Acceptance**:
  - Both existing sentinel tests pass under the new lock.
  - `cargo clippy -p camel-test --all-targets --all-features -- -D warnings` exits 0.
  - `cargo fmt --check --all` exits 0.

### Task 1.2 — ACL topology and named-user round-trip test

- **Files**:
  - `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- **Steps**:
  1. Add ACL topology constants (distinct from `SENT_*` and `SENT_AUTH_*`):
     `SENT_ACL_MASTER_PORT: u16 = 17489`, `SENT_ACL_REPLICA_PORT: u16 = 17490`,
     `SENT_ACL_SENTINEL_PORT: u16 = 27489`, `SENT_ACL_MASTER_NAME: &str = "mymaster"`,
     `SENT_ACL_DEFAULT_PASSWORD: &str = "default-secret"`,
     `SENT_ACL_USERNAME: &str = "camel"`, `SENT_ACL_PASSWORD: &str = "camel-secret"`,
     `SENT_ACL_LABEL_KEY: &str = "org.rust-camel.redis-repositories-sentinel-acl"`,
     `SENT_ACL_LABEL_VALUE: &str = "true"`.
  2. Add `async fn remove_stale_sentinel_acl_containers()` mirroring
     `remove_stale_sentinel_auth_containers` (bollard, label-filtered, force remove).
  3. Add `async fn sentinel_acl_topology() -> ContainerAsync<GenericImage>` mirroring
     `sentinel_auth_topology` (mapped ports, `+monitor` ready condition, ACL label)
     with this startup script — ACL rules single-quoted so `>` never redirects:
     master: `redis-server --port {master} --user 'default on >{default-secret} ~* +@all' --user 'camel on >{camel-secret} ~* +@all' --daemonize yes`,
     wait until `redis-cli --no-auth-warning -a {SENT_ACL_DEFAULT_PASSWORD} -p {master} ping` answers PONG;
     replica: `redis-server --port {replica} --daemonize yes --slaveof 127.0.0.1 {master} --masteruser {SENT_ACL_USERNAME} --masterauth {SENT_ACL_PASSWORD} --user 'default on >{default-secret} ~* +@all' --user 'camel on >{camel-secret} ~* +@all' --slave-announce-ip 127.0.0.1 --slave-announce-port {replica}`,
     wait until `redis-cli --no-auth-warning -a {SENT_ACL_DEFAULT_PASSWORD} -p {replica} ping` answers PONG
     (the replica requires auth — the default password — so the readiness probe must
     authenticate, unlike the SENT_AUTH replica);
     sentinel conf: `sentinel monitor {name} 127.0.0.1 {master} 1`,
     `sentinel auth-user {name} {SENT_ACL_USERNAME}`, `sentinel auth-pass {name} {SENT_ACL_PASSWORD}`,
     `down-after-milliseconds 2000`, `failover-timeout 10000`, `parallel-syncs 1`.
  4. Add `async fn sentinel_acl_master_port() -> Option<u16>` mirroring
     `sentinel_auth_master_port` (SENTINEL get-master-addr-by-name against
     `SENT_ACL_SENTINEL_PORT`, parse when ip == 127.0.0.1).
  5. Add `#[tokio::test(flavor = "multi_thread")] async fn cache_sentinel_acl_user_live()`:
     acquire `SENTINEL_TOPOLOGY_LOCK`; start `sentinel_acl_topology()`; wait (existing
     `support::wait::wait_until`, 30 s, 250 ms) until
     `sentinel_acl_master_port() == Some(SENT_ACL_MASTER_PORT)`.
  6. Build the context from TOML: `[default.cache_repo] backend = "redis"`,
     `sentinel_nodes = ["127.0.0.1:{SENT_ACL_SENTINEL_PORT}"]`,
     `master_name = "{SENT_ACL_MASTER_NAME}"`, `username = "{SENT_ACL_USERNAME}"`,
     `password = "{SENT_ACL_PASSWORD}"`, `stale_retention = "30s"`.
  7. `repo.set("acl-k", cache_entry(), None)` then `repo.get("acl-k")`; assert bytes
     equal (same shape as `cache_sentinel_data_auth_live`).
  8. Rejection proofs against `SENT_ACL_MASTER_PORT`:
     (a) client with password only (no username):
     `redis://:{SENT_ACL_PASSWORD}@127.0.0.1:{SENT_ACL_MASTER_PORT}` —
     `get_multiplexed_async_connection` or first PING fails with a string containing
     `WRONGPASS`; (b) bare client `redis://127.0.0.1:{SENT_ACL_MASTER_PORT}` —
     fails with `NOAUTH` or `WRONGPASS`. Follow the match shape of
     `cache_sentinel_data_auth_live`'s bare-client block.
- **Tests**:
  - name: `cache_sentinel_acl_user_live`
    - setup: Docker; ACL topology from this task's helpers.
    - action: config with username+password through `CamelConfig::configure_context`
      → `cache_repository("redis")` → set/get round-trip; then the two rejection probes.
    - assert: round-trip succeeds (username reached the ACL user); password-only
      client rejected WRONGPASS; bare client rejected NOAUTH/WRONGPASS.
    - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_sentinel_acl_user_live`
    - expected: pass.
- **Acceptance**:
  - Test passes; contains no `#[ignore]`; topology readiness probe authenticates.
  - Dev-time discriminating-power check (run once manually, NOT committed): dropping
    `username` from the TOML makes the test fail with WRONGPASS.
  - `cargo clippy -p camel-test --all-targets --all-features -- -D warnings` exits 0.
  - `cargo fmt --check --all` exits 0; `cargo xtask lint-unwrap` exits 0.

### Task 1.3 — sentinel db selection test

- **Files**:
  - `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- **Steps**:
  1. Add `#[tokio::test(flavor = "multi_thread")] async fn cache_sentinel_db_select_live()`:
     acquire `SENTINEL_TOPOLOGY_LOCK`; start the existing `sentinel_topology()`;
     wait until `sentinel_master_port() == Some(SENT_MASTER_PORT)` (existing helper).
  2. Build the context with `[default.cache_repo] backend = "redis"`,
     `sentinel_nodes = ["127.0.0.1:{SENT_SENTINEL_PORT}"]`,
     `master_name = "{SENT_MASTER_NAME}"`, `db = 2`, `stale_retention = "30s"`.
  3. `repo.set("db2-k", cache_entry(), None)`; `repo.get("db2-k")` returns the entry.
  4. Assert via raw clients against the sentinel-resolved master port:
     `EXISTS camel:cache:redis:db2-k` is true on db 2
     (`redis://127.0.0.1:{master}/2`) and false on db 0
     (`redis://127.0.0.1:{master}/0`). Use the suite's `raw_connection` helper.
- **Tests**:
  - name: `cache_sentinel_db_select_live`
    - setup: Docker; existing unauthenticated SENT_* topology.
    - action: cache_repo with `db = 2` through sentinel selection; round-trip; EXISTS
      probes on both dbs of the resolved master.
    - assert: key present in db 2, absent from db 0.
    - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_sentinel_db_select_live`
    - expected: pass.
- **Acceptance**:
  - Test passes; EXISTS-based assertions only (no DBSIZE).
  - `cargo clippy -p camel-test --all-targets --all-features -- -D warnings` exits 0.
  - `cargo fmt --check --all` exits 0; `cargo xtask lint-unwrap` exits 0.

### Task 1.4 — failover-under-auth test

- **Files**:
  - `crates/camel-test/tests/redis_repositories_test.rs` (modified)
- **Steps**:
  1. Add `const SENT_ACL_RECOVERY_DEADLINE: Duration = Duration::from_secs(60);` and
     `const SENT_ACL_POLL: Duration = Duration::from_millis(100);` near the ACL consts.
  2. Add `async fn acl_node_info_replication(port: u16) -> String`: open
     `redis::Client::open(format!("redis://{SENT_ACL_USERNAME}:{SENT_ACL_PASSWORD}@127.0.0.1:{port}"))`,
     multiplexed async connection, `INFO replication` as String; return `String::new()`
     on any error.
  3. Add `async fn trigger_acl_failover()`: async adaptation of
     `redis_sentinel_test.rs`'s `trigger_failover` — `SENTINEL FAILOVER
     {SENT_ACL_MASTER_NAME}` via `try_raw_connection` against
     `redis://127.0.0.1:{SENT_ACL_SENTINEL_PORT}` with `query_async`; on an error
     string mentioning no promotable replica (`NOGOODSLAVE` / "no good slave"),
     `tokio::time::sleep(250ms)` and retry bounded by a 30 s deadline; panic with the
     sentinel error otherwise.
  4. Add `#[tokio::test(flavor = "multi_thread")] async fn cache_sentinel_failover_reauth_live()`:
     acquire `SENTINEL_TOPOLOGY_LOCK`; start `sentinel_acl_topology()`; wait until
     sentinel tracks `Some(SENT_ACL_MASTER_PORT)`.
  5. Pre-failover context: cache_repo TOML with `sentinel_nodes` (ACL sentinel port),
     `master_name`, `username`, `password`, `db = 2`, `stale_retention = "30s"`;
     complete a set/get round-trip on key `failover-k` (proves the connection works
     pre-failover).
  6. Failover chain (exact order; each wait via `support::wait::wait_until`,
     30 s deadline, 250 ms poll):
     (a) `old_port = sentinel_acl_master_port()` (must be `Some(SENT_ACL_MASTER_PORT)`);
     (b) wait until `acl_node_info_replication(old_port)` contains
     `role:master` AND `connected_slaves:1` (auth-aware replica-connected gate);
     (c) `trigger_acl_failover()`;
     (d) wait until `sentinel_acl_master_port()` is `Some(p)` with `p != old_port`;
     (e) wait until `acl_node_info_replication(old_port)` contains `role:slave`
     (demotion complete — mirrors `wait_node_demoted`).
  7. Post-failover recovery loop (deadline `SENT_ACL_RECOVERY_DEADLINE`, poll
     `SENT_ACL_POLL`): `repo.set("post-failover-k", cache_entry(), None)` then
     `repo.get("post-failover-k")`. Success ONLY when the set returns Ok AND the get
     returns `Some(entry)` whose bytes equal `cache_entry().bytes`; retry (sleep,
     loop) on any error, on `None`, or on a bytes mismatch, until the deadline. The
     first write hits the stale connection to the demoted node
     (READONLY/connection error) and the repository refresh re-resolves,
     re-authenticates as the ACL user, and re-selects db 2; assert the loop reports
     success within the deadline.
  8. Final proof against the NEW master port: `EXISTS camel:cache:redis:post-failover-k`
     true on db 2, false on db 0 (raw clients with
     `redis://{SENT_ACL_USERNAME}:{SENT_ACL_PASSWORD}@127.0.0.1:{new_port}/2` and `/0`).
- **Tests**:
  - name: `cache_sentinel_failover_reauth_live`
    - setup: Docker; ACL topology with replica-connected master; repo context with
      username+password+db=2; pre-failover round-trip done.
    - action: `SENTINEL FAILOVER` → wait different master → wait demotion →
      bounded set/get retry loop → EXISTS probes on new master.
    - assert: post-failover key visible in new master's db 2, absent from its db 0
      (re-auth + re-SELECT on the refreshed connection).
    - command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test cache_sentinel_failover_reauth_live -- --nocapture`
    - expected: pass (recovery typically well under 60 s; failover itself adds ~10 s).
- **Acceptance**:
  - Test passes; whole failover chain inside the lock; no `#[ignore]`.
  - `cargo clippy -p camel-test --all-targets --all-features -- -D warnings` exits 0.
  - `cargo fmt --check --all` exits 0; `cargo xtask lint-unwrap` exits 0.

### Task 1.5 — document the new live surfaces

- **Files**:
  - `crates/services/camel-redis-repo/CONTEXT.md` (modified)
- **Steps**:
  1. In `## Test seams`, extend the live-coverage bullet (currently naming the suite
     file, integration-tests feature, testcontainers, ADR-0054) with: the suite also
     covers named-user ACL authentication, non-zero db selection on a
     sentinel-selected repository, and failover recovery with auth and db active
     (post-promote re-auth + re-SELECT); sentinel-topology tests serialize on a
     suite-wide lock (fixed ports + stale-container removal).
  2. Keep the paragraph under 8 lines; STE wording; reference bd rc-q3mb.
- **Tests**:
  - name: `context-citations`
    - setup: edited CONTEXT.md.
    - action: run the lint.
    - assert: exit 0.
    - command: `cargo xtask lint-context-citations`
    - expected: pass.
- **Acceptance**:
  - `sed -n '/## Test seams/,/## Dependency/p' crates/services/camel-redis-repo/CONTEXT.md | rg -c "ACL"` ≥ 1.
  - The same Test-seams section contains db-selection and failover/suite-lock
    wording: `sed -n '/## Test seams/,/## Dependency/p' crates/services/camel-redis-repo/CONTEXT.md | rg -ci "failover|serialize"` ≥ 1
    and `sed -n '/## Test seams/,/## Dependency/p' crates/services/camel-redis-repo/CONTEXT.md | rg -ci "non-zero db|db selection"` ≥ 1
    (adjust the doc wording to make these exact commands pass; keep STE style).
  - `cargo xtask lint-context-citations` exits 0.

- [x] 1.1
- [x] 1.2
- [x] 1.3
- [x] 1.4
- [x] 1.5
