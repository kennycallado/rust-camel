## CRITICAL

- Always read .opencode/instructions/behavior.md
- Read .opencode/instructions/openspec-skills.md for skills policy during OpenSpec commands
- Read CONTEXT-MAP.md for domain language and architecture decisions before any implementation task
- If you have access to caveman skill, activate it
- Never add to staging something that is in .gitignore
- For structured feature workflow, use OpenSpec commands (/opsx:propose, /bless, /opsx:apply, /opsx:archive)

## LANGUAGE POLICY

**Canonical language is English.** The project is international. Write all prose in English: ADRs, `CONTEXT.md`, `CONTEXT-MAP.md`, README, `docs/**`, OpenSpec artifacts, code comments, commit messages, bd issues, and PR descriptions.

- **Identifiers, code, commands, and file paths are never translated.** Translate prose only.
- **Existing non-English docs are technical debt.** Do not author new non-English docs. When you edit a non-English doc, translate the section you touch. Known Spanish backlog: `docs/reviews/**`, parts of `docs/superpowers/archived/**` and `docs/archived/**`. Live ADRs 0046, 0050, 0052 were translated to English on 2026-08-09.
- **Exceptions (non-English is intentional):**
  - Unicode test fixtures (`café`, `María`, `dirección`, `usuários`) that exercise UTF-8 handling.
  - Region-specific integration analysis whose source material is in another language (for example `docs/archived/analysis/mejoras-necesarias-cxf.md`, 112 Baleares). Keep the body in its language but add a short English summary at the top.
- **Quoting translated ADRs:** audit and review docs that quote ADR-0046/0050/0052 in Spanish now hold stale quotes. When you encounter one, update the quote to the English text.

## DOCS GOVERNANCE

- **`docs/audits/` is not versioned** (e_gpt ruling 2026-09-16, ADR-0076 adjacent): durable decisions go to `docs/adr/`, normative behavior to `openspec/specs/`, audit evidence to the fleet inbox or bd. Nothing under `docs/audits` is tracked or re-included from `.gitignore` — `cargo xtask lint-ignore` fails otherwise.
- Security findings are not decisions: accepted risks or rejected mitigations go to an ADR; actionable findings go to bd.

## IMPORTANT

- **ULTRATHINK** → read .opencode/instructions/ultrathink.md
- Code review critical/important issues must be resolved before continuing
- Canonical workflow references use **stable aliases only** (`w_low` / `w_balanced` / `w_high`, `r_glm` / `r_gpt`, `e_glm` / `e_gpt` / `e_opus`). Versioned worker files (`w_glm5.1`, `w_qwen3.7-pro`, …) are for experiments, never doc/prompt references.
- **Never run cargo build/test/clippy/check in the main checkout** — always in your feature worktree. The main `./target` (shared, 75G) must stay cold so it can be purged; an agent build in main re-heats it for hours and risks ENOSPC for all concurrent agents. Humans may run pre-push tests in main.
- **Profiles are debuginfo-free** (`debug=0`, dev/test). Panic messages still show `file:line` (track_caller) — triage works without DWARF. If a task truly needs line-level backtraces or gdb (segfault, dep-frame stacks), use `RUSTFLAGS="-Cdebuginfo=1"` for that single invocation only; never set it permanently (splits fingerprints and the sccache key).

## QUALITY GATES

