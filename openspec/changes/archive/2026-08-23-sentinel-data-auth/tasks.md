# Tasks: sentinel-data-auth

## camel-component-redis

### Task 1: data-node username field and db widening on RedisEndpointConfig

**Files:**
- `crates/components/camel-redis/src/config.rs` (modified)
- `crates/components/camel-redis/src/sentinel_config.rs` (modified)
- `crates/components/camel-redis/src/topology.rs` (modified)
- `crates/components/camel-redis/tests/config_roundtrip.rs` (modified)

**Steps:**
1. In `config.rs`, change `RedisEndpointConfig.db` from `u8` to `u16` (struct field, every constructor/default that assigns it, and the `?db=N` URI query parse — parse as `u16` and reject values above 16383 with an error naming the limit, updating the "0-255"-style error strings). In `sentinel_config.rs`, widen `pub db: u8` (~:122) to `u16` and its node-URL parse (~:178-180, error "expected integer 0-255") to match, also rejecting above 16383. Grep every `db:` site in the crate and fix types; `set_db(config.db as i64)` in topology stays correct.
2. In `config.rs`, add `pub username: Option<String>` to `RedisEndpointConfig` beside `password`, defaulting `None` in every constructor/default. SCOPE CUT (blessed plan decision): the URI parser is NOT modified — username is populated only by camel-config threading (Task 3); the parser's existing bare-username behavior (commit `60ccdb1b`) is untouched, and `redis_url()`/`redis_url_safe()` render unchanged.
3. Extend the crate's redaction surface so `RedisEndpointConfig.username` never prints (mirror the existing `password` handling in the redaction helper / Debug path).
4. In `topology.rs` `sentinel_node_conn_info`, propagate username: `.set_username(u)` on the `RedisConnectionInfo` when `config.username` is `Some`. Update the stale doc comments: topology.rs ~335-336 ("there is no node username field" — now there is) and config.rs ~867 ("no db slot" wording) to state the current truth.
5. Add the username-propagation unit test INSIDE `topology.rs`'s `mod tests` gated `#[cfg(feature = "sentinel")]` (the fn is private + feature-gated; an integration test cannot see it and an ungated unit test will not compile).

**Tests:**
- `db_u16_round_trip` (tests/config_roundtrip.rs): `?db=16383` parses to `db == 16383` and re-renders identically; `?db=256` parses (was u8 overflow, now valid); `?db=16384` fails validation with an error naming the 16383 limit.
- `sentinel_db_u16_round_trip` (unit test in `crates/components/camel-redis/src/sentinel_config.rs` `mod tests` — the parse lives there): sentinel node URL db segment `300` parses to db 300; `16384` is rejected with the 16383-limit error.
- `endpoint_debug_redacts_username` (tests/config_roundtrip.rs): endpoint built by struct literal with `username = Some("svc")`, `password = Some("hunter2")` → `format!("{:?}", endpoint)` contains neither literal.
- `sentinel_node_conn_info_carries_username` (topology.rs `#[cfg(feature = "sentinel")] mod tests`): `RedisEndpointConfig` struct literal, sentinel topology kind, `username = Some("svc")`, `password = Some("p")`, `db = 2` → the built `SentinelNodeConnectionInfo`'s redis settings carry username/password/db via their getters.
- commands: `cargo test -p camel-component-redis` AND `cargo test -p camel-component-redis --features sentinel` (both must run; the gated test lives only in the second).
- expected: fail before, pass after.

**Acceptance:**
- Both test commands exit 0 (all existing tests included).
- `cargo clippy -p camel-component-redis --all-targets --features sentinel -- -D warnings` and `cargo fmt --check` clean.
- `rg -n 'db: u8' crates/components/camel-redis/src/` returns no hits.

- [x] 1

## camel-config

### Task 2: data-node fields, validation matrix, and collision identity

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)
- `crates/camel-config/tests/idempotent_repo_config.rs` (modified)

**Steps:**
1. Add to `CacheRepoConfig` and `IdempotentRepoConfig` (beside the existing redis fields, same `deny_unknown_fields` structs): `pub password: Option<String>`, `pub username: Option<String>`, `pub db: Option<u16>` with serde defaults `None`. Rustdoc on each: sentinel-mode data-node credential / database selection; rejected in `url` mode (credentials ride the URL userinfo).
2. Extend `validate_redis_topology_fields` (the shared matrix helper): when `url` is set and any of `password`/`username`/`db` is set → error naming `<field>.<name>` (e.g. `cache_repo.password`, `idempotent_repo.db`) stating they require `sentinel_nodes`; when `sentinel_nodes` is set and `db` is `Some(d)` with `d > 16383` → error naming `<field>.db`. The existing `master_name`/`sentinel_*`-without-`sentinel_nodes` rejections gain the three new names in the same pattern.
3. Extend the `memory` arm (cache) and `redb` arms (cache + idempotent) `does not apply` rejections with `password`, `username`, `db` — extend the EXISTING tests `cache_redb_rejects_redis_fields` and `cache_redis_rejects_memory_fields` (cache_repo_config.rs ~:778, ~:827) with the three new fields rather than adding parallel ones, mirroring the idempotent side's structure.
4. `redis_database_key` (`config.rs:845-869`, call sites ~1807-1816): the sentinel branch currently hardcodes db 0 — thread each config's effective `db.unwrap_or(0)` into the identity so it reads `sentinel|<nodes>|<master>|<db>`; the standalone branch is unchanged.

