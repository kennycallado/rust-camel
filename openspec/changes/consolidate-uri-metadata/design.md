# Design: consolidate-uri-metadata

## Approach

**Hybrid single-source-of-truth.** The `#[derive(UriConfig)]` macro becomes the single
authoritative source for the mechanically-inferable fields of each `UriOption` (`name`,
`kind`, `default_value`, `required`), while accepting new `#[uri_param]` keys for the
semantic fields it cannot infer (`secret`, `deprecated`, `aliases`, `desc`). The macro
emits a `fn uri_options() -> Vec<UriOption>` helper on every derived config struct, and an
opt-in `#[uri_config(metadata(..))]` struct attribute generates a `fn metadata() ->
ComponentMetadata` inherent method on the config struct.

**Wiring: config-inherent → component-delegation.** The macro derives on the *config*
struct (`TimerConfig`, `SqlEndpointConfig`, etc.), which is a different type than the
*component* (`TimerComponent`, `SqlComponent`) that implements the `Component` trait. The
catalog queries `Component::metadata(&self)`, whose default returns
`ComponentMetadata::minimal(scheme)`. Therefore each component's `Component::metadata()`
override MUST delegate to the config struct's inherent `metadata()` (or compose:
`ComponentMetadata::minimal(scheme).with_uri_options(ConfigType::uri_options())`). This
delegation step is part of every migration task — the macro cannot wire it automatically
because the component and config are separate types.

This replaces the current split where `#[uri_param]` drives parsing and a hand-written
`fn metadata()` drives the catalog independently. Generation removes the drift failure
mode rather than policing it with a lint.

### OptionKind inference rule

Applied to the inner type after unwrapping `Option<T>`:

| Rust type | OptionKind |
|---|---|
| `Duration` (via `is_duration_type`) | `Duration` |
| `bool` | `Bool` |
| `u8`..`u64`/`usize`/`i8`..`i64`/`isize` | `Int` |
| `f32`/`f64` | `Float` |
| `String`, `&str` | `String` |
| `Vec<T>` | `List(Box<inner_kind(T)>)` |
| anything else (enum with FromStr) | **`String`** — never inferred `Enum` |

`Enum` is producible ONLY via explicit `kind = "enum:VariantA,VariantB"`. This is
guardrail G1: a worker cannot invent a variant list the type system does not expose.

### New `#[uri_param]` keys

| Key | Type | Maps to |
|---|---|---|
| `desc = ".."` | str | `UriOption.description` |
| `required` | flag | `UriOption.required` |
| `secret` | flag | `UriOption.secret` |
| `deprecated = "reason"` | str | `UriOption.deprecated` |
| `aliases = ["a","b"]` | list | `UriOption.aliases` |
| `kind = "duration"` / `"enum:A,B"` | str | `OptionKind` override |

Existing keys (`name`, `default`) are unchanged; truly-unknown keys still error.

## Affected crates

- **camel-endpoint-macros**: extend `UriParamAttr` parse; add `OptionKind` inference;
  generate `uri_options()` + opt-in `metadata()`; emit compile error on secret+default.
- **camel-api**: add `ComponentMetadata` builder methods (`with_description`,
  `with_capabilities`, `with_uri_options`) — currently absent (only `minimal()` exists).
- **camel-sql, camel-file, camel-cron, camel-opensearch, camel-ws, camel-container**
  (MACRO-ONLY): opt into `#[uri_config(metadata(..))]`; add semantic attrs where known.
- **camel-timer** (BOTH, already derives UriConfig): remove hand-written `fn metadata()`
  UriOption list; opt into `#[uri_config(metadata(..))]`.
- **camel-http** (3 manual `impl UriConfig` structs — `HttpEndpointConfig`,
  `HttpServerConfig`, `HttpStaticConfig` — with bespoke parsing): use
  `#[uri_config(skip_impl)]` to generate `parse_uri_components` + `uri_options()` while
  **retaining** the manual `impl UriConfig` and its bespoke `from_components` logic. Add
  `#[uri_param(secret)]` on auth fields. Metadata derives from the macro; parse stays
  byte-stable by construction. This is its own phase (Phase 3b) due to blast radius.
- **camel-log** (manual `impl UriConfig`, simple): convert to `#[derive(UriConfig)]` if
  parsing is trivial, or use `skip_impl` if bespoke; opt into metadata.
- **camel-direct, camel-component-seda** (free-function `from_uri`, no trait): adopt
  `#[derive(UriConfig)]` + `#[uri_param]` + opt-in metadata; replace free-function
  `from_uri` with trait impl.
- **camel-mock** (trivial — zero `UriOption::new`, no `from_uri`): metadata-attribute-only
  migration; no params to manufacture.
- **docs/adr/0041**: amendment documenting the macro-derived `uri_options` mechanism.
- **CONTEXT-MAP.md / camel-api CONTEXT.md**: glossary cross-link.

## Architecture boundaries

This change respects the data/control-plane boundary: the macro generates metadata at
compile time (authoring surface), `Registry::register()` harvests it once at startup
(control plane), and the runtime pipeline is untouched. No data-plane processor or
DSL runtime change is involved. The `Component` trait signature is unchanged; only new
opt-in attribute inputs and generated helper fns are added.