```
- name: fmt check
  run: cargo fmt --check --all
- name: clippy
  run: |
    cargo clippy --workspace --all-features \
      --exclude camel-cli \
      --exclude camel-component-kafka \
      --exclude security-keycloak \
      --exclude security-wasm-policy \
      --all-targets \
      -- -D warnings
    cargo clippy -p camel-component-kafka --all-targets -- -D warnings
    cargo clippy -p camel-cli -- -D warnings
    cargo clippy -p camel-cli --no-default-features \
      --features flavor-regular,exec --all-targets -- -D warnings
- name: lint-unwrap
  run: cargo xtask lint-unwrap
- name: lint-glob-imports
  run: cargo xtask lint-glob-imports
- name: lint-unsafe-safety
  run: cargo xtask lint-unsafe-safety
- name: lint-missing-docs-ratchet
  run: cargo xtask lint-missing-docs-ratchet
- name: lint-secrets
  run: cargo xtask lint-secrets
- name: lint-single-source
  run: cargo xtask lint-single-source
- name: lint-non-exhaustive
  run: cargo xtask lint-non-exhaustive
- name: lint-commits
  run: |
    git fetch origin main --depth=1
    cargo xtask changelog --check --from FETCH_HEAD --to HEAD
- name: lint-log-levels
  run: cargo xtask lint-log-levels
- name: lint-log-redaction
  run: cargo xtask lint-log-redaction
- name: lint-cancel-tokens
  run: cargo xtask lint-cancel-tokens
- name: lint-test-sleep
  run: cargo xtask lint-test-sleep
- name: lint-no-free-port
  run: cargo xtask lint-no-free-port
- name: lint-no-raw-http-test-server
  run: cargo xtask lint-no-raw-http-test-server
- name: lint-no-env-mutation-unguarded
  run: cargo xtask lint-no-env-mutation-unguarded
- name: lint-unbounded-wait
  run: cargo xtask lint-unbounded-wait
- name: lint-ignore
  run: cargo xtask lint-ignore
- name: lint-publish-cycles
  run: cargo xtask lint-publish-cycles
- name: lint-publish-registration
  run: cargo xtask lint-publish-registration
- name: lint-component-deps
  run: cargo xtask lint-component-deps
- name: lint-bridge-versions
  run: cargo xtask lint-bridge-versions
- name: lint-bridge-releases
  run: cargo xtask lint-bridge-versions --check-releases
- name: lint-gate-forwarding
  run: cargo xtask lint-gate-forwarding
- name: lint-gate-parity
  run: cargo xtask lint-gate-parity
- name: lint-no-deprecated
  run: cargo xtask lint-no-deprecated
- name: lint-context-citations
  run: cargo xtask lint-context-citations
- name: lint-metric-labels
  run: cargo xtask lint-metric-labels
- name: lint-gates-lanes
  run: bash scripts/gates-lanes-test.sh
- name: schema-check
  run: cargo xtask schema --check
- name: doc-build
  run: RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core
    -p camel-builder -p camel-dsl -p camel-endpoint
    -p camel-component-api -p camel-component-validator -p camel-xslt --no-deps
- name: doc-guide-test   # only when docs/** changed; else "N/A — no docs changed"
  run: cd docs && mdbook test
- name: cargo-audit
  run: cargo audit
```

**Local sweep**: `bash scripts/gates-local.sh` runs this registry locally by
parsing the block above (it cannot drift). Network and docs-toolchain gates
(`lint-commits`, `doc-guide-test`, `cargo-audit`, `audit`) are skipped by
default and left to CI; `GATES_ONLY=`/`GATES_SKIP=` narrow or widen the run.
The merge gate and any release bump require exit 0.

## COMPONENT LANES

Every workspace crate a change touches must declare its integration lane in the table below. `--lib` alone never compiles `tests/` or feature-gated test files — rc-2co7z: `tests/decision_http_resilience.rs` is `#![cfg(feature = "decisions")]`, so a `--lib` run stays green while the suite never builds.

When a touched crate has no lane entry, the sweep prints `NO LANE for <crate> — add one to AGENTS.md ## COMPONENT LANES` and fails if and only if the crate has an integration surface — at least one `tests/**/*.rs` file (feature-gated or not) or an explicit `[[test]]` target. A crate with no integration surface prints the line but does not block.

Add one `<crate> | <mode> | <command>` row per command (several rows per crate are normal and all run). Pick the feature set the code path needs by reading the tests' `#![cfg(feature = ...)]` gates and the crate's `[features]` — do not guess; keep cross-crate commands byte-identical to the CI step they come from.

`docker` rows are infra-dependent (testcontainers, live services, prebuilt bridge/guests). They are DECLARED (so they never trip the NO LANE rule) but run only under `GATES_LANES=all`; CI owns them (`.github/workflows/integration-*.yml`, ci.yml container jobs), so a machine without Docker passes the registry sweep. `GATES_LANES=0` disables the lane phase; `GATES_SKIP`/`GATES_ONLY` treat it as gate name `lanes`.

