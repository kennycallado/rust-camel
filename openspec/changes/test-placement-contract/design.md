# Design: test-placement-contract

## Approach

Two separable decisions, landed in one change so the blessed placement never
ships crashing (D-RUN is a precondition for the colocated scaffold test,
because the scaffold templates pin explicit `routes = ["routes/*.yaml"]`,
which is today the unfiltered crash path).

### D-RUN: reserved suffix owned by camel-dsl discovery

`discover_routes_inner` (`crates/camel-dsl/src/discovery.rs:220`) owns the
rule. Inside the glob-entry loop, immediately after the path resolves and
before the extension gate, any read, or interpolation:

1. Add a suffix predicate `is_test_document(path)` that checks the file name
   for the `.test.yaml` / `.test.yml` suffix. Export it; `camel-cli`
   (run/watch resolver and lint command) consumes this one function.
2. If the pattern contains no glob metacharacters (new helper, modeled on
   `pattern_targets_json` at `discovery.rs:147`) and the resolved name has
   the test suffix, return `DiscoveryError::ReservedTestSuffix { path }`.
   Error text: "Route file {path} uses the reserved '.test.yaml' suffix,
   which names a camel test document, not a route. Run it with
   'camel test {path}', or rename it if it is a route."
3. Otherwise (wildcard patterns), skip test-suffixed entries with no error.
   Route discovery loads `{routes:}` documents; a test document is never
   one. The suffix check runs before the extension and JSON gates, so a
   `.test.json` name is not test-suffixed (test documents are YAML-only)
   and keeps today's JSON gating unchanged.

`camel-cli` changes:

- Delete `expand_patterns_excluding_test_docs` and the filter branch in
  `resolve_route_patterns_with` (`run.rs:657-707`). Default globs, explicit
  `Camel.toml` `routes`, and `--routes` all pass verbatim to discovery;
  discovery applies the suffix rule uniformly. Watch reload inherits the
  fix because it calls the same resolver (run.rs:603 watch closure).
- `camel lint` consumption: the CLI command (`camel-cli`,
  `crates/camel-cli/src/commands/lint.rs:19-22` takes a single file path)
  applies the exported `is_test_document` predicate before invoking the
  engine and emits a one-line info diagnostic ("skipped: camel test
  document"). The `camel-lint` engine contract is unchanged (it receives
  source text, no file names). The corpus gate
  (`crates/camel-cli/tests/lint_corpus.rs:94-97`) replaces its local suffix
  copy with the shared predicate.

Spec consequence: the `mock-testkit` requirement "camel run
non-interference" currently honors an explicit `*.test.yaml` glob as a user
override and parses the matched files as routes (they fail on unknown
fields). This change replaces that behavior: wildcard matches are skipped;
an explicit no-wildcard path errors with `ReservedTestSuffix`. This is an
accepted breaking change: a route-shaped file that deliberately used the
reserved suffix under the old override must be renamed. The delta spec
records this MODIFIED requirement and the migration.

### D-TEST: routeFilesFromRoot sibling key

- `TestDocument` (`crates/camel-cli/src/commands/test/document.rs:37`) gains
  `route_files_from_root: Option<Vec<String>>` with serde rename
  `routeFilesFromRoot`. `deny_unknown_fields` keeps the surface closed.
- The exactly-one-source check (`document.rs:210-213`) becomes three-way:
  `routeFiles`, `routeFilesFromRoot`, `routes`. Violations fail with exit 2
  and name the offending pair.
- Resolution (`runner.rs`, `load_routes`): walk up from the test document's
  directory through `Path::ancestors()`. The first directory containing
  `Camel.toml` is the project root. Join each entry onto that root with
  `Path::join` (Windows-safe; no string concatenation). No `Camel.toml`
  found produces `TestDocError::NoProjectRoot { doc_dir }` with the walked
  path in the message. Resolution is cwd-independent.
- Monorepo semantics: nearest ancestor wins, so a per-service `Camel.toml`
  anchors to that service. This matches cargo workspace discovery and is
  documented in the ADR.
- `routeFiles` semantics are unchanged. Existing documents behave exactly
  as before.

### Scaffold

Template assets live in `crates/camel-cli/templates/basic/` (embedded by
`crates/camel-cli/src/template/embedded.rs`). Changes:

