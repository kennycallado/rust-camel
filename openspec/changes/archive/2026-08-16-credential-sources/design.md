# Design: credential-sources

## Approach

Two auth paths exist today and diverge:

- HTTP (pre-pipeline, ADR-0010): `SecurityPolicyLayer` wraps the pipeline.
  `RolePolicy`/`ScopePolicy::evaluate` call `authenticate()`
  (`camel-auth/src/built_in.rs:25`), which reads the token itself with a
  hardcoded `header_ic("authorization").strip_prefix("Bearer ")`. The policy
  owns extraction.
- WS/gRPC (component-level): the component extracts the token first
  (`extract_token_multi` in WS), then calls `policy.evaluate` on a synthetic
  exchange whose principal is pre-stored; `authenticate()` takes the
  `trust_upstream_principal` branch. The component owns extraction.

The change moves extraction behind the existing `extract_token_multi()` at the
`authenticate()` choke point, and threads a per-route source list from the DSL:

1. `RouteDslSecurityPolicy` (camel-dsl) gains `credential_sources:
   Option<Vec<CredentialSourceDsl>>`, defaulting at compile time to
   `[authorization_header]` when absent. The field threads through the
   `DeclarativeSecurityPolicy` `Roles`/`Scopes` variants (`model.rs`) —
   `Ref`/`Wasm`/`Permission` carry no sources and reject the key at
   conversion.
2. Compile maps the list onto both carriers: the `RolePolicy`/`ScopePolicy`
   constructors and `SecurityPolicyConfig` (camel-api contract) receive the
   same list.
3. `RolePolicy`/`ScopePolicy` carry the list; `authenticate()` calls
   `extract_token_multi` instead of the hardcoded prefix strip. First declared
   source with a present value wins.
4. `CredentialSource` gains `Header{name}` for API-key custom headers. The
   value flows into the same `StaticTokenAuthenticator` store lookup, so no
   new authenticator is needed.
5. camel-core `route_controller_trait.rs` builds the consumer
   `SecurityContext` with `with_credential_sources` instead of the
   `from_arc` default, activating the WS wiring.
6. HTTP diagnostic output never renders declared credential values: the
   error-context path reuses `redact_query_params` (extended to declared
   parameter names) and suppresses cookie/custom-header values; ADR-0051.
7. Short ADR: the two-path divergence (policy-owned vs component-owned
   extraction) is now load-bearing; document it and the first-match-wins
   precedence rule.

### HTTP extraction adapter

`extract_token_multi()` consumes header-map and query inputs. The HTTP path
holds an `Exchange` whose input message carries headers plus
`CamelHttpQuery` (camel-http `lib.rs:1431-1458`). Phase 1 adds a small
adapter that builds the extraction inputs from the exchange: header map from
input headers, query pairs from `CamelHttpQuery`, cookie values parsed from
the `Cookie` request header (`name=value` pairs, `;`-separated). All three
inputs are adversary-controlled (ADR-0032): malformed cookies, unparseable
query values, and missing names are treated as an absent source — the source
is skipped, and if every declared source misses, the result is
`Unauthenticated` mapped to 401. Malformed input never produces 500 and never
panics; parsing allocates bounded memory (no unbounded split).

### trust_upstream_principal semantics

Code truth today: `authenticate()` reads the token from the exchange
`Authorization` header; when no header is present, it consults the preloaded
`camel.auth.principal` exchange property only if the policy's
`trust_upstream_principal` flag is true (DSL default false — fail-closed).
On the component-owned path (WS), the component authenticates the extracted
token itself and calls `policy.evaluate` on a synthetic exchange that has no
`Authorization` header. The grant flows through the preloaded-principal branch
as of this change, after the serialization-format unification; before it, the
format mismatch made every WS `roles`/`scopes` + `trust` grant return 500
(latent since the WS path landed). Consequence: a WS route with `roles`/`scopes`
authenticates only when the route also declares `trust_upstream_principal: true`.

This change preserves that semantics and documents it: on component-owned
paths the flag means "accept the principal the component authenticated", and
the spoof caveat that applies to HTTP (an upstream filter could stamp the
property) does not apply, because the component calls `policy.evaluate` only
after successful token authentication. Two rules hold: the mechanism never
sets the flag implicitly — only the route YAML does — and a miss on every
declared source never reaches the trust branch (the component rejects before
evaluation). Rewiring the flag semantics (a separate verified-principal
channel) is out of scope.

