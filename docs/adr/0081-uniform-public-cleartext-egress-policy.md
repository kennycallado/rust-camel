# ADR-0081: Uniform public-cleartext egress policy (camel-http + MCP)

- Status: Accepted (decided 2026-09-18; mission 118, bd rc-hb4fr; decides
  bd rc-poeg8)
- Source: bd rc-hb4fr (e_opus ruling 2026-09-17 prescribed the mission)
- Supersedes: the DEFAULT-row semantics of ADR-0079. The
  `allow_internal=true` cell and the trap-closure of ADR-0079 stand.

## Context

ADR-0079 aligned the MCP IP-literal rule with camel-http. After that
change, all four validation paths behave alike. Cleartext `http://` to a
PUBLIC target (any address that fails `is_ssrf_blocked_ip`) is allowed
by default and rejected under `allow_internal=true`:

| Path (cleartext http:// to a PUBLIC target) | allow_internal=false | allow_internal=true |
|----------------------------------------------|----------------------|---------------------|
| camel-http, IP literal | allow | REJECT |
| camel-http, hostname (initial URL, override, redirect hop) | allow | REJECT |
| MCP, IP literal (config load) | allow | REJECT |
| MCP, hostname (connect, DNS-pinned) | allow | REJECT |

The default column was never decided. It is inherited from camel-http's
original behavior, and ADR-0079 copied it for parity instead of judging
it.

Two facts put that default in conflict with the rest of the workspace:

1. `camel_api::SsrfPolicy::PublicHttpsOnly` already requires HTTPS for
   camel-auth egress (JWKS fetch, OAuth2 introspection, OIDC discovery),
   camel-component-llm, and camel-component-keycloak. camel-http and MCP
   were the outliers that still shipped default-open public cleartext.
2. The `CamelHttpUri` override is untrusted exchange data (ADR-0032,
   ADR-0071). A route that consumes the override without the
   `allowedUriHosts` fence lets the caller choose the outbound URL.
   Default-open cleartext turns that choice into attacker-steerable
   exfil: the caller names a plain-HTTP collector under their control
   and reads what the route sends.

## Decision

One rule covers all four paths. Cleartext `http://` to a PUBLIC target
is rejected unless the operator consents for that endpoint or remote:

- camel-http endpoints: the URI option `allowCleartext=true`.
- MCP remotes: the TOML field `allow_cleartext = true`.

The consent flag is independent of `allow_internal`. Internal targets
still require `allow_internal=true`. The rule applies to the initial
URL, to `CamelHttpUri` overrides, and to every redirect hop on
camel-http, and to MCP remotes at config load (literals) and at connect
(hostnames). Error messages name the remedy: set the consent flag or use
`https://`.

The matrix after this change, against the pre-change state:

| Path (cleartext http:// to a PUBLIC target) | Before: default | Before: allow_internal=true | After: default | After: allow_internal=true | After: consent flag |
|---|---|---|---|---|---|
| camel-http, IP literal | allow | REJECT | REJECT | REJECT | allow |
| camel-http, hostname (initial, override, redirect) | allow | REJECT | REJECT | REJECT | allow |
| MCP, IP literal (config load) | allow | REJECT | REJECT | REJECT | allow |
| MCP, hostname (connect) | allow | REJECT | REJECT | REJECT | allow |

## Rejected Alternatives (escape-hatch design)

**Global config lever.** One workspace-wide key re-enables public
cleartext for every route and remote at once. That reopens the ban
workspace-wide and defeats least privilege. Rejected.

**Cleartext host allowlist.** Permit cleartext to listed public hosts.
This invents new matching semantics that duplicate the existing
machinery (`blocked_hosts`, `allowedUriHosts`). Rejected.

**No escape hatch.** Keep the rejection absolute. Legitimate plain-HTTP
egress (a lab fixture, a debug target, a migration endpoint) becomes
impossible. Rejected.

The per-endpoint boolean wins on surface: the operator sets it exactly
where the cleartext target lives, and no other route or remote changes
behavior.

One flag keeps one meaning. `allow_internal` grants internal
reachability. `allow_cleartext` records consent to public cleartext
transport. No flag set for internal reasons widens cleartext egress, so
the ADR-0079 trap stays closed.

## Deployment survey

No deployment inventory exists for this workspace. The statements below
are structural, not empirical.

- Every camel-http endpoint and every MCP remote whose effective URL is
  `http://` at a public target breaks loudly after this change:
  camel-http at send time, MCP literals at config load. Error messages
  name the remedy.
- Internal-only deployments are unaffected. Internal cleartext targets
  keep working under `allow_internal=true`.
- `https://` targets are unaffected.
- How many deployments carry public cleartext endpoints is unknown. No
  telemetry exists to estimate the breakage rate.

## Consequences

**Breaking default.** Cleartext HTTP to public targets now fails unless
the operator sets the consent flag or the target moves to `https://`.

**Migration.** Set `allowCleartext=true` on the camel-http endpoint or
`allow_cleartext = true` on the MCP remote, or switch the URL to
`https://`. Both remedies appear in the error message.

**`allowedUriHosts` keeps its role.** The consent flag approves a
transport. It does not confine hosts. Routes that accept the
`CamelHttpUri` override still need the fence (ADR-0071).

**Redirect hygiene unchanged.** Cross-origin redirects still strip
`Authorization` and `Cookie` headers. Every redirect hop passes the same
cleartext rule.

## References

- bd rc-hb4fr (this decision), rc-poeg8 (decided by this ADR)
- ADR-0079 (MCP allow_internal alignment; default row superseded here)
- ADR-0032 (untrusted exchange data), ADR-0071 (outbound URL policy and
  `allowedUriHosts`), ADR-0051 (redaction of rejected URLs in
  diagnostics)
- e_opus ruling 2026-09-17 (prescribed the mission)
- openspec change `cleartextpolicy` (spec `egress-cleartext-policy`)
