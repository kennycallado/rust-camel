# Tasks: redisgeo

## Task 1 — Register GEO commands in config

Files:
- crates/components/camel-redis/src/config.rs (modified)

Steps:
1. Add a `// Geo operations` section to the `RedisCommand` enum after the
   sorted-set variants: `Geoadd, Geopos, Geodist, Geosearch, Geohash`.
2. Add `FromStr` arms in `impl FromStr for RedisCommand`: `"GEOADD" =>
   Ok(RedisCommand::Geoadd)`, `"GEOPOS"`, `"GEODIST"`, `"GEOSEARCH"`,
   `"GEOHASH"` (same shape as the existing arms; the existing
   `to_uppercase()` makes parsing case-insensitive).
3. In `is_idempotent_command`, add `RedisCommand::Geopos`,
   `RedisCommand::Geodist`, `RedisCommand::Geosearch`,
   `RedisCommand::Geohash` to the read-only list. Do NOT add `Geoadd`.

Tests (inline `mod tests` in config.rs, next to `test_command_from_str`):
- name: `test_geo_command_from_str`
  setup: nothing (pure parse)
  action: `RedisCommand::from_str` for `"GEOADD"`, `"geosearch"`,
  `"GeoDist"`, `"GEOPOS"`, `"GEOHASH"`
  assert: yields `Geoadd`, `Geosearch`, `Geodist`, `Geopos`, `Geohash`
  command: `cargo test -p camel-component-redis test_geo_command_from_str`
  expected: fails before step 2, passes after
- name: `test_geo_idempotency_classification`
  setup: nothing
  action: `is_idempotent_command` for all five variants
  assert: true for `Geopos`, `Geodist`, `Geosearch`, `Geohash`; false for
  `Geoadd`
  command: `cargo test -p camel-component-redis test_geo_idempotency_classification`
  expected: fails before step 3, passes after

Acceptance:
- `cargo test -p camel-component-redis config::` exits 0
- `cargo fmt --check` and `cargo clippy -p camel-component-redis -- -D warnings` exit 0

- [x] task-1

## Task 2 — Geo family module skeleton and routing

Files:
- crates/components/camel-redis/src/commands/geo.rs (new)
- crates/components/camel-redis/src/commands/mod.rs (modified: add `pub mod geo;`)
- crates/components/camel-redis/src/executor.rs (modified)

Steps:
1. Create `commands/geo.rs` with:
   - `pub(crate) fn is_geo_command(cmd: &RedisCommand) -> bool` matching
     the five variants (mirror `is_zset_command` in zset.rs).
   - `pub(crate) async fn dispatch(cmd: &RedisCommand, conn: &mut MultiplexedConnection, exchange: &mut Exchange) -> Result<(), CamelError>`:
     asserts `is_geo_command`, `match` over the five variants delegating
     to per-op handlers added in Tasks 3-5 (for now each arm returns
     `Err(CamelError::ProcessorError("geo op not implemented"))`).
2. Add `pub mod geo;` to `commands/mod.rs`.
3. In `executor.rs::dispatch_command`, add a `// Geo commands` arm
   routing `RedisCommand::Geoadd | Geopos | Geodist | Geosearch |
   Geohash => commands::geo::dispatch(cmd, conn, exchange).await`.

Helpers (validators, unit resolution, f64 header) are defined in the
tasks that first use them, so nothing is dead code at any task boundary.

Tests (inline `mod tests` in geo.rs):
- name: `test_is_geo_command_matches_only_geo_variants`
  setup: nothing
  action: `is_geo_command` for `Geoadd`, `Geopos`, `Geodist`,
  `Geosearch`, `Geohash`, and `RedisCommand::Zadd`
  assert: true for the five geo variants, false for `Zadd`
  command: `cargo test -p camel-component-redis test_is_geo_command_matches_only_geo_variants`
  expected: fails before step 1, passes after

Acceptance:
- `cargo test -p camel-component-redis commands::geo` exits 0
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0

- [x] task-2

## Task 3 — GEOADD and GEOPOS handlers

Files:
- crates/components/camel-redis/src/commands/geo.rs (modified)

