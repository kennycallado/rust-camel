# cli-feature-profiles

## ADDED Requirements

### Requirement: Flavor markers are the single selection surface

camel-cli SHALL expose exactly three flavor marker features —
`flavor-slim`, `flavor-regular`, `flavor-full` — that forward to the
profile composition and serve as the single source of truth for
profile selection in release CI. `default` SHALL select
`flavor-regular`. Marker bodies in this change are aliases only:
`flavor-slim` forwards `slim-http`; `flavor-regular` forwards `full`;
`flavor-full` forwards `full` and `kafka`. Flavor content curation
(what each marker contains beyond these aliases) is out of scope here
and belongs to the flavor-matrix change. When several markers are
enabled simultaneously, the reported flavor SHALL follow the priority
full > regular > slim; builds with no marker enabled report `custom`.

#### Scenario: marker closures are aliases

- **GIVEN** the camel-cli feature table
- **WHEN** `cargo tree` resolves `--features flavor-full` and
  separately `--features full,kafka`; likewise `flavor-regular`
  vs `full`, and `--no-default-features --features flavor-slim`
  vs `--no-default-features --features slim-http`
- **THEN** each marker's resolved package set is identical to its
  forwarding target's package set

#### Scenario: default closure unchanged

- **GIVEN** the golden fixture pinning the default closure
  (`default_closure_matches_golden`)
- **WHEN** `default` is changed from `["full"]` to
  `["flavor-regular"]`
- **THEN** the default closure is byte-identical and the golden
  fixture passes without regeneration

#### Scenario: unmarked builds report custom

- **GIVEN** a build with `--no-default-features --features slim-http`
  (raw composition, no flavor marker)
- **WHEN** the binary reports its version
- **THEN** the flavor suffix is `custom`

### Requirement: version output reports the flavor

The interactive `camel --version` (the non-artifact CLI path, served by
the clap `version` attribute in `crates/camel-cli/src/main.rs`) SHALL
report the crate version followed by the compile-time flavor in a
parseable suffix: `<semver> (<flavor>)`, e.g. `camel 0.49.0 (regular)`.
The flavor SHALL be computed from the enabled marker cfgs with the
documented priority. The compiled-artifact manifest's `RUNTIME_VERSION`
SHALL stay semver-only (the suffix is CLI presentation, not manifest
data). The compiled-artifact runtime `--version` path
(`crates/camel-cli/src/compile/runtime.rs`, reached via
`self_detect_artifact` before clap parses) is OUT OF SCOPE for the
suffix in this change: it keeps printing `camel <semver>` (bare) so the
manifest-schema invariant (`RUNTIME_VERSION` semver-only, machine-read
by `--manifest`) is not weakened by CLI presentation. Adding the flavor
suffix to the artifact trailer path, if triage later needs it, is a
follow-up that MUST keep `RUNTIME_VERSION` and the `--manifest` JSON
semver-only and add the flavor as a separate presentation-only token.

#### Scenario: default build reports regular

- **GIVEN** a default-features build of camel-cli
- **WHEN** the interactive `camel --version` runs (non-artifact CLI)
- **THEN** the output line matches `<semver> (regular)`

#### Scenario: compiled-artifact --version stays bare

- **GIVEN** a `camel compile` artifact built from a flavored binary
- **WHEN** the artifact's `--version` runs (the pre-clap
  `self_detect_artifact` path)
- **THEN** the output line is `camel <semver>` with no flavor suffix,
  and the `--manifest` JSON `runtime_version` field stays semver-only

#### Scenario: marker build reports its flavor

- **GIVEN** builds with `--features flavor-full`, with
  `--no-default-features --features flavor-slim`, and with
  `--features flavor-regular`
- **WHEN** each binary's `--version` runs
- **THEN** the suffixes are `(full)`, `(slim)`, and `(regular)`
  respectively

### Requirement: release legs select a flavor marker, not composed feature lists

The release workflow's build step SHALL select each leg's feature set
by exactly one flavor marker (plus the allocator feature on legs that
need it) and SHALL NOT compose feature lists by string surgery over
multiple matrix keys. The per-leg closures SHALL be identical to the
pre-change closures. Existing post-build assertions (kafka capability
probe, jemalloc link assert) SHALL remain unchanged and green.

#### Scenario: kafka legs build flavor-full

- **GIVEN** the five kafka-capable release legs
  (x86_64-gnu, aarch64-gnu, both macOS, windows-msvc)
- **WHEN** the workflow builds them
- **THEN** the build command carries `--features flavor-full` (no
  kafka-features matrix key, no sed composition) and the closure
  matches the previous default+kafka set

#### Scenario: musl legs build flavor-regular plus allocator

- **GIVEN** the two musl legs
- **WHEN** the workflow builds them
- **THEN** the build command carries `--features flavor-regular,jemalloc`
  and the closure matches the previous default+jemalloc set

#### Scenario: version flavor probed per leg

- **GIVEN** any release leg whose binary is executable on its runner
- **WHEN** the post-build version check runs
- **THEN** the printed flavor suffix equals the leg's matrix flavor

## MODIFIED Requirements

### Requirement: Default feature closure is unchanged

camel-cli's default build SHALL reproduce, feature-for-feature, the
dependency closure of the pre-change default set. The re-aggregation
(`default = ["flavor-regular"]`, which forwards `full`; camel-core
consumed without language features and re-enabled through `lang-*`
forwards) MUST NOT change which crates, crate versions, or forwarded
features resolve in the default graph, and MUST NOT change
default-binary behavior.

#### Scenario: golden dependency snapshot matches

- **GIVEN** a committed golden snapshot of the default closure at the
  change base, generated with
  `cargo tree -p camel-cli -e features,no-dev --prefix none --locked | sed -E 's| \(/[^)]*\)||g; s| \(\*\)||g; s| \[\*\]||g' | sort -u`
  (package lines plus feature-edge lines)
- **WHEN** the profile test regenerates the closure on the changed tree
  with the same command, filtering BOTH sides (live and golden) of
  `camel-core feature "lang-*"` and `camel-core feature "camel-language-*"`
  lines — cargo tree renders feature nodes for dep-declaration activation
  but not for feature-forwarding, and the `lang-*` features moved from
  declaration to forwarding by design
- **THEN** the normalized sorted lists are identical; package-level
  presence of every `camel-language-*` crate in the default closure is
  still asserted by the surviving package lines
