## MODIFIED Requirements

### Requirement: Per-Component disposition for query-minimal and namespace-blocked

Each change that touches URI metadata SHALL record an explicit per-Component
disposition for every component in its Phase-2 task. The disposition reflects
the component's current state and may transition across changes:
`advisory` (legitimately query-minimal — `minimal(scheme)` is correct, no work),
`schema-blocked-deferred` (accepts an open-ended `param.*` namespace that exact
`UriOption` names cannot model — deferred until the macro/catalog support open-ended
namespaces), or `schema-published` (the open namespace is declared via a
`pattern`-based `#[uri_param]` and rich metadata is published through a
`skip_impl` descriptor with a `Component::metadata()` override). A component
may transition from `schema-blocked-deferred` to `schema-published` once
open-namespace macro support is available.

#### Scenario: exec recorded as advisory

- **GIVEN** the exec Component is profile-driven and ignores URI query strings
- **WHEN** its Phase-2 disposition is recorded
- **THEN** it is marked `advisory` with the reason "profile-driven; query strings ignored", and no `#[uri_param]` is authored

#### Scenario: xj/xslt recorded as schema-blocked-deferred

- **GIVEN** the xj and xslt Components accept an open-ended `param.*` key namespace
- **WHEN** their Phase-2 disposition is recorded in a change that predates open-namespace macro support
- **THEN** they are marked `schema-blocked-deferred` with the reason "open-ended param.* namespace unsupported by exact UriOption names", and a follow-up is noted for macro/catalog open-namespace support

#### Scenario: xj/xslt transitioned to schema-published

- **GIVEN** the xj and xslt Components were previously marked `schema-blocked-deferred`
- **WHEN** a change adds a `skip_impl` metadata descriptor with a `#[uri_param(pattern = "param.")]` field and a `Component::metadata()` override to each component
- **THEN** their disposition transitions to `schema-published`, the catalog returns non-empty `uri_options` for schemes `xj` and `xslt`, and the `param` option has `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`

#### Scenario: xj/xslt param namespace resolves via prefix match

- **GIVEN** the xj and xslt metadata descriptors declare a `param` option with `pattern = Some(UriOptionMatch::Prefix { separator: "param." })`
- **WHEN** the lint resolver encounters a URI key `param.foo=bar` on an `xj:` or `xslt:` endpoint
- **THEN** the key resolves to the `param` option via the Phase-2 longest-prefix match, and no `UnknownOption` diagnostic is emitted
- **AND** a bare `param.` (empty suffix) does NOT resolve and emits `UnknownOption`, matching the runtime parsers' `!param_key.is_empty()` guard
