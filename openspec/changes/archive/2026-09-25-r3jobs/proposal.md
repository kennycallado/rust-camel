# Proposal: r3jobs

## Why

The compile roadmap (epic `rc-rye74`, e_opus ruling) lists multi-entry jobs as a
MUST-CONVERGE item: any job document that runs correctly under `camel job` must,
when compiled, run identically sealed. R1 (bd `rc-86m92`) landed the virtual
document store and multi-document embedding; R2 (bd `rc-mval9`, commit
`9b5cfc2a`) landed embedded sources, explicit compile inputs, and the v2 job
boot seam (`run_embedded_job_store`). What R2 proved for jobs is the
single-indexed-route case: the `multi_job` fixture embeds a job document plus
exactly one indexed route file and boots it through the existing job outcome
lifecycle. The N-entry generalization — a job document plus two or more
embedded route documents — is unbuilt and untested: no fixture compiles a job
with more than one route entry, no test pins the boot order of multiple indexed
route entries, no test proves a failing entry is named at boot, and no test
asserts the manifest lists every route entry of a job artifact.

This is job-level parity, not a new wall: the deployment-equivalence north star
requires `camel job <doc>` and the compiled artifact to agree on the same
document set.

## What Changes

- Generalize the compiled-artifact job boot to iterate the N route documents
  embedded alongside the job document (R2's virtual-store surface in
  `crates/camel-cli/src/compile/`): every indexed route entry in the source
  plan boots through the existing job lifecycle, not just the single-entry
  case R2 exercised.
- Deterministic ordering across entries: the boot consumes route entries in
  source-plan order (declared pattern order, each pattern's matches sorted) —
  R2's store-ordering scenarios are the precedent; store offsets and plan
  order stay validated by the existing decoder gates.
- Failure semantics: any entry failing validation or parse at boot fails the
  whole job boot with the entry named (`compiled://<logical-path>` identity)
  and exit 2 — no partial boot, no silent skip.
- Tests: extend `compile_command_test` / `compiled_artifact_test` batteries in
  `camel-cli` — multi-entry happy path (job + N≥2 route files completing
  through the outcome report), ordering (pattern order preserved, per-pattern
  matches sorted), per-entry failure naming, and manifest listing N entries.
- openspec delta: `cli-compile` spec gains ADDED requirement block(s) for
  multi-entry job artifacts (no MODIFIED requirements — existing scenario
  names untouched).
- `CONTEXT.md` alignment if the compile map changes.

## Acceptance criteria

- A job artifact embedding a job document plus N≥2 route documents compiles,
  boots from the virtual store with no source tree, completes with the
  existing job outcome report, and every embedded route entry participates in
  the boot.
- Route-entry consumption order is the source-plan order: declared pattern
  order with each pattern's matches sorted; the artifact is byte-deterministic
  across compiles of the same input set.
- An artifact whose embedded route entry fails parse/validation at boot exits
  2 with a diagnostic naming that entry's `compiled://` identity, before any
  route starts (no partial boot).
- `--manifest` lists every route entry of a multi-entry job artifact with
  canonical logical paths, kinds, byte lengths, and digests.
- Job-level parity holds: the same document set (job + route files + config)
  run under `camel job` and under the compiled artifact produce the same
  outcome.
- No new reject wall between `camel job` and compiled job artifacts for
  supported document sets.

## Risk budget

Low-to-medium. The store, boot seam, and single-entry job path exist and are
tested; the work is generalization plus proof. The known hazard is latent
single-entry assumptions in the job boot path (for example, anything assuming
`Discovered` holds exactly one document's routes) — the multi-entry battery
exists to surface them. Out of scope, unchanged: job+job entries stay a named
rejection; the R3 tripwire on `artifact_kind` (bd `rc-p823t`) is respected —
no long-lived manifest entries ship here; R5 TLS and R4 signing are untouched;
`--embed-secrets`, payload cap, and manifest schema 3 semantics are unchanged.
