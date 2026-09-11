# Design: job-ux-reshape

Grounded in the sealed oracle ruling (e_opus, 2026-09-11). This design
records the two rulings that must survive as rationale; the rest is
mechanics.

## Approach

**1. Split the reserved-suffix predicate (camel-dsl).** Keep
`is_test_document` (`.test.yaml`/`.test.yml`) for `camel test`. Add
sibling `is_job_document` (`.job.yaml`/`.job.yml`). Add
`is_reserved_document = is_test_document || is_job_document`. The
route-discovery skip-gate (discovery.rs ~line 327) and the literal-name
error switch to `is_reserved_document`, so a stray `routes/**/*.yaml`
glob that brushes `jobs/x.job.yaml` skips it rather than parsing it as a
route. `DiscoveryError::ReservedTestSuffix` is renamed
`ReservedDocumentSuffix` with a message naming both families and their
runners (`camel test` vs `camel job`). Discovery unit tests mirror the
existing `.test.yaml` pair (~lines 1546-1680): `.job.yaml` skipped under
wildcard, erroring on literal.

**2. Job document gate (camel-cli `job/document.rs`).**
`parse_job_document` gates on `is_job_document`.
`JobDocError::NotTestSuffix` becomes `NotJobSuffix { path }` with message
"job document {path} must use the reserved `.job.yaml`/`.job.yml`
suffix". `JobDocumentDoc` gains `description: Option<String>`
(`deny_unknown_fields` intact). The existing cross-family guards
(scenario/unit-tier sections rejected beside `execute:`) stay as
belt-and-suspenders — their justification is now "reject test vocabulary
in a job document". Symmetrically, `camel test` dispatch of a
`.test.yaml` declaring `execute:` refuses with a pointer to
`camel job`.

**3. `[jobs]` config table (camel-config).** New top-level table, NOT a
key on `[context]`: routes are the always-on data plane; jobs are an
operator tool surface. Mixing them would overload a hot-path struct with
a CLI convenience. One field for v1:

```toml
[jobs]
dir = "jobs"     # default when [jobs] absent
```

`JobsCamelConfig { dir }` with `deny_unknown_fields` (family rule). Wired
through all three merge sites in `config.rs`: the resolved `CamelConfig`
(~15), the optional overlay (~140) with `unwrap_or` in the layer merge
(~182), and `Default` (~206). Round-trip test: absent `[jobs]` defaults
to `"jobs"`; explicit `dir` survives the merge.

**4. Bare-name resolution + listing (camel-cli `job/mod.rs`).**
`JobArgs.document` becomes optional. With a document, resolution order:
an argument containing `/` or ending in a recognised document suffix is
an explicit path (current behavior, always wins — including explicit
`.job.yml`); otherwise probe exactly `{jobs.dir}/<name>.job.yaml` and on
miss fail with "no job `<name>` in `{jobs.dir}/` (looked for
`<name>.job.yaml`)". The sugar never probes alternate spellings — one
deterministic resolution, one error.

With no document, list: read entries of `jobs.dir` (anchored at
`canonical_project_root(--config)` — the Camel.toml root, never the
CWD), filter reserved job suffixes, sort, print `name` (suffix stripped)
plus `description` or `(no description)`. The listing strips both
`.job.yaml` and `.job.yml` for display, but the bare token resolves
`.job.yaml` only — a listed `foo.job.yml` needs an explicit path; this
asymmetry is deliberate (one deterministic sugar spelling) and is
tested. Descriptions render on one line: embedded newlines become
spaces at render time (the document parser stays lenient — no new load
error). Descriptions come from a cheap
probe parse (`serde_yaml` to `{ description: Option<String> }`), never
the full grammar — a malformed sibling shows `(unparseable)` and does
not abort the listing. Empty/absent dir prints "No jobs found in
`{jobs.dir}/..." to stdout, exit 0 (ls semantics). `--report` with no
document stays a usage error, exit 2. Listing and the JSON report never
co-occur (listing implies no document); assert this in tests.

**Route source stays mandatory (deliberate asymmetry).** `jobs.dir`
governs where job documents live, never where a job's routes come from.
Defaulting a job's routes to `routes/` discovery would drag arbitrary
`from: kafka:` consumers into a job's boot and blow the fail-closed
allowlist wide. A job stays self-describing: it names its routes.

## Affected crates

- camel-dsl: `discovery.rs` — predicate split, error rename, skip-gate,
  unit tests.
- camel-cli: `commands/job/document.rs` (suffix gate, `description:`,
  `NotJobSuffix`), `commands/job/mod.rs` (optional document, bare-name
  resolution, listing, anchoring), `main.rs` (arg wiring), test
  dispatcher pointer, `tests/job_one_shot_test.rs` fixtures,
  CONTEXT.md / README.
- camel-config: `[jobs]` table, three merge sites, round-trip test.
- Docs: ADR-0062 amendment, CONTEXT-MAP (Key Terms + ADR index `Amended`
  marker; fixes a stale `ADR-0063` authority citation on the reserved-
  suffix terms — 0063 is the Redis repository ADR).

## Architecture boundaries

DSL owns the suffix rule (single choke point; CLI keeps no private
copy). Config owns `[jobs]` defaults. CLI owns resolution, listing, and
exit codes. No Runtime/Component surface changes. ADR-0069 Decision 6
(section classifies the document) is untouched — section classification
still holds within a suffix; the suffix is now the first discriminator.

## Alternatives considered

- Alias `.test.yaml` alongside `.job.yaml`: rejected — keeps the
  confusing spelling legal, adds a second one; compat value is zero at
  zero adoption.
- `[jobs]` as a key on `[context]`: rejected — data-plane/operator-tool
  separation (above).
- Default job routes to `routes/`: rejected — allowlist safety (above).
- New capability spec instead of in-place `cli-jobs` delta: rejected —
  one day old, zero consumers; a second spec would fragment the job
  story forever.
- New ADR: rejected — ADR-0062 already owns the reserved-suffix
  contract; this generalises it to two suffixes (amendment, `Amended`
  marker in the CONTEXT-MAP index).
