# ADR-0057: HTTP Header Emission Policy

**Date:** 2026-08-13
**Status:** Accepted
**References:** ADR-0001, ADR-0024, ADR-0046, RFC 7230, RFC 7231

## Decision

### Decision 1: Three RFC-derived header buckets

The HTTP component classifies every header name into one of three buckets
before it emits a request or response. The buckets come from RFC 7230 and
RFC 7231.

**Hop-by-hop / framing.** The component uses this compatibility hop-by-hop
set (drawn from RFC 2616 section 13.5.1 conventions and the per-section
definitions in RFC 7230). RFC 7230 section 6.1 additionally requires
removal of `Connection` and every header named by its connection-options.
A proxy must not forward these headers to the next hop:

`connection`, `keep-alive`, `proxy-authenticate`,
`proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`,
`proxy-connection`.

`proxy-connection` is not in the RFC list. Apache Camel and common proxy
implementations treat it as a synonym for `connection`. This ADR adopts
that convention to avoid leaking the field to the next hop.

**Request-only.** These headers have meaning only on client requests. A
proxy must not echo them back in a response:

`host`, `user-agent`, `accept`, `accept-encoding`, `accept-language`,
`accept-charset`, `accept-datetime`, `authorization`, `cookie`, `expect`,
`from`, `if-match`, `if-modified-since`, `if-none-match`, `if-range`,
`if-unmodified-since`, `max-forwards`, `range`, `referer`.

`proxy-authorization` is absent from this list. RFC 2616 section 13.5.1
places it in the compatibility hop-by-hop set. It appears only there.

**Server-owned.** RFC 7231 section 7.1.1.2 specifies that the origin
server sets the `date` field. A proxy must not copy a client-supplied
`date` into a response:

`date`.

### Decision 2: Dynamic Connection-named stripping

RFC 7230 section 6.1 states that a `Connection` header field can name
additional headers that are hop-by-hop for that connection. The HTTP
component reads every `Connection` field value and treats each named
token as hop-by-hop in both directions.

The parsing rules are:

