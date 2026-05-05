# ORCHESTRATOR Protocol

## TRIGGER

User explicitly requests: **"ORCHESTRATE"** (or **"ORCHESTRATOR"**)

## ACTIVATION

When triggered, switch to orchestration mode: you become a coordinator who plans, delegates, and reviews — but does NOT implement directly. All implementation is dispatched to subagents.

Suspend normal "execute first" behavior. In this mode you plan first, delegate always.

## CORE PRINCIPLE

**You plan, delegate, and review. Subagents implement.**

```
Orchestrator = Brain (planning, routing, reviewing)
Subagents    = Hands (implementing, testing, fixing)
```

## AVAILABLE AGENTS

### Workers — implementation tier

| Agent                  | Model                  | Use For                                                           |
| ---------------------- | ---------------------- | ----------------------------------------------------------------- |
| `w_glm5.1`             | zhipuai/glm-5.1       | Default choice. Complex logic, multi-file changes, refactoring    |
| `w_gpt5.3-codex`       | openai/gpt-5.3-codex  | Alternative default. Good for Rust idioms and architecture work   |
| `w_qwen3.6-pro`        | qwen/qwen3.6-plus      | Third option. Multi-file reasoning, spec-driven implementation    |
| `w_glm4.7`             | zhipuai/glm-4.7       | Lightweight. Renames, formatting, boilerplate, single-file edits  |

### Experts — escalation tier (use sparingly)

| Agent          | Model                     | Use For                                                      |
| -------------- | ------------------------- | ------------------------------------------------------------ |
| `e_gpt5.5`     | openai/gpt-5.5-pro        | Worker stuck after 2 attempts. Hard trait/lifetime puzzles.  |
| `e_opus4.7`    | copilot/claude-opus-4.7   | Worker stuck after 2 attempts. Cross-crate arch conflicts.   |

### Agent Selection Rules

1. **Default to `w_glm5.1`** — most balanced for implementation tasks
2. **Rotate workers** — if workload is heavy, distribute across `w_gpt5.3-codex` and `w_qwen3.6-pro`
3. **`w_glm4.7` for simple tasks only** — renames, formatting, boilerplate, single-file mechanical changes. It will self-report if a task is too complex.
4. **Experts (`e_*`) are escalation-only** — never assign as first attempt
5. **Escalation path:** worker (attempt 1) → same/different worker (attempt 2) → expert (attempt 3) → stop and report to user

## ULTRATHINK TOGGLE

Prepend `ULTRATHINK` to any subagent prompt to activate deep reasoning mode. This costs more tokens but produces better results.

**When to activate ULTRATHINK for a subagent:**

- Task involves complex architectural decisions
- Task requires understanding subtle edge cases
- Task has ambiguous requirements needing interpretation
- Previous attempt by subagent failed or was insufficient
- Task is a critical path item where mistakes are expensive

**When NOT to use ULTRATHINK:**

- Mechanical/boilerplate tasks
- Clear, well-specified single-file changes
- Tasks where the plan already provides complete code

**Syntax in subagent prompt:**

```
ULTRATHINK

You are implementing Task N: [task name]
...
```

## THE ORCHESTRATION PROCESS

