# Tasks: bdidem

## Task 1 — Idempotent wrapper + self-test + AGENTS.md pointer

- [x] 1.1 scripts/bd-create-idempotent.sh

**Files:** `scripts/bd-create-idempotent.sh` (new), `AGENTS.md` (one
line), `openspec/changes/bdidem/` (this change dir)

**Steps:**

1. Write `scripts/bd-create-idempotent.sh` (bash, `set -euo pipefail`):
   - Usage: `bd-create-idempotent.sh <title> [bd-create flags...]`;
     `--self-test` subcommand; empty/flag-first title → usage error,
     exit 2.
   - Dedup pre-check: `bd list --created-after <cutoff> --json` where
     cutoff = now − `${BD_CREATE_IDEMPOTENT_WINDOW:-600}` seconds
     (GNU date). jq-select non-closed entries with EXACT title match;
     if several (existing dup pairs), reuse the OLDEST.
   - Reuse path: `bd show <id> --json` → stdout
     `jq '. + {reused: true}'`; stderr note names the reused id and its
     created_at; exit 0. No create.
   - Create path: `bd create <title> [flags] --json` (append `--json`
     if absent) → stdout `jq '. + {reused: false}'`; propagate bd's
     exit code.
   - Fail-closed: pre-check failure (bd list/jq/date missing or error)
     → abort non-zero with clear stderr; never fall through to a
     blind create.
   - Header comment documents contract + rc-bwi7g.
2. `--self-test`: temp dir + `bd init` (isolated embedded Dolt), run
   the mission verification matrix, print PASS/FAIL per case, exit
   non-zero on any failure, cleanup temp dir:
   - M1 create-new → new id (reused:false, id non-empty)
   - M2 retry same title <10min → SAME id (reused:true), bd list count
     for that title still 1 (no dup)
   - M3 different title → different new id (reused:false)
   - M4 usage guard: no args / flag-first title → exit 2
3. `AGENTS.md` beads "Create new issues" block: add one line pointing
   to the wrapper (idempotent retry-safe create).

**Tests (executable):**

- Name: `bd-create-idempotent --self-test`
- Arrange: mktemp -d; `bd init` inside it (isolated DB; embedded
  Dolt, directory-scoped — verified during recon).
- Act: run the wrapper (path-form) three times per matrix + two
  usage-error invocations.
- Assert: M1 reused=false and id matches the listed bd; M2 id equals
  M1's id, reused=true, exactly ONE non-closed bd with that title in
  the temp DB; M3 id differs from M1, reused=false; M4 exit code 2
  both times. Self-test exits 0 only if all pass. Also covers M5
  closed-exclusion (a closed identical title is not reused) and M6
  oldest-wins (two identical open seeds → the older by created_at is
  reused).

**Acceptance:**

- `bash -n scripts/bd-create-idempotent.sh` clean.
- `scripts/bd-create-idempotent.sh --self-test` → all matrix cases
  PASS, exit 0 (recorded output goes in the park report).
- Fleet backlog untouched by tests (temp-dir DB only).
- `AGENTS.md` gains exactly the pointer line; no other doc changes.
- `openspec validate bdidem --type change` passes with
  `skip_specs: true` set in `.openspec.yaml`.
