# Proposal: component-metadata-coverage

## Why

The endpoint-metadata system landed by `consolidate-uri-metadata` (rc-4cos) lets
components declare URI options via `#[derive(UriConfig)]` + `#[uri_param]`, harvested
into `ComponentMetadataCatalog` for tooling. But only ~12 of the 30 component crates
adopted it. Several high-value Components — `kafka`, `jms`, `mqtt`, `redis`, `grpc`,
`keycloak`, `llm`, `surrealdb`, `wasm`, `controlbus` — expose **empty `uri_options`**,
so the catalog cannot describe their URI parameters. Populating the producer side is
what unblocks real param validation, alias/deprecation hints, and secret-in-URI
detection by downstream tooling (the consumer, `camel-lint`, is built separately).

## What Changes

**Included (Phase 1):** for each of the 10 high-value Components above, author a
private **metadata-descriptor struct** carrying only `#[uri_param]` fields for that
Component's accepted URI query keys, derive `#[derive(UriConfig)]` (with `skip_impl`)
on the descriptor, and wire `Component::metadata()` to delegate to the descriptor's
inherent `metadata()`. The descriptor is the universal pattern: the macro allows only
one non-`#[uri_param]` "path" field per struct, and production runtime configs carry
multiple non-URI fields (path-derived names, resolved values, injected handles,
connection state), so direct derives on the runtime config do not compile. The runtime
config struct and its `from_uri` are not modified.

**Included (Phase 2 — explicit per-Component disposition):**
- `cxf`, `validator` — have real params; annotate like Phase 1.
- `master`, `template`, `exec` — legitimately query-minimal (exec is profile-driven and
  ignores query strings); record as advisory (`minimal(scheme)` is correct), no work.
- `xj`, `xslt` — schema-blocked: they accept an open-ended `param.*` namespace that exact
  `UriOption` names cannot model. Deferred (needs macro/catalog support for open-ended
  namespaces); out of scope here.

**Excluded:** no change to the `UriConfig` macro itself, no change to `from_uri`
parsing (metadata is additive; parsing stays manual), no Runtime/DSL change, no
`camel-lint` change.

## Acceptance criteria

- For each Phase-1 Component scheme `s`, `catalog.get_metadata(s).uri_options` is
  non-empty and the entry names match the keys that Component's `from_uri` accepts.
- Existing `from_uri` parsing and all existing per-crate tests are unchanged.
- Per-Component test asserts the metadata canonical-name/alias set equals a reviewed
  fixture of the parser's accepted keys (executable parity, see spec).
- Per-crate `cargo fmt --check`, `cargo clippy -p <crate> -- -D warnings`,
  `cargo test -p <crate> --lib` green; `lint-non-exhaustive` and `lint-unwrap` clean.

## Risk budget

Low. Additive metadata annotation; `from_uri` parsing untouched, so no behavioral risk
to route execution. `skip_impl` ensures the derive adds only inherent metadata methods
(no conflict with existing manual `impl UriConfig` or redacting `Debug` impls). Param
lists are curated from each Component's own parsing code (not the full Apache Camel
surface) — gaps are follow-up, not blockers. Out of bounds: redesigning the macro,
changing `ComponentMetadata`/`UriOption` shapes, or supporting open-ended `param.*`
namespaces (that blocks xj/xslt, deferred).

Bd: rc-qbdt