Steps:
1. Validation helpers (defined here, first use):
   - `fn validate_longitude(v: f64) -> Result<(), CamelError>` — finite,
     `-180.0..=180.0`, else `ProcessorError` naming
     `CamelRedis.Longitude`.
   - `fn validate_latitude(v: f64) -> Result<(), CamelError>` — finite,
     `-90.0..=90.0`, else `ProcessorError` naming
     `CamelRedis.Latitude`.
   - `fn require_f64_header(exchange: &Exchange, name: &str) -> Result<f64, CamelError>`
     — reads via `get_f64_header`; missing or non-numeric fails with
     `ProcessorError` naming the header.
 2. `fn resolve_geoadd_args(exchange: &Exchange) -> Result<(String, f64, f64, String), CamelError>`:
   `let key = require_key(exchange)?;` longitude and latitude via
   `require_f64_header` then `validate_longitude`/`validate_latitude`;
   member via a new `require_member` helper reading `CamelRedis.Member`
   with `get_str_header` (missing fails naming `CamelRedis.Member`).
   Returns `(key, longitude, latitude, member)`.
3. `async fn execute_geoadd(exchange, conn) -> Result<(), CamelError>`:
   args via `resolve_geoadd_args`; run
   `redis::cmd("GEOADD").arg(&key).arg(longitude).arg(latitude).arg(member)`
   typed as `i64` via `query_async`; map errors through
   `crate::transport_error::redis_error_to_camel("GEOADD", e)`; set
   `exchange.input.body = Body::Json(serde_json::json!(n))`.
4. `async fn execute_geopos(exchange, conn)`:
   `require_key`; members via a `require_members` helper reading
   `CamelRedis.Members` with `get_str_vec_header` (missing/empty fails
   naming `CamelRedis.Members`); run
   `redis::cmd("GEOPOS").arg(&key).arg(&members[..])` typed as
   `Vec<Option<(f64, f64)>>`; build the body with pure helper
   `fn json_from_geopos(positions: &[Option<(f64, f64)>]) -> serde_json::Value`
   producing an array of `[lon, lat]` pairs or `null` entries.
5. Wire both arms in `dispatch` replacing the placeholder errors.

Tests (inline in geo.rs):
- name: `test_validate_latitude_rejects_out_of_range`
  setup: nothing
  action: `validate_latitude(95.0)`, `validate_latitude(-91.0)`,
  `validate_latitude(90.0)`, `validate_latitude(0.0)`
  assert: first two `Err` with message containing `CamelRedis.Latitude`;
  last two `Ok`
  command: `cargo test -p camel-component-redis test_validate_latitude_rejects_out_of_range`
  expected: fails before step 1, passes after
- name: `test_validate_longitude_rejects_out_of_range`
  setup: nothing
  action: `validate_longitude(-181.0)`, `validate_longitude(181.0)`,
  `validate_longitude(180.0)`
  assert: first two `Err` containing `CamelRedis.Longitude`; last `Ok`
  command: `cargo test -p camel-component-redis test_validate_longitude_rejects_out_of_range`
  expected: fails before, passes after
- name: `test_require_f64_header_missing_fails`
  setup: exchange without `CamelRedis.Longitude`
  action: `require_f64_header(&exchange, "CamelRedis.Longitude")`
  assert: `Err` containing `CamelRedis.Longitude`
  command: `cargo test -p camel-component-redis test_require_f64_header_missing_fails`
  expected: fails before, passes after
- name: `test_require_member_missing_fails`
  setup: exchange without `CamelRedis.Member`
  action: call the `require_member` helper
  assert: `Err` whose message contains `CamelRedis.Member`
  command: `cargo test -p camel-component-redis test_require_member_missing_fails`
  expected: fails before step 1, passes after
- name: `test_require_members_missing_and_empty_fail`
  setup: exchange without `CamelRedis.Members`; exchange with `[]`
  action: call `require_members`
  assert: both `Err` containing `CamelRedis.Members`
  command: `cargo test -p camel-component-redis test_require_members_missing_and_empty_fail`
  expected: fails before, passes after
- name: `test_json_from_geopos_shapes_pairs_and_nulls`
  setup: `vec![Some((13.361389, 38.115556)), None]`
  action: `json_from_geopos`
  assert: equals `serde_json::json!([[13.361389, 38.115556], null])`
  command: `cargo test -p camel-component-redis test_json_from_geopos_shapes_pairs_and_nulls`
  expected: fails before step 2, passes after
