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
| Config `pub struct` (deserialized from TOML; some also constructed by consumers) | **No** | Struct-literal side of §Rule 3 — `#[non_exhaustive]` blocks literal construction for no forward-compat gain on serde-owned shapes. 25/27 structs comply. `NativeIssuerConfig` (`config.rs:597`) and `NativeM2mClientConfig` (`config.rs:613`) carry it inconsistently — current state; proposed correction tracked as finding M3 (not prescribed by this doc). |
| `JournalDurability` (`config.rs:448`) | **No** | Closed 2-variant set (`Immediate` / `Eventual`) mirroring `redb::Durability`; the set **is** the contract — the §Rule 3 Exceptions clause (cf. `PipelineOutcome`). Matched exhaustively in-crate via the `From<JournalDurability> for camel_core::JournalDurability` impl (`config.rs:456`). |
| `PlatformCamelConfig` (`config.rs:181`) | **No (monitor)** | Feature-gated platform taxonomy (`Noop` / `Kubernetes`). The one external match (`camel-test/master_kubernetes_test.rs`) is wildcard-bearing, so a future `Aws`/`Gcp` variant is additive today. Flip to `#[non_exhaustive]` if a non-test consumer ever matches it exhaustively. |
| `OtelProtocol` (`config.rs:335`), `OtelSampler` (`config.rs:344`) | **No** | Config-value enums, serde-parsed from TOML and immediately lowered to the camel-otel runtime types (see Two-layer enum split). No external match; OTLP surface is spec-stable. |

Counts (verified mechanically, HEAD `98ace84e`): **27** `pub struct`, **2** `#[non_exhaustive]`,
**4** `pub enum` in `config.rs`.

## Architecture notes

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

**Route discovery delegation.**
`discovery.rs` delegates to `camel_dsl` and wraps its error as `DiscoveryError::Dsl`. This
crate implements no EIPs and no route steps (L7 N/A).

## Related decisions

- **ADR-0049 §Rule 3 + Exceptions** — the `#[non_exhaustive]` decision framework, cross-referenced
  only. camel-config is explicitly **out of** ADR-0049's binding scope.
- **ADR-0011** — CanonicalRouteSpec minimal contract; referenced by `wasm_limits.rs` for the
  "unset field falls back to the runtime default, no silent surprises" rule on `[limits]`.
- **camel-dsl DP-8** (`98ace84e`) — crate-local posture-table precedent this file follows.
- **Open question (does not block this doc):** finding M3 — remove vs keep `#[non_exhaustive]`
  on `NativeIssuerConfig` / `NativeM2mClientConfig`. The recommendation (remove, to align
  with the other 25 structs) is a mechanical, non-breaking widening tracked in the code stream;
  this doc records the current state either way.
