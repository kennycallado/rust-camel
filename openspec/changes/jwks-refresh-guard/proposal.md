# Proposal: jwks-refresh-guard

## Why

Security audit F2 (docs/SECURITY-report_12-09-26.md, oracle-verified; bd rc-rh3v6):
`LocalJwtValidator::validate_signature` forces a JWKS refresh on any cache-miss
`kid` decoded from the attacker-controlled JWT header — before signature
verification. `RemoteJwksProvider::refresh` calls `fetch_and_store` directly,
bypassing both the fresh-cache fast path and the `in_flight` single-flight mutex
that `get_signing_keys` uses. Syntactically decodable JWTs with distinct unknown
kids therefore each trigger an outbound JWKS GET, concurrently, with fresh cache
and no valid signature — attacker-driven fetch amplification against the IdP.
Not an auth bypass; a DoS-hardening gap.

## What Changes

- Route the forced (unknown-kid) refresh through the same `in_flight`
  single-flight mutex as TTL refresh, with a post-lock re-check.
- Add a bounded per-provider forced-refresh cooldown (default 5 s). Success,
  failure, and cancellation all consume the interval. At most one forced
  outbound fetch START per provider per interval, regardless of request count
  or kid cardinality.
- Document the key-rotation recovery bound: after the interval elapses, the
  next unknown-kid request is refresh-eligible again (cooldown never locks out
  rotation permanently).
- Regression tests with wiremock: concurrent distinct-unknown-kid amplification
  bound, failing-provider cooldown, cancellation consumes cooldown, rotation
  recovery.

Explicitly excluded: negative caching of unknown kids (cooldown alone bounds
outbound fetches irrespective of kid cardinality; a per-kid cache adds state
without tightening the bound), any change to ordinary TTL refresh semantics,
any trait/API surface change, and any change to jwt.rs caller logic.

## Acceptance criteria

- Primed fresh cache + N concurrent invalid tokens with distinct unknown kids →
  outbound JWKS GETs bounded by the cooldown policy (prime + 1 forced fetch),
  not by request count.
- Failing provider (HTTP 500 after successful prime): the first forced refresh
  fails; a further forced refresh within the cooldown performs no outbound fetch.
- A cancelled forced refresh consumes the interval: a follow-up unknown-kid
  token within the cooldown starts no new outbound fetch.
- A newly rotated key (new kid served by the mock) is refresh-eligible after the
  cooldown elapses; a correctly signed token with the new kid then validates.
- Existing `get_signing_keys` TTL behavior unchanged (fast path, single-flight,
  Cache-Control clamping).

## Risk budget

Acceptable: unknown-kid tokens arriving inside the cooldown are rejected
without a refresh attempt (cooldown-imposed delay ≤ cooldown; total recovery
also depends on request arrival and fetch latency). Out of bounds: any
change to successful-token validation, TTL semantics, trait shape, or SSRF
hardening. Affected crate: `camel-auth` (crates/services). bd: rc-rh3v6.
