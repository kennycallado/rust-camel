# rc-xb19 — Security config placeholder-resolution gap: escalation analysis

**Status:** research only (no code changed)
**Bug:** rc-xb19 (P1, security), confirmed E2E on v0.29.0
**Scope owner:** camel-config (resolution), camel-cli + camel-auth (authenticator boundary)

## Verdict

Choose the **hybrid**: keep the current field-by-field walk but replace ad-hoc field
enumeration with an explicit, exhaustive resolver that covers **every operator-supplied string
leaf that flows into runtime wiring** — routes, log_level, observability, platform, `[components]`,
beans, **and the missing high-risk credential sections `[security.*]` and `[datasources.*]`** —
while adding a **fail-closed rule for security/credential leaves** (missing env → hard error, never
"keep original") and an **independent boundary guard** in the authenticator builders that rejects
any credential still containing an unresolved `{{` marker. A blind "resolve every string in
`CamelConfig`" walk (pure option b) is unsafe here because `[components.*]`, `[datasources.*].extra`,
and bean config are untyped `toml::Value` maps whose values can legitimately contain component-owned
`{{ }}` templating that is NOT camel-config's to expand (`config.rs:1117`, `config.rs:1605`,
`datasource.rs:35-39`). The maintainer's mental model ("`{{env:...}}` is automatic for any field")
was never true: resolution is an allowlist enumerated by hand in `resolve_placeholders`
(`config.rs:1068-1140`), and the gap is **drift-by-omission**, not an intentional security boundary.
The redaction ADR (ADR-0051) governs a *different* concern (Debug/Serialize disclosure) and does not
justify skipping resolution — resolution runs before the value ever reaches a redacting sink.

---

## 1. Full inventory: what resolves today vs what does not

Single resolution site: `CamelConfig::resolve_placeholders()` (`config.rs:1068`), called once at
load in `load_config` (`config.rs:1569`), before `validate()` (`config.rs:1571`). No other
resolution call site exists — `discovery.rs` and `commands/run.rs` do **not** re-resolve
(verified: `discovery.rs` only delegates to camel-dsl; `run.rs` has no `PropertiesResolver` use).
So the allowlist in `resolve_placeholders` is the whole truth.

| Config path | Resolved today? | Evidence (file:line) | Risk if unresolved |
|---|---|---|---|
| `routes[]` (route file paths) | YES | `config.rs:1071-1077` | low (paths) |
| `log_level` | YES | `config.rs:1079-1083` | none |
| `observability.otel.endpoint` | YES | `config.rs:1086` | low |
| `observability.otel.service_name` | YES | `config.rs:1087-1091` | none |
| `observability.otel.resource_attrs.*` | YES | `config.rs:1092-1095` | low |
| `observability.prometheus.host` | YES | `config.rs:1099` | low |
| `observability.health.host` | YES | `config.rs:1103` | low |
| `platform.namespace` (k8s) | YES | `config.rs:1107-1109` | low |
| `platform.lease_name_prefix` (k8s) | YES | `config.rs:1110-1114` | low |
| `[components.*]` raw TOML (recursive) | YES | `config.rs:1117-1123`, `resolve_toml_value_placeholders` `config.rs:1605-1624` | — |
| `beans.*.plugin` | YES | `config.rs:1126` | low |
| `beans.*.config.*` (values) | YES | `config.rs:1127-1138` | med (can hold secrets) |
| **`security.native.bearer_token`** | **NO** | field `config.rs:715`; absent from `resolve_placeholders` | **CRITICAL — literal ships as valid credential** |
| **`security.native.api_key`** | **NO** | field `config.rs:717` | **CRITICAL** |
| **`security.oidc.client_secret`** | **NO** | field `config.rs:645` | **CRITICAL** |
| **`security.keycloak.client_secret`** | **NO** | field `config.rs:753` | **CRITICAL** |
| `security.keycloak.server_url` / `realm` / `client_id` | NO | `config.rs:749-751` | med (endpoints) |
| `security.oidc.issuer` / `jwks_uri` / endpoints | NO | `config.rs:636-649` | med |
| `security.policies.wasm.*.config.*` | NO | `config.rs:924-926` | med (can hold secrets, e.g. `ldap_url`) |
| `security.permissions.*.config.*` | NO | `config.rs:896-897` | med |
| **`datasources.*.db_url`** | **NO** | `datasource.rs:16`; not in walk | **HIGH — connection string carries `user:pass@`** |
| `datasources.*.ssl_*` (paths) | NO | `datasource.rs:28-34` | low (paths, ADR-0051 metadata) |
| **`datasources.*.extra.*`** (e.g. SurrealDB `password`) | **NO** | `datasource.rs:35-39`; SurrealDB reads `password` here (surrealdb README:58,61) | **HIGH** |
| `languages.{rhai,js,minijinja}.*` | N/A | numeric limits only (`language_limits.rs:203-215`) — no string secret fields | none |
| `_extra` catch-all | NO | `config.rs:86-87` | n/a (unconsumed) |

**Net:** four CRITICAL credential leaves and two HIGH datasource leaves manufacture a guessable
credential or silently drop a secret. Every one is documented as a working `{{env:...}}` recipe
(see §3).

---

## 2. Design intent: intentional boundary or drift?

**Drift.** Evidence:

- The walk is a hand-maintained enumeration, not a policy. Each covered field is a separate
  `resolve_string_in_place(...)` call (`config.rs:1085-1114`). Adding a field means editing this
  function; nothing enforces coverage. Security was simply never added.
- Git history is squashed into one commit (`1b97307f`), so there is no incremental record of a
  deliberate "exclude security" decision. The absence is silence, not intent.