### Load-time validation (ADR-0033)

Route loading rejects, with an error naming the field: an empty
`credential_sources` list; a cookie or query source with an empty `name` /
`param`; a custom-header source whose `name` is not a valid HTTP header
token; an unknown source form; and `credential_sources` attached to a
`security_policy` block that does not authenticate directly — concretely,
any block whose authenticating variants (`roles`, `scopes`) are absent,
including blocks that declare only `ref`, only `wasm`, or only `permission`
(`RouteDslSecurityPolicy` fields: `roles`, `scopes`, `ref`, `wasm`,
`permission`). `roles` and `scopes` authenticate via `authenticate()`;
`ref`, `wasm`, and `permission` are rejected because `ref` resolves to an
`Arc<dyn SecurityPolicy>` in the registry, which carries no
authentication-capability metadata, and adding such metadata would violate
the no-new-abstraction constraint. All rejection happens at load time,
never at request time.

### Constant-time comparison

Every source routes its extracted value through
`NativeCredentialStore::lookup` (`native_auth.rs:106-125`), which is
branchless constant-time over `max_len` with the length difference folded
in. Extraction differs per source; comparison does not. No new comparison
path is introduced.

### Redaction sinks (ADR-0051)

The sinks that exist today on the HTTP consumer path are the error-context
logs around `pipeline_error_to_reply` (`camel-http/src/lib.rs:2315`) and the
error reply body itself — camel-http has no request access log. The contract
is therefore written as redact-by-construction: no diagnostic record emitted
while handling a request may render a declared credential value (query, cookie,
custom header), and any access log added later inherits the same obligation.
Query rendering on the error path reuses `redact_query_params` (as WS does,
`camel-ws/src/lib.rs:427`) with the declared parameter names; the 401 reply
body carries no extracted value (the `Unauthenticated` message is generic).

### Compatibility of the Header variant

