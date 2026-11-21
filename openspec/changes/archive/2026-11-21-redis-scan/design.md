# Design: redis-scan

## Approach

One new command module `commands/scan.rs` in `camel-component-redis` owns the
whole family plus the shared page plumbing; the repository pager reuses that
plumbing instead of keeping its private SCAN build/parse.

**Behavioral probes (done, 2026-11-21, driver redis 1.8.1 pinned):**
- The driver's tuple-vec `FromRedisValue` decoder accepts both the RESP2 flat
  pair array (`as_chunks` by arity) and a RESP3 `Value::Map` (flattened), so
  `Vec<(String, String)>` (HSCAN) and `Vec<(String, f64)>` (ZSCAN) parse from
  the pinned client's default RESP2 wire form with map replies covered.
- The two-element SCAN reply `[BulkString(cursor), Array]` parses as
  `(u64, Vec<T>)` — precedent: `camel-redis-repo/src/executor.rs:158`.
- `TYPE` is buildable as a raw trailing arg (driver supports it; server
  ≥ 6.0 required — server-version failures surface through the existing
  transport error mapping; no client-side version sniffing).

## Contract

- Request headers: `CamelRedis.Cursor` (strict u64; absent/null/empty/0 =
  fresh start; malformed = named `ProcessorError` before send),
  `CamelRedis.Pattern` (MATCH, default `"*"`, always sent),
  `CamelRedis.Count` (COUNT hint, default 100 = the repository pager's
  `PAGE`, always sent, must be ≥ 1), `CamelRedis.Type` (SCAN only).
- Reply (mutated into the exchange like every command): body = the page's
  elements; `CamelRedis.Cursor` = next cursor (`0` on the final page);
  `CamelRedis.ScanComplete=true` set exactly on the final page, absent
  otherwise. One exchange per page, including an empty page with a non-zero
  cursor (iteration continues).
- Wire frame is always `SCAN|SSCAN|HSCAN|ZSCAN <key?> <cursor> MATCH <p>
  COUNT <n>[ TYPE <t>]` — deterministic shape, one branch.
- Body encodings reuse the crate's existing encoders: SCAN → `json!(keys)`
  (KEYS shape), SSCAN → `json_from_members`, HSCAN →
  `HashMap<String,String>` → JSON object (HGETALL shape), ZSCAN →
  `json_from_scored_members` (`[{"member","score"}]`).
- `is_idempotent_command`: the four classify retry-safe reads (a transport
  retry re-issues the same cursor page; SCAN re-runs are converging reads —
  the repository pager already treats SCAN as retry-safe).
- `command=multi` batch entries keep rejecting the family (typed allowlist;
  no code change — pinned by a scenario so nobody "fixes" it later).

## Affected crates

- `camel-component-redis`: `commands/scan.rs` (new), `config.rs` (4 variants
  + classifier), `executor.rs` (routing arm). The shared page primitives
  `encode_scan_page`/`parse_scan_reply` are `pub` with rustdoc (the
  repository crate imports them cross-crate; `lint-missing-docs-ratchet`
  applies — this is the module's whole public surface, nothing else leaks).
- `camel-redis-repo`: `scan_unlink_pattern_where` builds its SCAN command and
  parses its reply through the shared primitives (import path
  `camel_component_redis::commands::...` — the repo crate already depends on
  the component crate, verified `Cargo.toml:17`, so no inversion;
  retry/refresh/UNLINK semantics stay local).
- `camel-test`: live SCAN-family tests in `tests/redis_test.rs` (docker lane).
- `docs`: camel-redis component page section + cursor-loop example.

## Architecture boundaries

Components only: no Runtime/DSL/Services/API changes. The cursor is caller
state — nothing server-side, nothing component-global. `KEYS` untouched
(docs note only). ADR-0098's "no new command variants" was that change's
scope boundary (redis-values), not a standing ban; rc-r3m6x plans this
family (e_opus ranking 2026-11-13). Spec delta only, no ADR (order).

## Alternatives considered

- Four family modules (key/set/hash/zset) each owning its scan command —
  rejected: four copies of cursor resolution and option plumbing; one module
  keeps the contract in one reviewable place.
- Extracting the repo pager's loop into the component crate — rejected: the
  loop is welded to UNLINK batching + `execute_retry_safe` metrics; only the
  build/parse halves are genuinely shared.
- `SCAN` as N exchanges per element — rejected by the order (page = one
  exchange; N-exchange output was explicitly ruled out).
- Client-side `TYPE` on collection variants silently dropped — rejected:
  ADR-0032 fail-closed (named pre-send error).