- name: `test_geoadd_rejects_bad_latitude_before_command`
  setup: exchange with key, member, valid longitude, latitude `95.0`
  action: run the same header-resolution + validation sequence
  `execute_geoadd` performs BEFORE any `redis::Cmd` is built (factor it
  into `fn resolve_geoadd_args(exchange) -> Result<(String, f64, f64, String), CamelError>`
  and call that; the handler calls it too)
  assert: `Err(ProcessorError)` containing `CamelRedis.Latitude`; no
  command construction happens
  command: `cargo test -p camel-component-redis test_geoadd_rejects_bad_latitude_before_command`
  expected: fails before, passes after

Acceptance:
- `cargo test -p camel-component-redis commands::geo` exits 0
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0

- [x] task-3

## Task 4 — GEODIST and GEOHASH handlers

Files:
- crates/components/camel-redis/src/commands/geo.rs (modified)

Steps:
1. `fn resolve_geo_unit(exchange: &Exchange) -> Result<&'static str, CamelError>`
   (defined here, first use): reads `CamelRedis.Unit` via
   `get_str_header`; accepts `m`, `km`, `mi`, `ft` (case-insensitive),
   defaults to `"m"` when absent; unknown values fail with
   `ProcessorError` naming `CamelRedis.Unit`.
2. `async fn execute_geodist(exchange, conn)`:
   `require_key`; member via `require_member` (`CamelRedis.Member`);
   second member via a `require_member2` helper reading
   `CamelRedis.Member2` (missing fails naming `CamelRedis.Member2`);
   unit via `resolve_geo_unit`; run
   `redis::cmd("GEODIST").arg(&key).arg(&m1).arg(&m2).arg(unit)`
   typed as `Option<f64>`; body = `json!(distance)` where `None`
   serializes to `null`.
3. `async fn execute_geohash(exchange, conn)`:
   `require_key`; members via `require_members`; run
   `redis::cmd("GEOHASH").arg(&key).arg(&members[..])` typed as
   `Vec<Option<String>>`; body = `json!(hashes)` (nulls preserved).
4. Wire both arms in `dispatch`.

Tests (inline in geo.rs):
- name: `test_resolve_geo_unit_defaults_and_rejects_unknown`
  setup: exchanges with `CamelRedis.Unit` = `"KM"`, `"furlongs"`, absent
  action: `resolve_geo_unit`
  assert: `"km"`, `Err` naming `CamelRedis.Unit`, `"m"`
  command: `cargo test -p camel-component-redis test_resolve_geo_unit_defaults_and_rejects_unknown`
  expected: fails before step 1, passes after
- name: `test_geodist_unit_defaults_to_meters`
  setup: exchange with key, `CamelRedis.Member` `a`, `CamelRedis.Member2`
  `b`, no `CamelRedis.Unit`
  action: build the command bytes for GEODIST via the same arg-assembly
  code path the handler uses (extract the assembly into a small pure fn
   `fn build_geodist_cmd(key, m1, m2, unit) -> redis::Cmd` and call it)
  assert: the command's args end with `"m"`
  command: `cargo test -p camel-component-redis test_geodist_unit_defaults_to_meters`
  expected: fails before step 1, passes after
- name: `test_require_member2_missing_fails`
  setup: exchange with `CamelRedis.Member` but no `CamelRedis.Member2`
  action: call `require_member2`
  assert: `Err` containing `CamelRedis.Member2`
  command: `cargo test -p camel-component-redis test_require_member2_missing_fails`
  expected: fails before, passes after
- name: `test_geohash_body_preserves_nulls`
  setup: `vec![Some("sqdtr74hyu0".into()), None]` as the typed reply
  action: serialize through the same `json!` mapping the handler uses
  (extract `fn json_from_geohashes(hashes: Vec<Option<String>>) -> serde_json::Value`)
  assert: equals `json!(["sqdtr74hyu0", null])`
  command: `cargo test -p camel-component-redis test_geohash_body_preserves_nulls`
  expected: fails before step 2, passes after

Acceptance:
- `cargo test -p camel-component-redis commands::geo` exits 0
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0

- [x] task-4

## Task 5 — GEOSEARCH handler (radius and box)

Files:
- crates/components/camel-redis/src/commands/geo.rs (modified)

Steps:
1. `fn validate_positive(v: f64, header: &'static str) -> Result<(), CamelError>`
   (defined here, first use): finite and `> 0.0`, else
   `ProcessorError` naming the header.
