# config-loader-semantics Specification

## Purpose
TBD - created by archiving change configunify. Update Purpose after archive.
## Requirements
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

#### Scenario: Compile enforces the strict unknown-profile mirror

- **GIVEN** `camel compile --config C --profile prod` where `C`
  carries a `[default]` section and `[prod]` exists only in an
  include
- **WHEN** source resolution validates the selected profiles
- **THEN** compilation fails before any output with a local
  `compile::sources` error naming `prod` and the strict rule (a
  configuration with `[default]` must declare at least one selected
  profile itself; sections supplied only by includes do not satisfy
  the strict rule), decided through the canonical
  `has_profile_structure` / `has_selected_profile` predicates, and no
  artifact is written

#### Scenario: Mixed multi-profile selection stays acceptable

- **GIVEN** `camel compile --config C --profile prod --profile canary`
  where `C` carries `[default]` and `[prod]`, and `[canary]` exists
  only in an include
- **WHEN** source resolution validates the selected profiles and the
  artifact boots
- **THEN** compilation succeeds (at least one selected profile is
  declared in the configuration document) and the virtual-store
  assembly merges `[canary]` from the include without the
  strict-profile error, matching the filesystem loader

#### Scenario: Compile strict mirror defers to chain-wide absence

- **GIVEN** `camel compile --config C --profile prod --profile qa`
  where `C` carries `[default]`, `[prod]` exists only in an include,
  and `[qa]` exists nowhere in the chain
- **WHEN** source resolution validates the selected profiles
- **THEN** the per-profile `UnknownProfile("qa")` error takes
  precedence over the strict include-only rejection

#### Scenario: Flat configurations keep lenient include profiles at compile time

- **GIVEN** `camel compile --config C --profile prod` where `C` has
  no profile structure (no `[default]`, no selected section) and
  `[prod]` exists only in an include
- **WHEN** source resolution validates the selected profiles
- **THEN** compilation succeeds, the synthesized fragment and include
  are embedded, and the artifact's virtual-store assembly selects
  `[prod]` from the include leniently, matching the filesystem
  loader's resolved configuration

### Requirement: Resolution behavior is frozen by parity goldens

The effective resolved configuration SHALL be proven identical before
and after the refactor by golden tests over a representative matrix
(flat config; `[default]`+profile deep-merge with array replacement;
unknown profile with `[default]` present; ordered includes including a
recursive-`include` declaration; includes carrying their own profile
sections; `${env:VAR:-default}` overrides; virtual-store ordered
multi-profile selection; section-over-section `routes` replacement —
a selected profile section replacing the `[default]` section's
`routes`, as locked by the profile deep-merge case). Goldens SHALL be
captured from the pre-refactor code and compared byte-for-byte after;
error message strings are observable behavior and SHALL NOT change
except through an explicit openspec change that names the superseded
lock. The former root-level-keys-beside-`[default]` resolution golden
(which locked the silent discard of root keys) is superseded by
openspec change `cfgdrop`, and the mixed-document error lock
(`case_08_error.txt`) is superseded again by openspec change
`cfgdrop2` (root `routes` exemption): the lock is regenerated to the
new error `Display` that names only the still-rejected keys.

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

### Requirement: Profile-structured documents reject discarded root-level config keys

The camel-config filesystem loader SHALL reject, with an error naming
each offending root-level key and the accepted document shapes, any
configuration whose root table carries a recognized `CamelConfig` key
(a member of `KNOWN_TOP_LEVEL_KEYS`, table or scalar) while the
document has profile structure (a `[default]` section or a selected
`[<profile>]` section), with ONE exception: root-level `routes` is
exempt and SHALL take effect through the compile-identical overlay
semantic (root `routes` is the base pattern list; `[default]`, then
the selected profile section in walk order, replaces the accumulated
list — a section that declares no `routes` leaves the root list in
place). The error SHALL name the two accepted shapes: move the key
under `[default]` (overlaid by the selected profile section), or
remove the profile sections to use a flat document. `camel compile`
SHALL enforce the mirror policy on an explicitly selected
`--config` document with profile structure (per the canonical
`has_profile_structure` predicate over the selected profiles): the
same root known non-`routes` keys and the same near-miss root tables
are rejected with compile-local error strings of the same
key-naming and accepted-shapes class, so one document receives one
disposition on both front doors. Flat documents (no profile
structure) SHALL keep accepting root-level config keys unchanged on
both paths, and the rejection SHALL NOT alter the canonical helpers
in `camel_dsl::config_semantics` or the include processing pipeline.

#### Scenario: Root known table beside [default] is rejected

- **GIVEN** a `Camel.toml` with a `[default]` section and a top-level
  `[runtime_journal]` table outside any section
- **WHEN** the filesystem loader loads it (`camel run` / `camel job`
  config path)
- **THEN** loading fails with an error naming `runtime_journal` and
  the accepted shapes (under `[default]` / selected profile section,
  or a flat document), instead of silently discarding the table

#### Scenario: Root known scalar beside [default] is rejected

- **GIVEN** a `Camel.toml` with a `[default]` section and a root-level
  scalar key such as `log_level`
- **WHEN** the filesystem loader loads it
- **THEN** loading fails with the same reject-with-error shape,
  covering the scalar form of the silent-drop class

#### Scenario: Structure via a selected profile section also rejects