`CredentialSource` is a workspace-internal enum (not part of the published v1
contract surface under ADR-0049's scope). Adding `Header{name}` is a
source-compatibility break for exhaustive `match` sites; every in-workspace
match site is updated atomically in the same Phase 4 commit. Downstream
users outside the workspace are none (crate unpublished as stable contract).

## Affected crates

- camel-dsl: `route_ast.rs` (field + schema derive), `model.rs` (`Roles`/
  `Scopes` variants carry sources), `yaml.rs` (RouteDsl→Declarative
  conversion + load-time validation), `compile.rs` (two-carrier mapping).
- camel-api: `SecurityPolicyConfig` carries sources (contract change).
- camel-auth: `credential_source.rs` (`Header` variant + all match sites),
  `built_in.rs` (`authenticate()` refactor, policies carry sources, adapter).
- camel-core: `route_controller_trait.rs` (`with_credential_sources`).
- camel-component-http: error-context redaction (no request access log
  exists; the contract is redact-by-construction over the error path).
- camel-component-ws: consumes camel-core change; tests.
- camel-cli/xtask: schema regeneration (`cargo xtask schema`).

## Architecture boundaries

DSL → compile → camel-api contract → camel-auth service → components. Auth is
control-plane, pre-pipeline (ADR-0010); the data plane is untouched. Cookie,
query, and header values are adversary-controlled exchange data (ADR-0032).
Default stays header-only when the key is absent (ADR-0033). Diagnostic
redaction follows ADR-0051.

## Phases

### Phase 1: Contract + extraction refactor (no behavior change)

One disclosed exception to "no behavior change": routing the default
extraction path through `extract_token_multi` unifies HTTP Bearer parsing
with the pre-existing WS behavior per RFC 9110 — case-insensitive auth
scheme and whitespace tolerance. Acceptance widens only; no
previously-granted route changes outcome; fail-closed defaults hold. Pinned
by tests in camel-auth and recorded in ADR-0059.

- **Goal:** `credential_sources` flows `SecurityPolicyConfig` → policies →
  `authenticate()` via `extract_token_multi` + the HTTP adapter; default list
  is `[AuthorizationHeader]` everywhere.
- **Dependencies:** none (first phase).
- **Externally-visible types/interfaces:** `SecurityPolicyConfig` field;
  `RolePolicy`/`ScopePolicy` constructors gain a sources parameter.
- **Deliverable:** refactor; unit tests; divergence ADR draft.
- **Exit-criteria:** `cargo test -p camel-auth` green with new tests
  `authenticate_default_equals_bearer_prefix_strip`,
  `authenticate_header_source_reads_authorization`,
  `cookie_parse_malformed_is_absent_not_error`,
  `trust_false_preloaded_principal_unauthenticated`,
  `trust_true_preloaded_principal_fallback`; full
  `cargo test --workspace --lib` green (no behavior change anywhere else).

### Phase 2: DSL + schema + HTTP redaction

- **Goal:** YAML `security_policy.credential_sources` (authorization_header,
  query, cookie) reaches the HTTP path; redaction covers the new sources.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** `RouteDslSecurityPolicy`
  `credential_sources`, regenerated JSON schema.
- **Deliverable:** parser + compile + load-time validation + `cargo xtask
  schema` + HTTP redaction + integration tests in
  `camel-test/tests/http_test.rs`.
- **Exit-criteria:** `cargo test -p camel-dsl` green with new tests
  `load_rejects_empty_source_list`, `load_rejects_empty_cookie_name`,
  `load_rejects_ref_only_with_sources`,
  `load_rejects_wasm_only_with_sources`; `cargo test -p camel-test --test
  http_test --features integration-tests` green with `cookie_source_authenticates_img_request`,
  `cookie_miss_maps_401_not_500`, `multi_source_first_match_wins`,
  `default_absent_key_bearer_identical`; `cargo test -p camel-component-http`
  green with `error_context_redacts_cookie_sentinel`,
  `error_context_redacts_query_sentinel`; `cargo xtask schema --check`
  clean; `cargo xtask lint-secrets` clean; `cargo xtask lint-log-levels`
  clean.

### Phase 3: WS activation + docs

- **Goal:** WS resolves the same declared sources; operator docs complete
  for the Phase 2 source set.
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:** consumer `SecurityContext` built
  with `with_credential_sources` (camel-core).
- **Deliverable:** activation; new WS test file
  `crates/components/camel-ws/tests/credential_sources_test.rs`;
  `CONTEXT.md` updates (camel-auth, camel-http, CONTEXT-MAP key term);
  CSRF/SameSite/HttpOnly guidance; divergence ADR finalized. Spec delta is
  authored at plan time (this change's `specs/`) and validated with
  `openspec validate credential-sources --type change`.
- **Exit-criteria:** `cargo test -p camel-component-ws --test
  credential_sources_test` green with `ws_cookie_source_authenticates_with_
  explicit_trust` (route declares `trust_upstream_principal: true`),
  `ws_no_flag_rejects_even_with_valid_token` (fail-closed, mechanism never
  implies the flag), `ws_default_header_only_unchanged`;
  `cargo xtask lint-context-citations` clean; `openspec validate
  credential-sources --type change` passes.

### Phase 4: API-key custom header source

- **Goal:** `Header{name}` variant enables API-key style auth in pure YAML
  via `StaticTokenAuthenticator`.
- **Dependencies:** Phase 2 (extraction machinery + redaction machinery).
  Docs additions build on Phase 3 docs but are limited to the new form.
- **Externally-visible types/interfaces:** `CredentialSource::Header{name}`;
  DSL form `header: { name: X-API-Key }`.
- **Deliverable:** variant + extraction + DSL + schema regen + redaction of
  the named header + all in-workspace match-site updates in one commit;
  tests; `ApiKeyAuthenticator` documented as superseded for YAML use
  (programmatic API unchanged).
- **Exit-criteria:** `cargo test -p camel-auth` green with
  `header_source_authenticates_api_key`, `header_source_miss_maps_401`;
  `cargo test -p camel-dsl` green with
  `load_rejects_invalid_header_token_name`; `cargo test -p camel-test
  --test http_test --features integration-tests` green with `header_source_authenticates_api_key_http`,
  `error_context_redacts_custom_header_sentinel`; `cargo xtask schema
  --check` clean; `cargo xtask lint-secrets` clean.

## Alternatives considered

- Consumer-config per URI param: rejected — splits the security story across
  two DSL locations; auth is a route-security concern.
- WS-style per-path resolution as the HTTP mechanism: rejected —
  component-internal mechanism, not a DSL surface; HTTP's layer path needs the
  policy to carry sources.
- Exposing `ApiKeyAuthenticator` in the DSL: rejected — its store lookup is
  identical to `StaticTokenAuthenticator`; a `Header` source variant achieves
  the same result without a second authenticator surface.
