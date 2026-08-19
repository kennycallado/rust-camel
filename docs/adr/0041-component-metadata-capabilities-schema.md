# ADR-0041: Component Metadata and Capabilities Schema

**Date:** 2026-07-15
**Status:** Accepted (implemented in `0114dee3`)

## Context

The project needs a foundation for components to declare their URI options,
capabilities, and version. This is a cross-cutting dependency for the WASM SDK,
camel-catalog, OpenAPI generation, and IDE/registry tooling — all of which need
a stable introspection contract that does not rely on re-invoking component code
at query time.

## Decision

- Define a schema in `camel-api` (`component_metadata.rs`) with:
  - `OptionKind` — 7-variant enum of URI option value types (String, Int, Bool,
    Float, Duration, Enum, List).
  - `UriOption` — builder-pattern struct for a single URI-parameter definition:
    name, description, kind, required flag, default value, aliases, deprecation
    notice, and secret flag.
  - `ComponentCapabilities` — named boolean flags (consumer, producer,
    polling_consumer, streaming).
  - `CapabilityQuery` — tri-state query struct where each capability field is
    `Option<bool>`; `None` means "don't care".
  - `ComponentMetadata` — top-level descriptor: scheme, schema_version, version,
    description, uri_syntax, capabilities, and uri_options.

- Add a `metadata()` method to the `Component` trait returning
  `ComponentMetadata`, with a default `ComponentMetadata::minimal(scheme)`
  implementation so new components compile without change.

- Harvest metadata at registration time in `Registry::register()` — call
  `component.metadata()` once, store the result indexed by scheme. Re-registering
  a component replaces its metadata.

- `ComponentMetadataCatalog` trait returns owned `ComponentMetadata` values (not
  references) because the Registry is behind `Arc<Mutex<>>`.

- `RuntimeComponentMetadataCatalog` in camel-core wraps the registry and
  implements `ComponentMetadataCatalog` for trait-object use.

## Out of Scope

- JSON Schema generation from the metadata types (future xtask work).
- Crate-level version extraction from Cargo.toml (uses a hardcoded constant for
  now).

## Consequences

- Components can declare rich metadata without runtime overhead — harvested once
  at registration.
- Schema generation (xtask) can produce a JSON Schema type contract from the
  Rust types.
- SDK, catalog, OpenAPI, and IDE tooling have a stable contract to consume.
- `metadata()` default is non-breaking for all existing components.

## Amendment: Macro-Derived URI Options via `#[derive(UriConfig)]`

**Rationale:** Hand-written `UriOption::new` lists in component production
code duplicate the URI parameter information already present in config struct
fields and `#[uri_param]` attributes. The `#[derive(UriConfig)]` macro is now
the single source of truth for URI parameter metadata.

### `fn uri_options()` Generation

`#[derive(UriConfig)]` generates an inherent `pub fn uri_options() -> Vec<UriOption>`
on the config struct. The method iterates over `#[uri_param]`-annotated fields
and produces one `UriOption` per field, using builder methods (`.required()`,
`.secret()`, `.with_default(v)`, `.deprecated(reason)`, `.with_alias(a)`) to
encode the semantic attributes.

### OptionKind Inference Rules

The macro infers `OptionKind` from the Rust type of each `#[uri_param]` field:

| Rust Type | OptionKind |
|-----------|------------|
| `Duration` | `Duration` |
| `bool` | `Bool` |
| `u8`..`u64`, `usize`, `i8`..`i64`, `isize` | `Int` |
| `f32`, `f64` | `Float` |
| `String`, `&str` | `String` |
| `Vec<T>` | `List(Box::new(infer_option_kind(inner)))` |
| Anything else | `String` (NEVER `Enum`) |

`Enum` variant requires an explicit `kind = "enum:A,B,C"` override on the
`#[uri_param]` attribute. Inference never produces `Enum`.

`Option<T>` fields are unwrapped to their inner `T` before inference, and
their default `required` flag is `false`.

### Semantic Attributes via `#[uri_param]`

The `#[uri_param]` attribute accepts these keys:

| Key | Type | Semantics |
|-----|------|-----------|
| `desc = "text"` | `Lit::Str` | Human-readable description |
| `required` | flag or `Lit::Bool` | Marks the option as mandatory |
| `secret` | flag or `Lit::Bool` | Credential-bearing field; must not appear in diagnostics |
| `deprecated = "reason"` | `Lit::Str` | Deprecation notice shown in tooling |
| `aliases = ["a", "b"]` | `ExprArray` | Alternative parameter names |
| `kind = "string"` | `Lit::Str` | Explicit kind override (`"duration"`, `"bool"`, `"int"`, `"string"`, `"float"`, `"enum:A,B"`) |

If `secret` is `true` and `default` is non-empty, the macro emits a compile
error — a secret with a hardcoded default is a security hazard.

### `metadata()` Generation via Opt-in

