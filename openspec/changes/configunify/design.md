# Design: configunify

## Approach

Hoist the triplicated config-loader semantics into a new canonical
public module `camel_dsl::config_semantics`, then extract the
virtual-store config assembly out of `discovery.rs` into
`virtual_config.rs`. Dependency direction already permits the hoist:
camel-config and camel-cli both depend on camel-dsl; no Cargo.toml
changes.

The refactor is behavior-frozen. Parity golden tests are authored
FIRST, against the un-refactored code, locking the effective resolved
configuration for a representative matrix across all three resolution
paths. The refactor then must keep those goldens byte-identical,
including error message strings.

### Canonical API (`crates/camel-dsl/src/config_semantics.rs`)

All functions are pure `toml::Value` transforms, `pub`, unit-testable:

- `merge_toml_values(base, overlay)` — deep merge; tables recursive,
  every other value (arrays included) replaced by the overlay.
- `section_walk(profiles: &[String]) -> Vec<String>` — the canonical
  ordered section list: `"default"`, then each selected non-default
  profile in selection order, deduplicated. This one list feeds the
  include walk, the route-overlay walk, and include stripping.
- `strip_include_keys(value, profiles)` — removes `include` from the
  top-level table and from every walked section.
- `select_profile_sections(value, profiles)` — generalized ordered
  profile-section selection (store form): base = `[default]` if
  present else the first selected section present; each selected
  section overlays in order; the result replaces the root; a document
  with no profile structure stays as-is.
- `has_profile_structure(value, profiles) -> bool` —
  `[default]` present or any selected section present.
- `has_selected_profile(value, profiles) -> bool` — any selected
  profile section present. Together with `has_profile_structure` this
  is the strict unknown-profile predicate for the consumers whose
  policy is exactly that rule (camel-config's single-profile dispatch
  and the store-side `MalformedVirtualConfig` backstop): the strict
  error fires iff structure is present and no selected section is.
  camel-config's `"Unknown profile: {}"` text and the store's
  `MalformedVirtualConfig` message stay local. NOTE:
  `compile::sources` applies a STRICTER per-profile policy — every
  selected profile section must exist somewhere in the config chain
  (root config or any include), each missing profile rejected
  individually (`UnknownProfile("<name>")`). That per-profile,
  chain-wide validation loop stays in `compile::sources` verbatim and
  MUST NOT be replaced by the any-present predicate (which would
  silently accept `--profile prod --profile qa` when only `prod`
  exists).
- `include_declarations(value, profiles) ->
  Vec<(String, &toml::Value)>` — the ordered include-declaration
  collection in canonical walk order: top-level `include` first, then
  `include` inside each walked section, each entry carrying its origin
  label (`""` for top-level, the section name otherwise). Consumers
  validate each raw value with their own error strings
  (camel-config's loader text, sources.rs's `toml_string_list`
  `"{section}.include"` text) and flatten; no consumer assembles its
  own section/declaration walk.

Consumers keep their wrappers and their observable error strings:

- camel-config: `apply_profile` / `apply_profile_lenient` /
  `merge_toml_values` / `extract_includes` remain as thin wrappers
  (signatures unchanged, bodies delegate to canonical helpers). The
  strict/lenient dispatch delegates to `has_profile_structure` /
  `has_selected_profile`; the `"Unknown profile: {}"` error text stays
  local. The include walk iterates `include_declarations`.
- camel-dsl `discovery.rs`: `strip_include_keys`, `select_profile_sections`,
  `merge_toml_values` locals are deleted; `build_virtual_config` calls
  the canonical helpers. Its `MalformedVirtualConfig` unknown-profile
  message is preserved.
- camel-cli `compile/sources.rs`: the ordered include walk consumes
  `include_declarations` and the route-overlay walk iterates
  `section_walk()`; `toml_string_list` validation and `SourceError`
  strings stay local. The per-profile unknown-profile validation
  (every selected profile must exist somewhere in the config chain,
  each missing one rejected with its own `UnknownProfile("<name>")`)
  stays local and verbatim — the canonical any-present predicates do
  not apply to it.

The `clean_integer`/`clean_i64` mirror is explicitly out of scope
(different rule pair, crate-purity note in place).

### Parity golden proof

Fixture matrix (same content replicated per crate, inputs are data not
code): flat config; `[default]`+profile with deep-merge and
array-replace (`routes`); unknown profile with `[default]` present
(error string locked); ordered includes with a recursive-`include`
declaration (warn + strip); include carrying its own profile sections;
`${env:VAR:-default}` overrides; virtual-store ordered multi-profile
selection; partial multi-profile absence (`--profile prod --profile
qa`, only `prod` present — compile-path `UnknownProfile("qa")` locked);
section-level `routes` replacing top-level `routes`.

- camel-config: golden test in `config_tests` loading each matrix case
  through the public loader, serializing the resolved configuration
  (post profile/include/env resolution), compared byte-for-byte to
  committed goldens.
- camel-dsl: in-crate test building a `VirtualDocumentStore` per
  matrix case and golden-comparing `build_virtual_config` output.
