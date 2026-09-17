# ADR-0076: URL Redaction — Strictest-Wins Convergence

- Status: Accepted (2026-09-16)
- Supersedes: per-crate redaction drift (no prior ADR; see Context)
- Normative specs: `openspec/specs/http-url-resolution/spec.md`, `openspec/specs/jms/spec.md`
- Implementation: `crates/camel-api/src/redact.rs` (canonical helper)

## Context

Four per-crate URL-redaction helpers drifted apart: `camel-api`
`redact_broker_url`, `camel-config::redact_url`, a JMS variant, and an
HTTP diagnostics variant. Code reading (2026-09 audit, mission 100)
confirmed two real leaks: fragment credentials survived both arms
(rc-zf06t), and double-slash evaders leaked through the PARSED arm
(rc-y6101 — `rust-url` parses `scheme://user:pass@/`-shaped evaders).
Missions 100 (6f904f09) and 103 (136c1416, openspec change
`2026-09-16-redact2`) converged every helper onto one canonical
module in `camel-api`.

## Decision

Hardening dimensions converge to the **strictest** behavior per
dimension — "strictest wins":

| Dimension | Converged behavior |
|---|---|
| Userinfo mask | Windowed authority scan on every arm; fail-closed sentinel when unparseable |
| Window terminators | `/`, `?`, `#` |
| Slash-run evaders | Skip the full slash run before the window scan |
| Query | Drop the whole query; append `?[redacted]` |
| Fragment | Drop the whole fragment; append `#[redacted]` |
| Length cap | 256 bytes on a UTF-8 boundary |

**One exception:** JMS broker URLs keep a per-key substring allowlist
for the **query dimension only**. ActiveMQ failover URIs encode
non-secret transport policy in query parameters; that policy is the
sole diagnostic value of a broker URL in Debug output, and a
whole-query drop would make failover diagnosis impossible. Userinfo,
fragment, and length-cap dimensions converge without exception.

Adjacent surfaces stay separate — different threat models, do not
converge:

- `camel-api/src/endpoint_uri.rs` (`to_redacted_string`):
  catalog-driven secret detection for authored endpoint URIs.
- `services/camel-auth/src/credential_source.rs` (`redact_query_params`):
  typed `http::Uri`, caller-supplied key allowlist.
- `camel-http` `mask_base_url_userinfo`: byte-preserving surgery on
  the first authority segment, deliberate (avoids WHATWG
  normalization of authored base URLs); tracked in rc-yvjp3.

## Consequences

- The residual risk of backslash authority separators on non-special
  schemes is CLOSED (rc-f05q8): scheme-prefixed backslash runs open
  the authority window on every arm (`foo:\user:pass@evil/` masks).
- Double-encoding (`%2540`) stays out of scope by design: a
  single-decode consumer cannot yield credentials from it. Reopen
  only if a double-decoding consumer appears.
- Audit evidence (surface maps, adversarial batteries) lives in the
  fleet inbox, not in the repository — decisions go to ADRs,
  behavior to specs, evidence stays fleet-internal.

## Appendix (2026-09-17, mission redactconv, bd rc-yvjp3)

### 1. Fourth-variant convergence

The `camel-http` `mask_base_url_userinfo` surface named under
"Adjacent surfaces" above is now converged and removed. The
`HttpEndpointConfig` Debug `base_url` field routes through the canonical
`camel_api::redact::redact_url`. The canonical helper is string surgery:
no `url::Url` roundtrip occurs, so the original byte-preservation
rationale (WHATWG normalization of authored base URLs) does not apply to
it. Output changes, strictest-wins intentional: query bytes now drop
behind `?[redacted]`, fragment bytes behind `#[redacted]`, later
`//user:pass@` authority windows mask, and the result caps at 256 bytes.
The adjacent-surfaces bullet above is superseded by this appendix. The
other two adjacent surfaces are unchanged.

### 2. Key-position credential-shape symmetry (e_opus ruling)

A query pair matched by the sensitive-key denylist SHALL render as a
bare `<redacted>` (the key never echoes) when the raw key's single-pass
minimal decode — `%40`→`@`, `%3a`→`:`, `%2f`→`/`, hex case-insensitive —
contains `@` AND (`:` OR `//`); otherwise it renders as
`{raw_key}=<redacted>`. This applies the same credential-shape predicate
to the key position that the benign-key branch already applies to the
whole pair, closing the operator-authored-key echo channel preemptively
rather than at the reopen trigger (user-supplied broker URLs). Reasoning:
the denylist branch rendered the raw key verbatim, so a key such as
`user%3Asecret%40host` echoed the credential `user:secret@host` while a
benign-keyed pair with the same shape was fully suppressed — the
stricter treatment sat on the less suspicious position. Well-known
JMS/ActiveMQ parameter names do not decode to this shape, so the
exception's transport-policy diagnostic value is preserved intact.

Spec note: `openspec/specs/jms/spec.md` still carries the sentence
"the rendered key keeps its original encoded bytes" without this
qualifier. That canon edit requires an OpenSpec change; it is deferred
to the master. The code is spec-compatible meanwhile because the new
branch only over-masks, and over-masking is safe (ADR-0051).
Landed: openspec change jmscredkey (bd rc-tfugr) amended the jms spec
canon with this qualifier.