- **GIVEN** a `Camel.toml` with no `[default]` but a `[prod]` section
  selected (`CAMEL_PROFILE=prod`), and a root-level
  `[runtime_journal]` table
- **WHEN** the filesystem loader loads it
- **THEN** loading fails with the same error, because the strict
  selection path would discard the root key the same way

#### Scenario: Nested form keeps working

- **GIVEN** a `Camel.toml` with `[default.runtime_journal]` (the
  journal nested under `[default]`)
- **WHEN** the filesystem loader loads it
- **THEN** the loaded `CamelConfig` carries the journal settings —
  the accepted nested shape is unchanged

#### Scenario: Flat documents are unchanged

- **GIVEN** a flat `Camel.toml` (no `[default]`, no profile sections)
  with a root-level `[runtime_journal]` table
- **WHEN** the filesystem loader loads it
- **THEN** the root-level keys ARE the configuration and load
  unchanged — no rejection, no warning

#### Scenario: Flat includes beside a [default] main keep working

- **GIVEN** a main `Camel.toml` with `[default]` and an `include` of
  a flat file (no profile sections) carrying root-level keys
- **WHEN** the filesystem loader loads the chain
- **THEN** the include content applies at its documented priority —
  include processing never feeds the pre-selection root value, so the
  rejection cannot fire on it

#### Scenario: Root routes overlay with compile parity

- **GIVEN** a `Camel.toml` with profile structure and a root-level
  `routes` list
- **WHEN** the filesystem loader loads it and
  `camel compile --config` resolves the same document
- **THEN** both accept the document, and the effective `routes`
  resolution is identical on both paths: the root list applies when
  no walked section declares `routes`, and any declaring section
  (`[default]`, then the selected profile) replaces the list —
  `camel run`'s discovered route patterns and the compile plan
  agree

#### Scenario: Compile mirror-rejects root known keys beside profile structure

- **GIVEN** `camel compile --config C` where `C` carries a `[default]`
  section (or a selected profile section) and a root-level known
  non-`routes` key such as `timeout_ms` or a root `[runtime_journal]`
  table
- **WHEN** source resolution parses the configuration
- **THEN** compilation fails before any output with an error naming
  the offending key(s) and the accepted shapes, matching the
  filesystem loader's disposition for the same document (parity
  asserted by a shared-fixture test)

#### Scenario: Compile path and parity goldens are untouched

- **GIVEN** the compile parity fixtures that mix a root-level
  `routes` key with profile sections
- **WHEN** `camel compile` resolves them and the parity suite
  recomputes resolved configurations
- **THEN** compilation succeeds and every committed golden for an
  unchanged semantic stays byte-identical — the `routes` exception
  preserves the frozen fixtures, and the camel-config battery's
  mixed-document error lock (`case_08_error.txt`) is regenerated to
  name only the still-rejected keys

### Requirement: Root tables that misspell known config keys are rejected

Both config front doors (the camel-config filesystem loader and
`camel compile --config` source resolution) SHALL reject an unknown
root-level TABLE whose name is within Levenshtein distance 2 of a
known top-level key of length ≥ 8, whenever the document has profile
structure, naming the offending table and the probable intended key.
The discriminator SHALL NOT reject or warn root table names that are
not near-miss (they keep unselected-profile semantics: silently
dropped beside an active profile by design, rc-cflo warn when no
profile is active), and SHALL NOT fire on flat documents. The
near-miss predicate SHALL be implemented exactly once, in
camel-config's public root-key policy module, and consumed by
`camel-cli compile::sources` through that import.

#### Scenario: Near-miss table beside a selected profile is rejected

- **GIVEN** a `Camel.toml` with a `[prod]` section selected
  (`CAMEL_PROFILE=prod`) and a root-level `[obsevrability]` table
- **WHEN** the filesystem loader loads it
- **THEN** loading fails with an error naming `obsevrability`, the
  probable intended key `observability`, and the accepted shapes,
  instead of silently dropping the table

#### Scenario: Near-miss table with no active profile is rejected, not warned

- **GIVEN** a `Camel.toml` with `[default]` and a root-level
  `[runtime_jounal]` table, with no `CAMEL_PROFILE` set
- **WHEN** the filesystem loader loads it
- **THEN** loading fails with the near-miss error (selection drops the
  table identically), and the rc-cflo unselected-profile warning does
  not list the near-miss name

#### Scenario: Far-name tables keep unselected-profile semantics

- **GIVEN** a `Camel.toml` with `[default]`, `[staging]`, and
  `CAMEL_PROFILE=prod`
- **WHEN** the filesystem loader loads it
- **THEN** loading succeeds and `[staging]` is silently unselected —
  the discriminator never rejects or warns plausible profile names
  (negative lock)

#### Scenario: Compile mirror-rejects near-miss tables

- **GIVEN** `camel compile --config C --profile prod` where `C`
  carries `[prod]` and a root-level `[obsevrability]` table
- **WHEN** source resolution parses the configuration
- **THEN** compilation fails before any output with an error naming
  the table and the probable intended key, matching the filesystem
  loader's disposition for the same document

#### Scenario: Flat documents never trip the near-miss guard

- **GIVEN** a flat `Camel.toml` with a root-level `[obsevrability]`
  table and no profile structure
- **WHEN** the filesystem loader loads it
- **THEN** the document loads as flat config (unknown keys land in
  `_extra` per the flat-document leniency) — no rejection, no
  near-miss error