2. `fn resolve_geo_search_shape(exchange) -> Result<GeoSearchShape, CamelError>`:
   reads optional `CamelRedis.Radius` (`get_f64_header`), optional
   `CamelRedis.Width` and `CamelRedis.Height` (same helper). Returns
   `GeoSearchShape::Radius(f64)` or `GeoSearchShape::Box { width: f64,
   height: f64 }`. Radius present AND (width or height present) fails
   with `ProcessorError` stating radius and box are mutually exclusive.
   Neither present fails with `ProcessorError` stating a radius or a box
   is required. Positive values validated via `validate_positive`
   (`CamelRedis.Radius` / `CamelRedis.Width` / `CamelRedis.Height`).
3. `async fn execute_geosearch(exchange, conn)`:
   `require_key`; center longitude/latitude via `require_f64_header` +
   validators; shape via step 1; unit via `resolve_geo_unit`;
   `with_dist` = `get_bool_header(exchange, "CamelRedis.WithDist").unwrap_or(false)`,
   `with_coord` same for `CamelRedis.WithCoord`; optional `count` via
   `get_i64_header(exchange, "CamelRedis.Count")` appended as
   `COUNT n` when present and positive (else ignored).
   Build `redis::cmd("GEOSEARCH").arg(&key).arg("FROMLONLAT")
   .arg(lon).arg(lat)` then `"BYRADIUS".arg(radius)` or
   `"BYBOX".arg(width).arg(height)`, then `.arg(unit)`, then `WITHDIST`
   and/or `WITHCOORD` when requested, then `COUNT n`.
4. Reply mapping:
   - no extras: query as `Vec<String>`, body `json!(members)` (plain
     strings).
   - with extras: query the reply ONCE as `Vec<Vec<redis::Value>>` —
     Redis returns 2-element rows (`[member, dist]` or
     `[member, [lon, lat]]`) or 3-element rows depending on the extra
     combination, so a fixed redis-rs tuple type would fail on arity
     mismatch. Map through pure helper
     `fn json_from_geo_rows(rows: Vec<Vec<redis::Value>>, with_dist: bool, with_coord: bool) -> Result<serde_json::Value, CamelError>`
     where each row object carries `"member"` plus, when the
     corresponding flag is set, `"distance"` (number, parsed from the
     row element) and/or `"longitude"`/`"latitude"` (numbers, parsed
     from the 2-element bulk array; coordinates rounded to 6 decimals
     via `(x * 1e6).round() / 1e6`). Rows whose elements do not match
     the flags fail with `ProcessorError` naming `GEOSEARCH`.
5. Wire the arm in `dispatch`.

Tests (inline in geo.rs):
- name: `test_validate_positive_rejects_zero_and_nan`
  setup: nothing
  action: `validate_positive(0.0, "CamelRedis.Radius")`,
  `validate_positive(f64::NAN, "CamelRedis.Radius")`,
  `validate_positive(0.1, "CamelRedis.Radius")`
  assert: first two `Err` containing `CamelRedis.Radius`; last `Ok`
  command: `cargo test -p camel-component-redis test_validate_positive_rejects_zero_and_nan`
  expected: fails before step 1, passes after
- name: `test_geo_search_shape_rejects_radius_and_box_together`
  setup: exchange with `CamelRedis.Radius` `100` and `CamelRedis.Width`
  `50` and `CamelRedis.Height` `50`
  action: `resolve_geo_search_shape`
  assert: `Err` message contains `mutually exclusive`
  command: `cargo test -p camel-component-redis test_geo_search_shape_rejects_radius_and_box_together`
  expected: fails before step 2, passes after
- name: `test_geo_search_shape_rejects_neither`
  setup: exchange with valid center only
  action: `resolve_geo_search_shape`
  assert: `Err` message contains `radius or a box`
  command: `cargo test -p camel-component-redis test_geo_search_shape_rejects_neither`
  expected: fails before, passes after
- name: `test_geo_search_shape_rejects_non_positive_radius`
  setup: exchange with `CamelRedis.Radius` `0`
  action: `resolve_geo_search_shape`
  assert: `Err` containing `CamelRedis.Radius`
  command: `cargo test -p camel-component-redis test_geo_search_shape_rejects_non_positive_radius`
  expected: fails before, passes after
- name: `test_geo_search_shape_rejects_non_positive_height`
  setup: exchange with `CamelRedis.Width` `50`, `CamelRedis.Height` `-1`
  action: `resolve_geo_search_shape`
  assert: `Err` containing `CamelRedis.Height`
  command: `cargo test -p camel-component-redis test_geo_search_shape_rejects_non_positive_height`
  expected: fails before, passes after
