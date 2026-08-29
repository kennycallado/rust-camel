# Config

The configuration layer for rust-camel: `Camel.toml` (TOML) loading, profile and include
resolution, environment-variable override, route discovery (delegates to camel-dsl), and
property resolution. camel-config qualifies for a crate-local `CONTEXT.md` under the
CONTEXT-MAP.md coverage policy — it is user-visible (`Camel.toml` is the operator surface)
and security-sensitive (it owns the `[security.*]` config shapes).

## Language

Crate-specific vocabulary. Cross-cutting terms (Exchange, Message, ErrorHandler) live in
CONTEXT-MAP.md and are not repeated here.

**CamelConfig**:
Root config struct deserialized from `Camel.toml`. Profile-aware: fields live under
`[default]` / `[<profile>]` sections and are deep-merged with includes and `CAMEL_*` env
overrides. The runtime source of configuration truth.
_Avoid_: settings, app config, config file (when naming the Rust struct)

**ComponentsConfig**:
Raw per-component defaults keyed by component name (`http`, `kafka`, `jms`, ...), stored as
untyped `toml::Value` under `[components.*]`. Deliberately untyped at this layer: each
component bundle owns its own option parsing (`from_toml`), so camel-config never couples to
component option schemas.
_Avoid_: component settings, component map

**PropertiesResolver**:
Resolves `{{env:VAR}}` and file placeholders in config values. Public API; caching,
encoding, and multi-source chaining are partial (see CONFIG-015/016/017). Currently has no
external consumers.
_Avoid_: property loader, placeholder expander

**DiscoveryError** (camel-config):
Single-variant wrapper — `Dsl(#[from] camel_dsl::DiscoveryError)` — over
`camel_dsl::DiscoveryError`. A distinct type from the inner `camel_dsl::DiscoveryError`; it
exists to keep camel-config's public error surface independent of camel-dsl's error-enum
evolution. External callers match `DiscoveryError::Dsl(_)`, not the inner enum.
_Avoid_: route discovery error (ambiguous — two types share the name)

## `#[non_exhaustive]` posture (crate-local)

camel-config is **out of ADR-0049's mandatory scope** — that policy binds only the three
contract crates (`camel-api`, `camel-component-api`, `camel-language-api`). This table
records the crate-local posture, resolved as **selective** (case-by-case, not blanket) in
rc-vmrr (`98ace84e`), using ADR-0049 §Rule 3 and its Exceptions clause as the decision
framework — **not** as an extension of ADR-0049's binding scope. Precedent: camel-dsl
DP-8 (`98ace84e`), which applied the same §Rule 3 reasoning by choice.

The relevant §Rule 3 test: `#[non_exhaustive]` is a default whose cost is a forced `_ =>`
arm in **out-of-crate** matches; an enum is exempt when its **closed set is the contract**
(the `PipelineOutcome` / `ExchangePattern` reasoning). Config `pub struct`s follow the
struct-literal side of the same rule: consumers construct or deserialize them, so
`#[non_exhaustive]` imposes construction friction without a matching benefit.

| Type | non_exhaustive | Rationale (ADR-0049 §Rule 3 framework) |
|------|----------------|----------------------------------------|
| Config `pub struct` (deserialized from TOML; some also constructed by consumers) | **No** | Struct-literal side of §Rule 3 — `#[non_exhaustive]` blocks literal construction for no forward-compat gain on serde-owned shapes. 27/27 structs comply — the former exceptions `NativeIssuerConfig` / `NativeM2mClientConfig` were deleted with the mini-IdP surface (see finding M3 below). |
| `JournalDurability` (`config.rs:448`) | **No** | Closed 2-variant set (`Immediate` / `Eventual`) mirroring `redb::Durability`; the set **is** the contract — the §Rule 3 Exceptions clause (cf. `PipelineOutcome`). Matched exhaustively in-crate via the `From<JournalDurability> for camel_core::JournalDurability` impl (`config.rs:456`). |
| `PlatformCamelConfig` (`config.rs:181`) | **No (monitor)** | Feature-gated platform taxonomy (`Noop` / `Kubernetes`). The one external match (`camel-test/master_kubernetes_test.rs`) is wildcard-bearing, so a future `Aws`/`Gcp` variant is additive today. Flip to `#[non_exhaustive]` if a non-test consumer ever matches it exhaustively. |
| `OtelProtocol` (`config.rs:335`), `OtelSampler` (`config.rs:344`) | **No** | Config-value enums, serde-parsed from TOML and immediately lowered to the camel-otel runtime types (see Two-layer enum split). No external match; OTLP surface is spec-stable. |

