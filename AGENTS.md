## CRITICAL

- Always read (/home/kenny/.config/opencode/instructions/agent/behavior.md)
- Never add to staging something that is in .gitignore

## IMPORTANT

- When user explicitly requests: **"ORCHESTRATE"** must read (/home/kenny/.config/opencode/instructions/agent/orchestrator.md)
- Also explicitly requests: "ULTRATHINK" must read (/home/kenny/.config/opencode/instructions/agent/ultrathink.md)
- Code review critical or important issues always should be resolved before continue
- When implementing a new feature remember this:

  ```txt
  Usar superpowers siempre y worktree desde main en ./.worktrees/ y activar ORCHESTRATE esto es importante, delegando la implementación a @worker_gpt y siempre haciendo spec + code review a @worker_new. no pasamos de tarea si existe critical o important issues y si esta no está contemplada en una tarea posterior, los minor los consideramos si son relevantes. Si un subagente necesita ULTRATHINK debes indicar que lea AGENTS.md
  ```

- When user ask to merge to main:

  ```txt
  Usar opción 1 del skill finishing-a-development-branch (merge branch into main) pero con squash de un solo commit con title + body siguiendo patrón de otros commits, luego actualizar documento de implementación y agregar sección postmortem si procede. Limpiar branch y worktree y revisar que README/s esté/n actualizado/s (keep it simple). Por último actualiza si procede docs/roadmap.md y docs/status.md sin dar seguimiento a docs/ en git. También es necesario revisar ./scripts/publish-crates.sh por si es necesario actualiazar y camel-cli ya que debe incluir todas las características por defecto.
  ```

- When user asks to bump version or release a new version:

  ```txt
  Verificar la nueva versión con el usuario si no la ha indicado (semver desde la versión actual en Cargo.toml raíz). Actualizar la versión en Cargo.toml raíz y regenerar Cargo.lock con cargo check. Buscar y actualizar cualquier fichero con versión hardcodeada: README.md raíz, bridges/jms/build.gradle.kts (fallback), y ejemplos aislados que no usen version.workspace. Verificar que compila con cargo build. Commit siguiendo el patrón de otros commits de release con body listando cambios desde la versión anterior. Crear git tag vX.Y.Z. NO hacer push ni publicar a crates.io.
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
