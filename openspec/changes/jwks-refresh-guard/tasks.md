# Tasks: jwks-refresh-guard

## camel-auth (crates/services/camel-auth)

### Task 1.1: Forced-refresh single-flight + bounded cooldown in RemoteJwksProvider

**Files:**
- `crates/services/camel-auth/src/jwks.rs` (modified)

**Steps:**
1. Add `pub(crate) const FORCED_REFRESH_COOLDOWN: Duration = Duration::from_secs(5);` next to the existing TTL consts, with a doc comment stating the bound: at most one outbound forced JWKS fetch STARTS per provider per interval; success, failure, and cancellation consume it; after the interval the next unknown-kid request is refresh-ELIGIBLE (total rotation recovery also depends on request arrival and fetch latency).
2. Add per-provider forced-refresh state to `RemoteJwksProvider`: `forced_refresh: std::sync::Mutex<Option<Instant>>` (last forced-attempt START). Guard discipline: only read/write it inside the `in_flight` critical section, in short critical sections — NEVER hold it across network I/O.
3. Add a test seam next to the existing `new_for_test`: a `cooldown: Duration` field on `RemoteJwksProvider` (used by `refresh` instead of the const) plus `#[cfg(test)] pub fn new_for_test_with_cooldown(jwks_uri: String, cooldown: Duration) -> Self`; the production `new`/`with_client` path sets the field to `FORCED_REFRESH_COOLDOWN`.
4. Rewrite `JwksProvider::refresh` for `RemoteJwksProvider` to: (a) acquire `self.in_flight` (same single-flight as `get_signing_keys` slow path); (b) post-lock re-check — skip the outbound fetch when the cached keys were fetched within the cooldown window (read `self.cache`; `fetched_at.elapsed() < cooldown`); (c) skip when a forced attempt started within the cooldown window (`forced_refresh` state); (d) record the attempt START timestamp in `forced_refresh` BEFORE awaiting HTTP (a dropped future consumes the interval); (e) otherwise call `self.fetch_and_store()` under the guard. A cooldown skip returns `Ok(())` with a code comment explaining the contract: `refresh()` means "ensure a refresh attempt has started recently", not "fetch succeeded" — callers re-read the cache and report kid-miss as `TokenInvalid`. `fetch_and_store` errors still propagate.
5. Do NOT touch `get_signing_keys`, the `JwksProvider` trait, `fetch_and_store`, or `jwt.rs` logic.
6. Keep existing tests in the module compiling (they use `new_for_test`; no behavior change for them).

**Tests:** (executable spec — name, arrange, act, assert)
- `forced_refresh_skips_when_cache_recently_fetched`: wiremock server (0 requests expected); provider via `new_for_test_with_cooldown(server.uri(), 100ms)`; seed cache manually (existing pattern: `provider.cache.write().await = Some(CachedKeys{ keys: vec![Jwk{kid:"k1",..}], fetched_at: Instant::now(), ttl: 3600s })`) → call `refresh()` → assert `Ok(())` AND wiremock received 0 requests AND `get_signing_keys()` returns the seeded key.
- `forced_refresh_cooldown_bounds_attempts`: provider with 100ms cooldown; seed cache backdated (`fetched_at: Instant::now() - 300ms`, ttl 3600s, still TTL-fresh); wiremock mount 500 response → first `refresh()` → `Err(ProviderUnavailable)` AND mock request count == 1 → second `refresh()` immediately → `Ok(())` (cooldown skip) AND mock request count still == 1.
- `cancelled_forced_refresh_consumes_cooldown`: provider with cooldown 500ms; seed cache backdated 600ms; wiremock mount 200 response with `.delay(400ms)` → spawn `refresh()` on a tokio task, sleep 100ms, `handle.abort()` (drops the future mid-flight) → call `refresh()` again → `Ok(())` AND total wiremock request count == 1 (no third request; the aborted attempt consumed the interval).
- `ttl_expiry_coalesces_concurrent_fetches` (spec scenario: TTL-driven refresh unchanged): provider (cooldown irrelevant here); seed cache with `fetched_at` backdated PAST `ttl` (e.g. ttl 60s, fetched_at now-120s) so the fresh fast path misses; wiremock mount 200 response with `.delay(300ms)` → spawn 8 concurrent `get_signing_keys()` (join_all) → all 8 return the fetched key set AND total wiremock request count == 1 (single-flight coalescing — guards the untouched `get_signing_keys` slow path against this change).
- `command`: `cargo test -p camel-auth --lib jwks` — all new tests pass; before step 4 lands, `forced_refresh_cooldown_bounds_attempts` and `cancelled_forced_refresh_consumes_cooldown` FAIL (current `refresh` always fetches), `forced_refresh_skips_when_cache_recently_fetched` FAILS (current `refresh` ignores cache freshness).
- `expected`: red on the pre-implementation tree for the three cooldown tests (`forced_refresh_skips_when_cache_recently_fetched`, `forced_refresh_cooldown_bounds_attempts`, `cancelled_forced_refresh_consumes_cooldown` — current `refresh` always fetches and ignores cache freshness); `ttl_expiry_coalesces_concurrent_fetches` already passes (untouched slow path) and must stay passing.

