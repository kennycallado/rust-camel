# Proposal: auth-reinforcement

## Why

The camel-auth crate is a sound foundation (r_glm subsystem review 2026-08-17, KB:
`camel-auth-review-2026-08-17`), but the config-to-runtime wiring reduces it to a
demo: exactly one static credential end-to-end, OIDC silently ignored, dead config
surface documented as working, and docs that steer users into shipping placeholder
strings as live credentials (rc-xb19). The maintainer confirmed the delete direction
for the never-consumed mini-IdP surface (`token_issuer`/`clients`, ~1,231 LoC).
This change reinforces the wiring in delivery phases instead of a big-bang fix or a
redesign.

Umbrella bd epic: rc-5u38. Design input: r_glm review + the amended T1-T6 breakdown
in `docs/audits/rc-xb19-security-placeholder-resolution-analysis.md`.

## What Changes

Four delivery phases in one plan:

- **Phase 1 — Config honesty & fail-closed** (rc-xb19 T1-T6, rc-vnk2, rc-fxfl):
  `resolve_placeholders` resolves ALL `[security.*]` string leaves (credential and
  non-credential) and `[datasources.*]` (`db_url`, `extra` recursion incl.
  SurrealDB `password` — treated as credential-class) with UNIFORM fail-closed
  semantics for these subtrees: unset env var without a default, dash-prefixed
  defaults (the `{{env:X:-default}}` trap), and surviving `{{`/`${` literals all
  yield `ConfigError`; valid single-colon defaults resolve normally (T1-T3);
  authenticator boundary guard rejects marker-containing secrets as
  defense-in-depth (T4); docs stop recommending broken placeholder recipes (T5);
  regression tests (T6). OIDC-only config wires a real authenticator (`jwks_uri`
  required, no Keycloak default) or fails with explicit `ConfigError`; misleading
  auth error texts fixed; yaml-dsl `credential_sources` docs gap closed.
- **Phase 2 — Multi-credential native + dead-surface removal** (rc-8yvh, rc-7fxn):
  `[[security.native.credentials]]` array (`{subject, secret_env|secret, roles,
  scopes}`) maps onto the existing `NativeCredentialStore`; env-based secrets wired
  here (the array shape does not exist before this phase); scalar `bearer_token` and
  `api_key` remain as single-entry sugar. DELETE: `token_issuer`/`clients` config
  fields, `native_issuer.rs`, `native_client_store.rs`, `native_jwks.rs`,
  `ApiKeyAuthenticator`, `camel-http::auth` wrapper, and their docs rows. Scalar
  `api_key` stays and is wired.
- **Phase 3 — Named providers** (rc-hyds): authenticators keyed by provider name in
  `SecurityCompileContext`; `security_policy.provider` names its authenticator;
  omitted provider selects the sole configured authenticator, multiple configured
  providers require an explicit choice; keycloak/oidc/native XOR removed.
- **Phase 4 — gRPC credential sources** (rc-9f15): replace hardcoded `Bearer ` strip
  with shared extraction over gRPC metadata (header and authorization mappings);
  transport-unsupported sources (`query_param`, `cookie`) rejected at route load.

**Explicitly excluded:** rc-fzgm full transport auth unification (own change);
rc-w5bf placeholder syntax unification (composition note only — Phase 1 fail-closed
semantics must not conflict); OIDC discovery client (YAGNI per review).

Affected crates: camel-config, camel-cli, camel-auth, camel-dsl, camel-api,
camel-http, camel-component-grpc, docs.

## Acceptance criteria

- Multiple `[[security.native.credentials]]` entries each authenticate their
  principal; api_key-only configs start and enforce; legacy scalar configs behave
  as v0.29.0.
- `[security.oidc]`-only configs with a valid `jwks_uri` register a working
  authenticator; malformed or unreachable-at-startup JWKS fails with `ConfigError`.
  Silent `None` is not a permitted outcome.
- Placeholder syntax in security credential fields resolves or fails closed
  (`ConfigError`); no surviving `{{`/`${` literal reaches a credential store, and
  the placeholder string itself is never a valid credential.
- No `token_issuer`/`clients` fields, dead issuer code, or lying docs rows remain;
  `deny_unknown_fields` rejects stale configs loudly.
- Mixed-provider configs (e.g. keycloak + native) work with explicit per-route
  provider selection; single-provider configs are back-compatible without a
  `provider` key.
- gRPC routes honor declared `credential_sources` that gRPC metadata can carry
  (`header`, `authorization_header`); `query_param` and `cookie` sources on gRPC
  routes fail at route load with an error naming the source.
- All AGENTS.md quality gates pass.

## Risk budget

Accepted: breaking removal of never-consumed config fields and dead code across
camel-config (`token_issuer`, `clients`), camel-auth (issuer modules,
`ApiKeyAuthenticator`), camel-http (`auth` wrapper) — pre-1.0 window, ADR-0033;
`deny_unknown_fields` makes stale configs fail loudly. Accepted: internal shape
change of `SecurityCompileContext` for named providers (no trait changes); remote
JWKS fetch at startup for OIDC — fail-closed `ConfigError` when unreachable.
Out of bounds: redesigning camel-auth traits; turning a misconfigured secret into
an accepted credential (fail-closed is non-negotiable); merging without human
approval at the merge gate.
