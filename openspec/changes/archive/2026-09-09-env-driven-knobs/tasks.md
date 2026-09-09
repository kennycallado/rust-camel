# Tasks: env-driven-knobs

## Phase 1: Unit-tier document env layer

Single deliverable: the `env:` fixture map consulted by all three unit-tier
interpolation seams before inline defaults (spec: specs/mock-testkit/spec.md delta;
design D1-D4). No camel-dsl changes; scenario tier untouched.

### Task 1.1: document env map and identifier lookup threading

**Files**

- crates/camel-cli/src/commands/test/document.rs (modified)
- crates/camel-cli/tests/test_doc_identifier_interpolation.rs (modified)

**Steps**

1. Add to `TestDocument` (after the `repositories` field) the field
   `#[serde(default)] pub env: BTreeMap<String, String>` with a doc comment
   stating: fixture values consulted by every unit-tier interpolation seam
   (route files, inline routes, identifier fields) before inline
   `${env:NAME:-default}` defaults; never the ambient environment; values are
   never themselves interpolated.
2. Change the signature of `interpolate_identifier` from
   `(value: &str, position: &str)` to
   `(value: &str, position: &str, lookup: &dyn Fn(&str) -> Option<String>)`
   and pass `lookup` through to `interpolate_env_with` in place of the
   hardcoded `&|_| None`.
3. At the top of `interpolate_identifier_fields(&mut TestDocument)` insert
   `let env = doc.env.clone();` and define
   `let lookup = &|name: &str| env.get(name).cloned();` (the clone is
   required: the function mutates other `doc` fields while the closure reads
   the env map). Thread `lookup` into every `interpolate_identifier` call:
   the map-key sites live inside `rebuild_identifier_map`, which gains a
   `lookup: &dyn Fn(&str) -> Option<String>` parameter forwarded at its
   internal call (identifier maps, intercept keys/targets, expects/sequence
   mock refs, `inputs[].to`).
4. Update the doc comments on `interpolate_identifier` and the module-level
   identifier pass: the lookup is the document env closure (rc-l7m7t); the
   ambient environment is still never consulted in the unit tier.
5. Refresh the module doc of
   `crates/camel-cli/tests/test_doc_identifier_interpolation.rs` (its header
   currently claims the lookup is default-only by construction): the lookup
   is now the document `env:` closure first, then defaults; cite
   env-driven-knobs.
6. Add the tests below to `crates/camel-cli/tests/test_doc_identifier_interpolation.rs`,
   mirroring the file's existing fixture style (raw YAML strings through the
   same parse entry point the existing tests use).

**Tests**

- name: `env_non_string_value_rejected`
  setup: a `.test.yaml` body declaring `env: { CB_MS: 500 }` (YAML integer
  value) plus a minimal valid route source and `expects`
  action: parse the document
  assert: parsing fails with a serde error locating the `env` field and
  stating a string was expected (an integer/boolean/null value never reaches
  the identifier pass)
  command: `cargo test -p camel-cli --test test_doc_identifier_interpolation env_non_string_value_rejected`
  expected: fails before step 1 (deny_unknown_fields rejects the `env` key
  with an "unknown field" error instead) and passes after
- name: `env_no_default_identifier_resolves`
  setup: document declaring `env: { BEAN_NAME: audit }` and
  `beans: { "${env:BEAN_NAME}": { kind: echo } }`
  action: parse the document
  assert: parsing succeeds and the bean stub registers under `audit` — no
  unresolved-variable error
  command: `cargo test -p camel-cli --test test_doc_identifier_interpolation env_no_default_identifier_resolves`
  expected: fails before, passes after