Counts (verified mechanically, HEAD `745b2732`): **27** `pub struct`, **0** `#[non_exhaustive]`,
**4** `pub enum` in `config.rs`.

## Architecture notes

**`[binds]` public-exposure acknowledgements (ADR-0061 Rule 4).**
`CamelConfig.binds` maps bind addresses (`"0.0.0.0:8080"`) to
`BindExposureConfig { allow_public_exposure }`. The CLI threads the map to
the route controller and the MCP registry; a non-loopback bind serving any
Public route refuses startup without an acknowledgement. Config-shape only —
the gate lives in `camel-auth::bind_gate`.

**Two-layer enum split (config shape vs runtime shape).**
`OtelProtocol` and `OtelSampler` in camel-config are serde-parse shapes (`Grpc` / `Http`;
`AlwaysOn` / `AlwaysOff` / `Ratio`). camel-otel owns the runtime shapes (`HttpProtobuf`,
`TraceIdRatioBased(f64)`). Conversion is explicit in `context_ext.rs` (~L239); the config
enums never leak into runtime code. `JournalDurability` follows the same pattern via its
`From` impl to `camel_core::JournalDurability`. This deliberate split lets the TOML surface
evolve independently of the runtime types.

**Hot-reload wiring.**
The `watch` and `watch_debounce_ms` fields are consumed by camel-cli (`run.rs`, via
`--watch`) and implemented in camel-core (`reload_watcher::watch_and_reload`). The
stale `TODO(CONFIG-004)` comments (finding M1) have been removed.

**Unified `${env:}` placeholder interpolation (ADR: unify-config-interpolation-on-env).**
Camel.toml string leaves interpolate through ONE engine: camel-dsl
`interpolate_env` (`${env:NAME}` / `${env:NAME:-default}`, `$$` escapes),
applied by `resolve_tree_placeholders` — a recursive walk over the MERGED
raw tree (main file + includes + `CAMEL_*` env overrides), run after the
builder merges and before strict deserialization. Dispatch is by top-level
path prefix against `STRICT_PREFIXES` (`security`, `datasources`,
`idempotent_repo`, `cache_repo`); every other leaf is plain. STRICT-CLASS
CRITERION (the invariant that tells review when to extend the list): a
section is strict iff its string leaves reach an external authenticator or
connection secret — a residual placeholder marker in such a leaf is a
silent credential bug, so the escaped full form `$${env:X}` is rejected
there. Extending the class is a deliberate edit to the const + its content
tripwire test. `components.*` / `beans.*` are intentionally PLAIN: unset
`${env:}` still fails closed there, but malformed markers pass through; if a
component grows a genuine credential leaf, the principle says extend
`STRICT_PREFIXES`. Legacy `{{...}}` is a hard error on every leaf.
`PropertiesResolver` (properties.rs) is a legacy `{{...}}` public API
retained for compatibility; it is OFF the load path.

