# Design: cleartextpolicy

## Approach

Adopt one uniform egress rule for the four outbound paths, recorded in
a new ADR-0081 that supersedes the default-row semantics of ADR-0079:

**Rule.** For any outbound URL — initial, `CamelHttpUri` override,
redirect hop, MCP remote — cleartext `http://` to a PUBLIC (non-
`is_ssrf_blocked_ip`) target is rejected unless the operator sets
`allow_cleartext` on that endpoint (camel-http) or remote (MCP).
Independent of `allow_internal`.

**Full matrix (cleartext `http://` to a public target):**

| Path | false (default) | true `allow_internal` | `allow_cleartext` |
|---|---|---|---|
| camel-http literal (`ssrf.rs` validate_url_for_ssrf) | allow → **REJECT** | REJECT (unchanged) | **ALLOW** |
| camel-http hostname (`resolve_initial_url_for_ssrf`, redirect variant) | allow → **REJECT** | REJECT (unchanged) | **ALLOW** |
| MCP literal (`config.rs` validate_url) | allow → **REJECT** | REJECT (unchanged) | **ALLOW** |
| MCP hostname (`dns_pin.rs` validate_resolution) | allow → **REJECT** | REJECT (unchanged) | **ALLOW** |

Unchanged rows: https to public targets always allowed; internal
targets always need `allow_internal=true`; cleartext to internal
targets under `allow_internal=true` stays allowed. Default (no-flag)
semantics — not escape-enabled semantics — equal `SsrfPolicy`
(camel-api) as documented and already enforced by camel-auth/llm/
keycloak: the zone converges with the workspace's documented center
of gravity instead of inventing policy.

**Why the ban is right** (the weighed conclusion): (1) the permissive
default is inherited, never decided — ADR-0079 only repaired parity;
(2) every credential-bearing egress path already requires HTTPS by
default (`SsrfPolicy::PublicHttpsOnly`), so camel-http/MCP are the
workspace outliers; (3) `CamelHttpUri` override is untrusted exchange
data (ADR-0032/0071) — with an unarmed `allowedUriHosts` fence the
default-open cell is an attacker-steered cleartext exfil channel;
(4) e_gpt's ADR-0079 flip trigger names exactly this policy and
e_opus called it the coherent hardening layer.

**Deployment survey (who breaks):** no deployment inventory exists —
breakage frequency is unknown and ADR-0079 records no empirical
evidence about it. What the source review establishes: every
camel-http producer endpoint and MCP remote whose effective URL is
`http://` at a public target breaks at first send / config load;
redirect chains ending on public cleartext break at the hop;
`CamelHttpUri`-override routes that steer to public cleartext break
per-exchange. Internal-only deployments (loopback, RFC1918,
`allow_internal=true` everywhere) are unaffected. Breakage is loud
(send-time / config-load error naming the remedy), not silent.

**Escape hatch decision (decides bd rc-poeg8):** per-endpoint (URI
option, camel-http) / per-remote (TOML field, MCP) boolean
`allow_cleartext`. camel-http consent is endpoint-scoped and covers
the configured URL, `CamelHttpUri` overrides, and redirect targets —
deployments needing host confinement must also arm `allowedUriHosts`.
Rejected shapes: global lever (reopens the ban workspace-wide, defeats
least privilege); host allowlist (new matching semantics to review,
duplicates `blocked_hosts`/`allowedUriHosts` machinery); no escape
(makes legitimate plain-HTTP egress impossible — too hostile). The
flag is orthogonal: `allow_internal` = internal reachability only;
`allow_cleartext` = public transport downgrade consent only. The
ADR-0079 trap stays closed: no flag set for internal reasons widens
cleartext; only the flag NAMED for cleartext does.

## Affected crates

- `components/camel-http` (`ssrf.rs`, `lib.rs`): add `allow_cleartext`
  to `HttpEndpointConfig` + `allowCleartext` URI option (metadata via
  `uri_options()`, ADR-0041); flip the three conjuncts
  (`allow_internal && !blocked && http` → `!blocked && http &&
  !allow_cleartext`; hostname equivalents); thread the flag through
  `resolve_initial_url_for_ssrf` / `validate_redirect_target_for_ssrf`;
  update `CONTEXT.md` SSRF section.
- `components/camel-component-mcp` (`config.rs`, `adapter/dns_pin.rs`):
  `McpRemoteConfig.allow_cleartext` (`#[serde(default)]`,
  `deny_unknown_fields` intact); flip literal-branch conjunct;
  `validate_resolution`/`build_pinned_http_client` gain the flag;
  update field docs.
- `docs/adr`: new 0081 (matrix, survey, escape decision, migration);
  superseded-by note on ADR-0079's default row.

## Architecture boundaries

Components layer only — no Runtime/DSL/Services change. Config keys
are per-component (URI options / TOML remotes), owned by their crates.
The blocked-IP classifier (`camel_api::is_ssrf_blocked_ip`) is reused
untouched; camel-api itself is NOT modified (its `SsrfPolicy` doc
already matches the adopted rule). ADR-0032 (untrusted exchange data),
ADR-0071 (override + fence), ADR-0051 (redaction in errors) hold;
new rejection messages name the remedy without echoing raw URLs.

## Migration note

Configs that break: camel-http `to: http://<public>` endpoints and
MCP `http://` public remotes. Fix: set `allowCleartext=true` in the
endpoint URI, or `allow_cleartext = true` on the remote, or move to
`https://`. Every other target/scheme combination keeps its
pre-change acceptance verdict (error ordering and diagnostics may
differ). Changelog "Changed:" sentence ships in the landing commit
body.

Single-phase change (three small, tightly-coupled file groups sharing
one rule; splitting would land half a matrix).
