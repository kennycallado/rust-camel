# ADR-0059: Auth Extraction Path Divergence

**Date:** 2026-08-15
**Status:** Accepted
**References:** ADR-0010, ADR-0032, ADR-0033, ADR-0051
**Bd:** rc-7x1z

## Context

A route credential can arrive by more than one channel: an `Authorization`
header, a query parameter, a cookie, or a named custom header. Two code
paths validate that credential, and they diverge.

The HTTP layer owns the first path. `SecurityPolicyLayer` runs before the
pipeline (ADR-0010). It calls `authenticate()` inside `policy.evaluate`.
Extraction reads the route-declared `credential_sources` from the Exchange
input. The policy owns extraction.

Components own the second path. WS and gRPC pre-authenticate the request. They
extract the credential before `policy.evaluate` runs. They store the resulting
principal in the Exchange property `camel.auth.principal`. The policy then
falls back to that preloaded principal via `trust_upstream_principal`. The
component owns extraction.

Before this change, the two paths used different extraction code. HTTP used a
hardcoded `Authorization` prefix strip. WS used `extract_token_multi`. gRPC
still uses its own hardcoded prefix strip. The split caused two defects:

- A route could not declare a cookie or query source on the HTTP path, because
  the layer never reached the multi-source extractor.
- The preloaded-principal branch carried a serialization split: the writer
  stored a JSON string, but the reader expected a JSON object. Every
  `trust_upstream_principal` grant on the component path returned 500.

## Decision

Extraction is standardized behind `extract_token_multi`. Every source — the
`Authorization` header, a query parameter, a cookie, and a named custom
header — flows through the same extraction and the same constant-time store
lookup. A route declares its
sources in `credential_sources`, in order. First match wins.

The divergence stays. The HTTP layer calls extraction inside `authenticate()`.
Components call extraction before `policy.evaluate`. A WS `roles`/`scopes`
route requires explicit `trust_upstream_principal: true`. The component gates
evaluation on successful authentication, so the spoof caveat from ADR-0010 does
not apply on that path.

The preloaded-principal branch reads and writes one serialization format. The
canonical `store_principal_properties` / `principal_from_exchange` pair stores
the principal as a JSON string in `camel.auth.principal`. The trust branch now
delegates to `principal_from_exchange` (tracked in bd rc-7x1z), so the
read matches the write.

## Consequences

Unifying HTTP onto `extract_bearer_token` widened the default-path acceptance.
The auth scheme is now case-insensitive and whitespace-tolerant per RFC 9110
(RFC 7235). A lowercase or uppercase `bearer` scheme, a leading space, or a
double space now authenticate when the store holds the trimmed token.
Previously rejected malformed credentials now either authenticate (if the store
accepts the token) or are rejected through the normal path. No previously
granted route changes outcome. Fail-closed is preserved. WS behavior is
unchanged — it already used this extraction.

An empty token after the scheme (`Bearer ` with nothing following) is treated
as an absent source: with `trust_upstream_principal=false` the request stays
unauthenticated; with `trust=true` it can grant through a preloaded
principal — the same outcome as a request that carries no Authorization header
at all.

The preloaded-principal fix is load-bearing. The store/read pair was
historically split across two serialization formats (`store_principal_properties`
wrote a JSON string; `extract_principal_from_exchange` read a JSON object).
Every trust grant on the component path returned 500 as a result. The split was
latent since the WS preloaded-principal path landed. This change unifies both
sides on the canonical string format. The WS `roles`/`scopes` + `trust`
combination now works; before the fix it always failed.

A deferred Phase-1 note: header values rejected by `http::HeaderValue::from_str`
(non-ASCII bytes, control characters) are treated as absent sources, never
fatal (ADR-0032). On the layer path with `trust=true` and a preloaded principal
present, such a value flips an error to a grant — the same outcome as a request
without that header.

First-match-wins precedence is deterministic: the first declared source with a
present value wins, even when a later source holds a valid token.

gRPC is a fast-follow. Its `extract_principal` still hardcodes the
`Authorization` header and the Bearer prefix (`camel-component-grpc/src/server.rs`).
It does not read `credential_sources` yet.

`ref`, `wasm`, and `permission` policy variants carry no `credential_sources`.
They are rejected when the key is present because a registry-resolved
`Arc<dyn SecurityPolicy>` and the WASM/permission evaluators carry no
authentication-capability metadata, and adding that metadata would violate the
no-new-abstraction constraint.

## Alternatives considered

- Keep the two extraction code paths separate. Rejected. The split hid the
  serialization defect and blocked cookie/query sources on HTTP.
- Resolve all sources at the component layer only. Rejected. HTTP's layer path
  needs the policy to carry sources; component-internal resolution is not a DSL
  surface.
- Rewire `trust_upstream_principal` into a verified-principal channel. Deferred.
  Out of scope for this change.