**Env-override allowlist (L-C2).**
`CAMEL_*` overrides merge through a fixed allowlist (`ALLOWED_ENV_OVERRIDES`). Any other `CAMEL_*` variable is ignored with a warning. The merge dispatches on value kind. `LEGACY_STRING_ENV_OVERRIDES` (`CAMEL_CACHE_REPO_BACKEND`, `CAMEL_CACHE_REPO_PATH`, `CAMEL_CACHE_REPO_STALE_RETENTION`) passes the raw value through verbatim as a string, with no numeric or boolean coercion. It is deliberately NOT empty-skippable: an empty value overrides the file value and fails validation loudly. `STRING_ENV_OVERRIDES` (the six newer string-typed cache_repo scalars) and `EMPTY_SCALAR_ENV_OVERRIDES` (those six plus `CAMEL_CACHE_REPO_DB`) keep their scopes unchanged. The string set is a subset of the empty-skip set, so an empty value never reaches typed deserialization. `CAMEL_CACHE_REPO_SENTINEL_NODES` is the only CSV-list override. An empty value replaces the file list and normalizes to absent on redis. Duration fields (`stale_retention`, `sweep_interval`) require humantime units. A unitless numeric value fails validation with `cache_repo.stale_retention: invalid duration '604800' — use a unit-bearing form such as '7d' or '24h'` (same shape for `cache_repo.sweep_interval`). Contract: [cache-repo-configuration spec](../../openspec/specs/cache-repo-configuration/spec.md).

**Include entries are never interpolated.**
The loader strips all `include` keys from the raw tree pre-placeholder and resolves those entries as plain filesystem paths before any merge or interpolation: stripping sits in `fn extract_includes` (`config.rs:2242`), loading in `fn load_includes`, path resolution in `fn resolve_include_path` (`include.rs:12`).
`${env:}` expansion runs only post-merge and only on leaf VALUES (`fn resolve_tree_placeholders`, invoked post-merge at `config.rs:2585`).
An entry like `include = ["${env:CAMEL_INCLUDE_CONF}"]` therefore fails with "included file not found": the raw marker is treated as the filename.
Recursive includes are unsupported in V1: `fn load_includes` drops a nested `include` key with a warning (`include.rs:91`).

**Switching cache backend between profiles requires a complete section.**
Profile merges are additive: overlay scalars replace while omitted base keys survive, and TOML offers no remove-key sentinel (`fn merge_toml_values`, `config.rs:1623`).
`CamelConfig::validate` rejects every cross-backend key — the redis branch rejects `path`, `cache_size`, `sweep_interval`, `max_entries`, `max_capacity` (`config.rs:2056`), while the redb branch rejects `url` and its siblings (`config.rs:1977`) — so a partial profile overlay cannot switch backends.
It fails validation instead, e.g. `cache_repo.path does not apply to the "redis" backend`.
The supported pattern: define the complete `[<profile>.cache_repo]` table inside each profile and keep `cache_repo` out of `[default]`.
A table absent from the base inserts whole at merge, so absence swaps the table instead of deep-merging into it.

**Route discovery delegation.**
`discovery.rs` delegates to `camel_dsl` and wraps its error as `DiscoveryError::Dsl`. This
crate implements no EIPs and no route steps (L7 N/A).

## Related decisions

- **ADR-0049 §Rule 3 + Exceptions** — the `#[non_exhaustive]` decision framework, cross-referenced
  only. camel-config is explicitly **out of** ADR-0049's binding scope.
- **ADR-0011** — CanonicalRouteSpec minimal contract; referenced by `wasm_limits.rs` for the
  "unset field falls back to the runtime default, no silent surprises" rule on `[limits]`.
- **camel-dsl DP-8** (`98ace84e`) — crate-local posture-table precedent this file follows.
- **Finding M3 (resolved):** `NativeIssuerConfig` / `NativeM2mClientConfig` were deleted with the
  mini-IdP surface (`token_issuer` / `clients`, auth-reinforcement). The `#[non_exhaustive]`
  inconsistency they carried is gone; `NativeAuthConfig` now holds a
  `credentials: Vec<NativeCredentialEntry>` array instead. All `config.rs` structs comply without
  exceptions.
