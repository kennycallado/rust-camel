## CRITICAL

- Always read .opencode/instructions/behavior.md
- If you have access to caveman skill, activate it
- Never add to staging something that is in .gitignore

## IMPORTANT

- **ORCHESTRATE** → read .opencode/instructions/orchestrate.md
- **ULTRATHINK** → read .opencode/instructions/ultrathink.md
- Code review critical/important issues must be resolved before continuing

## NEW FEATURE WORKFLOW

```
Use superpowers + worktree from main in ./.worktrees/ + ORCHESTRATE.
Delegate implementation to workers (w_glm5.1 default, w_gpt5.3-codex or w_qwen3.6-pro as alternatives).
Spec + code review always (requesting-code-review skill).
Don't advance tasks with critical/important issues unresolved.
Minor issues: resolve if relevant.
Escalate to experts (e_gpt5.5, e_opus4.7) only when workers are stuck after 2 attempts.
If a subagent needs ULTRATHINK, tell it to read AGENTS.md.
```

## MERGE TO MAIN

```
Skill: finishing-a-development-branch, option 1 (squash merge into main).
Single commit: title + body following existing commit pattern.
Update implementation doc + postmortem if applicable.
Clean up branch + worktree.
Verify README/s are up to date.
Update docs/roadmap.md and docs/status.md (don't track docs/ in git).
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