- camel-cli: golden test compiling a matrix fixture with
  `--config`/`--profile`, reading the embedded store from the
  artifact, and golden-comparing the resolved merged configuration
  (the same value the runtime would compute).

Goldens are generated on the PRE-refactor tree (regeneration interface
is exactly `UPDATE_GOLDENS=1`), committed, and never regenerated after
Phase 2 begins. The existing `feature_profiles` golden deptree test is
an additional stability gate (run 3x).

## Affected crates

- camel-dsl: new `config_semantics.rs` (canonical helpers + unit
  tests); new `virtual_config.rs` (moved assembly); `discovery.rs`
  slims; `lib.rs` gains the two modules.
- camel-config: `config.rs` delegates merge/selection/walk; wrappers
  and error strings unchanged; parity golden test added.
- camel-cli: `compile/sources.rs` consumes `section_walk`; parity
  golden test added; ride-along A color-audit fixes (tests only).
- `.github/workflows/ci.yml`: gated-test compile-gate step
  (ride-along B).

## Architecture boundaries

camel-dsl is the DSL/parsing layer and already hosts the virtual-store
runtime assembly; pure `toml::Value` semantics belong there. No new
dependency edges, no runtime (camel-core) changes, no artifact/trailer
format change (ADR-0075 store layout untouched — only where the merge
code lives). The data/control-plane split is respected: config
resolution stays configuration-plane. camel-core is NOT touched (the
hexagonal boundary gate stays cold).

## Phases

### Phase 1: Parity golden harness

- **Goal:** lock pre-refactor behavior of all three resolution paths.
- **Dependencies:** none (runs against unmodified main code).
- **Externally-visible types/interfaces:** new test files + committed
  golden fixtures; no production surface.
- **Deliverable:** green parity golden tests on the un-refactored tree.
- **Exit-criteria:** `cargo test -p camel-config -p camel-dsl -p
  camel-cli` green with goldens committed; regeneration mode works.

### Phase 2: Canonical config_semantics + delegation

- **Goal:** single canonical implementation; mirrors delegate; SYNC
  comments die.
- **Dependencies:** Phase 1 goldens (the safety net).
- **Externally-visible types/interfaces:**
  `camel_dsl::config_semantics::{merge_toml_values, section_walk,
  include_declarations, strip_include_keys, select_profile_sections,
  has_profile_structure, has_selected_profile}`.
- **Deliverable:** delegated camel-config, slimmed discovery.rs,
  sources.rs on `section_walk`.
- **Exit-criteria:** all Phase 1 goldens byte-identical (no
  regeneration); full crate suites green; zero SYNC comments on the
  hoisted semantics.

### Phase 3: virtual_config.rs extraction

- **Goal:** pure move of the virtual config assembly (~230 lines:
  `VirtualConfigRefs`, `classify_virtual_config`,
  `build_virtual_config`, `virtual_config_text`,
  `parse_virtual_config_toml`, their tests) into
  `crates/camel-dsl/src/virtual_config.rs`.
- **Dependencies:** Phase 2.
- **Externally-visible types/interfaces:** `pub(crate)` module; no new
  public surface.
- **Deliverable:** `discovery.rs` materially smaller; module list in
  `lib.rs`.
- **Exit-criteria:** zero behavioral diff (goldens green); `cargo fmt`
  + clippy clean.

### Phase 4: Ride-along A — nested-cargo color audit (rc-0omir)

- **Goal:** per-test verdict for the 9 camel-cli tests spawning nested
  cargo under `CARGO_TERM_COLOR=always`: fixed with the
  feature_profiles pattern (env pin `CARGO_TERM_COLOR=never` on the
  cargo child + ANSI strip at parse sites) or documented immunity.
- **Dependencies:** none (independent of Phases 1-3).
- **Deliverable:** fixes where sensitive; audit verdicts recorded in
  the bd rc-0omir closure note (one verdict + reason per audited
  test); rc-0omir closed.
- **Exit-criteria:** camel-cli full suite green locally with
  `CARGO_TERM_COLOR=always` exported.

### Phase 5: Ride-along B — CI compile-gate (rc-khjnb)

- **Goal:** named CI step
  `cargo check -p camel-test --features integration-tests --tests`
  before "Test (full workspace)" in `full-tests-linux`, comment
  referencing rc-8g35d/rc-khjnb.
- **Dependencies:** none.
- **Deliverable:** ci.yml diff; step verified locally compile-only
  (worktree).
- **Exit-criteria:** YAML valid; command proven to compile the gated
  set including `master_kubernetes_test` locally.

## Alternatives considered

- **New shared crate (e.g. camel-config-semantics):** rejected — three
  small pure functions do not justify workspace/dependency churn;
  camel-dsl already sits below both consumers.
- **Invert the dependency (camel-dsl depends on camel-config):**
  rejected — creates a cycle; camel-config already depends on camel-dsl.
- **Trait-based abstraction shared by mirrors:** rejected — the mirrors
  must be behaviorally IDENTICAL, not polymorphic; one concrete
  implementation is the point.
- **Hoist during the extraction move (single phase):** rejected —
  separating "pure move" from "semantic hoist" keeps each diff
  reviewable and the goldens isolate which step could regress.
