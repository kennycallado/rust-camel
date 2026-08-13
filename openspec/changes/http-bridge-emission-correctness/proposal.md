# Proposal: http-bridge-emission-correctness

## Why

The HTTP component (`crates/components/camel-http`) emits headers and builds the outbound URL incorrectly for the bridge/proxy pattern (`http:` -> `http:`). Three demo-found defects break real proxies:

1. **Producer forwards `Host` and hop-by-hop headers** (rc-eoft, P0). The outbound request carries the incoming `Host` (for example `localhost`) instead of the destination host. Any CDN or vhost that routes by Host returns 403/404 with no hint that Host is the cause. The producer has no default exclusion list.
2. **Consumer strips valid response headers** (rc-2jj2, P1). The reply-finalizer drops `Cache-Control`, `Pragma`, `Warning`, and `Via` unconditionally, so a route that sets them can never emit them on the wire. No YAML workaround exists.
3. **`bridgeEndpoint` does not gate URL building** (rc-d3o4, P1). The flag only gates header injection; `resolve_url` always merges `CamelHttpPath`/`CamelHttpQuery`, so the flag name promises behaviour it does not provide.

Root cause: the producer and the consumer maintain two separate, drifted denylists with no RFC backing. The fix establishes one RFC-categorised classification module and a new ADR-0057, shared by both directions.

Bd: rc-vy6w (epic), rc-eoft, rc-2jj2, rc-d3o4, rc-f0cn.

## What Changes

**Included**

- A shared RFC-categorised header-classification module (three buckets: hop-by-hop/framing, request-only, server-owned) with dynamic `Connection`-named stripping (RFC 7230 section 6.1), used by both producer and consumer.
- ADR-0057 ("HTTP header emission policy").
- Producer outbound exclusion of hop-by-hop/framing plus Connection-named headers; outbound `Host` derived from the destination URL (rc-eoft).
- Consumer reply-finalizer stops stripping `Cache-Control`/`Pragma`/`Warning`/`Via`; `CONTEXT.md` re-cited to ADR-0057 (rc-2jj2).
- `bridgeEndpoint=true` gates `resolve_url` to use the endpoint URI as-is (rc-d3o4).
- A local, deterministic `http:` -> `http:` bridge integration test that validates the outbound Host (rc-f0cn).

**Excluded**

- The `remove_header` DSL verb (rc-dey8) -> separate change `dsl-remove-header`.
- The two docs tickets (rc-f2cj, rc-xwrx) -> separate `/trivial` change after code stabilises.
- Outcome-aware Segment composition (rc-65fs: rc-20yn, rc-n8rc, rc-65yi) -> lives in camel-processor, a different epic.

## Acceptance criteria

- An exchange carrying `Host`/`Content-Length`/`Connection` does NOT forward them outbound; the outbound `Host` matches the destination URL.
- A route with `set_header("Cache-Control", ...)` and `set_header("Via", ...)` emits both on the response.
- `bridgeEndpoint=true` ignores exchange `CamelHttpPath`/`CamelHttpQuery`.
- Producer and consumer use ONE classification module.
- ADR-0057 authored; `CONTEXT.md` corrected.
- The composed bridge integration test passes.

## Risk budget

Header filtering runs on the request/response hot path. The risk is over-stripping (dropping a header a route needs) or under-stripping (leaking a hop-by-hop header). The shared RFC buckets and tests in both directions mitigate this. No change to body, status, or `PipelineOutcome` (ADR-0024 stays untouched).

`bridgeEndpoint` is a public behaviour change. The Camel-compatible semantics (option 1) is chosen and escalated to e_gpt before Phase 3.
