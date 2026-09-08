# Design: doc-identifier-interpolation-parity

## Context

Two sides of one name-matching contract diverged in v0.42.0 (bd rc-4hexo):

- Route side (ac147d07): `camel_dsl::load_from_file` =
  `load_from_file_with_env(path, &|_| None)` — `interpolate_yaml_source`
  (tree-walk-first, fallback whole-text splice) resolves
  `${env:NAME:-default}` default-only before YAML parsing
  (`crates/camel-dsl/src/yaml.rs:2073-2101`).
- Doc side: `parse_test_document` serde-parses `TestDocument` RAW
  (`crates/camel-cli/src/commands/test/document.rs:834`); `repositories` is
  `Option<RepositoriesDoc>` (:86), validated by `validate_repositories`
  (:1054) after intercept (:936) and bean (:983) validation.

v0.41.0 matched by coincidence (neither side interpolated). v0.42.0:
route key → `persistent`, doc key stays raw → mismatch. The adjudicated
fix (papal e_opus, rc-93wct lineage): identifier-field interpolation,
NOT whole-file. This design implements that verdict; the engine canon is
not relitigated.

## Goals / Non-Goals

Goals:
- Restore name-match parity by construction (same scanner, same grammar,
  same default-only lookup) for the five identifier field-groups.
- Keep assertion data literal (hermeticity: docs never consult ambient env,
  and value positions never become env-dependent).
- Failure parity: no-default identifier → doc-validation error exit 2
  naming the VARIABLE, mirroring route-side wording
  (discovery.rs:40-41 / yaml.rs `load_from_file_with_env`).
- Preserve the rc-l7m7t stepping stone: the `&|_| None` closure call sites
  are the future LayeredEnv injection points.

Non-Goals:
- Whole-document interpolation (rejected — see Decision 1).
- Any change to `interpolate_yaml_source`, `interpolate_env_with`,
  `interpolate_env_tree`, or camel-dsl at all.
- Runner register logic, camel-lint, camel-lsp: untouched.
- openapi rest-block analogue (bd rc-gykds): out of scope.
- The LayeredEnv doc-env layer itself (rc-l7m7t): future change that swaps
  the closures on both sides.
- Path/glob fields (`routeFiles`, `routeFilesFromRoot`): NEVER interpolate.

## Decisions

### Decision 1: post-parse per-field interpolation through the engine scanner

One private helper in `document.rs`:

```rust
fn interpolate_identifier(value: &str, position: &str) -> Result<String, TestDocError>
```

(`position` is the doc-position label carried into `EnvUnresolved { var, field }`
— e.g. `repositories.cache`, `inputs[N].to`; callers annotate per call site.)
It calls `camel_dsl::env_interpolation::interpolate_env_with(value,
&|_| None)`. `interpolate_env_with` is the pub shared scanner
(`interpolate_string`) that BOTH engine paths route through — grammar
parity with route sources holds by construction (same `${env:X}`,
`${env:X:-default}`, `$${env:X}`, `$$` forms; same `Err(var_name)` shape;
same control-char sanitization of substituted defaults).

Mechanical note (why NOT `interpolate_yaml_source` on the bare field):
`interpolate_yaml_source` re-serializes its output as YAML text —
numeric-looking results keep string typing via QUOTING (pinned by the
existing camel-dsl test `numeric_leaf_stays_string_after_interpolation`:
`port: ${env:PORT}` → `port: '8080'`) and `to_string` appends a trailing
newline. Applied to an already-parsed field string that would corrupt
identifiers (`'123'`, `name\n`). A bare field value is not a YAML
document: comments and leaf typing are structurally absent, so the
tree-walk strategy layer buys nothing and its serialization artifacts are
pure hazard. The scanner layer (`interpolate_env_with`) is the correct
pub seam of the same engine; using it is engine reuse, not reimplementation.
Route side loads whole files (tree-walk then parse); doc side interpolates
parsed strings (scanner) — both resolve through identical token grammar
and identical default-only lookup, which is the parity that matters.

The lookup closure is passed as `&|_| None` — the identical shape the
route side uses — so rc-l7m7t option (c) later swaps BOTH closures to a
LayeredEnv lookup without structural change here.

Alternatives rejected:
- Whole-file interpolation before serde parse: makes assertion bodies,
  matchers, and bean config env-dependent — breaks hermeticity; also
  interacts with `deny_unknown_fields` classification and error positions.
- Interpolating only `repositories` keys: leaves `beans`, `intercepts`,
  `mock:` refs, and `inputs[].to` broken — the same regression class
  (identifier fields are one class; fix the class).
- Duplicating the scanner in camel-cli: forbidden reuse discipline; the
  engine is pub and dependency direction (camel-cli → camel-dsl) exists.

### Decision 2: the five field-groups (exact positions)

Interpolated (identifier positions only):

1. `repositories` KEYS in all three registry maps (`cache`, `idempotent`,
   `claimCheck`) — `RepositoriesDoc` maps are rebuilt with interpolated
   keys. Stub TARGET values (`memory`) stay literal.
