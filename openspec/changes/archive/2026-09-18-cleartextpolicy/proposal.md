# Proposal: cleartextpolicy

## Why

ADR-0079 made the four egress paths (camel-http literal, camel-http
hostname, MCP literal, MCP hostname) agree on one matrix: cleartext
`http://` to a PUBLIC target is allowed by default and rejected under
`allow_internal=true`. That alignment was parity repair, not policy
design — the permissive default column survived by inheritance from
camel-http's original behavior, never by decision.

Meanwhile every other outbound path in the workspace already runs the
strict posture: `camel_api::SsrfPolicy::PublicHttpsOnly` (camel-auth
JWKS/introspection/OIDC, camel-component-llm, camel-component-keycloak)
requires HTTPS outright. The documented enum contract says "enforces
HTTPS + public IPs only". camel-http and the MCP producer are the only
outbound surfaces that still emit cleartext to the public internet by
default, so credentials and payloads ride unencrypted hops unless an
operator knows this history.

e_opus final ruling (2026-09-17, rc-lztp2 landing) prescribed this
mission: evaluate a CONSISTENT public-cleartext egress policy — banning
cleartext `http://` to public targets under BOTH `allow_internal`
values for BOTH spellings — as its own ADR superseding/extending
ADR-0079 (bd rc-hb4fr, P2, not release-blocking).

## What Changes

- Adopt the uniform rule for all four paths: cleartext `http://` to a
  public target is REJECTED unless the operator sets a new
  `allow_cleartext` escape hatch (`allowCleartext` URI option on
  camel-http endpoints, `allow_cleartext` TOML field on MCP remotes).
  The rule applies to initial URLs, `CamelHttpUri` overrides, every
  redirect hop, and MCP remote config load / DNS-pin connect.
- `allow_internal` keeps exactly one meaning everywhere: internal-target
  reachability. It stops governing public transport entirely; the
  ADR-0079 cell (`allow_internal=true` + public cleartext → reject)
  stays closed unless `allow_cleartext` is also set explicitly.
- New ADR-0081 recording the matrix, deployment survey, escape-hatch
  decision (decides bd rc-poeg8), and migration note; supersedes the
  default-row semantics of ADR-0079. In-crate CONTEXT.md rows updated.
- TDD pins across all four paths × (default, allow_internal=true,
  allow_cleartext escape) with flipped and new test cells.

Excluded: camel-cli and openspec/specs/cli-feature-profiles (external
agent owns them); `SsrfPolicy` consumers (camel-auth, llm, keycloak —
already strict; camel-api is not modified); consumer-side (inbound)
paths; `blocked_hosts`/fence behavior.

## Acceptance criteria

- All four egress paths reject public cleartext by default and under
  `allow_internal=true`, and allow it with `allow_cleartext=true` —
  pinned by named tests in both crates.
- Internal-target and https behavior is byte-identical to today
  (regression pins).
- Existing configs without the new field/option keep deserializing
  (`#[serde(default)]`; URI option absent = false), except the
  documented breaking default (public cleartext now fails).
- ADR-0081 + migration note land; ADR-0079 carries a superseded-by
  note on the default row.
- Gates green: fmt, clippy on touched crates (`--all-targets
  -D warnings`), both crates' test suites, doc-build on touched pub
  crates, the 12 xtask lints, golden deptree WITHOUT regen.

## Risk budget

- Breaking default for plain-HTTP egress to public targets is
  ACCEPTED with a one-line mechanical migration (`allowCleartext=true`
  / `allow_cleartext = true`). No deployment inventory exists;
  breakage frequency is unknown — the errors are loud and name the
  remedy at send time / config load.
- No new dependency, no workspace-wide rebuild beyond the two crates,
  no public API removal. Out of bounds: touching forbidden zones,
  changing blocked-IP classification, weakening any existing
  rejection.