## Phases

### Phase 1: Macro extension + reference migration (sql)

- **Goal:** extend the macro to generate `uri_options()`/`metadata()` and prove it on one
  real component.
- **Dependencies:** ADR-0041 (types + harvesting, already implemented).
- **Externally-visible types/interfaces:** `fn uri_options()` on UriConfig structs;
  `#[uri_config(metadata(..))]` attribute; new `#[uri_param]` keys; `ComponentMetadata`
  builders (`with_description`, `with_capabilities`, `with_uri_options`).
- **Deliverable:** macro crate updated + sql migrated + ComponentMetadata builders added
  + macro rustdoc/README updated.
- **Exit-criteria:**
  - `cargo test -p camel-endpoint-macros` green incl. OptionKind inference cases,
    Option<T> unwrap, secret+default compile error, kind-typo error, kind-override
    round-trip.
  - `get_metadata("sql").uri_options` non-empty with known params.
  - sql `from_uri` parse tests pass unchanged.
  - fmt/clippy(-D warnings)/lint-unwrap/lint-non-exhaustive green.

### Phase 2: Migrate MACRO-ONLY (5) + timer (BOTH)

- **Goal:** fan out the proven pattern to the remaining derive-using components.
- **Dependencies:** Phase 1 (macro + sql reference green).
- **Externally-visible types/interfaces:** none new (macro surface unchanged).
- **Deliverable:** file, cron, opensearch, ws, container, timer migrated (one per commit).
- **Exit-criteria:**
  - Parametrized test: all 7 schemes (sql + 5 + timer) return non-empty `uri_options`.
  - timer's hand-written `fn metadata()` UriOption list removed; metadata derives from
    macro.
  - Each migrated component's `from_uri` parse tests pass unchanged.
  - Dedup invariant: no scheme has duplicate option names.
  - fmt/clippy/lints green.

### Phase 3a: Convert simple manual / no-UriConfig components (direct, mock, seda, log)

- **Goal:** unify the 4 lower-risk components. direct/seda adopt derive from scratch;
  log uses derive or skip_impl (trivial parsing); mock is metadata-attribute-only (no
  `from_uri`, zero existing `UriOption::new`).
- **Dependencies:** Phase 2 (macro proven on 7 components).
- **Externally-visible types/interfaces:** none new.
- **Deliverable:** direct, seda, log, mock migrated (one per commit).
- **Exit-criteria:**
  - direct, seda: `#[derive(UriConfig)]` + `#[uri_param]` adopted; free-function
    `from_uri` replaced by trait impl; existing parse tests pass.
  - log: derive or skip_impl; existing parse tests pass.
  - mock: trivial — no params manufactured; metadata derives from macro (or confirms
    zero params legitimately).
  - No hand-written `UriOption::new` in these 4 crates.
  - fmt/clippy/lints green.

### Phase 3b: Convert http (skip_impl for bespoke parse structs)

- **Goal:** unify http's 3 config structs (`HttpEndpointConfig`, `HttpServerConfig`,
  `HttpStaticConfig`) using `#[uri_config(skip_impl)]` — metadata derives from the macro,
  parse stays manual and byte-stable. Remove all hand-written `fn metadata()` blocks.
- **Dependencies:** Phase 3a (skip_impl pattern proven on log if used there).
- **Externally-visible types/interfaces:** none new.
- **Deliverable:** 3 http config structs annotated with `#[uri_config(skip_impl)]` +
  `#[uri_param]`; auth fields carry `secret`; 3 `fn metadata()` blocks removed; metadata
  derives via generated `uri_options()`.
- **Exit-criteria:**
  - All 3 http config structs: `parse_uri_components` generated; manual `impl UriConfig`
    + bespoke `from_components` logic RETAINED unchanged.
  - All 39 existing http `from_uri` tests pass unchanged (byte-stable parse).
  - `from_uri_with_defaults` (21 sites) unaffected.
  - No hand-written `UriOption::new` in camel-http.
  - http auth params (authUsername, authPassword, authBearerToken) carry `secret` in
    derived `uri_options`.
  - All 12 components now unified (grep: zero `UriOption::new` outside camel-api /
    camel-endpoint-macros / `#[cfg(test)]` modules).
  - fmt/clippy/lints green.

## Alternatives considered

- **Drift-check (two parallel sources + lint detecting divergence):** rejected. It does
  not close the 6-component gap (drift between "macro" and "nothing" cannot manufacture
  missing `UriOption` lists), and two truths rot — every new param must be added twice.
- **Full-auto (macro owns all fields):** rejected. The macro provably cannot express
  `description`/`secret`/`deprecated.reason`/`aliases` — those are not in the Rust type.
  A hybrid is the only design where every field has exactly one authoring home.
- **New `#[uri_meta]` attribute:** rejected. Widens the proc-macro attribute surface
  (a fourth ident) and re-fragments the single source. Extending the existing
  `#[uri_param]` key set keeps one attribute home.