- name: `test_json_from_geo_rows_member_dist_coord`
  setup: raw rows built as
  `vec![vec![redis::Value::BulkString("Palermo".into()), redis::Value::BulkString("123.456".into()), redis::Value::BulkArray(vec![redis::Value::BulkString("13.3613893389".into()), redis::Value::BulkString("38.1155563955".into())])]]`
  with both flags true; a second call with one single-element row
  `[BulkString("Catania")]` and both flags false
  action: `json_from_geo_rows`
  assert: first yields
  `json!({"member":"Palermo","distance":123.456,"longitude":13.361389,"latitude":38.115556})`
  (6-decimal rounding); second yields `json!([{"member":"Catania"}])`;
  no field is a quoted-JSON string
  command: `cargo test -p camel-component-redis test_json_from_geo_rows_member_dist_coord`
  expected: fails before step 4, passes after
- name: `test_json_from_geo_rows_dist_only_two_element_rows`
  setup: raw rows
  `vec![vec![BulkString("Palermo"), BulkString("190.44")]]` with
  `with_dist` true, `with_coord` false
  action: `json_from_geo_rows`
  assert: yields `json!([{"member":"Palermo","distance":190.44}])` —
  the 2-element WITHDIST-only reply shape parses without error
  command: `cargo test -p camel-component-redis test_json_from_geo_rows_dist_only_two_element_rows`
  expected: fails before step 4, passes after

Acceptance:
- `cargo test -p camel-component-redis commands::geo` exits 0
- `cargo clippy -p camel-component-redis --all-targets -- -D warnings` exits 0

- [x] task-5

## Task 6 — Repository geo surface

Files:
- crates/services/camel-redis-repo/src/cache_repo.rs (modified)
- crates/services/camel-redis-repo/src/lib.rs (modified)

Steps:
1. In cache_repo.rs add public items (doc comments in the file's style,
   citing the fail-early rule):
   - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum GeoUnit {
     Meters, Kilometers, Miles, Feet }` with
     `pub fn as_str(&self) -> &'static str` returning `"m"`, `"km"`,
     `"mi"`, `"ft"`.
   - `#[derive(Debug, Clone, PartialEq)] pub struct GeoSearchRow {
     pub member: String, pub distance: f64 }`.
   - Private `fn validate_geo_point(longitude: f64, latitude: f64) ->
     Result<(), CamelError>` (same ranges as the component; messages
     name the argument: `longitude` / `latitude`), and
     `fn validate_geo_extent(v: f64, name: &str)` (`> 0`, finite).
   - `pub async fn geo_add(&self, key: &str, longitude: f64, latitude:
     f64, member: &str) -> Result<i64, CamelError>`: validate, build
     `redis::Cmd` `GEOADD` against the namespaced key — call the
     `namespaced` helper exactly as `set_entry` builds its key
     argument — then execute through the same executor seam +
     `execute_retry_safe` pattern as `set_entry`, mapping errors with
     the crate's `to_camel_error`/transport mapping used by sibling
     methods.
   - `pub async fn geo_search_radius(&self, key: &str, longitude: f64,
     latitude: f64, radius: f64, unit: GeoUnit) -> Result<Vec<GeoSearchRow>, CamelError>`:
     validate point and radius; `GEOSEARCH key FROMLONLAT lon lat
     BYRADIUS r unit WITHDIST`; parse reply as
     `Vec<(String, f64)>` into rows.
   - `pub async fn geo_search_box(&self, key: &str, longitude: f64,
     latitude: f64, width: f64, height: f64, unit: GeoUnit) ->
     Result<Vec<GeoSearchRow>, CamelError>`: same with `BYBOX width
     height unit WITHDIST`.
2. Re-export `GeoUnit` and `GeoSearchRow` from lib.rs
   (`pub use cache_repo::{GeoSearchRow, GeoUnit};`) next to the
   `RedisCacheRepository` re-export.
3. These are inherent methods; do NOT touch `CacheRepository` in
   camel-api.

Tests (inline `mod tests` in cache_repo.rs):
- name: `test_geo_unit_as_str`
  setup: nothing
  action: `GeoUnit::Meters.as_str()` etc. for all four
  assert: `"m"`, `"km"`, `"mi"`, `"ft"`
  command: `cargo test -p camel-redis-repo test_geo_unit_as_str`
  expected: fails before step 1, passes after
