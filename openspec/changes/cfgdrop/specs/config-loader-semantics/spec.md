## MODIFIED Requirements

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
error message strings are observable behavior and SHALL NOT change.
The former root-level-keys-beside-`[default]` resolution golden
(which locked the silent discard of root keys) is superseded by
openspec change `cfgdrop`: that document shape is now a load-time
rejection, and the parity battery locks the rejection's full error
`Display` byte-for-byte in its place (`case_08_error.txt`).

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

## ADDED Requirements

### Requirement: Profile-structured documents reject discarded root-level config keys

The camel-config filesystem loader SHALL reject, with an error naming
each offending root-level key and the accepted document shapes, any
configuration whose root table carries a recognized `CamelConfig` key
(a member of `KNOWN_TOP_LEVEL_KEYS`, table or scalar) while the
document has profile structure (a `[default]` section or a selected
`[<profile>]` section). The error SHALL name the two accepted shapes:
move the key under `[default]` (overlaid by the selected profile
section), or remove the profile sections to use a flat document. Flat
documents (no profile structure) SHALL keep accepting root-level
config keys unchanged, and the rejection SHALL NOT alter the
canonical helpers in `camel_dsl::config_semantics`, the include
processing pipeline, or the compile path's frozen parity goldens.

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

#### Scenario: Compile path and parity goldens are untouched

- **GIVEN** the compile parity fixtures that mix a root-level
  `routes` key with profile sections
- **WHEN** `camel compile` resolves them and the parity suite
  recomputes resolved configurations
- **THEN** compilation and every committed golden stay byte-identical
  — the rejection lives in the camel-config filesystem loader only
