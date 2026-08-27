# Design: deployment-resolvable-cache-repo-topology

## Approach

Two mechanisms, one pipeline. Build order today (config.rs
`build_from_toml_value_inner`, :2443): profile merge → env overrides merged
into the TOML tree (:2505–2530) → `${env:}` placeholder expansion on tree
values (`resolve_tree_placeholders`, :2585) → deserialize into `CamelConfig`
(:2588) → `validate()` (:2595).

### FR1 — normalize empty topology values to absent

Add a private normalization step between deserialize and validate:

- `CacheRepoConfig` (:744) and `IdempotentRepoConfig` (:556) each gain
  `normalize_empty_topology(&mut self)` (or one free fn over both — implementer
  picks the shape matching crate style; both structs repeat the field set).
- String fields `url`, `master_name`, `username`, `sentinel_username`,
  `key_prefix`: `Some(s)` where `s.trim().is_empty()` → `None`.
- `sentinel_nodes: Option<Vec<String>>`: `Some(v)` where `v` is empty OR every
  entry trims empty → `None`. Mixed blank/non-blank arrays are NOT normalized
  — existing validation rejects them loudly.
- `db: Option<u16>` is typed; no empty representation exists.
  `password` / `sentinel_password` normalize blank → `None` (a blank value
  means unset and must not trip sentinel-only validation; non-blank
  credentials are never dropped — see proposal exclusions).
- Called from `build_from_toml_value_inner` immediately after deserialize,
  before `validate()`, and ONLY when the repo's `backend == "redis"` —
  unconditional normalization would legitimize `url = ""` on memory/redb
  sections that cross-backend validation rejects today (:1841, :1977).
  All loader entry points (sync, async, async-with-env, and hot-reload wiring)
  converge on this function, so one call site covers every file-based path.
  Scope is loader entry points only: programmatic construction and direct
  deserialization that bypass `build_from_toml_value_inner` are out of scope
  (spec requirement is scoped the same way).
- `validate_redis_topology_fields` (:871) itself is NOT changed: after
  normalization its `Option::is_some()` predicates see clean `None`s. Literal
  TOML `url = ""` gets the same treatment for free (parity with the
  `payload_dir` empty≡absent precedent).
- `key_prefix`: an explicitly empty prefix is invalid today (config.rs:964-968;
  keyspace validation), so `Some("")` → `None` selects the repository default
  prefix — a strict improvement, no working config can depend on the old
  behavior.

### FR2 — extend the env override allowlist

- Append 8 entries to `ALLOWED_ENV_OVERRIDES` (:2363). Names follow the
  existing `CAMEL_CACHE_REPO_<FIELD>` convention.
- **Empty scalar overrides are a no-op — scoped to the new vars only**: the
  merge loop skips a var whose raw value is the empty string ONLY when the
  var is one of the 7 new scalar overrides (const
  `EMPTY_SCALAR_ENV_OVERRIDES`; the 8th new var, `SENTINEL_NODES`, is CSV
  and not scalar). Pre-existing allowlisted vars keep today's semantics
  verbatim (e.g. `CAMEL_TIMEOUT_MS=""` still fails typed deserialization
  loudly) — no silent behavior change for existing deployments. An empty
  string must never reach the new vars' typed deserialization
  (`Option<u16>` for `db` would hard-fail on `""`). File/profile value stays
  effective.
- CSV coercion: only `CAMEL_CACHE_REPO_SENTINEL_NODES` uses a list kind — a
  per-var kind table (const list + helper producing `toml::Value::Array` of
  trimmed, non-empty strings; empty/all-blank input → empty array). An empty
  CSV override is DISTINCT from empty scalar: it replaces the field with `[]`,
  which FR1 normalization then maps to `None` — deliberate asymmetry
  (list-typed field can express "force unset"; scalars keep file values).
- Security shape unchanged: deny-by-default, L-C2 comment block (:2352–2366)
  stays authoritative; `STRICT_PREFIXES` continues to label `cache_repo`
  credential-bearing. `CAMEL_CACHE_REPO_URL`, `_USERNAME`, `_PASSWORD`,
  `_SENTINEL_USERNAME`, `_SENTINEL_PASSWORD` are absent from the allowlist and
  keep triggering the "not in override allowlist; ignored" warning.

### Docs

Guide configuration chapter (`docs/src/configuration/index.md`): table/note
with the new vars, CSV format, and the empty-scalar-vs-empty-CSV semantics.

## Affected crates

- `crates/camel-config`: normalization + allowlist + empty-scalar skip + CSV
  coercion + unit tests. No public API changes (`validate(&self)` signature
  untouched).
- `docs/src/configuration/index.md`: operator documentation.

## Architecture boundaries

Config crate is a leaf; the repository lowering boundary is
`camel-config::context_ext`, which lowers `CacheRepoConfig` into
`camel-redis-repo` / `camel-component-redis` (and redb/memory equivalents).
Those consumers are unchanged — they simply stop seeing `Some("")` for
topology fields. The env→tree merge stays pre-deserialize (existing boundary
between environment resolution and typed config), and normalization stays
post-deserialize (typed-domain concern). No new dependencies.

## Decisions

1. Normalize both repos, not just cache_repo: the topology validator is
   shared (:1762 idempotent, :2082 cache) and the env-parametrization problem
   is identical; a one-sided fix leaves the same trap one section over.
2. All-blank sentinel arrays normalize to `None` but mixed blanks stay an
   error: silent filtering of partially-blank lists would hide operator
   mistakes the validation exists to catch.
3. `sentinel_username` joins the credential exclusions (bd listed url /
   username / password / sentinel_password): it is credential-adjacent and
   unnecessary for the deployment model; when in doubt, the allowlist stays
   smaller.
4. Empty scalar override = no-op (scoped to the 7 new scalar vars), empty CSV
   override = force-unset: scalars cannot round-trip "" through typed fields
   without breaking deserialization, while list fields can express unset
   cleanly via `[]`. Pre-existing override vars keep their exact current
   behavior.
