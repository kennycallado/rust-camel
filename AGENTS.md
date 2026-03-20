## CRITICAL

- Always read (/home/kenny/.config/opencode/instructions/agent/behavior.md)
- Never add to staging something that is in .gitignore
- **context-mode**: Use sandbox tools for ALL commands/analysis — never dump raw output into context

### context-mode routing rules

**BLOCKED** — do NOT use directly:

- `curl` / `wget` → use `mcp__context-mode__ctx_fetch_and_index` or `ctx_execute`
- Direct `WebFetch` → use `ctx_fetch_and_index(url)` then `ctx_search(queries)`
- Shell commands with >20 lines output → use `ctx_batch_execute` or `ctx_execute`
- File reads for analysis/exploration → use `ctx_execute_file(path, language, code)`
- Large `grep`/search results → run inside `ctx_execute(language: "shell", ...)`

**Tool hierarchy** (use in this order):

1. `ctx_batch_execute(commands, queries)` — PRIMARY: multiple commands + auto-search in ONE call
2. `ctx_search(queries: [...])` — follow-up queries on indexed content
3. `ctx_execute(language, code)` / `ctx_execute_file(path, language, code)` — sandbox execution
4. `ctx_fetch_and_index(url)` then `ctx_search` — web content, raw HTML never enters context
5. `ctx_index(content, source)` — store docs/API refs for later search

## IMPORTANT

- When user explicitly requests: **"ORCHESTRATE"** must read (/home/kenny/.config/opencode/instructions/agent/orchestrator.md)
- Also explicitly requests: "ULTRATHINK" must read (/home/kenny/.config/opencode/instructions/agent/ultrathink.md)
- Code review critical or important issues always should be resolved before continue
- When implementing a new feature remember this:
  ```txt
  Usar superpowers siempre y worktree desde main y activar ORCHESTRATE esto es importante, delegando la implementación a @worker_new y siempre haciendo spec + code review. no pasamos de tarea si existe critical o important issues. Si un subagente necesita ULTRATHINK debes indicar que lea AGENTS.md
  ```
- When user ask to merge to main:
  ```txt
  Usar opción 1 del skill finishing-a-development-branch (merge branch into main) pero con squash de un solo commit con title + body siguiendo patrón de otros commits, luego actualizar documento de implementación y agregar sección postmortem si procede. Limpiar branch y worktree asegurarse de ejecutar formatter y revisar que README esté actualizado (keep it simple).
  ```