- name: `test_geo_add_rejects_bad_latitude`
  setup: repository instance built the same way existing cache_repo
  tests build one (follow the existing inline test constructor; no
  connection needed because validation runs first)
  action: `repo.geo_add("sicily", 13.0, 95.0, "Palermo").await`
  assert: `Err(CamelError::ProcessorError(_))` whose message contains
  `latitude`; no I/O attempted
  command: `cargo test -p camel-redis-repo test_geo_add_rejects_bad_latitude`
  expected: fails before, passes after
- name: `test_geo_search_radius_rejects_bad_radius`
  setup: repository instance as above
  action: `repo.geo_search_radius("sicily", 13.0, 38.0, 0.0,
  GeoUnit::Kilometers).await`
  assert: `Err(ProcessorError)` containing `radius`
  command: `cargo test -p camel-redis-repo test_geo_search_radius_rejects_bad_radius`
  expected: fails before, passes after
- name: `test_geo_search_box_rejects_bad_width`
  setup: repository instance as above
  action: `repo.geo_search_box("sicily", 13.0, 38.0, -5.0, 400.0,
  GeoUnit::Kilometers).await`
  assert: `Err(ProcessorError)` containing `width`
  command: `cargo test -p camel-redis-repo test_geo_search_box_rejects_bad_width`
  expected: fails before, passes after
