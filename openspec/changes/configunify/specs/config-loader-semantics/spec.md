## ADDED Requirements

### Requirement: Canonical config-loader semantics live once in camel-dsl

The TOML config-loader semantics — deep merge (`merge_toml_values`),
ordered section walk (`section_walk`), ordered include-declaration
collection (`include_declarations`), include-key stripping
(`strip_include_keys`), profile-section selection
(`select_profile_sections`), and the structure/presence predicates
(`has_profile_structure`, `has_selected_profile`) — SHALL be
implemented exactly once, in `camel_dsl::config_semantics`, as pure
`toml::Value` transforms. `camel-config` and `camel-cli`
`compile::sources` SHALL consume these canonical helpers through
delegation: no consumer SHALL assemble its own section/declaration
walk or selection body. Consumer-specific validation and error strings
SHALL remain local. No SYNC comment SHALL remain that pairs these
semantics across crates (the separate `clean_integer`/`clean_i64` pair
is out of scope and keeps its note).

#### Scenario: camel-config delegates to the canonical helpers

- **GIVEN** the camel-config filesystem loader resolving a
  configuration with profiles and includes
- **WHEN** profile selection, include stripping, and merging execute
- **THEN** every merge/selection/walk/declaration primitive is invoked
  from `camel_dsl::config_semantics` and camel-config contains no
  private copy of `merge_toml_values`, no `apply_profile` selection
  body, and no locally assembled include walk

#### Scenario: camel-cli compile walks the canonical section order

- **GIVEN** `camel compile` resolving `--config`/`--profile` sources
- **WHEN** the ordered include walk and the route-overlay walk iterate
  profile sections
- **THEN** the declaration order comes from
  `config_semantics::include_declarations` / `section_walk` and matches
  the filesystem loader's order (`[default]`, then each selected
  non-default profile in selection order, deduplicated), while
  validation and error strings stay local to `compile::sources`

#### Scenario: Strict unknown-profile predicate is canonical where the policy coincides

- **GIVEN** a configuration with `[default]` present and a selected
  profile section absent
- **WHEN** camel-config applies its strict single-profile rule or
  camel-dsl applies the store-side `MalformedVirtualConfig` backstop
- **THEN** the decision uses `has_profile_structure` /
  `has_selected_profile` from `camel_dsl::config_semantics`, and the
  error each consumer emits (camel-config's `"Unknown profile: {}"` or
  camel-dsl's `MalformedVirtualConfig` message) is byte-identical to
  its pre-refactor form

#### Scenario: Per-profile chain-wide compile validation is preserved

- **GIVEN** `camel compile --config C --profile prod --profile qa`
  where only `prod` has a section anywhere in the config chain (root
  or includes)
- **WHEN** source resolution validates the selected profiles
- **THEN** compilation fails with the per-profile
  `UnknownProfile("qa")` error, byte-identical to the pre-refactor
  form, and a golden test locks this partial multi-profile absence
  case

### Requirement: Resolution behavior is frozen by parity goldens

The effective resolved configuration SHALL be proven identical before
and after the refactor by golden tests over a representative matrix
(flat config; `[default]`+profile deep-merge with array replacement;
unknown profile with `[default]` present; ordered includes including a
recursive-`include` declaration; includes carrying their own profile
sections; `${env:VAR:-default}` overrides; virtual-store ordered
multi-profile selection; section-level `routes` replacement). Goldens
SHALL be captured from the pre-refactor code and compared
byte-for-byte after; error message strings are observable behavior and
SHALL NOT change.

#### Scenario: Filesystem loader golden parity

- **GIVEN** committed golden files for the matrix loaded by the
  camel-config public loader on the pre-refactor tree
- **WHEN** the same matrix is loaded after the delegation refactor
- **THEN** the resolved serialized configuration and error strings are
  byte-identical to the goldens

#### Scenario: Virtual store golden parity

- **GIVEN** committed golden files for the matrix built as virtual
  document stores on the pre-refactor tree
- **WHEN** `build_virtual_config` runs after the hoist and extraction
- **THEN** the merged TOML output is byte-identical to the goldens

#### Scenario: Compiled artifact golden parity

- **GIVEN** a compiled artifact built from the matrix fixture with
  `--config`/`--profile`
- **WHEN** the embedded store's resolved merged configuration is
  computed by the runtime discovery path
- **THEN** it is byte-identical to the corresponding golden and to the
  value the pre-refactor tree produced

### Requirement: Virtual config assembly lives in virtual_config module

The virtual-store config assembly (`VirtualConfigRefs`,
`classify_virtual_config`, `build_virtual_config`,
`virtual_config_text`, `parse_virtual_config_toml`, with their tests)
SHALL live in a dedicated `virtual_config.rs` module of camel-dsl,
moved verbatim from `discovery.rs` with no behavior change. The module
exposes no new public API beyond what discovery needs.

#### Scenario: Pure-move extraction

- **GIVEN** the virtual config assembly implemented in `discovery.rs`
- **WHEN** the code moves to `virtual_config.rs`
- **THEN** `discover_virtual_store` results, all parity goldens, and
  the existing camel-dsl and camel-cli suites stay green without any
  assertion modification