2. `beans` KEYS — `BTreeMap<String, BeanDeclDoc>` rebuilt. `methods`,
   `config` values stay literal.
3. `intercepts` KEYS (source URIs) AND action target values
   (`skipTo` / `divertCopyTo`) — map rebuilt, action strings interpolated.
4. `mock:` references — `expects` KEYS and `sequence` ENTRIES.
5. `inputs[].to` values.

Everything else stays literal, pinned by an anti-widening test: input
`body`/`headers`, `expectReply`, expectation matcher contents
(`ExpectSet` values), bean `methods`/`config` values, repository stub
targets, `settle`, `routeFiles`/`routeFilesFromRoot`/`routes`, and the
scenario (integration-tier) vocabulary — which this change never touches.

### Decision 3: ordering — step (a0) before ALL validation

Interpolation runs immediately after `serde_yaml::from_str` succeeds and
before steps (a)–(j). Consequences (all required):
- Blank/`memory` guards in `validate_repositories` see RESOLVED names
  (`${env:X:-}` → empty → blank-name error, not a raw-key pass).
- `expects` scheme check (c) and `sequence` checks (e) see resolved
  `mock:...` URIs.
- Intercept source/target validation (h) sees resolved URIs.
Map rebuilds follow the existing pattern the parser already uses when it
rebuilds `expects` at step (c): take the map, transform keys, reinsert.

Post-interpolation key collision: if two keys of one map resolve to the
same string (e.g. `"${env:A:-x}"` and `"x"`), silent shadowing is a
name-integrity hazard — reject as a document error naming the map and the
resolved value (string-message variants `InvalidRepositories` /
`InvalidBeans` / `InterceptInvalid` / `Yaml`-class for expects, whichever
the position already uses).

### Decision 4: failure parity — `TestDocError::EnvUnresolved`

```rust
/// A `${env:NAME}` placeholder in an identifier field resolved to
/// nothing (no value — ambient env is never consulted — and no default).
EnvUnresolved { var: String, field: String },
```

Display: `Environment variable '{var}' not set (required by {field})` —
mirrors route-side wording (`yaml.rs` `load_from_file_with_env`:
"Environment variable '{var}' not set (required by {path})", itself
mirroring discovery.rs:40-41). `field` names the doc position (e.g.
`repositories.cache`, `beans`, `intercepts`, `expects`, `sequence[N]`,
`inputs[N].to`). The message names the VARIABLE, never a resolved value
(no secret leakage). Doc-validation class → exit 2, not a runner/exit-1
failure — parse-time rejection like every other doc error.

### Decision 5: escapes and grammar parity for free

`$${env:X}` → literal `${env:X}`, `$$` → `$` — the scanner implements
this identically for route sources; a doc-side escape test pins it. A doc
key `$${env:CACHE_REPO_NAME:-persistent}` resolves to the literal text
`${env:CACHE_REPO_NAME:-persistent}`, matching the route side's identical
escape semantics — parity including the escape path.

### Decision 6: blast-radius containment

Only `crates/camel-cli/src/commands/test/document.rs` changes (plus tests
and CONTEXT.md prose). No new deps. `parse_test_document`'s signature and
`TestDocument`'s public surface are unchanged; validation order letters
(a)–(j) keep their meaning with (a0) prepended.

## Risks / Trade-offs

- Map rebuilds must preserve BTreeMap determinism (ordered iteration) —
  same pattern as the existing (c) rebuild; low risk.
- Interpolated keys participate in validation as if declared literally —
  the blank-default, built-in-`memory`, scheme, and collision guards are
  pinned by spec scenarios; remaining guard interactions (if any emerge)
  are pinned at task-test level.
- Behavior change surface: docs that PREVIOUSLY errored with raw-key
  mismatches now run green (the fix); docs whose keys contain no-default
  placeholders now fail at exit 2 instead of mismatching at route-add —
  strictly better diagnostics, aligned with boot parity (rc-93wct canon).
- Numeric-looking defaults: scanner output is the raw default text
  (`123`), matching the route side's post-parse String leaf — parity holds
  for number-like names too (this is exactly why the scanner layer was
  chosen over the re-serializing layer; see Decision 1).

## Open Questions

None — the papal investigation (e_opus) adjudicated the shape; the
scanner-layer refinement (Decision 1) is mechanically forced by the
quoting evidence and preserves the verdict's intent (engine reuse,
grammar parity, hermeticity).

## Migration Plan

None. This restores the pre-0.42.0 OUTCOME (name match) under the
0.42.0 canon (default-only, no ambient env). No document that was valid
and green on 0.42.0 becomes red unless it carries a no-default
placeholder in an identifier field — in which case it was already broken
at route-add with a worse message.

## Rollout

Single phase, four tasks: (1) helper + error + repositories/beans groups;
(2) intercepts + mock refs + inputs.to groups; (3) integration tests
(repro flip, failure parity, anti-widening, escapes, non-goals);
(4) CONTEXT.md prose. Gates per AGENTS.md; `cargo test -p camel-cli`
(ALL targets) is the authoritative local suite.
