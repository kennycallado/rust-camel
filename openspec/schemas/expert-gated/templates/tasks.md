# Tasks: <change-name>

<!--
  Optional delivery-phase grouping. Omit this level entirely for
  single-phase changes (the change looks exactly like the pre-phase
  template). When used, the WHOLE tasks.md — including every Phase
  block — is written and plan-blessed ONCE in STAGE 2. Phases are a
  STAGE 3 implementation-ordering construct, not a blessing construct.
-->

## <Module/Crate 1>

### Task 1.1: <one-line description>

**Files:**
- `path/to/file.rs` (new)
- `path/to/existing.rs` (modified)

**Steps:**
1. <specific implementation step>
2. <specific implementation step>

**Tests:** (executable spec — name, arrange, act, assert)
- `<test_fn_name>`: <setup> → <action> → <exact assertion>
- `<test_fn_name>`: <setup> → <action> → <exact assertion>

**Acceptance:**
- <criterion: e.g. cargo clippy clean>
- <criterion: e.g. all tests pass>

- [ ] 1.1

### Task 1.2: <one-line description>

**Files:**
- `path/to/file.rs` (new)

**Steps:**
1. <step>

**Tests:**
- <test>

**Acceptance:**
- <criterion>

- [ ] 1.2

## <Module/Crate 2>

### Task 2.1: <one-line description>

**Files:**
- `path/to/file.rs` (modified)

**Steps:**
1. <step>

**Tests:**
- <test>

**Acceptance:**
- <criterion>

- [ ] 2.1

<!--
  OPTIONAL: example with phase grouping. Delete this block from real
  tasks.md. The phase heading sits ABOVE the <Module/Crate> level.
  STAGE 3 iterates these groups in order, with an inter-phase r_glm
  review only between multi-task phases.

## Phase 1: <one-line phase goal>

### <Module/Crate A>

#### Task 1.1: <one-line description>
**Files:**
- `path/to/file.rs` (new)

**Steps:**
1. <step>

**Tests:**
- `<test_fn_name>`: <setup> → <action> → <exact assertion>

**Acceptance:**
- <criterion>

- [ ] 1.1

## Phase 2: <one-line phase goal>

### <Module/Crate B>

#### Task 2.1: <one-line description>
**Files:**
- `path/to/file.rs` (modified)

**Steps:**
1. <step>

**Tests:**
- `<test_fn_name>`: <setup> → <action> → <exact assertion>

**Acceptance:**
- <criterion>

- [ ] 2.1
-->