- ADR-0051 (`docs/adr/0051-...:12-59`) governs **disclosure at Debug/Serialize boundaries**, not
  resolution. Its `client_secret` / `bearer_token` redacting `Debug` impls (`config.rs:652-667`,
  `728-744`, `768-782`) run *after* the value is stored; they neither require nor benefit from the
  value staying a literal placeholder. Resolution and redaction are orthogonal: resolve first, then
  redact the resolved secret in any diagnostic.
- The crate's own `CONTEXT.md` calls camel-config "security-sensitive … it owns the `[security.*]`
  config shapes" — the shapes are owned here, but resolution coverage of them was omitted.
- The existing `dead-config-policy` spec (`openspec/specs/dead-config-policy/spec.md:6`) already
  bans "silently ignored config fields." A documented `{{env:...}}` that never resolves is exactly a
  silently-ignored field that, uniquely, degrades to a live credential. rc-xb19 is a
  dead-config-policy violation with a security blast radius.

Conclusion: no security boundary was intended by exclusion. The allowlist form is an accident of
incremental authoring.

---

## 3. Latent traps — documented-but-dead placeholders

Every credential doc below is a **working authentication-bypass recipe** today (literal string
becomes the credential because the resolver returns the input unchanged on a miss —
`properties.rs:112-116` only errors when there is no default, but for security fields the value is
stored verbatim through the config path that never calls resolve at all):

| Doc location | Placeholder shown | Actually resolves? |
|---|---|---|
| `crates/camel-config/README.md:334` | `client_secret = "{{env:OIDC_CLIENT_SECRET}}"` "# resolved from env" | **NO** (oidc.client_secret) |
| `crates/camel-config/README.md:357` | `bearer_token = "{{env:SYSTEM_BEARER_TOKEN}}"` | **NO** (the confirmed E2E bypass) |
| `crates/camel-config/README.md:396` | `client_secret = "{{env:KEYCLOAK_CLIENT_SECRET}}"` "# redacted in debug output" | **NO** (keycloak.client_secret) |
| `docs/src/configuration/schema.md:124` | `client_secret = "{{env:KC_SECRET}}"` | **NO** |
| `docs/src/configuration/schema.md:391` | `api_key = "{{env:API_KEY}}"` | **NO** (native.api_key) |

The `d758c859` roadmap/status entries ("`{{env:FOO}}` placeholder resolution") refer to **bean
config only** (`docs/roadmap.md:27`, `docs/status.md:22`) — that path *does* resolve
(`config.rs:1127-1138`). The docs generalized a bean-scoped feature to "any field," which is the
maintainer's exact mistaken assumption.

No other config surface shows placeholders that silently die: `[components.*]` and bean config both
resolve. Route-template `{{param}}` (ADR-0008, `docs/adr/0008-...:3`) and language `{{ }}`
(minijinja/rhai) are a **different** substitution mechanism operating on route bodies / exchange
data, not on `Camel.toml` config strings — they must stay out of scope (see §6).

---

## 4. Failure semantics for missing env on a security field

Today, for security fields the value is stored **verbatim** (no resolve call), so a literal
`{{env:BEARER_TOKEN}}` becomes a low-entropy, attacker-*known* credential — the worst possible
outcome (repro: `Cookie: session={{env:BEARER_TOKEN}}` → 200). Even after we add resolution, the
resolver's "keep original on miss" contract (`resolve_string_in_place` `config.rs:1576-1586`;
`properties.rs:112-116`) would reproduce the same manufacture if the env var is unset.

**Decision: fail-closed for security + datasource credential leaves.** When a `[security.*]` or
`[datasources.*]` credential string contains an unresolved placeholder after resolution (env missing
and no default), `load_config` must return `ConfigError` and abort startup. This aligns with:

- ADR-0033 fail-closed startup validation (5-disposition policy; a missing required credential is an
  Intent-Declaration / Require-Explicit-Choice failure).
- The existing `NativeCredentialStore::try_new` precedent, which already hard-errors on an unset env
  var and on an empty secret (`native_auth.rs:76-91`).

**Non-security fields keep today's behavior** (resolve; on miss `warn!` + keep original —
`config.rs:1576-1586`). A missing `otel.endpoint` env should not crash the process; a missing bearer
token must. The split is by *field classification*, not by resolver mechanics.

Note on `{{key:default}}` and `{{env:VAR:default}}`: a security field with an explicit default
(`{{env:BEARER_TOKEN:changeme}}`) resolves to the default and does NOT trip the fail-closed guard —
that is an operator's explicit (if unwise) choice, consistent with ADR-0033 per-item escape hatches.
The guard fires only on a **still-unresolved** `{{` after resolution.

---

## 5. Boundary hardening (independent of resolution)

**Yes — add a fail-closed guard in the authenticator/realm builders**, as defense-in-depth that does
not depend on camel-config getting resolution right.

- `native_authenticator` (`camel-cli/src/lib.rs:160-183`) takes `native.bearer_token` and feeds it
  as `NativeCredentialSecret::Plaintext` into `NativeCredentialStore::try_new`. The store already
  validates non-empty (`native_auth.rs:87-91`) — add a check that the secret does not contain `{{`
  and reject with `CamelError::Config`. Best placed in `try_new` (`native_auth.rs:71`) so **all**
  callers inherit it, not just the CLI.
- `keycloak_authenticator` (`lib.rs:185-194`) and `register_keycloak_uma_evaluator`
  (`lib.rs:267-286`) pass `keycloak.client_secret` into `with_client_secret`. Add the same
  `{{`-marker rejection before construction.
- oidc.client_secret is currently unused by `resolve_authenticator` (`lib.rs:237-263` returns `None`
  for oidc-alone), so it has no live boundary yet; the guard should land wherever oidc wiring is
  added, and the resolution fix (§7) covers the config surface regardless.

