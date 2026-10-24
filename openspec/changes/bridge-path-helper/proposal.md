# Proposal: bridge-path-helper

## Why

`ensure_binary_for_spec` (production, `crates/services/camel-bridge/src/download.rs` ~90-134) and the
test skip guard `workspace_binary_present` (~454-469) each independently construct the workspace
native-binary layout `bridges/{dir}/build/native/{name}` including a duplicated `{name}.exe`
Windows-suffix branch. If the production layout ever changes, the test guard diverges silently: the
guard checks a path production no longer takes, the guarded tests stop skipping (or skip for the
wrong reason), and the suite stays green — a false green. Found by r_glm review in mission 367,
tracked as bd rc-lnbdx.

## What Changes

- Extract the layout into one private source of truth in `download.rs`:
  - `binary_file_name(&BridgeSpec) -> Cow<'_, str>` — single `{name}[.exe]` rule, reused by both the
    workspace path and the cache bin path.
  - `workspace_binary_path_from(&Path, &BridgeSpec) -> PathBuf` — pure, root-parameterized layout
    construction (mirrors the existing `find_workspace_root_from_path` testability pattern).
  - `workspace_binary_path(&BridgeSpec) -> Option<PathBuf>` — root discovery + layout.
- Production workspace check and the test skip guard both call the helper. No other copy of the
  layout or suffix logic remains in the crate.
- Add two drift-detecting guard tests: an independent deterministic layout assertion (per bridge
  spec) and a production-wiring test proving `ensure_binary_for_spec` returns the workspace path.
- Excluded (non-goals): downloader/cache/spec-type refactors beyond the helper, bridge Java side,
  `process.rs` (owned by mission 390), version bump.

## Acceptance criteria

- `cargo test -p camel-bridge` green.
- Guard evidence: production-only layout drift makes the wiring test fail; helper-only drift makes
  the layout test fail (sabotage run for evidence only, never committed).
- No duplicated format strings for the layout or `.exe` suffix remain in the crate.
- Full AGENTS.md gate registry passes (with pre-existing exemptions rc-yfsw6 lint-unbounded-wait,
  rc-83l5f cargo-audit RUSTSEC-2026-0387, if still failing on main).

## Risk budget

Low: behaviour-preserving extraction inside one module plus tests. Acceptable risk: temporary
seeding of a dummy file inside the repo tree during the wiring test, mitigated with exclusive
`create_new`, cleanup-before-assert, and removal of only owned paths. Out of bounds: any change to
resolution precedence, cache layout, download/cache behaviour, or `process.rs`.

Refs: bd rc-lnbdx.