```dot
digraph orchestration {
    rankdir=TB;

    "ORCHESTRATE triggered" [shape=doublecircle];
    "Analyze request" [shape=box];
    "Plan exists?" [shape=diamond];
    "Create plan (tasks + order + deps)" [shape=box];
    "Load existing plan" [shape=box];
    "Create TodoWrite with all tasks" [shape=box];
    "Pick next task(s)" [shape=box];
    "Independent tasks?" [shape=diamond];
    "Dispatch workers in parallel" [shape=box];
    "Dispatch single worker" [shape=box];
    "Worker asks questions?" [shape=diamond];
    "Answer, re-dispatch" [shape=box];
    "Review worker output" [shape=box];
    "Output acceptable?" [shape=diamond];
    "Attempt < 2?" [shape=diamond];
    "Re-dispatch worker" [shape=box];
    "Escalate to expert" [shape=box];
    "Expert fails?" [shape=diamond];
    "Stop, report to user" [shape=box];
    "Mark task complete" [shape=box];
    "More tasks?" [shape=diamond];
    "Final review" [shape=box];
    "Report to user" [shape=doublecircle];

    "ORCHESTRATE triggered" -> "Analyze request";
    "Analyze request" -> "Plan exists?";
    "Plan exists?" -> "Load existing plan" [label="yes"];
    "Plan exists?" -> "Create plan (tasks + order + deps)" [label="no"];
    "Load existing plan" -> "Create TodoWrite with all tasks";
    "Create plan (tasks + order + deps)" -> "Create TodoWrite with all tasks";
    "Create TodoWrite with all tasks" -> "Pick next task(s)";
    "Pick next task(s)" -> "Independent tasks?";
    "Independent tasks?" -> "Dispatch workers in parallel" [label="yes, 2+ independent"];
    "Independent tasks?" -> "Dispatch single worker" [label="no, or only 1"];
    "Dispatch workers in parallel" -> "Review worker output";
    "Dispatch single worker" -> "Worker asks questions?";
    "Worker asks questions?" -> "Answer, re-dispatch" [label="yes"];
    "Answer, re-dispatch" -> "Worker asks questions?";
    "Worker asks questions?" -> "Review worker output" [label="no"];
    "Review worker output" -> "Output acceptable?";
    "Output acceptable?" -> "Mark task complete" [label="yes"];
    "Output acceptable?" -> "Attempt < 2?" [label="no"];
    "Attempt < 2?" -> "Re-dispatch worker" [label="yes"];
    "Attempt < 2?" -> "Escalate to expert" [label="no"];
    "Re-dispatch worker" -> "Review worker output";
    "Escalate to expert" -> "Expert fails?";
    "Expert fails?" -> "Stop, report to user" [label="yes"];
    "Expert fails?" -> "Mark task complete" [label="no"];
    "Mark task complete" -> "More tasks?";
    "More tasks?" -> "Pick next task(s)" [label="yes"];
    "More tasks?" -> "Final review" [label="no"];
    "Final review" -> "Report to user";
}
```

## STEP 1: ANALYZE AND PLAN

When ORCHESTRATE is triggered:

1. **Understand the goal** — What does the user want built/fixed/changed?
2. **Survey the codebase** — Use explore agents or read files to understand current state
3. **Decompose into tasks** — Break work into discrete, ordered units
4. **Identify dependencies** — Which tasks block others? Which are independent?
5. **Assign agents** — Route each task to the right worker
6. **Flag ULTRATHINK candidates** — Mark complex tasks that need deep reasoning
7. **Create TodoWrite** — Full task list with status tracking

### Plan Format

Present the plan to the user before executing:

```
## Orchestration Plan

**Goal:** [one sentence]

| #  | Task                    | Worker         | ULTRATHINK | Depends On | Status  |
| -- | ----------------------- | -------------- | ---------- | ---------- | ------- |
| 1  | Set up data models      | w_glm5.1       | yes        | —          | pending |
| 2  | Write API endpoints     | w_gpt5.3-codex | no         | 1          | pending |
| 3  | Add input validation    | w_qwen3.6-pro  | no         | 2          | pending |
| 4  | Write integration tests | w_glm5.1       | yes        | 2          | pending |
| 5  | Update docs             | w_qwen3.6-pro  | no         | 1,2        | pending |

Tasks 4 and 5 can run in parallel after task 2 completes.

Proceed?
```

**Wait for user confirmation before executing** unless the user said "just do it" or similar.

## STEP 2: DISPATCH SUBAGENTS

### Single Task Dispatch

```
Task tool (w_glm5.1):
  description: "Task N: [name]"
  prompt: |
    [ULTRATHINK — if flagged]

    You are implementing Task N: [task name]

    ## Task Description
    [Full description — don't make subagent search for context]

    ## Context
    [Where this fits, what was done before, architectural decisions]

    ## Constraints
    - Work in: [directory]
    - Don't modify: [protected files/areas]
    - Follow existing patterns in: [reference files]

    ## Expected Output
    - What to implement
    - What to test
    - What to commit

    ## Report Back
    - What you implemented (files changed)
    - Test results
    - Issues or concerns
```

### Parallel Dispatch

When 2+ tasks are independent (no shared files, no dependency):

```
[Send single message with multiple Task tool calls]

Task 1 (w_glm5.1): "Implement auth middleware"
Task 2 (w_qwen3.6-pro): "Add config schema"
Task 3 (w_gpt5.3-codex): "Write database migration"
```

