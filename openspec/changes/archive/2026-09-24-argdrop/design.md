# Design: argdrop

## Context

Ruling rc-d5cgc (docs/rulings/RULING-camel-job-orientation.md, main
checkout) oriented `camel job` around a declared-args interface on the
job-document schema. The delta30 camel-cache batch adjudicated that
the interface's CLI side is the dynamic `--<name>` flag surface, not
the static `--arg NAME=VALUE` flag. bd rc-rsvky orders the flag's
removal pre-1.0; rc-uxvm6 (--arg last-wins/empty-value) closes as
moot with it.

## Current shape (what exists today)

- `JobArgs::args: Vec<(String,String)>` — the `--arg` clap field
  (job/mod.rs), parsed by `parse_arg_pair`; tail re-parse recovers
  pairs given after the document reference.
- `lower_dynamic_flags(decls, tail, arg_pairs)` merges phase-1 pairs,
  tail pairs, and dynamic-flag pairs into one NAME=VALUE channel.
- Legacy path (no `args:` block): pairs inject raw send-time headers
  (`JobRun::cli_args` → send loop), with a deprecation note.
- Declared path: pairs resolve through declarations (unknown-name,
  missing-required, typed coercion, `${arg:}` interpolation).
- `CrossFormConflict` rejects the same key through both forms.

## Decisions

**D1 — Remove the flag and its exclusive plumbing.** Gone:
`JobArgs::args`, `parse_arg_pair`, tail pair recovery,
`lower_dynamic_flags`'s `arg_pairs` parameter (seeds empty),
`LoweredDynamic` pair-merge comments, `CrossFormConflict` variant +
detection, `LEGACY_ARG_DEPRECATION`, `legacy_arg_headers()`,
`JobRun.cli_args`, and the send-loop header-injection loop
(`send_with_startup_retry` drops the `cli_args` parameter; embedded
run paths stop passing empty vectors).

**D2 — Dynamic flags remain the sole CLI surface.** The phase-2
lowering machinery is untouched otherwise: colon-namespaced `dyn:` IDs,
bool negation twins, stray-positional rejection, help precedence,
terminator semantics. `parse_job_document_with_args(path, text, pairs)`
keeps its pairs parameter — dynamic flags lower into it.

**D3 — Legacy implicit-header path removed, no replacement.** rc-uxvm6
dies moot. Document `send.headers` remains the header mechanism for
authors; its coverage stays in the existing document-parsing tests.
Ruling intent check done: A2's back-compat clause is reworded at A2
filing time (this removal lands first), so no spec text survives to
contradict the ruling.

**D4 — `UnknownArgumentName` document error removed.** With pairs
originating only from declared dynamic flags, an undeclared name fails
earlier as clap unknown-argument (with suggestion hint). The variant,
its Display arm, its production site in resolution, and its tests go.

**D5 — Missing-required diagnostic reworded.**
`missing required argument '{name}': pass --arg {name}=<value>` →
`pass --<name> <value>`.

**D6 — Reserved names drop `arg`.** `RESERVED_ARGUMENT_NAMES` becomes
`["help", "config", "report"]` (array size 4 → 3). Shared by run,
`--help`, and `camel compile` declaration validation — the reserved
names requirement covers all three.

**D7 — Help spelling note.** `FLAG_SPELLING_NOTE` drops
`; or --arg <name>=<value>`; byte-pinned tests update.

**D8 — Compiled artifacts unchanged in behavior.** They never had
`--arg`; they keep rejecting it as an unknown flag. Tests asserting
rejection stay (comments reworded); the artifact
"must not suggest the unavailable --arg surface" assertion still
holds after D5's reword.

## Spec delta map (cli-jobs)

REMOVED: `arg flag header injection`, `legacy argument compatibility`,
`declared arguments replace legacy header injection`,
`cross-form argument conflicts`.
MODIFIED: `batch mode drains until empty`, `declared argument
validation`, `argument interpolation`, `argument validation exit
status`, `job-scoped help`, `typed argument coercion`, `dynamic
declared-argument flags`, `bool flag spellings`, `dynamic flags
require an args block`, `reserved argument names`.

## Boundary check

Zone lease: `crates/camel-cli` job command (+ its shared
declaration-validation constant used by compile). No job-document
schema grammar change, no `mode: batch` machinery change, no other
crate touched. ADR refs: CONTEXT-MAP constitution (fail-loud
diagnostics, exit-2 taxonomy) — the removal keeps every failure class
exit-2 with a targeted diagnostic.

## Testing strategy

Unit (`--lib`): flags_tests (lowering + reworded diagnostics),
document_tests (resolution minus unknown-name), help_tests (note
bytes), batch_drain_tests (`--arg tier` → `--tier gold`). Integration:
job_one_shot_test (header-injection section removed; declared cases →
dynamic flags), compiled_artifact_test (rejection assertions kept).
Gates per mission order.