**False-positive risk:** a legitimate credential literally containing `{{` is implausible for
bearer tokens / client secrets (base64url / hex / opaque provider strings never contain `{{`). The
guard checks the raw two-char marker `{{`, which is the placeholder open-delimiter
(`properties.rs:84`); the risk is negligible and the fail-closed direction is correct. Cost is ~2
lines per builder. Benefit: even if a future config field is added and someone forgets the resolver
allowlist again, the credential can never silently authenticate.

---

## 6. Systemic-resolution risks (why not blind option b)

Values that must **NOT** be blindly resolved by camel-config:

1. **`[components.*]` raw `toml::Value`** (`config.rs:1117`, `datasource.rs`-style untyped maps).
   Already resolved today *at the config layer*, but a full recursive walk over *new* sections must
   preserve the "component owns its option parsing" contract (CONTEXT.md: ComponentsConfig
   "deliberately untyped"). Safe because `resolve_toml_value_placeholders` only replaces `{{...}}`
   spans and leaves non-placeholder braces intact.
2. **Component/language internal `{{ }}` templating.** MiniJinja and Rhai use `{{ }}` and `{% %}`,
   but those templates live in **route bodies and exchange data**, never in `Camel.toml` string
   leaves. A `Camel.toml` value is operator config; a minijinja template body is authored in route
   YAML or template files (ADR-0047), a different load path. So there is **no** double-templating
   collision at the config layer — but this is precisely why we must scope resolution to
   `CamelConfig` string leaves and not extend it into route-body content.
3. **Double-resolution / already-resolved braces.** The resolver is idempotent on non-placeholder
   text (`test_resolve_no_placeholders`, `properties.rs:153-159`) and passes through unclosed `{{`
   (`test_resolve_unclosed_placeholder_passthrough`, `properties.rs:180-187`). A resolved secret
   that happens to contain `}}` is not re-scanned because resolution runs exactly once at load
   (`config.rs:1569`); there is no second pass.
4. **`{{key:default}}` named property sources.** `PropertiesResolver` supports named sources via
   `set()` (`properties.rs:58-63`), but no `application.properties`-style file loader is wired today
   (CONFIG-015/016/017 TODOs, `properties.rs:28-37`). So named-source keys other than `env:` resolve
   only from in-process `set()` calls, of which there are none in the load path. This is orthogonal
   to rc-xb19 and should not expand in this change.

Because of (1)–(2), the correct shape is **explicit exhaustive coverage of typed `CamelConfig`
string leaves** (add security + datasources to the existing walk), not a reflective "resolve every
string" that could reach into untyped component/template content with different escaping rules.

---

## 7. Recommendation & OpenSpec scope

**Hybrid = explicit exhaustive resolution of security + datasource leaves, fail-closed on unresolved
credential leaves, plus an independent authenticator boundary guard.**

Affected capabilities (existing specs to delta): `security`, `dead-config-policy`, and cross-ref
`trust-boundary-validation` / `credential-lint`. No new capability needed.

### Files touched
- `crates/camel-config/src/config.rs` — extend `resolve_placeholders` (`:1068`) to cover
  `security.*` credential + endpoint leaves and `datasources.*.db_url` + `datasources.*.extra.*`;
  add a classified fail-closed path for credential leaves; possibly a small
  `resolve_security_in_place`/`resolve_credential_leaf` helper mirroring
  `resolve_string_in_place` but returning `Err` on unresolved `{{`.
- `crates/services/camel-auth/src/native_auth.rs` — `NativeCredentialStore::try_new` (`:71`):
  reject any secret containing `{{`.
- `crates/camel-cli/src/lib.rs` — `keycloak_authenticator` (`:185`) and
  `register_keycloak_uma_evaluator` (`:267`): reject `client_secret` containing `{{`.
- Docs: `crates/camel-config/README.md:334,357,396`, `docs/src/configuration/schema.md:124,391` —
  keep the `{{env:...}}` examples (now true), and add the "missing env → startup fails" note.

### Task-level breakdown (suitable for tasks.md)

- **T1 — Resolve `[security.*]` string leaves.** Extend `resolve_placeholders` to walk
  `security.native` (bearer_token, api_key, subject, issuer), `security.oidc`
  (client_secret, issuer, jwks_uri, endpoints), `security.keycloak` (client_secret, server_url,
  realm, client_id), `security.policies.wasm.*.config.*`, `security.permissions.*.config.*`.
  Non-credential endpoint leaves use warn-on-miss; credential leaves use T3 fail-closed.
- **T2 — Resolve `[datasources.*]`.** Resolve `db_url` and recurse `extra` via
  `resolve_toml_value_placeholders`; `db_url` and known password keys are credential leaves (T3).
- **T3 — Fail-closed credential classification.** Add a resolver variant that returns
  `ConfigError` when a *credential* leaf still contains `{{` after resolution (env missing, no
  default). Wire into `load_config` so startup aborts. Non-credential leaves keep warn+keep-original.
- **T4 — Authenticator boundary guard.** In `NativeCredentialStore::try_new`, reject secrets
  containing `{{` (mirrors the existing empty-secret rejection, `native_auth.rs:87`). In
  `keycloak_authenticator` / `register_keycloak_uma_evaluator`, reject `client_secret` with `{{`.
- **T5 — Docs correction.** Update README + schema.md placeholders with the fail-closed note;
  ensure `dead-config-policy` framing is cited.
