# Proposal: scenario-tier-p3-sweep

## Why

Epic rc-enbw (scenario-tier pilot follow-ups, ADR-0069) is 19/21 complete. The
five remaining P3 children are small, independent, and each blocks pilot teams
from expressing real assertions or trusting emitted URLs/diagnostics:

- **rc-tdgh5** — the scenario tier cannot assert on log content (contains /
  regex / level-cap). Teams shell out or eyeball logs today.
- **rc-vf7z7** — `resolve_url`'s bridge arm round-trips the base URL through
  `url::Url` (WHATWG normalization: dot-segment collapse, default-port strip,
  scheme/host lowercasing) while every other arm assembles strings verbatim:
  the same endpoint emits different bytes depending on arm. Dot-segment-
  sensitive upstreams get silently rewritten paths.
- **rc-0ahfl** — `crates/camel-integration-test/src/document.rs` is 1431
  lines (thermo-nuclear file-size finding); `DocError` + conversion helpers
  belong in their own module.
- **rc-o072s** — `ParsedTarget::parse`'s empty-path apparatus error echoes
  the raw declaration unredacted (`http://host?authPassword=x` leaks).
- **rc-2miu** — pilot teams fan out N bindVars for one multi-path partner
  because the one-listener/dynamic-reference-by-authority pattern is
  undocumented (a pilot incident on 2026-09-06 hit exactly this).

One change closes all five; two papal (e_opus) design verdicts govern the
log-capture seam and the resolve_url consistency direction (recorded in
design.md).

## What Changes

- **integration-tier spec** (deltas):
  - ADDED requirement: log-content assertion vocabulary (document-level
    `logs:` block: `contains`, `regex`, `noLevelAbove`; harness-owned capture
    layer at tier entry; time-window attribution; conservative cross-talk
    semantics; seat-loss hard error).
  - MODIFIED requirement: Wire-fidelity lane key and mismatch diagnostics —
    the empty-path apparatus error redacts sensitive query values (parity
    with wire-path diagnostics, ADR-0051).
- **http-emission-correctness spec** (delta):
  - MODIFIED requirement: bridgeEndpoint URL bridging — the bridge arm
    assembles the outbound URL verbatim (authored bytes, no WHATWG
    normalization), byte-identical to the non-bridge arms for the same
    declared base and resolved query.
- **Code**:
  - `crates/camel-integration-test`: new `log_capture` module + `logs:`
    grammar (document.rs + runner + new module family; `tracing-subscriber`
    dep added); the capture subscriber installs at the DRIVER seams before
    any boot — the itest `tests/common` helper and the `camel test`
    integration-tier driver entry
    (`crates/camel-cli/src/commands/test/scenario.rs`); document.rs split —
    `DocError` + conversion helpers move to `document/error.rs` (pure move,
    public surface stable); `ParsedTarget::parse` threads the secret-key
    set and redacts every declaration echo.
  - `crates/components/camel-http`: `resolve_url` bridge arm becomes string
    assembly; bridge pins that asserted normalized output flip to verbatim;
    stale comment replaced.
  - New log-assertion fixtures land under a fresh `tests/common/` layout
    (plus a dedicated foreign-subscriber test binary).
- **Docs/examples** (rc-2miu, no spec delta — the two-key contract is
  already specced): `docs/src/testing/index.md` "Scenario documents" gains
  the multi-path partner pattern (one declared endpoint + one bindVar +
  dynamic-reference receives per path + scripted responses per path); a
  runnable example pair in `examples/integration-testing/`; the
  `camel-integration-test/CONTEXT.md` arrival-lane entry gains the pattern
  note; the crate README's grammar area gains the two-key rule citation.

## Zones

`crates/camel-integration-test/**` (src + tests + docs surfaces + Cargo.toml),
`crates/camel-cli/src/commands/test/scenario.rs` (one capture-install call),
`crates/components/camel-http/src/lib.rs` (resolve_url + tests),
`docs/src/testing/index.md`, `examples/integration-testing/**`,
`crates/camel-integration-test/{README.md,CONTEXT.md}`, `openspec/**`.

FORBIDDEN (concurrent change in flight): camel-sql, DatasourceCatalog, any
sql/datasource work, camel-dsl env-engine internals.

## Out of Scope

- The three >1k-line scenario-tier test-file splits (rc-tdgh5 NOTES
  extraction debt): only the NEW log-assertion fixtures start the
  `tests/common/` layout; rewiring `http_partner_scripting_test.rs`,
  `partner_verification_test.rs`, `doc_parse_test.rs` is deferred (separate
  follow-up; conductor informed).
- Action-indexed log windows (a `validate target: logs` grammar): deferred
  per papal ruling — window-close mid-document reopens cross-talk questions.
- camel-dsl env-engine changes of any kind.

## bd

Closes rc-tdgh5, rc-vf7z7, rc-0ahfl, rc-o072s, rc-2miu (epic rc-enbw →
21/21).