Justified exclusions:

- `component_emission_test` and `cache_payload_offload` are cross-cutting (camel-core + camel-otel + camel-component-api / camel-config) with no single trigger crate — CI runs them.
- The WASM per-target `-- --ignored` loop (ci.yml, allowlist-driven per ADR-0054) needs pre-built wasm32-wasip2 guests — CI's wasm job owns it; the `camel-component-wasm` local row covers the non-ignored surface.
- `camel-cli` as a trigger keeps the fail-safe until a mission declares its lane deliberately.

```lanes
# Format: one row per command, `<crate> | <mode> | <command>`.
# mode `local`  -> runs in every local sweep
# mode `docker` -> infra-dependent (testcontainers, live services, prebuilt
#                  bridge/guests): declared, runs only under GATES_LANES=all
# The parser splits on the FIRST TWO `|` only, so a command may contain `|`.
# Several rows per crate are normal; all of them run. Blank lines and lines
# starting with `#` are ignored.

# --- camel-component-llm: own tests/ + feature-gated suite (rc-2co7z)
camel-component-llm | local | cargo test -p camel-component-llm --features decisions
# --- camel-core: ci.yml:146 boundary suite
camel-core | local | cargo test -p camel-core --test hexagonal_architecture_boundaries_test
# --- own tests/ suites (default features)
camel-component-redis | local | cargo test -p camel-component-redis
camel-component-mail | local | cargo test -p camel-component-mail
camel-component-http | local | cargo test -p camel-component-http
camel-component-jms | local | cargo test -p camel-component-jms
camel-component-cxf | local | cargo test -p camel-component-cxf
camel-xj | local | cargo test -p camel-xj
camel-xslt | local | cargo test -p camel-xslt
camel-component-validator | local | cargo test -p camel-component-validator
camel-component-mcp | local | cargo test -p camel-component-mcp
camel-processor | local | cargo test -p camel-processor
camel-json-schema | local | cargo test -p camel-json-schema
# --- camel-cli: declared deliberately (mission 601) — the lane sweep
#     fail-safe asked for this declaration; matches ci.yml:288
camel-cli | local | cargo test -p camel-cli --no-default-features --features flavor-regular,exec --no-fail-fast
# --- camel-component-mock: own tests/ suite
camel-component-mock | local | cargo test -p camel-component-mock
# --- camel-integration-test: tier.rs BuilderStep walk touched (sqltrans-portmodel);
#     own tests/ suite, local (no infra needed for the unit surface)
camel-integration-test | local | cargo test -p camel-integration-test
# --- xtask: own tests/ (triggered by trustpub-registrations.toml edits)
xtask | local | cargo test -p xtask
camel-language-rhai | local | cargo test -p camel-language-rhai
camel-auth | local | cargo test -p camel-auth
camel-bundles | local | cargo test -p camel-bundles
# --- camel-config: own tests/ + integration surface (mission 603)
camel-config | local | cargo test -p camel-config
# own tests/ (#[ignore] targets excluded by default)
camel-component-wasm | local | cargo test -p camel-component-wasm
# --- sqltrans-portmodel: crates with integration surfaces touched by the change
camel-api | local | cargo test -p camel-api
camel-dsl | local | cargo test -p camel-dsl
camel-builder | local | cargo test -p camel-builder
camel-lint | local | cargo test -p camel-lint
# WITH the security feature: the bundle boot-rejection fixture cfg's out without it
camel-bundles | local | cargo test -p camel-bundles --features security