- name: `test_geo_add_targets_namespaced_key`
  setup: repository wired to the crate's `FakeRepoExecutor` exactly as
  the existing `set_entry`/invalidate tests wire it (follow the
  `cmd_args(&fake.commands()[0])` assertion pattern already in this
  file's tests)
  action: `repo.geo_add("sicily", 13.361389, 38.115556, "Palermo").await`
  assert: the fake recorded one command; its args are `GEOADD`,
  the namespaced key (`camel:cache:<name>:sicily` shape — match how
  sibling tests assert the `set` key), `13.361389`, `38.115556`,
  `Palermo`
  command: `cargo test -p camel-redis-repo test_geo_add_targets_namespaced_key`
  expected: fails before step 1, passes after
- name: `test_geo_search_radius_builds_geosearch_withdist`
  setup: repository wired to `FakeRepoExecutor` as above
  action: `repo.geo_search_radius("sicily", 15.0, 37.0, 200.0,
  GeoUnit::Kilometers).await`
  assert: the recorded command args contain `GEOSEARCH`, the namespaced
  key, `FROMLONLAT`, `15`, `37`, `BYRADIUS`, `200`, `km`, `WITHDIST`
  (order as built); the call returns rows parsed from the fake's reply
  only if the fake supplies one — otherwise assert on the recorded
  command and the propagated parse error per how sibling tests handle
  fake replies
  command: `cargo test -p camel-redis-repo test_geo_search_radius_builds_geosearch_withdist`
  expected: fails before step 1, passes after

Acceptance:
- `cargo test -p camel-redis-repo` exits 0
- `cargo clippy -p camel-redis-repo --all-targets -- -D warnings` exits 0

- [x] task-6

## Task 7 — Component GEO integration tests (TestContainers)

Files:
- crates/camel-test/tests/redis_test.rs (modified)

Steps:
1. Following the existing suite conventions (`use
   support::redis::shared_redis;`, `#[tokio::test]`, producer exchanges
   with `CamelRedis.*` headers exactly as `redis_string_commands` and
   `redis_hash_commands` do), add:
   - `async fn redis_geo_commands()`: gets `shared_redis()`; GEOADD
     Palermo `(13.361389, 38.115556)` asserting body `1`; GEOADD
     Catania `(15.087269, 37.502669)` asserting `1`; GEOADD Palermo
     again with a new coordinate asserting `0`; GEOADD far-away member
     `(0.0, 0.0)` named `Farpoint` asserting `1`; GEOPOS both members
     asserting pairs within `1e-4`; GEOPOS with an absent member
     asserting its entry is null; GEOPOS on an absent key asserting an
     array of null entries; GEODIST Palermo-Catania `km`
     asserting `> 100.0`; GEODIST with absent member asserting null;
     GEODIST on an absent key asserting null; GEODIST without unit
     asserting `> 100000.0` (meters); GEOHASH both members asserting
     non-empty strings and null for absent member; GEOSEARCH center
     `(15.0, 37.0)` radius `200` km asserting body is an array of plain
     strings containing both members and no objects; GEOSEARCH with
     `CamelRedis.WithDist` asserting objects with numeric `distance` in
     km; with `CamelRedis.WithCoord` asserting `longitude`/`latitude`
     within `1e-4`; GEOSEARCH BYBOX width/height `400` km asserting
     both Palermo and Catania present and `Farpoint` absent; GEOSEARCH
     radius `200` km on a fresh absent key asserting empty array.
2. Header names and exchange construction must copy the exact pattern
   the neighboring tests use for `CamelRedis.Key` and friends.

Tests:
- name: `redis_geo_commands`
  setup: Docker running; `shared_redis()` container
  action: the sequence above through the redis producer
  assert: each step's body assertion as listed
  command: `cargo test -p camel-test --features integration-tests --test redis_test redis_geo -- --test-threads=1`
  expected: fails before Tasks 3-5, passes after

Acceptance:
- The command above exits 0
- `cargo test -p camel-component-redis` still exits 0 (no regressions)

- [x] task-7

## Task 8 — Repository GEO integration tests (TestContainers)

Files:
- crates/camel-test/tests/redis_repositories_test.rs (modified)

Steps:
1. Following the existing conventions in that file (`own_redis()`,
   `raw_connection`, how `cache_roundtrip_and_ttl` constructs the
   repository), add `async fn geo_repo_roundtrip()`:
   - build the repo; `geo_add("sicily", 13.361389, 38.115556,
     "Palermo")` asserting `1`; `geo_add("sicily", 15.087269,
     37.502669, "Catania")` asserting `1`
   - `geo_search_radius("sicily", 15.0, 37.0, 400.0, GeoUnit::Kilometers)`
     asserting both members present with `distance > 0`
   - `geo_search_box("sicily", 15.0, 37.0, 400.0, 400.0,
     GeoUnit::Kilometers)` asserting both members present
   - `geo_search_radius` on absent key `nowhere` asserting empty vec
2. Add `async fn geo_repo_namespace_isolation()`: two repositories with
   different cache names against the same `own_redis()` URL; one stores
   a member under `sicily`; the other's `geo_search_radius` on `sicily`
   returns empty.

Tests:
- name: `geo_repo_roundtrip`
  setup: Docker; `own_redis()`
  action: sequence above
  assert: as listed
  command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test geo_repo -- --test-threads=1`
  expected: fails before Task 6, passes after
- name: `geo_repo_namespace_isolation`
  setup: Docker; `own_redis()`
  action: cross-name search
  assert: empty result for the foreign name
  command: `cargo test -p camel-test --features integration-tests --test redis_repositories_test geo_repo_namespace_isolation -- --test-threads=1`
  expected: fails before Task 6, passes after

Acceptance:
- Both commands above exit 0
- `cargo test -p camel-redis-repo` still exits 0

- [x] task-8

## Task 9 — Docs and example entry

Files:
- docs/src/components/redis.md (modified)
- examples/redis-example/src/main.rs (modified)

Steps:
1. In `docs/src/components/redis.md` under `## Commands`: add rows for
   GEOADD, GEOPOS, GEODIST, GEOSEARCH, GEOHASH to the existing commands
   table/list using the same format as neighboring rows; add a short
   "Geospatial" subsection after the commands table documenting the
   `CamelRedis.*` headers from design D3 and the result shapes
   (plain members vs `{member, distance, longitude, latitude}` rows).
2. In `examples/redis-example/src/main.rs`: add one GEO route in the
   existing route style — a timer-driven producer route to
   `redis://<conn_str>?command=GEOADD` (same connection-string
   interpolation the existing routes use) setting `CamelRedis.Key`,
   `CamelRedis.Longitude`, `CamelRedis.Latitude`, `CamelRedis.Member`
   headers, and a second step issuing GEOSEARCH with
   `CamelRedis.WithDist`, logging the result. Update the file's `//!`
   route list comment. No new dependencies.

Tests:
- name: docs table + example build
  setup: Tasks 3-5 merged
  action: `cargo build -p redis-example`
  assert: exits 0; grep confirms GEOADD/GEOSEARCH present in both files
  command: `cargo build -p redis-example && grep -c GEOADD docs/src/components/redis.md examples/redis-example/src/main.rs`
  expected: fails before step 2 (missing route), passes after

Acceptance:
- Build command exits 0
- `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-redis -p camel-redis-repo --no-deps` exits 0

- [x] task-9
