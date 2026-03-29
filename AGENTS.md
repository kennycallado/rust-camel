## CRITICAL

- Always read (/home/kenny/.config/opencode/instructions/agent/behavior.md)
- Never add to staging something that is in .gitignore

## IMPORTANT

- When user explicitly requests: **"ORCHESTRATE"** must read (/home/kenny/.config/opencode/instructions/agent/orchestrator.md)
- Also explicitly requests: "ULTRATHINK" must read (/home/kenny/.config/opencode/instructions/agent/ultrathink.md)
- Code review critical or important issues always should be resolved before continue
- When implementing a new feature remember this:

  ```txt
  Usar superpowers siempre y worktree desde main y activar ORCHESTRATE esto es importante, delegando la implementación a @worker_gpt y siempre haciendo spec + code review a @worker_new. no pasamos de tarea si existe critical o important issues y si esta no está contemplada en una tarea posterior. Si un subagente necesita ULTRATHINK debes indicar que lea AGENTS.md
  ```

- When user ask to merge to main:

  ```txt
  Usar opción 1 del skill finishing-a-development-branch (merge branch into main) pero con squash de un solo commit con title + body siguiendo patrón de otros commits, luego actualizar documento de implementación y agregar sección postmortem si procede. Limpiar branch y worktree y revisar que README/s esté/n actualizado/s (keep it simple).
  ```