# --- integration-ollama-smoke.yml:64 (docker)
camel-component-llm | docker | cargo test -p camel-component-llm --test ollama_live --features ollama -- --ignored ollama_embed
# --- integration-sql.yml (docker)
camel-component-sql | docker | cargo test -p camel-cli --no-default-features --features integration-sql,itest-e2e --test test_scenario_cli_e2e
# --- integration-redis.yml (docker)
camel-component-redis | docker | cargo test -p camel-cli --no-default-features --features integration-redis,itest-e2e --test test_scenario_cli_e2e
camel-component-redis | docker | cargo test -p camel-integration-test --features redis,redis-live --lib
camel-component-redis | docker | cargo test -p camel-integration-test --features redis --test redis_state_test
# --- ci.yml container job suites (docker)
camel-component-redis | docker | cargo test -p camel-test --features integration-tests --test redis_test
camel-redis-repo | docker | cargo test -p camel-test --features integration-tests --test redis_repositories_test
camel-component-kafka | docker | cargo test -p camel-test --features integration-tests --test kafka_test
camel-component-opensearch | docker | cargo test -p camel-test --features integration-tests --test opensearch_test
camel-component-container | docker | cargo test -p camel-test --features integration-tests --test container_test
camel-component-cxf | docker | cargo test -p camel-test --features integration-tests --test cxf_test
camel-component-jms | docker | cargo test -p camel-test --features integration-tests --test jms_test
camel-xj | docker | cargo test -p camel-test --features integration-tests --test xml_bridge_xj_test
camel-xslt | docker | cargo test -p camel-test --features integration-tests --test xml_bridge_xslt_test
camel-component-validator | docker | cargo test -p camel-test --features integration-tests --test xml_bridge_validator_test
# ci.yml:193 (soft-skips without the native xml-bridge binary)
camel-component-validator | docker | cargo test -p camel-test --features integration-tests --test xml_bridge_xerces_diagnostics_test
# ci.yml:235 (HTTP consumer kernel auth)
camel-component-http | docker | cargo test -p camel-test --features integration-tests --test kernel_fail_closed_test
# ci.yml:235 (kernel_authenticate side; duplicate owner row is intentional)
camel-auth | docker | cargo test -p camel-test --features integration-tests --test kernel_fail_closed_test
# ci.yml:233
camel-auth | docker | cargo test -p camel-test --features integration-tests --test auth_multi_credential_test
# ci.yml:231
camel-platform-kubernetes | docker | cargo test -p camel-test --features integration-tests --test kubernetes_test
# --- integration-mail.yml (docker)
camel-component-mail | docker | MAIL_ITEST=1 cargo test -p camel-component-mail --test greenmail_tier --test mailpit_tier
# --- integration-http.yml (docker)
camel-component-http | docker | cargo test -p camel-integration-test --features http
camel-component-http | docker | cargo test -p camel-cli --features integration-http,itest-e2e --test test_tier_filters
camel-component-http | docker | cargo test -p camel-cli --features integration-http,itest-e2e --test test_scenario_cli_e2e
# --- integration-http.yml:47 (docker)
camel-bundles | docker | cargo test -p camel-bundles --features kafka
# --- integration-rhai.yml (docker)
camel-language-rhai | docker | cargo test -p camel-integration-test --features lang-rhai --test reconcile_budget_test
# --- integration-surreal.yml (docker)
camel-component-surrealdb | docker | cargo test -p camel-cli --no-default-features --features integration-surreal,itest-e2e --test test_scenario_cli_e2e
camel-component-surrealdb | docker | cargo test -p camel-integration-test --features surreal --test surreal_state_test
# --- integration-wasm.yml (docker)
camel-component-wasm | docker | cargo test -p camel-integration-test --features wasm
# --- camel-test owns the live suites above; a change that edits camel-test
#     itself must still compile its whole integration surface locally
#     (rc-2co7z: --lib never compiles tests/), and runs it under GATES_LANES=all
camel-test | local | cargo test -p camel-test --features integration-tests --no-run
# --- camel-test workspace-unification suites (rc-4xka4): under `--workspace`, feature
#     unification enables camel-core/lang-rhai (rc-lhd0y), a class invisible to `-p` lanes;
#     default list in-row (rc-lhd0y's fix set), `CAMEL_TEST_WORKSPACE_SUITES` overrides; NOT full `cargo test --workspace` (Docker/bridges stay CI).
camel-test | local | suites="${CAMEL_TEST_WORKSPACE_SUITES-dev_watch_script_test dev_watch_proto_test}"; allowed="dev_watch_script_test dev_watch_proto_test"; [[ -n "${suites//[[:space:]]/}" ]] || { echo "camel-test workspace lane: empty CAMEL_TEST_WORKSPACE_SUITES suite list" >&2; exit 2; }; set -f; for s in $suites; do case "$s" in *[\*\?\[]*) echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES must list literal suite names, got '$s'" >&2; exit 2;; esac; ok=0; for a in $allowed; do if [[ "$s" == "$a" ]]; then ok=1; break; fi; done; if [[ "$ok" != 1 ]]; then echo "camel-test workspace lane: CAMEL_TEST_WORKSPACE_SUITES entry '$s' is not an allowlisted local suite (allowed: $allowed)" >&2; exit 2; fi; done; rc=0; for s in $suites; do cargo test --workspace --test "$s" || rc=1; done; exit "$rc"
camel-test | docker | cargo test -p camel-test --features integration-tests
# --- sqltrans-exec step 8: PG runtime fixtures P1-P6 (transacted-block
#     probes). Docker/mapped-port lane is the acceptance authority; the local
#     host-network path is opt-in via SQLTRANS_PG_URL.
camel-test | docker | cargo test -p camel-test --features integration-tests --test sql_transacted_test
```

## NEW-CODE-ONLY STYLE NORMS (ADR-0091)

These norms apply to new code. Existing code is grandfathered; do not retrofit in a feature PR. Full policy: `docs/adr/0091-rust-style-policy.md`.

- Qualify top-level function calls with the module (`mem::transmute(x)` not `transmute(x)`).
- Import types unqualified (`use regex::Regex; Regex::new()`); call functions via their module.
- Locally-scoped trait imports unless the trait is used file-wide.
- One module per `use` statement (no `use foo::{Bar, baz::Baz}` nested braces).
- `enum` over `bool` for non-obvious API parameters.
- Named fields over multi-field tuples.
- Prefer `Into<T>` and `impl IntoIterator` in library APIs; `&[T]`/`&str`/`&T` over `&Vec`/`&String`/`&Box`.
- `expect("reason")` over `unwrap()` outside tests (`lint-unwrap` enforces the ban).
- Options structs implement `Default`; newtypes over aliases when the alias would leak invariants.
- Pattern-match over check-then-unwrap.
- Context strings in `assert!` when failure is not self-evident.
- Macros: prefer functions, then `macro_rules!`, then proc-macros.
- `#[must_use]` on public error types and pure functions where ignoring the return is a bug.

