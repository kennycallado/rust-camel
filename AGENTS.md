## CRITICAL

- Always read .opencode/instructions/behavior.md
- Read CONTEXT-MAP.md for domain language and architecture decisions before any implementation task
- If you have access to caveman skill, activate it
- Never add to staging something that is in .gitignore

## IMPORTANT

- **ORCHESTRATE** → read .opencode/instructions/orchestrate.md
- **ULTRATHINK** → read .opencode/instructions/ultrathink.md
- Code review critical/important issues must be resolved before continuing

## NEW FEATURE WORKFLOW

```
Use superpowers + worktree from main in ./.worktrees/
activate ORCHESTRATE (.opencode/instructions/orchestrate.md) and TDD where possible.
Delegate implementation to workers (w_gpt5.3-codex default, w_qwen3.6-pro or w_glm5.1 as alternatives).
IMPORTANT! force subagentes to read AGENTS.md.
CRITICAL! Spec + code review always after a task using reviewer `r_glm5.1`.
Don't advance tasks with critical/important issues unresolved and resolve minor issues if relevant.
Escalate to experts (e_gpt5.5, e_opus4.7) only when workers are stuck after 2 attempts.
If a subagent needs ULTRATHINK instructions are in .opencode/instructions/ultrathink.md
```

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
      -- -D warnings
    cargo clippy -p camel-component-kafka --all-targets -- -D warnings
    cargo clippy -p camel-cli -- -D warnings
- name: lint-unwrap
  run: cargo xtask lint-unwrap
- name: lint-secrets
  run: cargo xtask lint-secrets
- name: lint-log-levels
  run: cargo xtask lint-log-levels
- name: schema-check
  run: cargo xtask schema --check
- name: cargo-audit
  run: cargo audit
```

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

## MERGE TO MAIN

```
Skill: finishing-a-development-branch, option 1 (squash merge into main).
Single commit: title + body following existing commit pattern.
Update implementation doc + postmortem if applicable.
Clean up branch + worktree.
Verify README/s are up to date.
Update docs/roadmap.md and docs/status.md (don't track docs/ in git).
After that you should clean docs/superpowers/ moving related files to docs/superpowers/archived
Check ./scripts/publish-crates.sh and camel-cli features.
```

## VERSION BUMP / RELEASE

```
Confirm version with user (semver from root Cargo.toml).
Update root Cargo.toml → regenerate Cargo.lock with cargo check.
Update hardcoded versions: root README.md, bridges/jms/build.gradle.kts, isolated examples.
Verify: cargo build.
Commit following release pattern + body with changes since previous version.
Create git tag vX.Y.Z.
NO push, NO publish to crates.io.
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
