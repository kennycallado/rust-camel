# Tasks: r0nongoals

## Documentation

### Task 1.1: Verify the blessed compile non-goals delta

**Files:**
- `openspec/changes/r0nongoals/specs/cli-compile/spec.md` (modified)

**Steps:**
1. Confirm the blessed delta contains one `Permanent v1 non-goals` requirement under `## ADDED Requirements`.
2. Confirm the requirement states all five exclusions exactly: watch/hot-reload; runtime file discovery/globbing; ambient `Camel.toml`; artifact arguments limited to `--report`, `--help`, `--version`, and `--manifest`, with R4 signature verification as the single sanctioned future surface extension; and compile-time `CAMEL_*` configuration overrides.
3. Confirm the requirement states that deployment-time `${env:NAME}` interpolation remains permitted and is distinct from compile-time `CAMEL_*` overrides.
4. Confirm the three GIVEN/WHEN/THEN scenarios cover proposal rejection, no external configuration or runtime discovery, and rejection of unsupported artifact arguments.

**Tests:**
- `cli_compile_delta_structure`: setup the delta file with the added requirement; action `openspec validate r0nongoals --type change --json`; assert JSON reports `valid: true` and no issues.
- `cli_compile_non_goal_inventory`: setup the blessed delta file; action run one `rg -q -e '<term>' openspec/changes/r0nongoals/specs/cli-compile/spec.md` command per required term, joined with `&&`; assert every command exits 0 for `watch`, `hot-reload`, `runtime file discovery`, `globbing`, `Camel.toml`, `--report`, `--help`, `--version`, `--manifest`, `R4 signature verification`, `CAMEL_*`, and `${env:NAME}`.

**Acceptance:**
- `openspec validate r0nongoals --type change --json` exits 0 with `valid: true`.
- The requirement has at least one parser-valid scenario and names every permanent non-goal.
- No canonical `openspec/specs/` file is edited directly.

- [x] 1.1

### Task 1.2: Verify and retain the compile scope boundary in CONTEXT-MAP

**Files:**
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Confirm the pre-applied `Compiled artifact v1 non-goals` key-term entry is immediately after `Compiled artifact`.
2. Confirm it lists the five permanent exclusions using the same terms as the cli-compile delta.
3. Confirm it states that no external configuration is loaded while deployment-time `${env:NAME}` resolution in the embedded document remains permitted.
4. Confirm it cites the cli-compile specification and ADR-0075, and identifies `camel-cli + camel-dsl` ownership.

**Tests:**
- `context_map_compile_non_goals_citation`: setup the new key-term entry; action `cargo xtask lint-context-citations`; assert the command exits 0.
- `context_map_compile_non_goals_alignment`: setup the delta and CONTEXT-MAP files; action run one `rg -q -e '<term>' <file>` command per required term/file pair, joined with `&&`; assert every command exits 0 for the five exclusions, `R4 signature verification`, and `${env:NAME}` in both documents, and run a separate `rg -q -e 'ADR-0075' CONTEXT-MAP.md` check.

**Acceptance:**
- `cargo xtask lint-context-citations` exits 0.
- The key-term format matches neighboring entries and cites ADR-0075.
- The entry does not duplicate or redefine the existing `Compiled artifact` term.

- [x] 1.2

### Task 1.3: Verify documentation-only change

**Files:**
- `openspec/changes/r0nongoals/proposal.md` (modified)
- `openspec/changes/r0nongoals/design.md` (modified)
- `openspec/changes/r0nongoals/specs/cli-compile/spec.md` (modified)
- `openspec/changes/r0nongoals/tasks.md` (modified)
- `CONTEXT-MAP.md` (modified)

**Steps:**
1. Run `rg -n -e 'TBD|TODO|implement later|fill in details|add appropriate error handling|write tests for the above|similar to Task' openspec/changes/r0nongoals/{proposal.md,design.md,specs/cli-compile/spec.md}` and confirm no forbidden placeholder remains.
2. Run `openspec validate r0nongoals --type change --json` and inspect the result.
3. Run `cargo xtask lint-context-citations` from the worktree.
4. Run `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint --no-deps` because this mission changes durable documentation context.

**Tests:**
- `r0nongoals_documentation_gates`: setup the complete change artifacts and context entry; action run the four commands above; assert each exits 0 and OpenSpec JSON reports `valid: true`.

**Acceptance:**
- Placeholder scan returns no forbidden tokens.
- OpenSpec validation, context citation lint, and required documentation build all exit 0.
- The worktree diff contains only the intended OpenSpec change and CONTEXT-MAP prose.

- [x] 1.3
