# Implementation dispatch prompt

Paste the block below as-is for the standard case. To use different agents, find-and-replace the three mentions (`w_fast`, `r_glm`, `e_gpt`) below.

---

Proceeding with implementation. Load the skills `using-git-worktrees`, `test-driven-development`, `executing-plans` with `subagent-driven-development`, using `@workers/w_fast` to implement and `@reviewers/r_glm` for spec + code review (in the same iteration if possible). Do not advance past a task if any critical/important/minor issue remains, whether blocking or not; only disregard absurd minor issues.

If a task gets complicated: escalate to `@experts/e_gpt` for both consultation and implementation as appropriate. The expert may also be consulted on decision deadlocks.

**Workflow refs use stable aliases only** (`w_fast`/`w_balanced`/`w_heavy`, `r_glm`, `e_gpt`/`e_opus`). ULTRATHINK: `.opencode/instructions/ultrathink.md`.