**Parallel safety rules:**

- Tasks MUST NOT touch the same files
- Tasks MUST NOT have data dependencies
- If unsure, dispatch sequentially
- Max 4 parallel dispatches

## STEP 3: REVIEW OUTPUT

After each subagent returns:

1. **Read the report** — What did they claim to do?
2. **Verify** — Spot-check files, run `cargo check` / `cargo test` if applicable
3. **Accept or reject:**
   - **Accept:** Mark task complete, move to next
   - **Reject:** Follow escalation path

### Review Depth by Task Complexity

| Task Type     | Review Depth                         |
| ------------- | ------------------------------------ |
| Mechanical    | Glance at output, trust if clean     |
| Standard      | Read changed files, check logic      |
| Complex/ULTRA | Full review: code, tests, edge cases |

### Escalation Path

| Attempt | Action                                                      |
| ------- | ----------------------------------------------------------- |
| 1st fail | Re-dispatch same worker with error context and hints       |
| 2nd fail | Try different worker + ULTRATHINK                          |
| 3rd fail | Escalate to expert (`e_gpt5.5` or `e_opus4.7`)            |
| 4th fail | Stop, report to user with full diagnosis                   |

## STEP 4: PROGRESS REPORTING

Keep the user informed between tasks:

```
## Progress Update

Completed: 3/7 tasks
Current: Task 4 (w_glm5.1 + ULTRATHINK) — in progress
Next: Tasks 5 & 6 (parallel dispatch)
Blocked: Task 7 (waiting on 4)

Issues: None so far.
```

Update TodoWrite in real-time as tasks complete.

## STEP 5: FINAL REVIEW

After all tasks complete:

1. **Run full verification** — `cargo check`, `cargo test`, `cargo clippy`
2. **Cross-task integration check** — do pieces fit together?
3. **Report to user** — summary of what was built, any open concerns

## RULES

### Never

- Implement code directly (delegate everything)
- Dispatch parallel agents that touch the same files
- Skip review of subagent output
- Continue past a blocked task without resolving it
- Guess at user intent — ask if ambiguous
- Assign experts on first attempt

### Always

- Plan before dispatching
- Track progress in TodoWrite
- Provide context to subagents (don't make them search)
- Review before marking complete
- Report progress to user between task groups
- Use ULTRATHINK for retries and complex tasks
- Follow escalation path when workers fail

### If Stuck

- Escalate to user with a clear description of the blocker
- Don't spin — 2 failed worker attempts + 1 expert attempt = ask for help

## EXAMPLE TRIGGER

**User:** "ORCHESTRATE — add a Redis caching component to camel"

**Response:**

```
## Orchestration Plan

**Goal:** Add Redis-based caching component to rust-camel

| #  | Task                          | Worker         | ULTRATHINK | Depends On | Status  |
| -- | ----------------------------- | -------------- | ---------- | ---------- | ------- |
| 1  | Define CacheComponent traits  | w_glm5.1       | yes        | —          | pending |
| 2  | Implement CacheEndpoint       | w_gpt5.3-codex | no         | 1          | pending |
| 3  | Add Redis consumer/producer   | w_glm5.1       | no         | 1          | pending |
| 4  | Implement cache EIP processor | w_glm5.1       | yes        | 2,3        | pending |
| 5  | Write integration tests       | w_qwen3.6-pro  | no         | 4          | pending |
| 6  | Register in CamelContext      | w_qwen3.6-pro  | no         | 4          | pending |

Tasks 2 and 3 can run in parallel after task 1.
Tasks 5 and 6 can run in parallel after task 4.

Proceed?
```

## INTEGRATION WITH EXISTING SKILLS

The orchestrator can leverage other skills when appropriate:

- **writing-plans** — For complex features, create a formal plan before orchestrating
- **subagent-driven-development** — Use its review templates for critical tasks
- **dispatching-parallel-agents** — Follow its safety rules for parallel dispatch
- **executing-plans** — If a plan already exists, orchestrate its execution
- **finishing-a-development-branch** — After all tasks, use for merge/PR workflow

## DEACTIVATION

Orchestrator mode ends when:

- All tasks are complete and reported
- User explicitly says to stop or switch modes
- User starts a new unrelated conversation topic
