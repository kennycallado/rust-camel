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
