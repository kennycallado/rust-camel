# Proposal: bdidem — idempotent bd create wrapper

## Why

`bd create` is not idempotent. When a fleet agent retries the command
after a transient failure (timeout, aborted call), the retry creates a
DUPLICATE bd with an identical title. e_opus backlog ruling 2026-09-25
evidenced 6 duplicate pairs created 7–50s apart (rc-s472e/z23xn,
rc-qynxs/9xzlw, rc-c3nn6/9qjuu, rc-zzmyz/wrcek, rc-pu6wa/sda0m,
rc-9n945/mgu7u; seventh rc-1wm8s dup of rc-i4yjl). bd rc-bwi7g (P1).

Recon finding: there is NO central bd-invocation helper in the repo —
agents call `bd create` directly, guided by the AGENTS.md beads
section. The retry lives in the agent layer (LLM re-invocation), so the
fix is a wrapper script + a one-line pointer at the canonical call-site
doc.

## What Changes

1. `scripts/bd-create-idempotent.sh` (NEW): wrapper around
   `bd create`. Before creating, it lists non-closed bds created in the
   last 10 minutes (`bd list --created-after <RFC3339> --json`,
   server-side filter); if one has an IDENTICAL title it reuses it.
   Output contract: one JSON object on stdout (the bd, plus `"reused":
   true|false`) so agents uniformly parse `| jq -r .id`. Fail-closed:
   if the pre-check itself fails, abort non-zero so the caller's retry
   re-runs the whole wrapper (dedup engages once bd responds again).
2. `scripts/bd-create-idempotent.sh --self-test`: runs the mission
   verification matrix against a throwaway temp-dir bd database
   (`bd init` is directory-scoped, embedded Dolt — zero fleet
   pollution): create-new → new id; retry same title <10min → same id,
   no dup; different title → new id; plus usage-guard checks.
3. `AGENTS.md` beads section: one-line pointer telling agents to use
   the wrapper instead of bare `bd create`.

## Impact

- Affected: fleet tooling only (`scripts/`, `AGENTS.md`). No Rust, no
  specs, no bd/opencode internals modified.
- Unblocks backlog-gate condition (zero non-epic P1) and stops the
  duplicate inflow at its root.
- Explicitly out of scope: modifying bd itself; touching opencode
  server/plugin internals; closing the 7 evidenced duplicate pairs
  (owner-gated per bd rc-bwi7g note); rewiring every historical call
  site (mission orders are immutable records).

## Spec delta

None (`skip_specs: true`) — shell wrapper + docs pointer, no capability
spec changes.
