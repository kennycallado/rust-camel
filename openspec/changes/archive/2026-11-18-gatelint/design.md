# Design: gatelint

## Approach

Add a single CI step to the `quality` job in `.github/workflows/ci.yml`,
placed immediately **before** the `clippy` step (right after `fmt check`):

```yaml
- name: lint-gate-parity
  run: cargo xtask lint-gate-parity
```

Placement rationale: GitHub Actions skips later steps by default (`success()`
gating) once a step fails. If the parity lint ran after `clippy`, clippy drift
that breaks clippy would also suppress the parity diagnostic — the exact
failure mode this change exists to surface. Running it before `clippy` keeps
the diagnostic in the log even when clippy then fails. The step name matches
the registry gate name in AGENTS.md `## QUALITY GATES` exactly, so a human can
diff the two lists by eye. No custom `if:` condition and no
`continue-on-error`: the step must be able to fail the job.

The comment above the clippy step (lines 411-413) is rewritten to state the
truth: AGENTS.md `## QUALITY GATES` is the source of truth for the clippy
command (bd rc-2s1vm), both files change in the same commit, and the
`lint-gate-parity` step above runs in CI and fails the job on drift between
the two.

No new toolchain requirement: the `quality` job already installs stable
rustfmt + clippy; the lint is pure file parsing (per the `gate-parity-lint`
capability spec: no cargo builds, no new dependencies, millisecond runtime),
so it needs nothing beyond the existing checkout. No extra setup, cache entry,
or `--locked` change.

Proof-of-wiring (all recorded verbatim in the park):

1. **Wiring diff check**: the committed workflow diff (vs merge-base) adds
   exactly one step — named `lint-gate-parity`, running
   `cargo xtask lint-gate-parity`, no `if:` condition, no
   `continue-on-error` — plus only the permitted comment edit adjacent to the
   clippy step. Note the lint itself deliberately does not check for its own
   workflow step, so this check is done by diff inspection, not by the lint.
2. **Parsed-YAML assertion**: the committed `.github/workflows/ci.yml` is
   parsed with a local YAML parser (no new dependency, no CI step) and the
   parse asserts `jobs.quality.steps` contains exactly one `lint-gate-parity`
   step running `cargo xtask lint-gate-parity`, positioned between `fmt
   check` and `clippy`, with no `if:` and no `continue-on-error`. This
   catches indentation mistakes that keep a string diff green while breaking
   the file (e.g. a `run:` indented one space too little makes the step a
   string, not a mapping). This substitutes for the repo's absent
   workflow-lint check (no actionlint/yamllint; `cargo xtask schema --check`
   does not validate workflow YAML).
3. **Clean run**: `cargo xtask lint-gate-parity` on the committed tree →
   exit 0, `lint-gate-parity: OK (2 gates in parity)`.
4. **Mutation**: delete `--all-targets` from the workspace clippy invocation
   only (line 424; the three single-crate invocations stay untouched) → the
   same lint exits non-zero, reporting a `clippy` mismatch.
5. **Restore**: tree restored byte-identically — `git diff` empty,
   `git status --porcelain` empty — and the lint re-run exits 0.

Workflow-lint check: the repo has no workflow linter or YAML-schema check
(no actionlint/yamllint config; `cargo xtask schema --check` does not
validate workflow YAML). Its absence is recorded explicitly in the park; the
parsed-YAML assertion above covers the parse-level risk it would have caught.

## Affected crates

- None. This is CI wiring; no Rust source, no Cargo.toml, no registry change.

## Architecture boundaries

No runtime, DSL, component, service, language, or function surface is touched.
The behaviour of the gate itself is unchanged and already specified by the
`gate-parity-lint` capability spec plus the lint's own xtask tests; this change
only executes the existing gate in a second venue (CI instead of local sweep
only). The `ci-clippy-gate` spec's "no other workflow behaviour ... SHALL
change" clause scoped the original alignment change (ci-clippy-parity), whose
proposal explicitly deferred this CI wiring; this change is that deferred
follow-up, and the comment edit it makes is the one that spec's own step
comment requirement anticipates.

## Alternatives considered

- **Run the full `gates-local.sh` in CI** — rejected: CI already enumerates the
  gates step-by-step for visibility; the registry parse sweep is the local
  convenience, and duplicating it wholesale would restructure the job (out of
  mission scope).
- **Wire via a reusable workflow/composite action** — rejected: overkill for
  one command, adds indirection to a file whose convention is flat steps.
- **Place the step after `clippy`** — rejected: `success()` gating would skip
  it exactly when clippy fails, hiding the parity diagnostic (see above).
- **Generic drift check comparing all registry gates to CI steps** — that is a
  different, larger lint (`lint-gate-forwarding`/`lint-gates-lanes` already
  cover neighbouring invariants). This mission wires the existing parity gate,
  it does not design a new one.

Bd: rc-sa7r6.
