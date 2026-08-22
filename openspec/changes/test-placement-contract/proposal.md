# Proposal: test-placement-contract

## Why

`camel run` route discovery and `camel test` documents conflict when test
documents share a directory with route files:

- `Camel.toml` `routes = [...]` globs use the `glob` crate. It supports no
  negation patterns. A colocated `routes/demo.test.yaml` matches
  `routes/*.yaml`, and discovery fails to parse it as a route document.
- The suffix exclusion exists today in three places, and each place applies it
  differently. `expand_patterns_excluding_test_docs`
  (`crates/camel-cli/src/commands/run.rs:707-731`) filters `*.test.yaml` only
  on the default glob path. Explicit `Camel.toml` `routes` entries and
  explicit `--routes` values pass through with no filter (`run.rs:741-757`).
  A user with the common explicit `routes = ["routes/*.yaml"]` plus a
  colocated test document gets a crash, on start and on watch reload. The
  lint corpus gate (`crates/camel-cli/tests/lint_corpus.rs:94-97`) applies a
  third copy of the same rule.
- A test document in a separate directory resolves `routeFiles` only against
  its own directory (`crates/camel-cli/src/commands/test/runner.rs:83`), so it
  needs `../../` climbs to reach route files.

Design verdicts (oracle consultation, 2026-08-21):
`docs/reviews/2026-08-21-test-placement-root-anchoring-verdict.md` and
`docs/reviews/2026-08-21-scaffold-layout-verdict.md`. bd issue: rc-6760.

## What Changes

- **Reserved suffix law (D-RUN).** `.test.yaml` / `.test.yml` becomes a
  documented reserved suffix owned by `camel test`. Route discovery in
  `camel-dsl` (`discover_routes_inner`) always skips test-suffixed files,
  on every pattern path (default glob, `Camel.toml` `routes`, `--routes`,
  watch reload). An explicit pattern with no wildcards that names a
  test-suffixed file returns a hard error (`DiscoveryError::ReservedTestSuffix`)
  with an actionable hint. This is the explicit-gate idiom already used by
  `JsonRequiresExplicitPattern`, not a silent skip.
- **Single source of truth.** Delete `expand_patterns_excluding_test_docs`
  from `run.rs`. Run and watch consume discovery output. The suffix rule
  lives in `camel-dsl` only.
- **`routeFilesFromRoot` (D-TEST).** New sibling key in test documents.
  It resolves paths against the nearest ancestor `Camel.toml` directory
  (cargo-style walk-up, cwd-independent). Exactly one of `routeFiles`,
  `routeFilesFromRoot`, or inline `routes` is allowed. No ancestor
  `Camel.toml` produces a hard `NoProjectRoot` error. All in-string sigils
  (`@/`, `$ROOT`, `~/`, URI forms) are rejected; rationale goes in the ADR.
- **Placement contract.** Colocation (sidecar `foo.yaml` + `foo.test.yaml`)
  is the blessed default. A separate test directory stays first-class through
  `routeFilesFromRoot`.
- **Scaffold.** `camel new` basic template gains a colocated
  `routes/hello.test.yaml`. The sample route changes from `timer:` + `log:`
  to the testable `direct:` → `set_header` → `mock:` shape. README template
  gains a `## Test` section above `## Run`.
- **Lint.** `camel lint` skips test-suffixed files with a one-line info
  diagnostic instead of a silent corpus-level filter only.
- **ADR.** New ADR records the reserved suffix law, the placement contract,
  and the sigil rejection. CONTEXT-MAP.md gains two key terms.

Excluded: `routeFilesFromRoot` demo example (bd rc-xt3k), mock assertion
matchers (rc-3kwt), any glob library change.

## Acceptance criteria

- `camel run` with explicit `Camel.toml` `routes = ["routes/*.yaml"]` and a
  colocated `*.test.yaml` starts with no error; watch reload is a no-op when
  only a test document changes.
- `camel run --routes foo.test.yaml` fails with the `ReservedTestSuffix`
  error and a hint to use `camel test`.
- A nested test document with `routeFilesFromRoot: [routes/orders.yaml]`
  resolves from any working directory.
- A document with two of the three route-source keys fails with exit 2.
  `routeFilesFromRoot` with no ancestor `Camel.toml` fails with
  `NoProjectRoot`.
- A scaffolded project passes `camel test routes/hello.test.yaml` and starts
  under `camel run`.
- The suffix rule exists in exactly one place: `camel-dsl` discovery.
- ADR and CONTEXT-MAP key terms land in the same change.

## Risk budget

- Discovery behavior change is the main risk, and it includes one accepted
  breaking change: the previous contract honored an explicit
  `--routes 'routes/*.test.yaml'` glob as a user override and parsed the
  matched files as routes (they failed on unknown fields). After this
  change, wildcard matches are skipped and an explicit no-wildcard path
  errors. A route-shaped file that deliberately used the reserved suffix
  must be renamed. Migration is a rename; no automatic path exists.
- Explicit-glob users who today get a crash get an exclusion instead. This
  is a strict improvement.
- `camel run --routes 'routes/*.test.yaml'` (wildcard) matched files get
  skipped, so the command can report no routes found. Accepted; recorded in
  the ADR.
- Watch wakes on test-document saves and reloads to an identical route set.
  Accepted no-op churn; recorded in the ADR.
- Out of bounds: route file schema changes, mock component semantics, any
  change to `routeFiles` resolution for existing documents, changes to the
  `camel-lint` engine input contract.
