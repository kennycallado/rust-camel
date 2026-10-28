# cli-feature-profiles Specification (delta)

## MODIFIED Requirements

### Requirement: Slim profile excludes the controllable bridge set

camel-cli SHALL provide a `flavor-slim` feature naming the
`--no-default-features` baseline: an http-only deployment that links the
non-optional core plumbing (http, direct, seda, log, mock, timer, file,
controlbus, container, validator, cron, and the DSL/config/core stack)
and SHALL NOT link the camel-cli-controllable optional set — kafka,
grpc, wasm, llm, mcp, mqtt, mail, surrealdb, exec, the lsp stack (camel-lsp,
tower-lsp), the language runtimes gated by the `lang-*` features (js,
rhai, jsonpath, xpath), and seven of the eight bridge chains
(camel-component-jms, camel-component-sql, camel-component-opensearch,
camel-component-ws, camel-component-cxf, camel-xj, camel-xslt) — when
unselected. The historical `slim-http` and `slim-benchmarks` names no
longer exist: `flavor-slim` is the only slim marker. `camel-component-redis`
remains linked in slim through the
unconditional camel-config → camel-redis-repo path (documented
out-of-zone deferral; the camel-cli-owned redis edge is optional as of
this change). `camel-language-minijinja` remains linked in every
profile: the workspace consumes camel-template with default features;
the in-workspace flip requires the named-feature shape whose deptree
rendering forces golden-fixture regeneration, so it stays deferred to a
follow-up change (the engine itself is optional at the source since
mission 115).

#### Scenario: slim closure excludes the controllable set

- **GIVEN** a build configured `--no-default-features --features flavor-slim`
- **WHEN** `cargo tree -p camel-cli --no-default-features -e no-dev`
  resolves
- **THEN** none of camel-component-{kafka,grpc,wasm,llm,mcp,mqtt,mail,
  surrealdb,exec,jms,sql,opensearch,ws,cxf}, camel-lsp, tower-lsp,
  camel-xj, camel-xslt, or the excluded language-runtime crates (js,
  rhai, jsonpath, xpath — NOT minijinja, see the requirement text)
  appears in the graph (camel-component-redis may appear via
  camel-config → camel-redis-repo; ariadne may appear: `camel lint`
  keeps it non-optional)

#### Scenario: slim build compiles and boots an http route

- **GIVEN** the flavor-slim release binary
- **WHEN** it boots the cold-start fixture (an http consumer route that
  must bind, plus the one-shot timer→log marker route)
- **THEN** it reaches the route-ready marker, proving the profile is
  self-coherent (no dangling forward such as `otel` without the http
  bridge, and the LSP-free binary still parses and boots routes)

#### Scenario: slim lint degrades to unverified-scheme notes

- **GIVEN** a slim build (bridge features unselected)
- **WHEN** `camel lint` runs on a route whose endpoint scheme is a
  gated bridge scheme (for example `jms:queue`)
- **THEN** the scheme surfaces as an `unverified-scheme` note — the
  same accepted graceful-degradation class as wasm/exec — and no bridge
  code is linked into the binary
