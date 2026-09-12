# Design: jwks-refresh-guard

## Approach

All changes in `crates/services/camel-auth/src/jwks.rs` (`RemoteJwksProvider`);
`jwt.rs` caller logic and the `JwksProvider` trait stay unchanged.

`refresh()` (the forced path invoked on unknown-kid cache miss) becomes:

1. Acquire the existing `in_flight` mutex (shared with `get_signing_keys`
   slow path — concurrent misses coalesce on the same mechanism).
2. Post-lock re-check, skip the outbound fetch when EITHER:
   - the cached keys were fetched within the cooldown window (another task —
     TTL slow path or forced path — refreshed recently), or
   - a forced attempt STARTED within the cooldown window.
3. Record the forced-attempt start timestamp BEFORE awaiting HTTP: a dropped
   future (cancellation) still consumes the interval, so cancellation cannot
   drive amplification. Timestamp lives in state guarded by `in_flight`; no
   separate lock is held across network I/O.
4. Otherwise `fetch_and_store()` under the guard (holding `in_flight` during
   the fetch is the pre-existing slow-path behavior; fresh-cache readers stay
   lock-free).

Cooldown: `const FORCED_REFRESH_COOLDOWN: Duration = 5 s` (max 0.2 forced
fetches/s/provider vs. the 60 s minimum TTL clamp). Test seam: a
`#[cfg(test)]`/`pub(crate)` constructor field override (small value, e.g.
100 ms) — no production config surface. Rotation recovery bound, documented at
the cooldown const and in the delta spec: after the interval elapses, the next
unknown-kid request is refresh-eligible; total recovery also depends on request
arrival and fetch latency (the spec words this as refresh-ELIGIBLE, never
"key usable within N seconds").

Tests (wiremock mock JWKS endpoint, `#[cfg(test)]` provider constructor that
bypasses SSRF loopback rejection; test cooldown ≈100 ms, TTL fresh throughout):

- `forced_refresh_coalesces_concurrent_unknown_kids`: prime cache; age it past
  the test cooldown while still TTL-fresh; delayed mock response (barrier) so
  32 concurrent invalid tokens with distinct unknown kids overlap inside one
  attempt; assert EXACTLY 2 outbound GETs total (prime + 1 forced) and that all
  32 tokens fail with `TokenInvalid`.
- `failed_forced_refresh_honors_cooldown`: prime successfully, then mock 500s;
  first forced attempt fails (outbound GET observed); second forced attempt
  within cooldown performs no outbound GET and still returns the stale-cache
  error path.
- `rotated_key_refresh_eligible_after_cooldown`: prime with kid1; consume one
  forced attempt; mock switches to kid2; wait out cooldown; a correctly signed
  kid2 token validates (requires one forced fetch + successful verify).
- `cancelled_forced_refresh_consumes_cooldown`: prime cache; age past the test
  cooldown while TTL-fresh; start a forced refresh (mock response gated on a
  barrier) and DROP the future while the outbound request is in flight; then a
  follow-up unknown-kid token within the cooldown starts no new outbound
  request — total GETs stay at exactly 2 (prime + the cancelled forced fetch).
- Existing SSRF/TTL tests unchanged.

## Affected crates

- `camel-auth` (crates/services): `src/jwks.rs` — forced-refresh single-flight
  + cooldown state + tests. No other crate. No public API change.

## Architecture boundaries

Services layer only, inside the ADR-0061 auth kernel's JWT validator provider.
The kernel wiring (`security_boot.rs`), transports, and the data/control plane
split are untouched; the observable contract is purely the provider's outbound
fetch pattern. Relevant ADRs: ADR-0061 (unified transport authentication
kernel — validator internals are kernel-internal).

## Alternatives considered

- Bounded negative cache of unknown kids: rejected — cooldown already bounds
  outbound fetches independent of kid cardinality (attackers vary kids; a
  per-kid cache cannot tighten the aggregate bound and adds eviction state).
- Per-kid or token-bucket rate limiter: rejected — per-kid limits are defeated
  by kid cardinality; a global token bucket duplicates what the interval
  cooldown expresses more simply.
- Queueing/refresh scheduler: overkill for a P2 hardening; no evidence of need.

Single-phase change (one coherent slice; no milestone split needed).
