## ADDED Requirements

### Requirement: Single source of truth for the workspace binary path

The `camel-bridge` crate SHALL construct the workspace native-binary location
(`{workspace_root}/bridges/{dir}/build/native/{name}`, with a `.exe` suffix on Windows) in exactly
one place: private helpers `binary_file_name(&BridgeSpec) -> Cow<'_, str>`,
`workspace_binary_path_from(&Path, &BridgeSpec) -> PathBuf`, and
`workspace_binary_path(&BridgeSpec) -> Option<PathBuf>` in `download.rs`. The production workspace
check in `ensure_binary_for_spec`, the cache binary filename tail, and the test skip guard
`workspace_binary_present` SHALL all consume these helpers, and no other construction of the layout
or the `.exe` suffix rule SHALL remain in production code or the skip guard. The drift-guard tests
are deliberately exempt: they MUST restate the layout as independent oracle literals to catch
shared-helper drift. The path constructors SHALL NOT check existence, canonicalize, or create
directories.

#### Scenario: Both sites resolve through one helper

- **GIVEN** the extracted helpers in `download.rs`
- **WHEN** `ensure_binary_for_spec` performs its workspace check and the test guard
  `workspace_binary_present` evaluates skip state
- **THEN** both call `workspace_binary_path`, the cache path uses `binary_file_name`, and a
  crate-wide scan of production code and the skip guard finds no other
  `bridges`/`build`/`native` join chain or `format!("{}.exe")` copy (the guard tests' independent
  oracle literals are exempt).

#### Scenario: Identical paths for every spec kind

- **GIVEN** each bridge spec (`JMS_BRIDGE`, `XML_BRIDGE`, `CXF_BRIDGE`)
- **WHEN** `workspace_binary_path_from(root, spec)` is evaluated
- **THEN** it returns `root/bridges/{dir}/build/native/{name}` where `{dir}` is the spec name minus
  the `-bridge` suffix and `{name}` carries the `.exe` suffix exactly when the Windows cfg is active.

### Requirement: Resolution behaviour unchanged

The extraction SHALL NOT change observable behaviour: resolution precedence (explicit env override →
workspace build → verified cache → download), cache layout and integrity checks, returned paths for
every spec kind, and the skip semantics of the existing download tests SHALL remain identical.

#### Scenario: Skip semantics preserved

- **GIVEN** a development tree where a real workspace binary exists at the helper-computed path
- **WHEN** the existing download tests run
- **THEN** they skip exactly as before the change, because the guard consumes the same path
  production checks.

#### Scenario: Cache and download paths unchanged

- **GIVEN** the extracted `binary_file_name` helper
- **WHEN** `ensure_binary_for_spec` builds the cache bin path (`{cache}/{version}/{tarball}/bin/{name}`)
- **THEN** the resulting path is byte-identical to the pre-change construction on every platform.

### Requirement: Drift-detecting guard tests

The crate SHALL contain two guard tests whose expectations are constructed independently of the
implementation helpers: (1) a deterministic layout test asserting `workspace_binary_path_from`
against a literal `temp_root/bridges/{dir}/build/native/{expected}` for every bridge spec, and
(2) a production-wiring test asserting `ensure_binary_for_spec` returns exactly the workspace path.
The wiring test SHALL use a spec none of the existing guarded tests depend on for skip decisions
(`CXF_BRIDGE`; worker verifies no other test resolves the CXF workspace binary, else falls back to a
shared `Mutex` spanning seeding→resolution→cleanup taken by every `workspace_binary_present`
caller) so a transient seeded file cannot flip other tests' skip state. When the target path already
holds a regular file the test SHALL assert production returns it without mutating the tree; seeding
SHOULD use exclusive `create_new`. `create_new` error handling: `AlreadyExists` → recheck and use
the existing regular file (no skip); `PermissionDenied` or read-only filesystem → the only permitted
environmental skip, reported with a visible message; any other setup error → fail loudly, never
skip. Cleanup SHALL remove only the file and directories the test created. Any layout divergence
between the documented layout, the helper, and the production check SHALL fail these tests instead
of silently skipping.

#### Scenario: Production-only drift fails the wiring test

- **GIVEN** the pre-extraction tree with production's join chain drifted (e.g. `build` → `out`)
- **WHEN** the production-wiring test seeds a dummy binary at the guard's path and runs
  `ensure_binary_for_spec`
- **THEN** the test fails with an assertion failure (production misses the seeded path and falls
  through to cache/download), not a skip and not a setup error.

#### Scenario: Shared-helper drift fails the layout test

- **GIVEN** the extracted shared helper's join chain drifted (e.g. `build` → `out`)
- **WHEN** the deterministic layout test compares `workspace_binary_path_from` against the literal
  documented path
- **THEN** the test fails for every affected spec kind (the wiring test stays green because both
  call sites drift together — this is why the independent oracle exists).

#### Scenario: Wiring run with no real binary present

- **GIVEN** a checkout without a prebuilt workspace binary at `workspace_binary_path(CXF_BRIDGE)`
- **WHEN** the production-wiring test executes
- **THEN** it seeds via exclusive `create_new`, asserts `ensure_binary_for_spec` returns exactly the
  seeded path with an empty temp cache and a loopback release URL (no network), and afterwards
  removes only the file and directories it created.

#### Scenario: Wiring run with an existing regular file

- **GIVEN** a regular file already present at `workspace_binary_path(CXF_BRIDGE)`
- **WHEN** the production-wiring test executes
- **THEN** it does not seed or delete anything and asserts `ensure_binary_for_spec` returns exactly
  that existing path.

### Requirement: Extraction evidence recorded

The change SHALL record an evidence note in `openspec/changes/bridge-path-helper/` containing three
explicit sabotage checkpoints with the actual commands and exit codes: (A) on the pre-extraction
duplicated tree, mutating ONLY production's join chain makes the production-wiring test fail with an
assertion (not a skip, not a setup error), then the mutation is restored; (B) after extraction,
mutating ONLY the shared helper's join chain makes the deterministic layout test fail (wiring test
green — both call sites drift together), then the mutation is restored; (C) a final clean run with
full `cargo test -p camel-bridge` green, at least one executed non-skipped production-wiring run,
and no sabotage retained in the committed tree.

#### Scenario: Evidence note is complete

- **GIVEN** implementation and gate runs are complete
- **WHEN** the evidence note is reviewed
- **THEN** checkpoints A, B, and C each name the mutation applied, the observed outcome, the command
  and exit code, and the committed tree contains no drifted layout.