`#[uri_config(metadata(scheme = "x", description = "d", producer, consumer, polling_consumer, streaming))]`
generates an inherent `fn metadata() -> ComponentMetadata` on the config
struct. The method returns `ComponentMetadata::minimal(scheme).with_description(desc).with_capabilities(ComponentCapabilities { ... }).with_uri_options(Self::uri_options())`.

Without `metadata(..)`, no `metadata()` method is generated — only `uri_options()`.

### `skip_impl` Path

Structs with bespoke URI parsing logic (e.g., `HttpEndpointConfig`: custom
`impl UriConfig` with multi-segment path handling and legacy compatibility)
use `#[uri_config(skip_impl, metadata(..))]`. This retains the manual
`impl UriConfig` (including `from_uri`) while deriving `uri_options()` and
(if opted in) `metadata()` from the field annotations.

### Component-to-Config Delegation Convention

The `Component` trait's `metadata()` default returns `ComponentMetadata::minimal(scheme)`.
Each migrated component MUST override `metadata()` to delegate:

```rust,ignore
fn metadata(&self) -> ComponentMetadata {
    ConfigType::metadata()
}
```

Or, when the config has no `metadata(..)` opt-in but does have `uri_options()`:

```rust,ignore
fn metadata(&self) -> ComponentMetadata {
    ComponentMetadata::minimal(scheme).with_uri_options(ConfigType::uri_options())
}
```

### Single-Source-of-Truth Invariant

`cargo xtask lint-single-source` scans component crate source for
`UriOption::new` calls outside `#[cfg(test)]` modules. A violation means
metadata is being hand-written instead of macro-derived. The lint enforces
that the macro is always the single source of truth for URI parameter metadata.

### Inner-Config-Struct Mirror Pattern

Components with bespoke URI parsing (manual `impl UriConfig` on the public
config struct) use the inner-config-struct mirror pattern via `skip_impl`.
The mirror struct (e.g., `HttpEndpointUriConfig`, `SedaUriConfig`) is a
metadata-only anchor — its `#[uri_param]` fields must stay synchronized with
the bespoke parser's recognized params. Each component using this pattern
includes a parity test (`uri_options_count_parity`) that asserts the mirror
struct's `uri_options().len()` equals the expected param count. This catches
silent drift when a param is added to or removed from the manual parser but
the mirror is not updated.

## Amendment: Open Namespace Pattern Matching

**Rationale:** Some components accept URI query keys of the form
`param.<name>=<value>` where any non-empty `<name>` is valid. For example,
`camel-xj` and `camel-xslt` accept stylesheet parameters via
`param.foo=bar&param.baz=qux` pairs into a `Vec<(String, String)>` field. The
exact-name `UriOption` model cannot describe this open namespace — each key is
valid, so a fixed list of option names is impossible. Before this amendment,
these components returned `ComponentMetadata::minimal(scheme)` with empty
`uri_options`, causing the lint to silently no-op for them.

**Decision:**

- Add `UriOptionMatch`, a `#[non_exhaustive]` enum in `camel-api`, with one
  initial variant: `Prefix { separator: String }`.
- Add an optional `pattern: Option<UriOptionMatch>` field to `UriOption`,
  serialized with `#[serde(default, skip_serializing_if = "Option::is_none")]`
  so existing JSON output stays byte-identical.
- Add a consuming builder `UriOption::pattern_prefix(separator: &str) -> Self`
  that sets `pattern: Some(UriOptionMatch::Prefix { separator: separator.to_string() })`.
- Add a `#[uri_param(pattern = "<separator>")]` macro key, valid only on
  `Vec<(String, String)>` fields, with nine compile-time guardrails:
  incompatible with `required`, `default`, `secret`, `name`, `aliases`, and
  any non-`string` `kind`; empty separator rejected; separator without
  trailing `.` rejected; bare `.` rejected (strips to an empty name).
- Extend the shared lint helper `resolve_option` with two-phase resolution:
  **Phase 1** — combined exact-name OR alias match, considering only options
  whose `pattern` is `None`; **Phase 2** — longest-prefix-wins pattern match,
  considering only options whose `pattern` is `Some(_)`, with a non-empty
  suffix requirement (bare `param.` does NOT match `Prefix { separator: "param." }`).

### Consequences

- Components with open namespaces can now declare a single `UriOption` with a
  `Prefix` pattern, making their metadata visible to lint, schema-gen, catalog,
  and doc-gen.
- The `#[non_exhaustive]` enum allows future match variants (e.g. `Glob`,
  `Regex`) without a Rust-side schema break.
- **Forward-compat cost:** each future `UriOptionMatch` variant expands a
  closed JSON-Schema union. Stale validators or exhaustive generated consumers
  may reject the new variant at parse time despite the Rust-side
  forward-compat guarantee. Each future variant requires schema compatibility
  review and regenerated downstream consumers.
- Component migration (`camel-xj`, `camel-xslt`) is deferred to a follow-up
  change.
