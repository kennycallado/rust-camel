---
description: Expert blessing gate — dispatches fresh expert and stores machine-readable attestation
agent: build
---

# Blessing gate for OpenSpec change: $ARGUMENTS

You are performing a **BLESSING** (not consultation) for an OpenSpec change.
This means fresh eyes — the expert must NOT reuse any prior task_id.

## Input

The argument after `/bless` is the change name (kebab-case), e.g. `/bless add-dark-mode`.
If no argument provided, infer from conversation context or prompt the user.

## Steps

### 1. Validate change exists

```bash
openspec status --change "$1" --json
```

If the change doesn't exist, stop and tell the user.

### 2. Read all artifacts

Read every artifact file in `openspec/changes/$1/`:
- `proposal.md`
- `design.md`
- `specs/**/*.md`
- `tasks.md`

### 3. Compute artifact hash

```bash
cargo run -p xtask -- hash-artifacts --change-dir "openspec/changes/$1"
```

Store the output (format: `sha256:<hash>`).

### 4. Dispatch expert for blessing

Dispatch `@experts/e_gpt` **WITHOUT task_id** (fresh eyes) with this context-pack:

```
## BLESSING REQUEST

Change: $1
Artifacts directory: openspec/changes/$1/

Read all artifacts in that directory and evaluate whether this change
is ready for implementation. Apply the reviewer rubric from
.opencode/agents/reviewers/r_glm.md as your evaluation baseline.

Return EXACTLY one verdict:
- BLESSED — artifacts are sound, proceed to implementation
- BLESS-WITH-FIXES: [list specific fixes needed]
- REJECTED: [reason]

Include findings ordered by severity (Critical, Important, Minor).
```

Wait for the expert's verdict.

### 5. Sign attestation (HMAC)

Use the xtask signer to produce a cryptographically signed attestation.
This prevents workers from forging a BLESSED verdict.

```bash
ATTESTATION_HMAC_SECRET=$ATTESTATION_HMAC_SECRET \
  cargo run -p xtask -- sign-attestation \
    --change-dir openspec/changes/$1 \
    --verdict BLESSED \
    --expert e_gpt \
    --kind bless
```

This writes `.attestation.json` with: verdict, hash, expert, HMAC signature.
The HMAC is computed over `verdict|hash|expert` — CI verifies it offline.

If the expert returned BLESS-WITH-FIXES, sign with that verdict (the apply
gate checks for `verdict == BLESSED` and will block).

### 6. Report to user

**If BLESSED:**
```
✓ Blessed by @experts/e_gpt
  Hash: sha256:<first 12 chars>
  Ready for /opsx-apply $1
```

**If BLESS-WITH-FIXES:**
```
⚠ Bless-with-fixes by @experts/e_gpt
  Fixes required:
  - <fix 1>
  - <fix 2>
  Apply fixes, then re-run /bless $1
```

**If REJECTED:**
```
✗ Rejected by @experts/e_gpt
  Reason: <reason>
  Address the issues and re-run /bless $1
```

## Guardrails

- NEVER bless without computing the hash first
- NEVER reuse a task_id for a blessing (fresh eyes is the point)
- NEVER write an attestation manually — use `xtask sign-attestation`
- The HMAC signature is the provenance proof; only conductor-light has the secret
  invalidates it (enforced by /opsx-apply gate check)
