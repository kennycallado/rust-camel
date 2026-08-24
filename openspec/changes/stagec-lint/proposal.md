# Proposal: stagec-lint

## Why

ADR-0064 §5 defines Stage C: deprecate inline `to: mock:` sends in
production routes now that the declarative test surface is complete. Stages
A (route interception primitive), B (`intercepts:` block), and the two gate
epics have landed: bean steps are testable (`beans:` stubs, rc-07qh) and
synchronous direct replies are assertable (`expectReply:`, rc-66c5). Every
send-side reason to keep an inline `mock:` in a `camel run` route is gone —
yet nothing tells users their fixture routes are now anti-patterns. A
lint rule closes the loop: warn, let migration happen lazily, escalate to
error only after the ecosystem converts (no flag-day, mirroring ADR-0045 §4).

## What Changes

Add one lint rule to the existing catalog, scoped to exactly the surface
the declarative test tier replaces.

- New rule `RMockRule` (`crates/camel-lint/src/rules/rmock.rs`), diagnostic
  code `R-MOCK-IN-PRODUCTION`, severity **Warning** (never affects exit
  code — the lazy-migration contract).
- **Origin-scoped detection**: `Endpoint` (camel-lint route view) gains an
  additive `key` annotation recording the URI-bearing key it was emitted
  from (`to`, `from`, `uri`, `wire_tap`, `endpoints[i]`→`endpoints`, …).
  The rule fires ONLY on origins `to` and `endpoints` — precisely the send
  surfaces that compile to `BuilderStep::To` and are therefore replaceable
  by `intercepts:` (`skipTo`/`divertCopyTo`). `wire_tap`, `enrich`,
  `poll_enrich`, `dead_letter_channel`, and `from` origins are NOT flagged:
  interception does not cover them, so the migration advice would be false
  there (e_opus A1 honored at surface granularity).
- **Path-scoped exemption** (ADR-0064 §5: "Inline `mock:` stays legitimate
  in pure test-fixture routes that `camel run` never loads"): the lint CLI
  suppresses `R-MOCK-IN-PRODUCTION` (only that code) for files under a
  `tests/fixtures/` path component — the engine itself stays source-only
  and path-free. The corpus gate applies the same shared predicate, so
  component test fixtures are neither warned nor baselined.
- Test documents are never scanned: the existing skip predicate
  (`is_test_document`) already excludes `*.test.yaml` from both `camel lint`
  and the corpus gate.
- Message points at the migration: declare `intercepts:`
  (`skipTo`/`divertCopyTo`) in a `*.test.yaml` — see the testing guide.
- Escalation path documented, not encoded: Error-severity flip happens in a
  future change after ecosystem conversion (ADR-0064 §5; the e_opus A1 gates
  — rc-66c5/rc-07qh — are closed by this branch's stack).
- Corpus baseline: with origin+path scoping, 4 in-tree files light up
  (`mock-demo.yaml`, `intercepts-demo.yaml`, both `routes-eip-advanced`
  pairs — 14 `to: mock:` occurrences collapsing to 4 per-file entries) —
  each gets a justified entry ("camel-run demo route, teaching fixture
  pending migration"). The xslt/xj component fixtures sit under
  `tests/fixtures/` and are exempt — no entry, no warning.
- Docs: rule table + code/severity surfaces in `crates/camel-lint/CONTEXT.md`;
  a note in `docs/src/testing/index.md` that `camel lint` now warns on inline
  `to: mock:`/`endpoints: mock:` sends; a dated amendment to ADR-0064
  recording the rc-07qh/rc-66c5 gap closures and Stage C warn-phase
  completion (deferred housekeeping from the reply-capture holistic review).

Explicitly excluded:
- Flagging `wire_tap`/`enrich`/`poll_enrich`/`dead_letter_channel`/`from`
  mock URIs (no declarative replacement exists — would be false advice).
- Error-level default or any `--deny`/escalation flag (future change).
- Interceptable-URI registry (e_opus A3: not needed).
- `.test.yaml` scanning, Rust-inline mock strings (never scanned).
- Rewriting the 4 in-tree demo routes (baselining is honest; they teach).
- Any change to `camel run`, the test tier, camel-mock, or the engine's
  source-only contract.

## Acceptance criteria

- A route file with `to: mock:out` (or an `endpoints: [mock:a, mock:b]`
  recipient list) lints to one `R-MOCK-IN-PRODUCTION` Warning per mock
  occurrence, with the migration message; `wire_tap: mock:tap` and
  `from: mock:x` stay silent.
- `camel lint` on a file under `tests/fixtures/` emits no
  `R-MOCK-IN-PRODUCTION`; other rules' diagnostics are unaffected by the
  suppression.
- Exit code stays 0 on Warning-only findings (1 reserved for Error).
- Corpus gate green: emitted == baseline including the 4 new justified
  entries; the xslt/xj fixtures contribute nothing.
- Engine rule-count test updated to six rules.
- Docs updated (CONTEXT.md rule table, testing guide note).

## Risk budget

One rule + one additive field on a view struct + one CLI-side suppression
predicate; zero engine/exit-code/dependency changes. Accepted risks: the
`tests/fixtures/` path convention is the fixture-exemption mechanism (the
lint cannot see Camel.toml reachability — the convention is the honest
syntactic proxy, documented as such). Out of bounds: severity escalation
machinery, registry, reachability analysis, any non-lint crate behavior
change.