- `routes/hello.yaml`: replace `timer:tick` + `log:` with the proven shape
  from `examples/yaml-dsl/config/mock-demo.yaml:12-17`: `direct:start` →
  `set_header` → `mock:result`.
- Add `routes/hello.test.yaml`: `routeFiles: [hello.yaml]`, one
  `direct:start` input with a text body, `expects: {mock:result: {count: 1}}`
  plus one header assertion. Deterministic; no `settle` needed.
- `README.md.tpl`: add a `## Test` section (`camel test routes/hello.test.yaml`)
  above `## Run`.
- Both `Camel.toml.env` and `Camel.toml.simple` keep their explicit
  `routes = ["routes/*.yaml"]`; after D-RUN this is safe.
- One sample test only. No `tests/unit/` tier: every test document boots a
  real `CamelContext`, so the route is the smallest unit (scaffold ruling,
  `docs/reviews/2026-08-21-scaffold-layout-verdict.md`).

### ADR-0062 and CONTEXT-MAP

New ADR `0062-reserved-test-suffix-and-placement-contract.md` records:

1. `.test.yaml` / `.test.yml` is a reserved suffix owned by `camel test`.
2. Discovery always skips it; explicit no-wildcard naming errors
   (explicit-gate idiom, same family as `JsonRequiresExplicitPattern`).
3. The suffix rule lives in `camel-dsl` discovery only.
4. Colocation is the blessed default; a separate directory is first-class
   through `routeFilesFromRoot` anchored at the nearest ancestor
   `Camel.toml`.
5. All in-string sigils rejected, with reasons: `@/` is frontend idiom;
   `$` collides with `${env:}` interpolation (`discovery.rs:277`);
   `~/` reads as home; URI pseudo-schemes muddy the endpoint model.
6. Known costs: watch no-op wake on test-document saves; a wildcard glob
   naming only test documents can report no routes found.

CONTEXT-MAP.md gains key terms "reserved test suffix" and
"route/test placement" citing ADR-0062.

## Affected crates

- `camel-dsl`: suffix predicate, no-wildcard helper, `ReservedTestSuffix`
  error variant, skip logic in `discover_routes_inner`; exported predicate.
- `camel-cli`: delete run.rs filter; `TestDocument` field + three-way
  exclusivity; `NoProjectRoot` + root walk-up in runner; lint command
  applies the shared predicate and emits the info diagnostic; corpus gate
  consumes the shared predicate; templates (hello.yaml shape,
  hello.test.yaml, README section).
- `camel-lint`: no changes (engine receives source text; the predicate
  lives in `camel-cli`).
- `docs/adr` + `CONTEXT-MAP.md`: ADR-0062 and key terms.

## Architecture boundaries

- Data/control plane untouched: no runtime, no bus, no component changes.
- `camel-dsl` stays the single parse/discovery authority; `camel-cli` keeps
  no suffix knowledge of its own.
- Test execution keeps the in-process runner contract
  (`mock-testkit` spec, "In-process route execution"); only route-source
  resolution changes.
- ADR-0017 / ADR-0026 (DSL parse to `RouteDefinition`) unchanged; the suffix
  rule is a discovery-input filter, not a schema change.

## Phases

- **Phase 1 — Reserved suffix law (D-RUN).** `camel-dsl` discovery rule +
  error, run.rs filter deletion, lint shared predicate + info diagnostic,
  ADR-0062, CONTEXT-MAP key terms. Independently shippable; fixes the live
  explicit-`routes` crash.
- **Phase 2 — Root-anchored test documents (D-TEST).** `routeFilesFromRoot`
  field, three-way exclusivity, walk-up resolver, `NoProjectRoot` error.
  Depends on nothing in Phase 1 (separate code path), but ships in the same
  change per the coupling rule.
- **Phase 3 — Scaffold teaches the contract.** hello.yaml testable shape,
  colocated hello.test.yaml, README `## Test` section. Requires Phase 1
  (scaffold pins explicit `routes`, the former crash path).

Phase-exit criteria: Phase 1 exit = explicit-`routes` + colocated test
starts and reloads clean, `--routes foo.test.yaml` errors, suffix rule in
one place. Phase 2 exit = nested document resolves from any cwd, three-way
exclusivity and `NoProjectRoot` fail closed. Phase 3 exit = scaffolded
project passes `camel test` and starts under `camel run`.
