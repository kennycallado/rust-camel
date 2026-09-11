# Proposal: job-ux-reshape

## Why

A job is not a test, but `camel job` today consumes `*.test.yaml`
documents — the reserved suffix that ADR-0062 assigns to `camel test`.
The product owner ruled this a classification defect: `create-user.test.yaml`
names a test, yet runs as an operator job. There is also no discovery
surface: `camel job` demands an explicit path, and job documents have no
canonical home.

The feature landed one day ago (bd rc-pjm4, archived change `cli-jobs`).
Adoption is zero. A clean break is free; an alias would keep the confusing
spelling legal and add a second one (oracle ruling, e_opus, sealed
2026-09-11 — `.opencode/fleet/inbox/oracle-ruling-camel-job-reshape.md`).

## What Changes

**In:**

- Naming: `.job.yaml` / `.job.yml` becomes THE job suffix. A
  `*.test.yaml` declaring `execute:` is a load error that directs the
  author to rename. No alias, no compat shim.
- Discovery: new top-level `[jobs]` table in `Camel.toml` (key `dir`,
  default `"jobs"`). Bare `camel job foo` resolves
  `{jobs.dir}/foo.job.yaml` only — one deterministic spelling. The
  document's explicit route source stays mandatory; a job never inherits
  `routes/` discovery (deliberate asymmetry, see design).
- Listing: `camel job` with no argument lists the jobs directory —
  name plus optional `description:` key (cheap probe parse). Empty or
  absent directory prints a friendly message and exits 0.
- camel-dsl discovery: split the reserved-suffix predicate
  (`is_test_document` + new `is_job_document` + `is_reserved_document`),
  switch the route-discovery skip-gate, rename
  `ReservedTestSuffix` → `ReservedDocumentSuffix`.
- Docs: amend ADR-0062 (reserved-document contract, `Amended` marker in
  the CONTEXT-MAP ADR index), land the new terms in CONTEXT-MAP Key
  Terms in this change, update camel-cli CONTEXT.md / README, rename
  example job documents.

**Out:** `--arg` header injection and `mode: batch` (Change B per the
ruling). Any change to the test-family grammar, the body-scalar sentinel
classification, or the batch-reserved / allowlist / exit-taxonomy
requirements.

Affected crates: camel-dsl, camel-cli, camel-config. bd: rc-10d50.

## Acceptance criteria

- A `*.job.yaml` document with `execute:` parses; the same content under
  `*.test.yaml` fails with rename guidance, exit 2.
- Route discovery skips `.job.yaml` under wildcard globs and errors on
  literal naming, with a message naming both reserved families and their
  runners.
- `camel job foo` loads `{jobs.dir}/foo.job.yaml` ONLY; a listed
  `foo.job.yml` is not reachable via the bare token (explicit path
  required); a miss yields one error naming the probed file; explicit
  paths (including `.job.yml`) always win.
- No-arg `camel job` lists jobs (exit 0), one line per job; empty/absent
  dir prints the friendly message, exit 0; `--report` with no document
  stays exit 2.
- `description:` is optional; unknown keys still fail
  (`deny_unknown_fields` intact); listing renders a multiline
  `description:` on one line (newlines become spaces).
- Repo-wide sweep: no job-related `.test.yaml` reference survives in
  live code, tests, fixtures, examples, or docs (archived OpenSpec
  history excluded).
- `jobs.dir` resolves against the Camel.toml root — invoking from a
  subdirectory lists the same directory.
- Gates green: fmt, clippy, lint-unwrap, lint-non-exhaustive,
  lint-context-citations, doc gate (camel-dsl, camel-cli, camel-config,
  camel-api/core/builder/endpoint), schema-check,
  `cargo test -p camel-dsl -p camel-cli -p camel-config`.

## Risk budget

The breaking rename is accepted (zero adoption, pre-1.0). In bounds:
camel-dsl `discovery.rs`, camel-cli `commands/job/**` + `main.rs` job
wiring, camel-config `[jobs]` table, ADR-0062 amendment, CONTEXT-MAP
term landing. Out of bounds: `camel-dsl` `rest.rs`, `camel-api`,
`camel-processor`, http components, `benches/` (concurrent missions),
test-family grammar, `classify()` sentinel protocol.
