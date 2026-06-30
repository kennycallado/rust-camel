# Implementation dispatch prompt

Paste the block below as-is for the standard case. To use different agents, find-and-replace the three mentions (`w_balanced`, `r_glm`, `e_gpt`) below.

---

Vamos a proceder con la implementación. Cargamos los skills `using-git-worktrees`, `test-driven-development`, `executing-plans` con `subagent-driven-development` usando `@workers/w_balanced` para implementar y `@reviewers/r_glm` para spec+code review (si es posible en misma iteración). No pasamos de tarea si queda algún issue critical/important/minor sea o no bloqueante; solo obviamos minor absurdos.

Si una tarea se complica: escala a `@experts/e_gpt` tanto para consulta como para implementación si procede. El experto también puede consultarse ante bloqueo por decisión.

**Workflow refs use stable aliases only** (`w_fast`/`w_balanced`/`w_heavy`, `r_glm`, `e_gpt`/`e_opus`). ULTRATHINK: `.opencode/instructions/ultrathink.md`.