- name: `env_steers_identifier_key`
  setup: document declaring `env: { CACHE_REPO_NAME: faststub }` and a
  `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`
  block
  action: parse the document
  assert: the repository stub key resolves to `faststub` (parse-level half of
  the steering scenario; the run-level half is Task 1.2's driver test)
  command: `cargo test -p camel-cli --test test_doc_identifier_interpolation env_steers_identifier_key`
  expected: fails before, passes after
- name: `env_two_keys_collide_via_doc_values`
  setup: document declaring `env: { A: y }` and
  `repositories: { cache: { "${env:A:-x}": memory, "y": memory } }`
  action: parse the document
  assert: parsing fails with exit-class error naming the map
  (`repositories.cache`) and the resolved value `y` — the duplicate guard
  sees doc-env-resolved keys
  command: `cargo test -p camel-cli --test test_doc_identifier_interpolation env_two_keys_collide_via_doc_values`
  expected: fails before, passes after
- name: `env_value_text_never_scanned_identifier`
  setup: document declaring `env: { A: "${env:B}" }` and
  `beans: { "${env:A:-d}": { kind: echo } }`
  action: parse the document
  assert: the bean key is the literal text `${env:B}` — fixture values are
  data, never re-scanned
  command: `cargo test -p camel-cli --test test_doc_identifier_interpolation env_value_text_never_scanned_identifier`
  expected: fails before, passes after

**Acceptance**

- `cargo test -p camel-cli --test test_doc_identifier_interpolation` exits 0
  (all pre-existing cases plus the five above)
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- `cargo fmt --check` clean
- `rg -n 'std::env|env::var' crates/camel-cli/src/commands/test/document.rs`
  shows no new ambient reads in the added code paths

- [x] 1.1

### Task 1.2: route-source lookup threading in the LEAN runner

**Files**

- crates/camel-cli/src/commands/test/runner.rs (modified)
- crates/camel-cli/src/commands/test/driver_tests.rs (modified)

**Steps**

1. In `load_routes(doc: &TestDocument, doc_dir: &Path)` define, before the
   three arms, `let lookup = &|name: &str| doc.env.get(name).cloned();`
   (`doc` is borrowed immutably here; no clone needed).
2. In the `route_files_from_root` arm, replace
   `camel_dsl::load_from_file(&full)` with
   `camel_dsl::load_from_file_with_env(&full, lookup)`. Keep the
   `format!("{}: {e}", full.display())` error mapping unchanged.
3. In the `route_files` arm, apply the same swap.
4. In the inline arm, replace
   `camel_dsl::interpolate_yaml_source(&text, &|_| None)` with
   `camel_dsl::interpolate_yaml_source(&text, lookup)`. Keep the
   `Environment variable '{var}' not set (required by inline routes)`
   error mapping unchanged.
5. Update the `load_routes` doc comment: the lookup consults the document
   `env:` map first, then inline defaults, and never the ambient
   environment; typing semantics are unchanged — an int-typed field carrying
   a placeholder still fails the load exactly as `camel run` rejects it,
   including when the document env map supplies the value (string-typed
   substitution; numeric knobs stay on the rc-v1sw track).
6. Add the tests below to `driver_tests.rs` beside the existing
   `lean_*_placeholder_*` tests (lines ~1325-1500), mirroring their
   temp-dir fixture and assertion helpers.

**Tests**

- name: `doc_env_steers_file_route_field`
  setup: temp doc declaring `env: { LEAN_T: docval }` whose `routeFiles`
  reference a route with `title: ${env:LEAN_T:-hello}`
  action: run the document through the LEAN driver path the existing
  `lean_file_route_string_placeholder_loads` test uses
  assert: the run passes and the route carries `title == "docval"`
  command: `cargo test -p camel-cli doc_env_steers_file_route_field`
  expected: fails before, passes after
- name: `doc_env_steers_inline_routes_field`
  setup: temp doc declaring `env: { P: two }` with inline `routes:` carrying
  a string field `${env:P:-one}`
  action: run the document
  assert: the run passes with the field `"two"`
  command: `cargo test -p camel-cli doc_env_steers_inline_routes_field`
  expected: fails before, passes after
- name: `doc_env_no_default_resolves`
  setup: temp doc declaring `env: { LEAN_DEF: supplied }` whose route file
  contains `title: ${env:LEAN_DEF}` (no default)
  action: run the document
  assert: the run passes with `title == "supplied"` — no unresolved-variable
  error
  command: `cargo test -p camel-cli doc_env_no_default_resolves`
  expected: fails before, passes after
- name: `doc_env_int_position_still_fails`
  setup: temp doc declaring `env: { CB_MS: "500" }` whose `routeFiles`
  reference a route with
  `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}`
  action: run the document
  assert: the run fails with a document error (the substituted leaf keeps
  string typing; boot parity holds even when the doc env supplies the value)
  command: `cargo test -p camel-cli doc_env_int_position_still_fails`
  expected: fails before (the `env` key itself would be rejected) and passes
  after
- name: `env_value_never_interpolated_route`
  setup: temp doc declaring `env: { A: "${env:B:-x}literal" }` whose route
  file contains `title: ${env:A:-d}`
  action: run the document
  assert: the run passes and the field carries the verbatim text
  `${env:B:-x}literal`
  command: `cargo test -p camel-cli env_value_never_interpolated_route`
  expected: fails before, passes after
- name: `steering_repository_e2e`
  setup: temp doc declaring `env: { CACHE_REPO_NAME: faststub }`,
  `repositories: { cache: { "${env:CACHE_REPO_NAME:-persistent}": memory } }`,
  and a route file whose cache step declares
  `repository: "${env:CACHE_REPO_NAME:-persistent}"`
  action: run the document
  assert: the run passes — route-side reference and doc-side stub key both
  resolve to `faststub`, the stub registers, and no repository-registration
  error occurs; additionally the test asserts the tier derivation directly:
  `matches!(unit_tier(&doc, &defs), Tier::Lean)` using the `unit_tier` seam
  from `commands/test.rs` (visible to driver_tests as a child module), so
  the env-map-present document provably stays unit tier
  command: `cargo test -p camel-cli steering_repository_e2e`
  expected: fails before, passes after
- name: `ambient_only_stays_unresolved`
  setup: temp doc (no `env:` map) whose route file contains
  `title: ${env:AMBIENT_ONLY}` while `AMBIENT_ONLY` is set in the process
  environment using the same mechanism the existing
  `lean_ignores_ambient_env` test uses
  action: run the document
  assert: the run fails naming `AMBIENT_ONLY` — ambient is never a
  resolution source
  command: `cargo test -p camel-cli ambient_only_stays_unresolved`
  expected: passes before and after (regression pin; must not regress with
  the layer added)

**Acceptance**

- `cargo test -p camel-cli lean_` exits 0 (all carried scenarios intact)
- the seven tests above pass, each invoked with a single name filter:
  `cargo test -p camel-cli doc_env_` (the four `doc_env_*` tests),
  `cargo test -p camel-cli env_value_never_interpolated_route`,
  `cargo test -p camel-cli steering_repository_e2e`, and
  `cargo test -p camel-cli ambient_only_stays_unresolved`
- `cargo clippy -p camel-cli -- -D warnings` exits 0
- `cargo fmt --check` clean

- [x] 1.2

### Task 1.3: user docs, crate context, and bd correction note

**Files**

- docs/src/testing/index.md (modified)
- crates/camel-cli/CONTEXT.md (modified)

**Steps**

1. In `docs/src/testing/index.md`, add a subsection `### Env fixtures`
   inside "Declarative camel test", placed after "Repository stubs" and
   before "CI output and filters". Content: the optional `env:` map of
   string fixture values; consulted by route files, inline routes, and
   doc-side identifiers before inline defaults; steering example
   (`env: { CACHE_REPO_NAME: faststub }` with the placeholder on both the
   route reference and the stub key); values are never interpolated
   themselves; ambient environment is never read; int-typed fields carrying
   placeholders still fail (string-typed substitution; tracked on the
   rc-v1sw track) — mirror the tone and formatting of sibling sections.
2. In `crates/camel-cli/CONTEXT.md`, amend BOTH unit-tier hermeticity
   paragraphs (the rc-4hexo-era text at ~:71 stating route loading resolves
   `${env:}` placeholders default-only through `camel_dsl::load_from_file`,
   AND the identifier-fields paragraph at ~:73): the three unit-tier seams
   now consult the document `env:` map first (rc-l7m7t) — file forms load
   through `camel_dsl::load_from_file_with_env` — then inline defaults;
   ambient remains never-read; typing semantics and boot parity unchanged.
3. From the repo root, append a bd note to rc-l7m7t recording: the
   "(c) covers BOTH numeric knobs and string steering" claim is corrected —
   a string-valued layer cannot serve int-typed positions (string-typed
   substitution is the canon; boot parity), numeric knobs stay deferred to
   the rc-v1sw track (option (a) long-term link refreshed).

**Acceptance**

- `rg -n '### Env fixtures' docs/src/testing/index.md` matches; the section
  sits between "Repository stubs" and "CI output and filters"
- `rg -n 'rc-l7m7t' crates/camel-cli/CONTEXT.md` matches the amended
  paragraph
- `bd show rc-l7m7t --json` notes contain both `rc-v1sw` and a sentence
  stating the covers-both claim is corrected
- `git diff --stat` for this task shows only the two markdown files

- [x] 1.3
