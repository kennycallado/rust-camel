# Design: http-bridge-emission-correctness

## Approach

Introduce a single header-classification module in `crates/components/camel-http` that classifies policy-relevant HTTP headers into one of three RFC-derived buckets and is consumed by both the producer (outbound request) and the consumer (response emission). Headers outside these buckets remain unclassified and pass unless a direction-specific rule excludes them. This removes the two drifted denylists.

**Three buckets:**

1. **Hop-by-hop / framing** (RFC 7230 section 6.1): `connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`, plus `proxy-connection`. Excluded in BOTH directions. Any header named by a `Connection` field value is dynamically excluded as well (RFC 7230 section 6.1).
2. **Request-only**: `host`, `user-agent`, `accept*`, `authorization`, `cookie`, `from`, `expect`, `if-*`, `max-forwards`, `range`, `referer`. Valid on requests only; excluded from responses.
3. **Server-owned**: `date` (RFC 7231 section 7.1.1.2). Excluded from responses (the server owns the authoritative value).

**Direction rules:**

- **Producer (outbound request):** exclude bucket 1 (hop-by-hop/framing + dynamic Connection-named) and `content-length` (re-derived from the body). Re-derive `Host` from the destination URL. Forward bucket 2 (request-only), because an outbound request legitimately carries `User-Agent`, `Accept`, and similar headers.
- **Consumer (response emission):** exclude buckets 1, 2, and 3. Do NOT exclude `cache-control`, `pragma`, `warning`, or `via` (the rc-2jj2 fix); these are valid response headers.

`content-type` stays re-derived at emission (already correct).

**`bridgeEndpoint` (rc-d3o4):** when `bridge_endpoint == true`, `resolve_url` returns the endpoint base URL plus configured query params and ignores exchange `CamelHttpPath`/`CamelHttpQuery`. This matches Apache Camel semantics.

## Affected crates

- `crates/components/camel-http`: new classification module; producer header loop (`lib.rs` ~L2107-2121); consumer reply-finalizer (`lib.rs` ~L1587-1632); `resolve_url` (`lib.rs` ~L1970-2027); tests.
- `docs/adr/`: new ADR-0057.
- `crates/components/camel-http/CONTEXT.md`: re-cite the header policy to ADR-0057.

## Architecture boundaries

This change stays inside the **Components** layer (HTTP transport). It touches no Runtime, DSL, Processor, or EIP execution surface. `PipelineOutcome`, status, and body semantics remain governed by ADR-0024 and stay untouched. The classification module is a private implementation detail of camel-http, not a public API.

## Phases

### Phase 1: Policy foundation

- **Goal:** establish the RFC-categorised classification module, ADR-0057, and the producer fix.
- **Dependencies:** none (keystone).
- **Externally-visible types/interfaces:** ADR-0057 document; private classification module.
- **Deliverable:** ADR-0057, classification module, producer exclusion, rc-eoft unit tests.
- **Exit-criteria:** a unit test asserts `Host`/`Content-Length`/`Connection` are not forwarded; outbound `Host` is derived from the destination URL; the module exposes tested producer and consumer direction predicates, and the producer references it.

### Phase 2: Response emission

- **Goal:** the consumer stops stripping valid response headers; CONTEXT.md is corrected.
- **Dependencies:** Phase 1 (shared module).
- **Deliverable:** rc-2jj2 finalizer change, `Cache-Control`/`Via` regression tests, CONTEXT.md re-cite.
- **Exit-criteria:** `set_header("Cache-Control", ...)` and `set_header("Via", ...)` emit on the response; producer and consumer reference the shared module.

### Phase 3: Bridge URL semantics

- **Goal:** `bridgeEndpoint=true` gates `resolve_url`.
- **Dependencies:** Phase 1. The semantics decision is escalated to e_gpt before this phase starts.
- **Deliverable:** rc-d3o4 `resolve_url` change plus tests.
- **Exit-criteria:** `bridgeEndpoint=true` ignores exchange `CamelHttpPath`/`CamelHttpQuery`.

### Phase 4: Composed acceptance

- **Goal:** the end-to-end bridge integration test gates epic close.
- **Dependencies:** Phases 1-3.
- **Deliverable:** rc-f0cn local deterministic `http:` -> `http:` test validating the outbound Host.
- **Exit-criteria:** a composed proxy forwards a request whose outbound Host matches the destination and emits route-set response headers.

## Alternatives considered

- **One universal denylist (rejected):** the producer and the consumer need direction-specific predicates; a single flat list re-introduces drift.
- **Rename `bridgeEndpoint` (rc-d3o4 option 2, rejected):** keeps the name honest but breaks Apache-Camel user expectations. Option 1 (implement the semantics) is chosen, escalated to e_gpt before Phase 3.
- **Namespace request-derived headers on intake (rejected):** large blast radius across every server component; deferred to backlog rc-t6eq (P4).
- **Amend ADR-0024 (rejected):** it never decided headers. A new ADR-0057 keeps the status/body/Stop contract clean.
