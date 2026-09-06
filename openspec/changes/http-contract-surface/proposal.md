# Proposal: http-contract-surface

## Why

Three ADR-0032-class gaps sit in camel-http's outbound URL resolution
(`resolve_url`, `crates/components/camel-http/src/lib.rs:2479`). Each lets
untrusted exchange data reach a resource decision — the outbound URL — with
either no fence, no documentation, or no signal:

1. **No fence (rc-rbfxq, e_opus C2 — most serious).** A `CamelHttpUri`
   exchange header replaces the entire outbound URL — scheme, host, path,
   query (lib.rs:2504-2529). SSRF validation (`allow_internal=false`) blocks
   private addresses only, so an attacker-influenced header can redirect the
   producer to any public host: exfiltration / SSRF-to-public. ADR-0034
   fenced the same threat class in control-bus (`authorizedRoutes`,
   fail-closed); camel-http is contractually inconsistent — it has no fence.
2. **Undocumented default proxy reflection (rc-k3pir, e_glm B-1).** The
   consumer sets `CamelHttpPath`/`CamelHttpQuery` from the inbound wire
   request (lib.rs:1803-1806); a non-bridged producer consumes both
   (lib.rs:2531-2554). So `from: http://...` → `to: http://upstream/api`
   appends the inbound path and query to the outbound URL by default, with
   zero route steps. Apache Camel parity, yes — but nothing in
   `camel-http/CONTEXT.md` names this reflection, and it interacts with (3).
3. **Silent drop of trusted config (rc-69fie, e_opus C1).** When
   `CamelHttpQuery` is present, the verbatim early-return at lib.rs:2544-2554
   skips `resolve_endpoint_query` entirely: operator-configured endpoint
   query (`raw_query` + `query_params`) is discarded — no merge, no warning.
   In the default proxy shape the winning header is consumer-set from the
   wire, so untrusted data silently replaces trusted operator config on the
   most common proxy route.

## What Changes

- **One merge-policy decision (rc-69fie + rc-k3pir together):** the
  `CamelHttpQuery` header and the endpoint query compose instead of silently
  dropping the endpoint side. Collision precedence, the exact composition
  order, and whether the reflection default gains an opt-out endpoint option
  are design decisions (design.md), blessed at the spec gate.
- **CamelHttpUri fence (rc-rbfxq):** allowlist-style hardening in the
  ADR-0034 lineage, with one explicitly recorded compatibility exception:
  the fence is opt-in, not mandatory (unarmed endpoints keep pre-fence
  override behavior). Gate form and fail mode are design decisions; the
  fence fails closed when armed.
- **Contract surface:** `camel-http/CONTEXT.md` "Outbound query fidelity"
  section amended — reflection default named next to `bridgeEndpoint`, merge
  policy specified, fence documented. Whether this ships as a mini-ADR is a
  design decision.
- Explicitly out of scope: the `bridgeEndpoint=true` arm (Wave A settled
  law), the raw-preserving serializer, redaction law, and the consumer's
  header-setting behavior itself.

Affected crates: `camel-component-http`; docs (`CONTEXT.md`, possibly a new
ADR). bd: rc-rbfxq, rc-k3pir, rc-69fie (children of epic rc-enbw).

## Acceptance criteria

- In the base arm, endpoint-configured query pairs are never silently
  discarded when `CamelHttpQuery` is present — they compose per the blessed
  policy. When a `CamelHttpUri` override replaces the base URL, the
  endpoint query is intentionally not carried (existing semantics,
  recorded in the ADR).
- The default proxy reflection is documented in the contract surface with its
  interaction with the merge policy.
- `CamelHttpUri` override is fenced and fails closed; the existing pinned
  behavior (`test_http_producer_uri_override`) is reconciled explicitly, not
  silently broken.
- Wave-A law preserved verbatim: raw-preserving serialization, forbidden-byte
  resolve errors, redaction law untouched; both feature builds green.

## Risk budget

- The default proxy pattern (`from: http` → `to: http`) must keep working —
  policy changes compose in the default shape; they do not reject it.
- Breaking-change surface limited to the fenced override path; any break is
  documented and test-pinned.
- No new dependency; no schema change. Docs-only outcomes are unacceptable
  for the fence (contract inconsistency with ADR-0034 was the refuted
  position).
