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
