# Proposal: nondsl-channel-fuzzing

## Why

bd rc-rs2v (P3, backlog from e_opus ruling 2026-09-03): the three non-DSL
route channels — `rest.rs`, `openapi.rs`, `mcp.rs` in camel-dsl — are
distinct grammars with distinct trust models, deliberately excluded from
canonical-path fuzzing (rc-fvah) to avoid scope explosion. The existing
fuzz corpus (dsl_yaml/dsl_json/dsl_parity/dsl_template) contains zero
`rest:`/`mcp:` content, so even though the front-ends lower those blocks
on every parse, coverage-guided mutation almost never reaches the
lowering logic. These channels construct network listeners (rest, mcp)
and produce an externally served document (openapi), so a panic in them
is an availability defect on a listener-facing surface.

## What Changes

- Three new fuzz targets in the existing `fuzz/` crate, one per channel:
  `dsl_rest`, `dsl_mcp`, `dsl_openapi` (input is a DSL document for all
  three; the openapi target additionally drives `generate_openapi` on the
  parsed `rest` blocks, mirroring the camel-cli caller).
- Entry-point audit per channel, ranked by untrusted-input exposure per
  decision doc §3.1 (recorded in design.md). Audit finding: openapi.rs is
  generation-only — no external-document ingestion path exists; the bd's
  "ingestion plausibly HIGHER risk" hypothesis is falsified and the
  ranking is corrected to rest > mcp > openapi.
- Minimized committed seed corpora per target (`fuzz/seeds/<target>/`),
  pinned by seed-contract tests (existing pattern).
- xtask `KNOWN_TARGETS` and `scripts/fuzz-legs.sh` leg-selection rules
  extended; fuzz-smoke CI picks the new legs up with no workflow-structure
  change.

Excluded: runtime MCP protocol handling, HTTP listener runtime, new
production code changes (fuzz-only + tooling wiring), corpus sharing
beyond committed minimized seeds.

## Acceptance criteria

- Fuzz harnesses exist for `rest.rs`, `openapi.rs`, and `mcp.rs`
  grammars, ranked by usage and trust model (audit recorded in
  design.md).
- Each target has its own minimized committed seed corpus pinned by
  tests; all seeds pass through the harness without panic.
- `cargo xtask fuzz <target>` accepts the three new names; fuzz-legs.sh
  selects them per the documented rules and its self-test passes.
- A first scoped fuzz run per target completes; any crash is triaged
  into the standard pipeline (tmin → committed regression test → bd).

## Risk budget

Low risk: fuzz-only code plus two tooling lists; no production crate
changes. Anti-obstruction policy holds (nothing enters QUALITY GATES;
findings become bd issues). Disk budget stays inside the existing
worktree-local `target-fuzz/` isolation; main checkout stays cold.

Affected crates: camel-fuzz (fuzz/), scripts/xtask, scripts/fuzz-legs.sh.
bd: rc-rs2v.