**Tests:** (exact, follow each file's `make_cfg` pattern)
- `cache_data_fields_rejected_in_url_mode`: cache `url` + `password` → error names `cache_repo.password`; `url` + `db` → names `cache_repo.db`.
- `cache_data_db_out_of_range_rejected`: sentinel + `db = 20000` → error names `cache_repo.db`.
- (extended) `cache_memory_rejects_data_fields` — new: memory + `username` → error names `cache_repo.username`; (extended) `cache_redb_rejects_redis_fields` / `cache_redis_rejects_memory_fields` cover their arms' three new fields.
- `idempotent_data_fields_mirror_matrix`: idempotent `url` + `db` → `idempotent_repo.db`; redb + `password` → `idempotent_repo.password`.
- `collision_uses_effective_sentinel_db`: both repos redis on the same sentinel_nodes/master with identical effective prefixes, cache `db = 2` and idempotent `db = 3` → validate OK; both `db = 2` → collision error; both unset (db 0) → collision error (regression of existing behavior).
- command: `cargo test -p camel-config`
- expected: fail before, pass after (ALL green at this task's boundary — endpoint-threading assertions live in Task 3).

**Acceptance:**
- `cargo test -p camel-config` exits 0 (complete suite green at this boundary).
- `cargo clippy -p camel-config --all-targets -- -D warnings`, `cargo fmt --check` clean.
- `cargo xtask lint-secrets` exits 0.

- [x] 2

### Task 3: endpoint threading and redacting Debug

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/tests/cache_repo_config.rs` (modified)
- `crates/camel-config/tests/idempotent_repo_config.rs` (modified)

**Steps:**
1. `redis_endpoint_from_fields` (`context_ext.rs:215-253`): extend the helper's signature with `password: Option<&str>`, `username: Option<&str>`, `db: Option<u16>`; in the sentinel branch, after the URI parse set `endpoint.password`, `endpoint.username`, and `endpoint.db = db.unwrap_or(0)`. Update BOTH callers (`redis_endpoint_from_cache_repo`, `redis_endpoint_from_idempotent_repo`) to pass the config fields. The `url` branch is untouched (validation already rejects the fields in url mode).
2. Extend both hand-written redacting `Debug` impls (`CacheRepoConfig`, `IdempotentRepoConfig`): `password` and `username` render as `Some(***)` when set, mirroring the sentinel-password handling.

**Tests:**
- `cache_data_credentials_reach_endpoint`: cache sentinel mode + `password = "master-secret"` + `username = "svc"` + `db = 2` → `redis_endpoint_from_cache_repo` endpoint `.password == Some("master-secret")`, `.username == Some("svc")`, `.db == 2`.
- `idempotent_data_credentials_reach_endpoint`: idempotent sentinel + `password = "x"` + `db = 3` → endpoint carries both.
- `cache_debug_redacts_data_credentials`: cache sentinel mode with `password = "master-secret"`, `username = "svc-user"` → `format!("{:?}")` contains neither literal.
- `idempotent_debug_redacts_data_credentials`: idempotent sentinel mode with `password = "idem-secret"`, `username = "idem-user"` → neither literal in `{:?}`.
- command: `cargo test -p camel-config`
- expected: the four new tests fail before step 1-2, pass after.

**Acceptance:**
- `cargo test -p camel-config` exits 0 (complete suite).
- `cargo clippy -p camel-config --all-targets -- -D warnings`, fmt clean.
- `cargo xtask lint-secrets` exits 0.

- [x] 3

## camel-test

### Task 4: live authenticated-sentinel cache round-trip

**Files:**
- `crates/camel-test/tests/redis_repositories_test.rs` (modified)

**Steps:**
1. Following the existing sentinel provisioning PATTERN (master + replica + sentinel, `redis:7-alpine`, fixed loopback ports, announce `127.0.0.1`, quorum 1, label-scoped stale cleanup, `+monitor` readiness), provision a SEPARATE authenticated topology — do NOT modify the existing unauthenticated one: master with `--requirepass master-secret`, replica with `masterauth master-secret`, sentinel with `sentinel auth-pass mymaster master-secret`. Own fixed ports (e.g. 17389/17390/27389) and own label (e.g. `org.rust-camel.redis-repositories-sentinel-auth`) so the two never collide.
2. Add `cache_sentinel_data_auth_live`: `[default.cache_repo] backend = "redis"`, `sentinel_nodes` (authenticated topology, bare host:port), `master_name`, `password = "master-secret"` → context build succeeds, `cache_repository("redis")` resolves, `set`/`get` round-trip works, and a raw redis client WITHOUT the password fails with an auth error (NOAUTH/WRONGPASS) — proving the master really enforces auth.
3. No `#[ignore]` attributes.

**Tests:**
- `cache_sentinel_data_auth_live`: as above — command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test` — expected: pass (Docker required).

**Acceptance:**
- The live suite passes including the new test; zero `#[ignore]` in the file.
- `cargo clippy -p camel-test --features integration-tests --all-targets -- -D warnings` clean.

- [x] 4

## docs

### Task 5: schema field tables

**Files:**
- `docs/src/configuration/schema.md` (modified)

**Steps:**
1. In the `cache_repo` and `idempotent_repo` reference tables: add rows `password` (sentinel-mode data-node credential, redacted, rejected in `url` mode), `username` (same posture), `db` (`Option<u16>`, 0..=16383, sentinel-mode data database, rejected in `url` mode). Mark all three "Redis (sentinel mode) only". Note in prose that in `url` mode the password and db ride the URI (`redis://:pass@host:port?db=N` — userinfo password, query db; username in the URI is not supported).
2. ste-writing rules on new prose.

**Tests:**
- `mdbook_build_resolves`: command `nix shell nixpkgs#mdbook -c mdbook build docs` — expected exit 0.

**Acceptance:**
- mdbook build exits 0.

- [x] 5
