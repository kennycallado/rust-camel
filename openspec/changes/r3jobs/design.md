# Design: r3jobs

## Approach

**Empirical baseline (probed on main `5efcfc2d`, scratch fixtures under
`/home/shared/tmp/r3exp`):** the N-entry job boot ALREADY works end to end.
R2's seam — `run_embedded_job_store` (`commands/job/mod.rs`) plus
`discover_virtual_store` (`camel-dsl/src/discovery.rs`), which loops every
source-plan reference — generalized the boot when it landed. A job document
declaring `routeFiles: [routes/b.yaml, routes/c*.yaml, routes/a.yaml]`
(4 route files) compiles, lists all entries in `--manifest`, boots the direct
chain `B.C1.C2.A` to outcome `Completed`, rejects a structurally invalid entry
at boot with exit 2 naming `compiled://routes/c2.yaml`, and `camel job` runs
the same set to exit 0. What R3 actually owes is the parity walls the probes
exposed and the proof the batteries pin:

- **Wall P1 (reject-asymmetry):** a job document whose `routeFiles` overlaps a
  configuration `routes = [...]` pattern runs under `camel job` (exit 0 — the
  CLI job path never consults configuration route patterns; the job route
  source is exactly-one of `routeFilesFromRoot` / `routeFiles` / inline
  `routes:`, see `resolve_route_source` in `commands/job/document.rs`), but
  `camel compile` seeds the plan from configuration route patterns for BOTH
  kinds and then rejects the duplicate claim: `duplicate source 'routes/b.yaml'`.
  A document set that runs under `camel job` must not be rejected by compile —
  that is the epic's deployment-equivalence bar and this mission's "no new
  wall" clause.
- **Wall P2 (accept-asymmetry):** a job document whose file-form pattern
  resolves zero files is rejected by `camel job` (`job route source resolved
  zero route definitions`, CONTEXT.md job-safety rejection class), but
  `camel compile` exits 0 and embeds a guaranteed-dead artifact (its boot then
  fails with the same wording). Compile must fail before any output byte,
  mirroring the CLI verdict.

**Fix D1 — job-kind plan semantics (`compile/sources.rs`).** `resolve()` already
carries `kind: TrailerKind`. For `TrailerKind::Job` entry documents:

1. Skip the configuration-route-pattern plan seeding (the block commented
   `Config route patterns, declared order, matches sorted`). Route-kind
   behavior is untouched — configuration patterns remain a route-document
   mechanism there. The job plan then contains exactly what the artifact boot
   consumes and what `camel job` boots: the job document plus its own
   file-form expansions. Configuration/include/profile entries still embed for
   jobs (the job boot projection needs them).
2. Reject a file-form job route source that resolves zero route entries with a
   new `SourceError` variant (`JobRouteSourceEmpty`, carrying the document
   logical path and the declared patterns), using the CLI's exact rule string
   `job route source resolved zero route definitions` (`commands/job/mod.rs`
   job-safety class) so parity greps match both surfaces; for the zero-file
   case the wording is accurate — zero files means zero definitions. This is a
   count-of-entries check only — compile keeps allowing files whose contents
   are structurally invalid (fail-late, fail-closed at boot with the entry
   named; same philosophy as bd rc-epx5i), and inline `routes:` job documents
   keep their existing v1-compatible compile behavior.

**Fix D2 — nothing else changes.** The store codec, boot seam, manifest
schema 3, ordering rules, and trailer framing are untouched. Ordering stays
exactly as R2 validated it: source plan = declared pattern order with each
pattern's matches sorted by normalized logical path; store entries pack in
canonical lexicographic path order; identical inputs produce identical
artifact bytes. The R3 tripwire (bd `rc-p823t`) is respected — no long-lived
manifest entries, `artifact_kind: "job"` unchanged. Job-referencing-job stays
the existing named rejection (`reserved test/job document, not a route
source`).

**Proof D3 — battery.** Extend the two mission-named batteries:
`compile_command_test.rs` (plan order, config-pattern skip with a `camel job`
accepts-the-same-set pin, zero-entry rejection, byte determinism) and
`compiled_artifact_test.rs` (multi-entry boot via a direct-endpoint chain
across ≥3 route files — job-legal consumers only: `direct`/`seda`/`log`/
`mock`/`stream`; per-entry failure naming; manifest listing N entries without
boot). Parity is executed, not assumed: the boot test also runs
`camel job` on the same document set through the battery's existing
`CARGO_BIN_EXE_camel` pattern and asserts the same outcome and reply as the
artifact. The chain shape is the observability trick: one one-shot send walks
every file's route, so a missing or unbooted entry breaks the chain and the
reply body proves all entries ran.

## Affected crates

- `camel-cli` — `src/compile/sources.rs` (D1), `tests/compile_command_test.rs`,
  `tests/compiled_artifact_test.rs`, `CONTEXT.md` (compile-map alignment).

## Architecture boundaries

All edits sit inside the `camel-cli-compile` lease (`crates/camel-cli/src/compile/`
plus its batteries). No `camel-dsl`, `camel-config`, or `commands/job/**` changes
(the boot already iterates N; touching it would cross the lease for no need).
The job-plan rule mirrors a CLI semantic that already exists — it adds no new
surface, it removes an asymmetry.

## Phases

### Phase 1: Compile-side parity fixes (w_balanced)

D1.1 + D1.2 in `sources.rs`, with `compile_command_test.rs` additions: plan
order `[job, b, c1, c2, a]`, config-pattern skip for jobs, zero-entry
rejection (literal + from-root), byte determinism re-compile.

### Phase 2: Boot battery (w_balanced)

Two new fixtures in `compiled_artifact_test.rs` (`multi_job_n` chain,
`multi_job_bad` invalid entry), three tests: chain boot outcome, failure
naming with no partial boot, manifest listing without boot.

### Phase 3: Documentation alignment (w_fast)

`crates/camel-cli/CONTEXT.md` compile section: job-kind plan semantics
sentence (config patterns do not seed the job plan; zero-route-entry file
source fails at compile). No ADR — this refines ADR-0075's framework under the
epic's existing parity ruling; the spec delta carries the normative text.

## Alternatives considered

- **Generalize the boot loop first** — rejected: empirically already general;
  writing a no-op generalization would be noise. The battery is the
  generalization proof.
- **Dedup overlapping config patterns for jobs instead of skipping** —
  rejected: dedup would embed route files the CLI never boots, silently
  widening the artifact beyond `camel job` semantics. Exact-source parity is
  the goal.
- **Parse route contents at compile to mirror "zero route definitions"** —
  rejected: compile deliberately tolerates structure-invalid documents
  (fail-late, fail-named at boot); a content parser at compile would create a
  new validation wall. The count-of-entries check matches the CLI verdict for
  the zero-file case, which is the only case compile can see without parsing.
- **Spec delta shape** — one MODIFIED block is REQUIRED, not just ADDED: the
  P1 fix falsifies the existing "Overlapping sources fail closed" scenario of
  the "Resolve and confine compile-time sources" requirement (it is not
  kind-scoped, so a pure-ADDED delta would leave the merged spec with two
  opposite verdicts on an overlapping GIVEN — r_glm plan finding F1). The
  delta therefore carries that requirement's seven scenarios with the job-kind
  carve-out added to the requirement text and to the overlapping-sources
  scenario; every scenario name is preserved verbatim. Everything else is
  ADDED requirements; no other existing text changes.
