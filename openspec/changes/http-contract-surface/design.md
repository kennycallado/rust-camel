# Design: http-contract-surface

## Approach

Three policy decisions, one contract surface. All live in
`resolve_url` (`crates/components/camel-http/src/lib.rs:2479`) and its
config; all reuse Wave-A serializer primitives unchanged.

### D1 — Query composition replaces the silent drop (rc-69fie)

The `CamelHttpQuery` early-return (lib.rs:2544-2554) is replaced by
composition. The higher-precedence query source differs per arm — an
override replaces the base URL entirely, so the endpoint query rides only
when no override wins:

- **Base arm** (no `CamelHttpUri` header): higher-precedence source =
  endpoint query from `resolve_endpoint_query(config)` — raw-preserving,
  consumed-option-filtered, exactly Wave-A behavior (operator config).
- **Override arm** (`CamelHttpUri` present): higher-precedence source = the
  override URI's own query. The endpoint base query does NOT ride — the
  override replaces the base URL, preserving existing semantics. The
  override URI itself remains untrusted exchange data under ADR-0032; the
  fence (D3) is the control for that untrusted source.
- In both arms: header pairs append after, for keys absent from the
  higher-precedence set. **The higher-precedence source wins collisions** —
  in the base arm this protects operator config from untrusted exchange
  data (consistent with Wave-A's authored-wins rule); in the override arm
  it keeps the override's own pairs deterministic.
- `CamelHttpPath` applies to the path component BEFORE query composition,
  in both arms (existing append semantics, then query merges).
- Header bytes preserved verbatim via `raw_query_pairs` spans; a forbidden
  byte in a header value is a resolve error naming the offending byte —
  never re-encoded (Wave-A law).
- Latent bug repaired in the same pass: the `CamelHttpUri` arm pushes `?`
  unconditionally before the header query (lib.rs:2525), producing
  `...?a=1?b=2` when the override URI already carries a query. Composition
  merges at pair level instead of string-concat.

This deliberately diverges from Apache Camel (header-wins-verbatim) —
recorded in the ADR below.

### D2 — Reflection default: documented parity, no new option (rc-k3pir)

Reflection stays ON by default (parity; the plain proxy keeps working).
After D1, its worst effect is gone: inbound query no longer *replaces*
operator query — it composes, operator pairs winning collisions. The
reflection default is documented in `camel-http/CONTEXT.md` "Outbound query
fidelity" next to `bridgeEndpoint`, naming the consumer headers that feed
it and the composition rule. A `disableReflection`-style option is
explicitly deferred: route steps can remove headers today, and config
surface is a commitment (revisit only if the bless gate insists).

### D3 — `CamelHttpUri` fence (rc-rbfxq)

New consumed URI option `allowedUriHosts` (comma-separated exact hosts,
optional `:port`; no wildcards). Semantics — ADR-0034 lineage, opt-in:

- Endpoint declares `allowedUriHosts` → a `CamelHttpUri` override resolving
  to a host not on the list is a **resolve error** (fail-closed, loud),
  with the rejected URL passed through `redact_url_for_diagnostics`.
- Endpoint unarmed (no option) → behavior unchanged; SSRF still applies.
  This is defense-in-depth hardening, not incident response — no silent
  downgrade of existing dynamic-URI routes (`test_http_producer_uri_override`
  stays green).
- **Entry parsing and matching:** empty comma segments are dropped. A
  declared `allowedUriHosts` that yields zero valid entries after trimming
  fails endpoint creation (a declared-but-empty allowlist is a
  configuration error, not an armed deny-all). Any other malformed entry
  fails endpoint creation. DNS names compare case-insensitively (canonical
  lowercase); IPv6 literals compare in bracketed canonical form. A
  host-only entry permits any port; a `host:port` entry matches only when
  the override's effective port equals it. An armed override URI that
  fails to parse a host fails resolution.
- Registered as `#[uri_param]` on `HttpEndpointUriConfig` →
  `uri_options()` metadata → parity pin 21→22 → `is_consumed_option`
  filters it from the outbound raw query automatically.

### ADR-0071 + contract surface

A small ADR records the three decisions (composition-divergence, reflection
default, fence shape). It explicitly records one compatibility exception:
unlike control-bus (ADR-0034 mandatory `authorizedRoutes` fence) and
ADR-0032's per-route validation rule, the camel-http fence is **opt-in**
and unarmed endpoints keep pre-fence override behavior — a deliberate
compatibility trade-off (hardening, not incident response), not a claim of
identical posture. Future bug reports check the ADR before filing.
`camel-http/CONTEXT.md` is amended to match.

## Affected crates

- `camel-component-http`: `resolve_url` (composition + fence), the
  `CamelHttpUri` arm merge fix, `HttpEndpointUriConfig` (+1 uri_param,
  parse + validate), parity-pin test bump, unit tests.
- `docs/adr/0071-*.md`: new mini-ADR.
- `camel-http/CONTEXT.md`: "Outbound query fidelity" + contract surface.

## Architecture boundaries

Component layer only — no Runtime, DSL, Core, or camel-endpoint changes.
Wave-A serializer primitives (`raw_query_pairs`,
`validate_raw_query_span`, `encode_query_component`) are consumed as-is;
the serializer itself is untouched. Redaction law extends to the new error
path (fence rejection renders only redacted URLs). New URI-option metadata
may require regenerating the checked-in component schema — the schema gate
decides, no silent skip.

## Testing approach

Unit tests in camel-http, TDD per task: composition (merge, collision
endpoint-wins, forbidden header byte errors, override-URI-with-query merge
fix), fence (armed-allow, armed-deny errors redacted, unarmed unchanged),
parity pin 22, and the full existing suite green (no pinned test moves
without explicit reconciliation in its task block).