- **T6 — Regression tests** (all must be new, executable):
  - `security_native_bearer_token_resolves_from_env`: set env, assert stored token == env value.
  - `literal_placeholder_never_authenticates`: config with `bearer_token = "{{env:X}}"` and `X`
    unset → `load_config` returns `Err` (fail-closed); asserts the literal is never a live
    credential. (Directly encodes the rc-xb19 repro.)
  - `missing_env_for_bearer_token_fails_closed`: assert `ConfigError`, not warn.
  - `keycloak_client_secret_resolves_from_env` + `_fails_closed_when_missing`.
  - `oidc_client_secret_resolves_from_env`.
  - `datasource_db_url_resolves_from_env`.
  - `native_credential_store_rejects_brace_marker`: `try_new` with `{{`-bearing plaintext → `Err`.
  - `security_field_with_explicit_default_uses_default`: `{{env:X:fallback}}` unset → `fallback`,
    no error (ADR-0033 escape hatch).
  - `component_and_template_bodies_untouched`: a `[components.*]` value and a route-body `{{ }}`
    are not double-resolved / not broken by the config walk.

### Spec deltas
- `security/spec.md`: ADD requirement "Security credential config fields resolve `{{env:...}}` and
  fail closed on unresolved credential placeholders," with the three scenarios (resolves,
  fails-closed-on-missing, default-honored).
- `dead-config-policy/spec.md`: ADD scenario "documented `{{env:...}}` in a security field must
  resolve or fail — never ship as a literal credential" (this is the policy's security instance).
- Cross-reference `trust-boundary-validation` and `credential-lint` (no delta required unless the
  team wants a `camel lint` rule flagging a literal-looking secret in `[security.*]`, which the
  RSecret rule already conceptually covers for routes — see
  `openspec/changes/archive/2026-08-10-add-camel-lint/tasks.md:214`).

---

## Open questions for the human

1. **oidc live wiring.** `resolve_authenticator` returns `None` for oidc-alone (`lib.rs:260-262`).
   Should T4's oidc boundary guard block on wiring oidc authentication (out of scope here), or land
   the config-layer resolution now and defer the oidc *builder* guard until oidc is wired?
2. **Credential-leaf classification source of truth.** Encode the credential-leaf set as an explicit
   list in `resolve_placeholders`, or introduce a small marker (e.g. a `#[credential]`-style
   convention / newtype) so future fields inherit fail-closed automatically? The latter is more
   future-proof but larger; the former matches the current hand-enumerated style.
3. **`datasources.*.extra` password keys.** SurrealDB reads `password` from `extra`
   (`datasource.rs:37-38`, surrealdb README:61). Treat *all* `extra` values as warn-on-miss, or
   classify a known key set (`password`, `secret`, `token`) as fail-closed credential leaves?
4. **Scope of endpoint-leaf resolution.** Do we also want `keycloak.server_url` / `oidc.issuer`
   resolvable from env (operator convenience, currently dead), or keep this change strictly to
   credential leaves to minimize blast radius?

---

# Follow-up (same session): the `:-` default-separator confusion

## Verdict