## COMMIT CONVENTION (caveman-commit, strict)

```
Subject: <type>(<scope>): <imperative summary>  — ≤50 chars when possible, cap 72
Body:    ONLY when "why" is non-obvious, breaking, migration, or security
         wrap 72, bullets `-` not `*`, reference bd at end: `Bd: rc-xxx`

Types: feat, fix, refactor, perf, docs, test, chore, build, ci, style, revert

NEVER:
- Restate what the diff already says ("this commit does X")
- Explain oracle/reviewer reasoning in commit body (that lives in bd/CONTEXT.md)
- "Generated with..." / AI attribution / emoji
- Paste spec/plan text into body

Body length budget: 3-8 lines for normal commits. Major feature merges
(e.g. new component) may use bullet list of technical changes, still no
prose. If body exceeds ~15 lines, the commit is doing too much — split it.
```

# context-mode — MANDATORY routing rules

You have context-mode MCP tools available. These rules are NOT optional — they protect your context window from flooding. A single unrouted command can dump 56 KB into context and waste the entire session.

## BLOCKED commands — do NOT attempt these

### curl / wget — BLOCKED

Any shell command containing `curl` or `wget` will be intercepted and blocked by the context-mode plugin. Do NOT retry.
Instead use:

- `context-mode_ctx_fetch_and_index(url, source)` to fetch and index web pages
- `context-mode_ctx_execute(language: "javascript", code: "const r = await fetch(...)")` to run HTTP calls in sandbox

### Inline HTTP — BLOCKED

Any shell command containing `fetch('http`, `requests.get(`, `requests.post(`, `http.get(`, or `http.request(` will be intercepted and blocked. Do NOT retry with shell.
Instead use:

