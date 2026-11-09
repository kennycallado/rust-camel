# Design: gate-report-partial

## Approach

Two different facts about a gate that did not execute today collapse into one silent
`continue` at `gates-local.sh:110-112`: the script's own policy skips (network/infra,
CI owns them) and the operator's ad-hoc `GATES_ONLY` scoping. The fix separates them
in the summary and labels the mode.

1. **Counting.** The `GATES_ONLY` filter branch appends the gate name to a new
   `not_selected` array instead of discarding it. The summary gains a
   `not selected: N — names` line, printed only when `GATES_ONLY` is set.
   To make the accounting match the order's `not selected: 26` arithmetic
   (32 gates − 2 ran − 4 policy-skipped), the SKIP check moves BEFORE the
   `GATES_ONLY` check in the loop: a policy-skipped gate keeps its `skipped`
   bucket even in a partial run. Execution outcomes are identical (a gate
   must pass both filters to run, in either order) — only the accounting
   bucket changes, and accounting is exactly what this change fixes.
2. **Denominator.** A `ran: X of Y` line (X = passed + failed, Y = registry size) is
   added in BOTH modes, as the last summary line. This is the chosen mechanism from
   the order's requirement-1 alternatives: unlike a bare `not selected` count it makes
   the denominator visible in a full sweep too, so any summary quote is self-auditing
   (a park claiming "full sweep" with `ran: 2 of 32` contradicts itself on its face).
3. **Mode label.** When `GATES_ONLY` is non-empty: a startup preamble line
   (`gates-local: PARTIAL sweep — GATES_ONLY="..."`) and the summary divider becomes
   `──── gates-local summary (PARTIAL — GATES_ONLY set) ────`. A copied summary block
   therefore carries the qualification even when the preamble is not copied. Full
   mode keeps the divider byte-identical.
4. **Untouched:** the `skipped:` line meaning (policy skips, documented in the script
   header), the exit-code rule (`failed == 0`), `GATES_SKIP` semantics
   (`${GATES_SKIP-…}`, including set-but-empty disabling skips), the registry parser,
   and the gate set.

## The two-mode wording (requirement-3 choice)

Full mode — identical to today plus exactly one added line:

```
──────── gates-local summary ────────
passed:  28
failed:  0
skipped: 4 — lint-commits lint-bridge-releases doc-guide-test cargo-audit
ran:     28 of 32
```

Partial mode — qualified divider, not-selected accounting, same denominator line:

```
──── gates-local summary (PARTIAL — GATES_ONLY set) ────
passed:  1
failed:  0
skipped: 4 — lint-commits lint-bridge-releases doc-guide-test cargo-audit
not selected: 27 — clippy lint-glob-imports …
ran:     1 of 32
```

Invariant: `passed + failed + not selected (partial mode) + skipped == Y` always.

## Why not-selected and skipped are different facts

`skipped` means "excluded by the effective skip list" — `SKIP="${GATES_SKIP-…}"`,
defaulting to the network/infra policy set documented in the header comment (git
fetch, mdbook toolchain, advisory DB, GitHub release API) but overridable by the
operator: `GATES_SKIP="…"` widens or narrows it, and set-but-empty disables it
entirely. It answers "which gates did the effective policy exclude from THIS run?"
and is identical for every operator only when `GATES_SKIP` is unset.
`not selected` is the remaining scoping fact: gates the effective skip list did NOT
exclude but the invocation's `GATES_ONLY` value did. Merging the two would make a
2-gate run read like the routine policy skip and re-create the exact lie this
change fixes, one line lower. The design preserves this fact under operator
override: with `GATES_SKIP=""`, those gates execute (or fail) and the `skipped:`
count drops accordingly — the summary never claims a skip that did not happen.

## Affected crates

- None. `scripts/gates-local.sh` only (developer tooling, outside all runtime planes).

## Architecture boundaries

No data/control-plane surface: the script orchestrates gate commands already listed
in the AGENTS.md registry and adds no runtime, DSL, component, or language behavior.
No ADR governs the local sweep; the script header (2026-11-08, v0.66.0
`lint-secrets` / `lint-unbounded-wait` escapes) and the AGENTS.md "Local sweep" note
are the rationale canon this change extends. Spec canon check: `openspec/specs/`
contains no gates/local-sweep capability (verified 2026-11-09), so `skip_specs: true`
is set in `.openspec.yaml` — expert to rule at the blessing gate.
Related canon (e_gpt ruling 2026-11-09, acknowledged): `openspec/specs/bridge-release-guard/spec.md`
references `scripts/gates-local.sh` and its default release-check skip
(lines 67-85); this change alters neither the executed gate set nor the default
skip list, so that requirement is preserved without a delta spec.

## Alternatives considered

- **Merge not-selected into `skipped:`** — rejected: conflates policy skips with
  invocation scope; the order forbids it, and it would hide the distinction the
  summary exists to convey.
- **Keep ONLY-first loop order** (partial run would read `skipped: 0`,
  `not selected: 30` for a 2-gate sweep) — rejected: diverges from the order's
  `not selected: 26` arithmetic and makes the constant policy-skip fact vanish in
  partial mode, where it is most needed for evidence quotes.
- **`not selected:` line only, no `ran:` line** — viable, but the denominator then
  appears only in partial mode and full-sweep summaries stay unauditable ("28 of
  what?"). The `ran:` line in both modes costs one `printf`.
- **Error when `GATES_ONLY` matches zero registry gates** — rejected: changes exit
  semantics (order requirement 4 fixes exit codes); `ran: 0 of 32` already makes the
  mistake obvious without inventing a new failure mode.
- **Non-zero exit for any partial run** — rejected: a scoped green run must stay
  scriptable (requirement 4).