The maintainer's belief — that `Camel.toml` uses `{{env:VAR:-default}}` (shell/Spring `:-`) and
"nothing else" — is a **conflation of two separate, independently-implemented interpolation systems
with different syntax and different failure semantics**. The `:-` shell-style separator is real, but
it belongs to **camel-dsl route-file interpolation** (`crates/camel-dsl/src/env_interpolation.rs:8`,
regex `\$\{env:([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}`, `${env:VAR:-default}`, **fail-closed** on
missing — `:36-64`). The **camel-config `Camel.toml`** resolver (`PropertiesResolver`,
`properties.rs`) uses a **completely different** syntax: `{{env:VAR:default}}` with a **single colon**
and no `${…}` wrapper, born that way in `d758c859` ("Supports `{{env:FOO}}` and `{{env:FOO:default}}`
syntax matching Apache Camel convention"). There was **no `:-` → `:` migration** in camel-config; the
single-colon form is original and unchanged (git log of `properties.rs` shows only additive commits:
`d758c859` introduced env resolution, `1ea3670f` added newline sanitize; no separator change). I
verified the runtime behavior by extracting and executing the exact parse logic: with the var unset,
`{{env:VAR:-default}}` silently resolves to the corrupted literal **`-default`** (leading dash
retained), with **no warning** — and when the var IS set, the `:-` is silently ignored and the real
value is used, which is *why the maintainer never noticed*. This is a third silent-config trap of the
**same dead-config-policy family** as rc-xb19: a placeholder that "looks right" but produces a wrong,
guessable value. It should be **folded into the rc-xb19 fix scope**, because for a *security* leaf the
combination is dangerous: once T1 adds security resolution, `bearer_token = "{{env:X:-changeme}}"`
with `X` unset would resolve to `"-changeme"` — a corrupted-but-guessable credential — unless the
fix also fail-closes on `:-`-style input.

## 1. Syntax history

- `git log --all -- crates/camel-config/src/properties.rs`: touched by `1b97307f` (squash),
  `1ea3670f` (`feat(sec): batch8 config/dsl/log hardening` — added `sanitize_env_value`),
  `d758c859` (`feat(camel-config): add {{env:FOO}} placeholder resolution in bean config`), and
  earlier audit sweeps. No predecessor file (`git log --diff-filter=D '*properties*'` empty). No
  commit ever contained `:-` in camel-config (`git log -S':-' -- crates/camel-config/` empty).
- **`d758c859` commit body (verbatim):** "PropertiesResolver.resolve() now handles env: prefix …
  Supports `{{env:FOO}}` and `{{env:FOO:default}}` syntax matching Apache Camel convention." The diff
  shows the single-colon parse `env_key.find(':')` → `(&inner[..4 + colon], Some(&env_key[colon+1..]))`
  landing in that commit and unchanged since (`properties.rs:90-101`). **Single-colon was the design
  from birth; `:-` was never supported in `Camel.toml`.**

## 2. Today's actual behavior with `{{env:VAR:-default}}` (verified, not traced-by-eye)

Parse (`properties.rs:90-101`): inner = `env:VAR:-default` → `strip_prefix("env:")` →
`env_key = "VAR:-default"` → `env_key.find(':')` = index 3 → `key = &inner[..4+3] = "env:VAR"`,
`default = &env_key[4..] = "-default"`. Then `lookup("env:VAR")` → `env::var("VAR")` (the `strip_prefix`
in `lookup` `properties.rs:66-72` strips `env:` and `.trim()`s to `VAR`). I compiled and ran the exact
resolver logic:

```
{{env:MY_TOKEN:-default}}  (unset) => Ok("-default")     ← silent corruption, NO warning
{{env:MY_TOKEN:default}}   (unset) => Ok("default")      ← intended single-colon form works
{{env:MY_TOKEN}}           (unset) => Err(MissingKey)    ← no default → error path
{{env:MY_TOKEN:-default}}  (set)   => Ok("realsecret")   ← :- ignored; real value used
```

So the trap is **worse than rc-xb19's "keep whole literal"**: it neither errors nor preserves the
literal — it manufactures `"-default"`, a value the operator did not write and did not intend. It is
silent because `resolve_string_in_place` only warns on `Err`, and this path returns `Ok`
(`config.rs:1576-1586`). For a numeric field like `period=${…}` this yields a parse error downstream;
for a *string credential* it yields a live wrong secret.

Confirmation cost was cheap (standalone `rustc` of the extracted function); no need to spin the full
`cargo test -p camel-config`. The logic is a byte-for-byte copy of `properties.rs:80-127`.

## 3. Where the belief came from

The `:-` convention is **documented and correct — but only for route files (camel-dsl)**, and the
maintainer generalized it to `Camel.toml`:

| Doc | Shows | System | Correct there? |
|---|---|---|---|
| `crates/camel-dsl/README.md:23,496,505-511` | `${env:VAR:-default}`, "`:-` follows shell parameter expansion" | camel-dsl route files | **YES** — matches `env_interpolation.rs:8` |
| `docs/src/configuration/env-interpolation.md:13` | `${env:VAR:-default}` | camel-dsl route files | YES |
| `examples/env-interpolation/routes/routes.yaml:6,34,36-37` | `${env:VAR:-default}` in a `.yaml` route | camel-dsl route files | YES |
| `crates/camel-config/README.md:334,357,396`; `docs/src/configuration/schema.md:124,391` | `{{env:VAR}}` single-brace-colon | **camel-config `Camel.toml`** | syntax right, but **dead** (rc-xb19) |

**No doc anywhere shows `{{env:VAR:-default}}`** (double-brace + `:-`). The maintainer fused the
route-file `:-` semantics with the `Camel.toml` `{{…}}` delimiter into a form that exists in neither
system. Ecosystem baseline confirms two legitimate conventions collided in his memory: **Apache Camel**
uses `{{key:default}}` (single colon — what camel-config copied), **Spring** uses `${var:default}`,
**shell** uses `${var:-default}` (what camel-dsl copied for `${env:…}`). rust-camel legitimately runs
Camel-style in `.toml` and shell-style in route files.

## 4. Canonical-syntax recommendation

**Option (b), scoped to fail-closed detection — fold into rc-xb19.** For `Camel.toml`
(`PropertiesResolver`):

- **Keep `{{key:default}}` / `{{env:VAR:default}}` single-colon** as the canonical `Camel.toml` form
  (Apache-Camel-aligned, already documented, already tested `properties.rs:161-239`). Do **not** add
  `:-` as a second accepted separator in config — two separators in one system invites exactly this
  confusion, and `:-` collides with legitimate default values that begin with `-`.
- **Fail-closed on the ambiguous form.** When a default segment begins with `-` (i.e. the operator
  wrote `:-`), the resolver cannot tell "shell-style separator, strip the dash" from "the default
  literally is `-foo`". Rather than guess, **reject** with a `ConfigError` that names the field and
  says: "`Camel.toml` uses `{{env:VAR:default}}` (single colon); `:-` is route-file syntax." This
  converts a silent corruption into an actionable startup error and teaches the correct form. This is
  the same fail-closed disposition as rc-xb19's credential path (ADR-0033), applied to *all* fields
  (not just security) because the corruption is never intended anywhere.
- **Do not** try to unify the two systems' syntax in this change — that is a larger,
  behavior-breaking design question (route files already ship `${env:…:-…}` in examples and users'
  route YAML). Unification is a separate proposal, not part of a P1 security fix.

**Fold vs separate:** fold into rc-xb19. The `:-` trap and the security-resolution gap share one root
(camel-config placeholder resolution has silent-miss semantics) and one fix surface
(`resolve_placeholders` + the credential-leaf fail-closed path). Shipping the security fix without the
`:-` guard would leave `bearer_token = "{{env:X:-changeme}}"` resolving to `"-changeme"` — a new
guessable-credential path introduced *by the very change that closes rc-xb19*. They must land together.

## Does this change the earlier 6-task recommendation?

**Amends, does not replace.** The hybrid verdict, the credential-leaf inventory, the fail-closed
disposition, and the authenticator boundary guard all stand. Adjustments:

- **T3 (fail-closed classification) — extend.** Add: the resolver rejects a placeholder whose default
  segment begins with `-` (the `:-` shell form) with a `ConfigError` naming the field and the correct
  single-colon syntax. This applies to **all** fields, not only credential leaves, because
  `"-default"` corruption is never intended. Credential leaves additionally fail-closed on unresolved
  `{{` per the original T3.
- **T4 (authenticator guard) — extend the marker check.** `NativeCredentialStore::try_new` and the
  keycloak builders should reject secrets containing `{{` **and** should not be reachable with a
  `-`-corrupted value, since T3 now errors before construction. The `{{` check remains the
  defense-in-depth backstop (`native_auth.rs:71`).
- **T5 (docs) — add a syntax-boundary note.** Explicitly document in `crates/camel-config/README.md`
  and `docs/src/configuration/schema.md` that **`Camel.toml` uses `{{env:VAR:default}}` (single
  colon)** and that **`:-` is route-file (`${env:…}`) syntax only** — the one sentence that would have
  prevented this. Cross-link `crates/camel-dsl/README.md:496-511`.
- **T6 (tests) — add three:**
  - `config_placeholder_shell_default_separator_rejected`: `{{env:X:-default}}` with `X` unset →
    `load_config` returns `Err` naming the field and the single-colon form (replaces the current
    silent `Ok("-default")`).
  - `config_placeholder_single_colon_default_honored`: `{{env:X:default}}` unset → `"default"`
    (guards against over-broad rejection breaking the canonical form).
  - `security_bearer_token_shell_separator_fails_closed`: `bearer_token = "{{env:X:-changeme}}"`
    unset → `ConfigError`, never `"-changeme"` (proves the two fixes compose safely).
- **Spec deltas — extend `dead-config-policy`.** Add a scenario: "a `Camel.toml` placeholder using the
  route-file `:-` separator is rejected with an error, never silently coerced to a dash-prefixed
  default." Keep the security scenario from the original delta.

## New open question

5. **Two-system syntax convergence (out of scope, flag for roadmap).** Route files use
   `${env:VAR:-default}` (shell) and `Camel.toml` uses `{{env:VAR:default}}` (Camel). This split is
   defensible (each matches its ecosystem lineage) but is a documented foot-gun. Do we want a future
   proposal to unify — and if so, toward which convention — or do we accept the split and rely on the
   T5 doc note plus the T3 fail-closed guard to prevent cross-application? Not part of the P1 fix.

---

## Proposal evaluation: `${env:}` unification

**Second follow-up — adversarial evaluation of the maintainer's endgame:** replace `{{…}}` in
`Camel.toml` entirely with the camel-dsl route-file syntax `${env:NAME:-default}`, then make GLOBAL
resolution (every string leaf, no allowlist) safe because the `{{` collision concern disappears.

### Verdict

**Adopt the proposal — with high confidence (≈0.8) — as the correct endgame, but stage it in two
landings, not one.** The collision audit vindicates the core claim: **zero** `Camel.toml` files in the
entire repo use `${…}` as a config value today, and **zero** use `{{…}}` either (verified below), so
the migration cost is near-nil and the "`${1}` regex group-ref" hazard Conductor flagged does not
exist at the config layer. The proposal is strictly better than the amended hybrid on **completeness**
(global walk kills the whole dead-config class, so no future section is ever forgotten again — the
exact failure mode that produced rc-xb19) and on **doc coherence** (one interpolation syntax across
routes AND config, reusing the *same already-`pub` function*). It is worse on nothing material. The
only reason to stage rather than land atomically is **fail-closed blast radius**: importing DSL's
fail-closed-on-missing semantics to *all* config leaves is correct for security but changes behavior
for optional non-security fields, and the P1 security bug (rc-xb19) should not wait on a
syntax-migration deprecation window. So: **Landing 1** = the amended hybrid (close rc-xb19 now on the
existing `{{}}` parser, fail-closed secrets, `:-` rejection, store guard). **Landing 2** = the
`${env:}` unification + global walk as a deliberate, spec'd migration. The hybrid is not thrown
away — it is the security-critical subset that ships first and whose tests carry forward.

Decisive enabling fact: **camel-config already depends on camel-dsl** (`crates/camel-config/Cargo.toml:21`,
`camel-dsl.workspace = true`) and `interpolate_env` is already `pub`
(`crates/camel-dsl/src/env_interpolation.rs:36`, `pub mod env_interpolation` in `lib.rs:10`). The
unification is not "write a new parser" — it is "call the route-file resolver over config string
leaves." One resolver, one syntax, one fail-closed contract, zero new regex.

### 1. Collision audit for `${` (config-reachable surfaces)

Grepped `${` across `crates/`, `docs/`, `examples/` for anything that could appear as a **value in a
`Camel.toml` section**. Result: every real `${…}` usage lives in **route YAML, component URIs, or SQL
templates — never in a `.toml` config value.**

| `${…}` form | Where it appears | Reaches `Camel.toml` values? | Prefix-gating (`${env:`) avoids it? |
|---|---|---|---|
| `${env:VAR:-default}` | route YAML (`examples/env-interpolation/routes/routes.yaml:34-37`), DSL README (`camel-dsl/README.md:505-508`) | no (route files only) | n/a — this IS the target prefix |
| `${body}`, `${header.X}` | Simple-language exprs in route YAML / logs (`examples/container-nginx/routes/route.yaml:5-31`, `camel-cxf/README.md:71`, `camel-bench/.../forced_experiment.rs:185`) | **no** — Simple exprs are route-step values, not config | yes (no `env:` prefix) |
| `${file:name}`, `${file:name.noext}` | file-component URI options in routes (`camel-file/src/poll_logic.rs:644-645`, `examples/file-p1-features/src/main.rs:52`) | **no** — endpoint URI params, not `Camel.toml` | yes |
| `:#${expr}`, `:#${body.id}` | SQL parameter templates in route steps (`camel-sql/src/query.rs:24,668`, `camel-sql/README.md:184`) | **no** — SQL query strings in routes | yes |
| `${1}`, `${name}` regex group refs | **not found** anywhere in config or component options (`rg '\$\{1\}|\$\{name\}'` over `crates/components/` empty) | **no** — hypothetical | yes (moot; none exist) |
| `${ENV:default}` | ADR prose only (`docs/adr/0036-bridge-ipc-mtls.md:34`) | no (documentation) | yes |
| `${…}` in a `.toml` file | **one** hit total: `examples/env-interpolation/Cargo.toml:3` — a *Cargo* package description string, not a `Camel.toml` value | **no** | yes |

**Conclusion:** prefix-gating on `${env:` fully avoids every real collision. Conductor's `${1}`
regex-group-ref hazard is **not present** in the codebase's config surface. The `${body}`/`${file:…}`
forms are the reason global config resolution must **prefix-gate**, but they never appear in
`Camel.toml` today, so even an ungated walk would not corrupt existing configs — gating is
defense-in-depth for the future, and it is free (the DSL regex already gates on `env:`,
`env_interpolation.rs:8`).

**Escape hatch:** because the resolver only fires on the literal prefix `${env:`, a config value that
legitimately needs the string `${env:FOO}` (e.g. passed through to a component that does its own
interpolation) needs an escape. The DSL resolver has **no escape today** (its regex greedily matches
`${env:…}`). This is a genuine gap the unification must close: adopt a `$${env:…}` → literal `${env:…}`
escape (doubled `$`), matching common convention. **Flag:** this is a new requirement the DSL side
does not currently satisfy, so unification slightly expands the DSL resolver contract (add
`$$`-escape), which must be spec'd and tested on both surfaces to keep them identical. Low effort,
but it is real scope — not "free reuse."

### 2. Migration math

- **`{{` in `.toml` files, whole repo:** `rg -l '\{\{' -g '*.toml'` → **0 files.** No example, test
  fixture file, or user config on disk uses `{{`.
- **`{{env:` in user-facing docs:** **3 files** — `crates/camel-config/README.md` (lines 334, 357,
  396), `docs/src/configuration/schema.md` (124, 391). **Every one is a dead rc-xb19 placeholder that
  never resolved** (security fields). So the "migration" of docs is the *same edit* rc-xb19 already
  requires (T5).
- **`{{` inside Rust (inline test fixtures + resolver tests):** `config.rs:2255-2264` (a load-config
  test using `{{env:…:default}}`) and `properties.rs:135-318` (the resolver's own unit tests). These
  are internal and move with the parser change; they are not a user-migration cost.
- **External deployment (`examples/credential-sources`, the credential-sources reporter):** its
  `Camel.toml` uses **no** placeholders (verified: `examples/env-interpolation/Camel.toml` and the
  credential-sources example carry plain values; interpolation is in route YAML only). A hard error on
  `{{` would **not** break it.

**Recommendation — hard-error on `{{env:` in `Camel.toml` at Landing 2, no multi-release warn window.**
Justification: (a) pre-1.0 (`0.x`), the project's stated posture; (b) the syntax being removed has
*zero on-disk usage* and its only doc occurrences are broken security examples; (c) a silent
warn-then-keep would perpetuate the rc-xb19 "value silently wrong" failure class the whole change
exists to kill. The hard error must be *actionable*: "`Camel.toml` placeholders use `${env:NAME}` /
`${env:NAME:-default}`; `{{…}}` is no longer supported — see <link>." A one-release
deprecation *warning* is acceptable insurance if the team wants it, but it is not required by the
evidence. Do **not** silently accept both — that recreates the two-syntax foot-gun.

### 3. Proposal vs amended hybrid

| Dimension | Amended hybrid (`{{}}`, allowlist+security+datasources) | `${env:}` unification (global walk) |
|---|---|---|
| **Risk (now)** | Low — touches only enumerated leaves; parser unchanged | Medium — global walk + fail-closed import + new `$$`-escape; larger surface |
| **Effort** | Moderate — enumerate security/datasource leaves, classify credentials | Higher — parser swap, global walk, deprecation path, escape hatch, dual-surface tests |
| **Completeness** | **Leaves the class alive** — a future `[foo].secret` is forgotten again unless someone edits `resolve_placeholders` | **Kills the class** — every string leaf resolves; no allowlist to forget |
| **User surprise** | Two syntaxes forever (`{{}}` config, `${}` routes) | One syntax everywhere; matches the DSL docs users already read |
| **Doc coherence** | Must forever explain the split (my T5 note) | Single interpolation chapter; `camel-dsl/README.md:485-511` becomes the config story too |
| **Security posture** | Fail-closed on credential leaves only | Fail-closed uniformly (stronger, but see §4 blast radius) |
| **Time-to-close-rc-xb19** | **Immediate** | Gated on migration design |

The hybrid's one structural weakness is decisive for the long term: **it does not fix the class.**
rc-xb19 happened because `resolve_placeholders` is a hand-maintained allowlist (`config.rs:1068-1140`);
the hybrid keeps that allowlist and just adds two more entries. The next security-bearing config
section added by a contributor who does not read this audit will be dead again. The unification's
global walk is the only option that makes "forgot to add the field to the resolver" structurally
impossible.

### 4. Fail-closed semantics import

- **DSL exact behavior** (`env_interpolation.rs:36-64`, verified): `${env:VAR}` missing → `Err(var_name)`
  → `DiscoveryError::Env` (`discovery.rs:231`), the literal is **not** left in place. `${env:VAR:-def}`
  missing → uses `def`. `${env:VAR:-}` missing → empty string. Present → env value, default ignored.
  This is strictly fail-closed on missing-without-default.
- **Config today** (`properties.rs:109-117`): `{{env:VAR}}` missing-no-default → `Err(MissingKey)`
  (same disposition!), but `resolve_string_in_place` (`config.rs:1576-1586`) **swallows the Err with a
  warn and keeps the original** — that downgrade is the actual rc-xb19 mechanism, not the resolver.
  So importing DSL fail-closed = *stop swallowing the Err at the call site*, which is the same fix the
  hybrid's T3 already makes for credential leaves.
- **Where global fail-closed-on-missing is WRONG:** optional non-security fields where an operator
  commented-out or unset var should fall back to the struct default, not crash. Candidates:
  `observability.otel.endpoint` (has a default `config.rs:991`), `platform.namespace` (Option),
  `prometheus.host`/`health.host` (defaults). Under global fail-closed, `endpoint = "${env:OTEL:-http://localhost:4317}"`
  is fine (default present) but `endpoint = "${env:OTEL}"` with `OTEL` unset would now **abort
  startup** where today it warns. That is a behavior change for non-security fields.
  **Mitigation:** require a default for non-security leaves *by convention in docs*, but keep
  fail-closed uniform — because "env var referenced but unset and no default" is a genuine operator
  error everywhere, not just in security. The DSL already treats it as fatal in routes; making config
  match is *more* coherent, not less. Net: uniform fail-closed is defensible; the only real cost is
  that a few doc examples must show `:-default`.
- **Existing tests/examples broken by global fail-closed:** `properties.rs:205-212`
  (`test_resolve_env_missing_without_default_returns_error`) already asserts the Err — **consistent**.
  No on-disk example uses `{{env:MISSING}}` expecting passthrough (the passthrough only happens via the
  `resolve_string_in_place` swallow, which no example exercises). So the import breaks **no** example
  or integration test; it only changes the internal call-site disposition.

### 5. Scope for OpenSpec

**Recommended: two OpenSpec changes.**

**Change A — `close-rc-xb19-security-placeholder` (ship now, the amended hybrid).** Exactly the
6 tasks in this document as amended by the first follow-up (T1 security-leaf resolution, T2 datasource,
T3 fail-closed + `:-` rejection, T4 store guard, T5 docs, T6 tests). Unblocks the P1 security bug on
the existing parser. No syntax migration.

**Change B — `unify-config-interpolation-on-env` (roadmap, larger).** New change:
- **B1 — Reuse the DSL resolver in config.** Replace `PropertiesResolver` placeholder scanning with a
  call to `camel_dsl::env_interpolation::interpolate_env` (already `pub`, already a dependency
  `Cargo.toml:21`) over each config string leaf, or lift `interpolate_env` into a shared crate if the
  layering is wrong. One syntax: `${env:NAME}` / `${env:NAME:-default}`.
- **B2 — Global string-leaf walk.** Replace the hand-maintained allowlist (`config.rs:1068-1140`) with
  a recursive walk over all `CamelConfig` string leaves (typed fields + the untyped `toml::Value` maps
  via the existing `resolve_toml_value_placeholders` shape `config.rs:1605-1624`), prefix-gated on
  `${env:` so component/template `${body}`/`${file:…}`/`${1}` content is untouched.
- **B3 — `$$`-escape.** Add `$${env:…}` → literal `${env:…}` to the DSL resolver regex and honor it on
  both surfaces (routes + config). This is the one new contract requirement; spec + test on both.
- **B4 — Deprecation of `{{…}}` in config.** Hard-error on `{{env:` in `Camel.toml` with an actionable
  message pointing to `${env:}`. Optional one-release warn window if the team prefers.
- **B5 — Store guard carries forward** (T4) — unchanged; `{{`/unresolved-marker rejection in
  `NativeCredentialStore::try_new` and keycloak builders remains as defense-in-depth regardless of
  syntax.
- **B6 — Docs.** Collapse the two-syntax explanation into one interpolation chapter; update
  `crates/camel-config/README.md`, `docs/src/configuration/schema.md`, `docs/src/configuration/env-interpolation.md`
  to show `${env:}` for config too; remove the split note added in Change A's T5.
- **B7 — Tests:** rc-xb19 repro re-expressed in `${env:}` (`bearer_token = "${env:X}"` unset →
  fail-closed); the `-default` dash bug becomes impossible (no `:` ambiguity — `:-` is the *only*
  default form); `${1}`/`${body}` config-value passthrough (prefix-gating proof); `$${env:…}` escape;
  `{{env:…}}` in config → deprecation error; global-walk covers a *newly added* section without editing
  the walk (the anti-regression test that proves the class is dead).

**Spec deltas:**
- `dead-config-policy/spec.md`: Change B ADD requirement — "config placeholder resolution is exhaustive
  over all string leaves; no per-field allowlist" with a scenario proving a new section resolves
  without a code change. This is the capability that structurally closes the rc-xb19 class.
- `security/spec.md`: the credential fail-closed requirement (from Change A) is re-stated in `${env:}`
  terms by Change B; no semantic change, only syntax.
- New capability doc note (optional): a short `configuration-interpolation` spec if the team wants the
  one-syntax contract recorded as its own capability.

### New open questions / bd follow-ups to file

6. **File `unify-config-interpolation-on-env` as a bd feature** (roadmap, `discovered-from: rc-xb19`),
   priority P2, blocked-by the rc-xb19 security fix (Change A). This formalizes first-follow-up open
   question #5.
7. **Layering for the shared resolver:** is `camel-config → camel-dsl` the right direction to reuse
   `interpolate_env`, or should `interpolate_env` move to a lower shared crate (`camel-api`?) so both
   config and dsl depend downward? Current `camel-config.Cargo.toml:21` already imports camel-dsl, so
   reuse works today, but check `lint-publish-cycles` (ADR-0055) before committing to the direction.
8. **`$$`-escape on the DSL surface:** adding `$$` to `env_interpolation.rs:8` changes route-file
   behavior (a route value containing `$${env:…}` now un-escapes). Confirm no existing route YAML/URI
   relies on a literal `$$` (grep clean expected, but must verify before Change B).
9. **Uniform fail-closed on non-security config leaves:** accept the behavior change (missing env +
   no `:-` default → startup abort for e.g. `otel.endpoint`), or exempt fields that carry a struct
   default? Recommendation: uniform fail-closed (coherent with DSL, and "referenced but unset" is
   always an operator error), documented with `:-default` examples — but this is a call for the human.