- `context-mode_ctx_execute(language, code)` to run HTTP calls in sandbox — only stdout enters context

### Direct web fetching — BLOCKED

Do NOT use any direct URL fetching tool. Use the sandbox equivalent.
Instead use:

- `context-mode_ctx_fetch_and_index(url, source)` then `context-mode_ctx_search(queries)` to query the indexed content

## REDIRECTED tools — use sandbox equivalents

### Shell (>20 lines output)

Shell is ONLY for: `git`, `mkdir`, `rm`, `mv`, `cd`, `ls`, `npm install`, `pip install`, and other short-output commands.
For everything else, use:

- `context-mode_ctx_batch_execute(commands, queries)` — run multiple commands + search in ONE call
- `context-mode_ctx_execute(language: "shell", code: "...")` — run in sandbox, only stdout enters context

### File reading (for analysis)

If you are reading a file to **edit** it → reading is correct (edit needs content in context).
If you are reading to **analyze, explore, or summarize** → use `context-mode_ctx_execute_file(path, language, code)` instead. Only your printed summary enters context.

### grep / search (large results)

Search results can flood context. Use `context-mode_ctx_execute(language: "shell", code: "grep ...")` to run searches in sandbox. Only your printed summary enters context.

## Tool selection hierarchy

1. **GATHER**: `context-mode_ctx_batch_execute(commands, queries)` — Primary tool. Runs all commands, auto-indexes output, returns search results. ONE call replaces 30+ individual calls.
2. **FOLLOW-UP**: `context-mode_ctx_search(queries: ["q1", "q2", ...])` — Query indexed content. Pass ALL questions as array in ONE call.
3. **PROCESSING**: `context-mode_ctx_execute(language, code)` | `context-mode_ctx_execute_file(path, language, code)` — Sandbox execution. Only stdout enters context.
4. **WEB**: `context-mode_ctx_fetch_and_index(url, source)` then `context-mode_ctx_search(queries)` — Fetch, chunk, index, query. Raw HTML never enters context.
5. **INDEX**: `context-mode_ctx_index(content, source)` — Store content in FTS5 knowledge base for later search.

## Output constraints

- Keep responses under 500 words.
- Write artifacts (code, configs, PRDs) to FILES — never return them as inline text. Return only: file path + 1-line description.
- When indexing content, use descriptive source labels so others can `search(source: "label")` later.

## ctx commands

| Command       | Action                                                                            |
| ------------- | --------------------------------------------------------------------------------- |
| `ctx stats`   | Call the `stats` MCP tool and display the full output verbatim                    |
| `ctx doctor`  | Call the `doctor` MCP tool, run the returned shell command, display as checklist  |
| `ctx upgrade` | Call the `upgrade` MCP tool, run the returned shell command, display as checklist |

<!-- BEGIN BEADS INTEGRATION v:1 profile:full hash:f65d5d33 -->

## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-powered version control with native sync
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
scripts/bd-create-idempotent.sh "Issue title" ...  # retry-safe create: reuses an identical open bd <10min old
```

**Claim and update:**

```bash
bd update <id> --claim --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task atomically**: `bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Quality

- Use `--acceptance` and `--design` fields when creating issues
- Use `--validate` to check description completeness

### Lifecycle

- `bd defer <id>` / `bd supersede <id>` for issue management
- `bd stale` / `bd orphans` / `bd lint` for hygiene
- `bd human <id>` to flag for human decisions
- `bd formula list` / `bd mol pour <name>` for structured workflows

### Auto-Sync

bd automatically syncs via Dolt:

- Each write auto-commits to Dolt history
- Use `bd dolt push`/`bd dolt pull` for remote sync
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

## Session Completion

**When ending a work session**, you MUST complete ALL steps below.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Commit locally** - Ensure all changes are committed:
   ```bash
   git status  # MUST show clean working tree
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed locally
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**

- **NEVER run `git push`** - Pushing costs money on GitHub Actions. Only the human does push.
- **NEVER run `git push`** - This is PROHIBITED. No exceptions.
- Commit locally and stop. The user will push when ready.
- If you accidentally push, that is a critical error

<!-- END BEADS INTEGRATION -->
