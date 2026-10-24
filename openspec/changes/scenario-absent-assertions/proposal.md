# Proposal: scenario-absent-assertions

## Why

In the scenario tier, `receive.extract` hard-fails when the selector resolves to nothing ("extract ... resolved to nothing"), and the matcher grammar has no negative form. A suite therefore cannot assert that a response header is NOT present. camel-112 needs exactly this to prove internal headers do not leak, and today works around it by extracting the whole headers object and regex-allowlisting — a recipe that neither reads as an absence claim nor generalizes to body paths (bd rc-5pwmg).

## What Changes

- Add an explicit absence matcher: `Expectation::Absent` in `camel-matchers`, authored as `{absent: null}` (payload rules mirror `exists`).
- Extend the `lastReceived` validate target with an optional `select: "<selector>"` field that projects the last received message through the same selector grammar `extract` uses (`headers.<name>`, `body.<path>`). `select` exists solely for absence assertions in this change: pairing it with any other expectation is a load error.
- Load-time validation: a `select` head must be `headers` or `body` with a non-empty sub-path; `absent` outside a `lastReceived`+`select` pairing is a load error. A misspelled selector head fails loudly at load, never as a vacuous pass.
- `receive.extract` behavior is UNCHANGED: absent sources keep hard-failing, preserving the typo protection existing suites rely on. No null-on-absent default, no optional extract.
- Document the semantics (case table for: absent header, present header with null value, misspelled selector, absent response body) in the crate README (grammar reference) and the scenario guide, with a worked camel-112-style leak-proof example and a migration note (the workaround, not behavior, migrates).

Affected crates: `camel-matchers`, `camel-integration-test`, docs (`crates/camel-integration-test/README.md`, `docs/src/testing/scenario-documents.md`).

## Acceptance criteria

- A scenario can assert an absent response header via `lastReceived` + `select: headers.X` + `{absent: null}`; the same form asserts an absent body path.
- Tests fail before and pass after for the four mission cases: absent header, present header with a null value (absent FAILS, equals-null passes), misspelled selector (load error under select; unchanged runtime hard failure under extract), absent response body.
- `absent` outside `lastReceived`+`select`, a bare/scalar `select` head, and `select` paired with a non-absent expectation are load errors naming the action index.
- A literal string expectation `"absent"` still parses as `equals "absent"`; `{absent: null}` is the only absent form.
- `receive.extract` hard-failure tests from before this change still pass unmodified.
- Full gate registry per AGENTS.md (with the two mission-ordered pre-existing exemptions) plus `cargo test -p camel-integration-test`.

## Risk budget

- In bounds: additive matcher variant on a `#[non_exhaustive]` enum (ADR-0049), one optional target field, load-time rejections, docs.
- Out of bounds: changing `extract` semantics, generalizing `select` to non-absent expectations, new matcher DSL beyond absence, JMS/HTTP step handler changes, version bump.