**Acceptance:**
- `cargo test -p camel-auth --lib` exits 0 (existing SSRF/TTL tests included).
- `cargo clippy -p camel-auth --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- `cargo xtask lint-unwrap` introduces no new findings in jwks.rs.

- [x] 1.1

### Task 1.2: Validator-level amplification + rotation regressions through validate_signature

**Files:**
- `crates/services/camel-auth/src/jwt.rs` (modified — tests only, inside the existing `#[cfg(test)] mod tests`)

**Steps:**
1. Build the harness inside the existing tests module: a `LocalJwtValidator` whose `jwks` is an `Arc<RemoteJwksProvider>` created by `new_for_test_with_cooldown(server.uri(), 100ms)` (from Task 1.1), the existing `TEST_RSA_PRIVATE_PEM`/`TEST_RSA_PUBLIC_PEM` fixtures, the existing `multi_role_mapper`/claims-mapper helpers, and the existing PEM-in-`n` Jwk convention (mock JWKS bodies carry the public PEM in `n`, per `jwk_to_decoding_key`).
2. Reuse the EXISTING `make_token(kid, claims)` helper in the tests module (jwt.rs ~273) — it already encodes RS256 with a chosen `kid` via `EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_PEM)`; tokens carry an attacker-chosen `kid` and a valid signature (the signature is never reached on kid-miss, so this models unknown-kid attack traffic exactly).
3. Write the two tests below. Prime the provider cache via one real `get_signing_keys()` call against the mock (prime GET), then backdate `fetched_at` past the test cooldown while keeping `ttl` long (TTL-fresh), matching Task 1.1's seeding pattern.

**Tests:** (executable spec — name, arrange, act, assert)
- `validate_signature_concurrent_unknown_kids_bounded`: wiremock serving a kid1 key set (kid "test-key", PEM-in-n, `Cache-Control: max-age=3600`); validator as above; prime + backdate past cooldown; remount/serve with `.delay(300ms)` so concurrent calls overlap; spawn 32 `validate_signature` calls with tokens signed kid `atk-1`..`atk-32` (join_all) → assert ALL 32 return `Err(AuthError::TokenInvalid)` AND total wiremock GET count == 2 (prime + exactly one forced fetch — bounded by policy, not request count).
- `rotated_key_recovery_after_cooldown`: wiremock serving kid "test-key" set; prime; consume one forced attempt (one unknown-kid token → TokenInvalid, mock count 2); mount the rotated set (kid "rotated-key", same PEM-in-n); sleep 150ms (cooldown elapsed); `validate_signature` with a token signed kid `rotated-key` → assert `Ok(principal)` (a new forced fetch started — mock count 3 — kid found, signature verified).
- `command`: `cargo test -p camel-auth --lib validate_signature` — both pass.
- `expected`: tests compile only after Task 1.1 lands (they use `new_for_test_with_cooldown`); green after Task 1.1 — validator-level regression armor pinning the sealed contract (before Task 1.1's guard, the concurrent test would observe unbounded fetches).

**Acceptance:**
- `cargo test -p camel-auth --lib` exits 0.
- `cargo clippy -p camel-auth --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 1.2