1. Split each `Connection` value on `,`.
2. Trim whitespace from each segment.
3. Lowercase the segment for comparison.
4. Keep only segments that are valid RFC 7230 `token`s. A `token` is one
   or more `tchar` characters. A `tchar` is an ASCII alphanumeric
   character or one of `! # $ % & ' * + - . ^ _` ` `| ~`.
5. Drop empty or malformed segments. The parser never panics.
6. De-duplicate the result, preserving first-seen order.

This makes the hop-by-hop set dynamic. A header that the static list in
Decision 1 does not name becomes hop-by-hop when a `Connection` field
names it.

### Decision 3: Producer outbound direction rule

The producer builds the outbound request from the exchange headers. It
excludes a header when any of these conditions hold:

- The header name is hop-by-hop / framing (Decision 1, static list).
- The header name is `content-length`. The HTTP client re-derives this
  field from the request body.
- The header name is `host`. The HTTP client derives this field from the
  destination URL.
- The header name appears in the dynamic Connection-named set (Decision
  2).

The producer forwards request-only headers. A bridging proxy must pass
`accept`, `authorization`, and similar fields to the destination.

### Decision 4: Consumer response direction rule

The consumer builds the outbound HTTP response from the exchange reply
headers. It excludes a header when any of these conditions hold:

- The header name is hop-by-hop / framing (Decision 1, static list).
- The header name is request-only (Decision 1, static list).
- The header name is server-owned (Decision 1, static list).
- The header name is `content-length`. The HTTP server re-derives this
  field from the response body.
- The header name is `content-type`. The HTTP server re-derives this
  field from the inferred or user-supplied content type.
- The header name appears in the dynamic Connection-named set (Decision
  2).

The consumer does **not** exclude `cache-control`, `pragma`, `warning`,
or `via`. These are valid response headers. RFC 7231 and RFC 7234 define
them for server-to-client communication. A bridging proxy must pass them
through.

### Decision 5: ADR-0024 is not amended

ADR-0024 defines the `PipelineOutcome` contract. It covers HTTP status,
body, and the `Stop` signal. This ADR does not change that scope.

ADR-0057 governs header emission only. The two ADRs are complementary
and do not overlap.

## Rejected alternatives

### Strip all non-standard headers

A policy that allows only a fixed allowlist of standard headers would be
safe but rigid. Custom headers (`X-Request-Id`, `X-Correlation-Id`,
vendor-specific fields) carry business value in proxy and integration
scenarios. The three-bucket denylist approach removes the fields that
break HTTP semantics and lets all other headers pass.

### Forward hop-by-hop headers unconditionally

Some proxies forward `connection` and `keep-alive` without harm in
HTTP/1.1 keep-alive pools. This is incorrect per RFC 7230 section 6.1
and breaks when the next hop uses a different transport (HTTP/2, Unix
sockets). The denylist is the standards-compliant choice.

### Static-only Connection handling (ignore dynamic tokens)

Ignoring the `Connection` field's token list would simplify the
implementation. It would also leak connection-specific headers that the
client or server intended to keep local. RFC 7230 section 6.1 makes
dynamic stripping mandatory. This ADR follows the RFC.

### Exclude cache-control and via from responses

An earlier draft of the consumer reply logic excluded `cache-control`,
`pragma`, `warning`, and `via`. These are valid response headers defined
by RFC 7231 and RFC 7234. Stripping them breaks caching directives and
proxy traceability. This ADR explicitly preserves them.

## Context

### Problem

Before this ADR, the HTTP component had no documented header emission
policy. The producer forwarded the exchange `host` header to the
destination. The consumer stripped valid response headers such as
`cache-control` and `via`. Both behaviours violated RFC 7230 and RFC
7231.

The issues surfaced during a bridge-proxy correctness review (epic
rc-vy6w). Four bd issues track the defects:

- rc-eoft: the producer forwards `host` and hop-by-hop headers outbound.
- rc-2jj2: the consumer strips valid response headers (`cache-control`,
  `pragma`, `warning`, `via`).
- rc-d3o4: the `bridgeEndpoint` flag does not gate URL resolution.
- rc-f0cn: no end-to-end test verifies the bridge header contract.

### Forces

- **RFC compliance.** RFC 7230 section 6.1 and RFC 7231 section 7.1.1.2
  define mandatory proxy behaviour for hop-by-hop and server-owned
  headers. The policy must follow these standards.
- **Bridge-proxy correctness.** A bridge proxy forwards requests to a
  destination and returns responses to the caller. It must not leak
  client-side or server-side headers into the wrong direction.
- **Apache Camel as inspiration.** ADR-0046 establishes Apache Camel as
  a reference, not a conformance authority. The header lists follow the
  RFCs first and align with Apache Camel where the RFCs permit
  interpretation (e.g. `proxy-connection`).
- **Determinism.** The policy must be testable without network access.
  Header classification is a pure function. The dynamic Connection
  parser must not panic on malformed input.
- **Non-amendment of ADR-0024.** The `PipelineOutcome` contract is
  stable. Header policy must not alter it.

## Consequences

### Shared classification module

The policy lives in a single classification module
(`crates/components/camel-http/src/header_policy.rs`). The producer and
the consumer both call the same functions. This prevents drift between
the two emission paths.

### Direction-aware exclusion

The producer and consumer use different exclusion predicates. The
producer excludes hop-by-hop, `content-length`, `host`, and
Connection-named headers. The consumer excludes hop-by-hop,
request-only, server-owned, `content-length`, `content-type`, and
Connection-named headers. Both predicates share the same Connection
parser.

### Re-derived framing headers

The HTTP client and server re-derive `content-length`, `content-type`,
and `host` from the actual body and destination. The exchange never
supplies these fields to the wire. This prevents mismatched lengths and
stale host values.

### Valid response headers pass through

`cache-control`, `pragma`, `warning`, and `via` reach the HTTP client.
Caching directives and proxy traceability work as the RFCs specify.

### ADR-0024 scope unchanged

This ADR adds header policy. It does not modify the `PipelineOutcome`
contract. Status, body, and `Stop` behaviour remain governed by
ADR-0024.

## Load-bearing citations

| Source | Element |
|---|---|
| RFC 2616 section 13.5.1 | Compatibility hop-by-hop header set (static members) |
| RFC 7230 section 6.1 | Dynamic Connection-named stripping; removal of Connection and connection-option headers |
| RFC 7231 section 7.1.1.2 | Server-owned `date` field |
| RFC 7234 | `cache-control`, `pragma`, `warning`, `via` as valid response headers |
| ADR-0024 | `PipelineOutcome` contract (status, body, Stop) - not amended |
| ADR-0046 | Apache Camel as inspiration, not conformance authority |
| rc-eoft | Producer forwards host and hop-by-hop headers outbound |
| rc-2jj2 | Consumer strips valid response headers |
| rc-d3o4 | bridgeEndpoint does not gate URL resolution |
| rc-f0cn | No end-to-end bridge header contract test |
| rc-vy6w | Epic: bridge-proxy correctness review |
