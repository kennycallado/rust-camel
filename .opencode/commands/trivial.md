# /trivial — Fast path for small fixes and chores

Bypasses expert review for changes that don't need it: typos, dependency
bumps, small refactors, log-level fixes, CI tweaks.

## Usage

```
/trivial <change-name> "<one-line description>"
```

## Flow

### 1. Create minimal change directory

```bash
mkdir -p openspec/changes/$1
```

Write `openspec/changes/$1/proposal.md`:

```markdown
# $1

$2

## Why

Trivial change — no spec breakdown needed.

## What changes

<!-- 1-3 bullet points, enough context for the commit -->
```

### 2. Write TRIVIAL blessing

Compute artifact hash:

```bash
bash scripts/bless-hash.sh "openspec/changes/$1"
```

Store the output hash (wrapper execs prebuilt xtask; no cargo lock). Then write `openspec/changes/$1/.bless.json`:

```json
{
  "verdict": "TRIVIAL",
  "hash": "<hash from above>",
  "expert": "conductor",
  "kind": "trivial"
}
```

No expert review needed for trivial changes. The hash still enables drift detection.

### 3. Report

```
✓ Trivial change $1 signed (TRIVIAL)
  Implement, then /opsx-archive $1
```

## Guardrails

- Use for: typos, deps, log levels, CI config, small refactors (< ~50 lines)
- Do NOT use for: new features, API changes, security, breaking changes
- If unsure whether something is trivial, use the full /opsx-propose flow
