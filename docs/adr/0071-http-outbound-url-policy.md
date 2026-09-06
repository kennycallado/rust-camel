# ADR-0071: HTTP Outbound URL Policy (Query Composition and CamelHttpUri Fence)

## Status

Accepted (2026-09-06). Implements the query-composition half of the
http-contract-surface change for the camel-http producer; the capability
spec `openspec/changes/http-contract-surface/specs/http-url-resolution/spec.md`
is normative for the scenario detail. Companion to ADR-0032 (exchange-data
trust boundary) and ADR-0034 (control-bus authorizedRoutes fence precedent).

## Context

Three bd findings name the outbound URL as a contract surface under
ADR-0032, which defines exchange headers as untrusted, adversary-controlled
data:

- **rc-rbfxq** — a `CamelHttpUri` exchange header replaces the entire
  outbound URL — scheme, host, path, query — before SSRF validation, with no
  allowlist tying it to the endpoint's configured host. SSRF default
  (`allow_internal=false`) blocks private addresses only, so an attacker who
  can set `CamelHttpUri` redirects the producer to any public host. The
  e_glm counter-report lowered it to p2 (defense-in-depth, not default-path
  exploitability): reachable only when a route step explicitly copies
  untrusted data into an exactly-cased `CamelHttpUri` header. But the
  contract inconsistency with ADR-0034 stands — control-bus fenced the same
  header-driven control-target class with `authorizedRoutes`, while camel-http
  had no fence.
- **rc-k3pir** — default inbound reflection is undocumented: the consumer
  installs   `CamelHttpPath`/`CamelHttpQuery` from the inbound wire request
  (lib.rs:1821-1825), and a non-bridged producer consumes both by default, so
  `from: http://.../in -> to: http://upstream/api` appends the inbound path
  and query to the upstream URL with zero route steps. Through the rc-69fie
  else-if, the inbound query silently replaced operator-configured
  `query_params`.
- **rc-69fie** — a `CamelHttpQuery` header present on an exchange carrying
  endpoint-configured query params took the header branch and silently
  discarded the configured params — no merge, no warning. Under ADR-0032 this
  is worse: the untrusted exchange header fully replaced trusted operator
  config.

ADR-0032 establishes that no untrusted exchange datum may drive a control
plane, numeric, or resource decision, or an executable/interpretable sink,
without validation, bounding, or a capability check. The outbound URL is a
resource decision. ADR-0034 supplies the fence precedent: control-bus fenced
its header-driven route target with a mandatory `authorizedRoutes` allowlist
that fails closed when absent.

## Decision

1. **Outbound query composition (rc-69fie).** When the exchange carries a
   `CamelHttpQuery` header, the producer composes the outbound query from the
   arm-specific higher-precedence source followed by the header pairs whose
   keys are absent from that set; the higher-precedence source wins any key
   collision.
   - **Base arm** (no `CamelHttpUri`): the higher-precedence source is the
     endpoint query — `raw_query` (consumed option keys filtered) plus
     programmatic `query_params`.
   - **Override arm** (`CamelHttpUri` present): the higher-precedence source
     is the override URI's own query. The endpoint base query does NOT ride
     an override — the override remains untrusted exchange data under
     ADR-0032, so trusted operator config never rides an untrusted override.
   - `CamelHttpPath` applies to the path component before query composition
     in both arms.
   - Header pair bytes are carried verbatim; a raw byte forbidden in a query
     component inside a header value produces a resolve error naming the
     offending byte, never a re-encoding (Wave-A law, raw-preserving
     serializer).
   - A present-but-empty `CamelHttpQuery` is a no-op: the higher-precedence
     source is emitted unchanged with no additional `?` marker.
   - **Override-URI merge fix:** when an override URI carries its own query
     and the exchange also carries `CamelHttpQuery`, the two merge at pair
     level — override pairs first (winning collisions), header pairs appending
     for absent keys — instead of concatenating a second `?` marker.
   - **Deliberate divergence from Apache Camel.** Apache Camel applies the
      `CamelHttpQuery` header verbatim, replacing the endpoint query entirely
      (header-wins-verbatim). This ADR composes instead, so collisions resolve
      to the higher-precedence (operator) source: the endpoint config in the
      base arm, the override URI's own pairs in the override arm. The
      divergence is explicit and test-pinned.

2. **Default inbound reflection retained (rc-k3pir, Apache Camel parity).**
   Consumer-installed `CamelHttpPath`/`CamelHttpQuery` ride the outbound URL
   by default and compose per rule 1. The plain-proxy shape keeps working;
   the operator query pair is not replaced by reflected inbound data.

3. **Opt-in `allowedUriHosts` fence (rc-rbfxq).** The endpoint URI accepts a
   comma-separated `allowedUriHosts` option: exact hosts, each optionally
   `host:port`.
   - Bracketed IPv6 literals compare in canonical form; DNS names compare
     case-insensitively; a host-only entry permits any port; a `host:port`
     entry matches only the override's effective port (explicit port, or the
     scheme default: 443 https / 80 http).
   - A malformed entry — a segment carrying a path or userinfo, an entry the
     `url` crate rejects, or a declared option that yields zero valid entries
     after trimming/dropping empty comma segments — fails endpoint creation.
   - An armed fence fails closed: a `CamelHttpUri` override resolving to a
     host not matching any entry, or failing to yield a host, is a resolve
     error, and the rejected URL is rendered only through the diagnostics
     redaction path (ADR-0051).
   - An unarmed endpoint (option absent) keeps the pre-fence override
     behavior unchanged. The option is consumed: it never appears in the
     outbound query.

4. **Compatibility exception versus ADR-0034.** Unlike control-bus's
   MANDATORY `authorizedRoutes` fence (which fails closed when absent),
   the camel-http fence is opt-in: unarmed endpoints keep the pre-fence
   override behavior. This is a deliberate compatibility trade-off —
   defense-in-depth hardening, not incident response — preserving Apache
   Camel override semantics for operators who did not ask for the fence.

## Consequences

- **Migration for Apache Camel users** relying on header-wins-verbatim:
  their `CamelHttpQuery` headers now COMPOSE. Collisions resolve to the
  higher-precedence source — the endpoint config in the base arm, the
  override URI's own pairs in the override arm — and only absent keys append.
  A route that previously overwrote a static operator pair must delete that
  pair from the endpoint URI or drop the header.
- **Fence adoption path** for routes that copy untrusted inbound data into
  `CamelHttpUri`: declare `allowedUriHosts` with the exact target hosts. An
  armed fence turns a redirect-to-any-public-host into a redacted resolve
  error. The fence is enforced at override resolution time only: redirect
  hops under `followRedirects` (off by default) are NOT fence-checked and
  rely on the existing per-hop SSRF validation and cross-origin credential
  stripping.
- **Reflection is now documented** as the default, next to `bridgeEndpoint`,
  rather than an undocumented side effect.
- **Wave-A law** continues to govern authored and header query bytes: the
  raw-preserving serializer carries them verbatim, forbidden bytes error
  naming the byte, and the redaction path protects credentials in any
  diagnostic rendering of a rejected URL.
- **Contract surface updated** in `crates/components/camel-http/CONTEXT.md`;
  future bug reports about outbound URL/query behavior check the composition
  and fence rules there first.
