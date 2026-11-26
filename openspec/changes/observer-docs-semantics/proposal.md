# Proposal: observer-docs-semantics

## Why

An external team built an MCP server on rust-camel 0.70.0 and hit eight behaviors
they had to reverse-engineer from source. Each behavior is a documented design
decision or a spec requirement, but the rendered guide does not carry the
sentence that explains it. The e_opus adjudication of the observer report
(2026-11-26) classified all eight as documentation gaps on designed surfaces and
produced the eight statements this change writes down.

## What Changes

Five pages under `docs/src/` gain prose. No code changes. No spec deltas
(`skip_specs: true`).

1. `eip/circuit-breaker.md` — a business error counts as a success for both
   breakers, in every state; `okStatusCodeRange` shapes what counts as a
   technical failure (ADR-0097, owner decision Q4).
2. `components/direct.md` — Sequential admission is FIFO, one call in flight,
   and the admission wait counts toward `timeout_ms`; `concurrency: concurrent`
   opts into parallel calls (direct-dispatch spec). A failed called route
   returns the caller to the pre-call exchange; `throw.data` carries the
   diagnostics (do-try spec). Concurrent admission applies back-pressure and
   never rejects (ADR-0044).
3. `configuration/env-interpolation.md` — the probe cap arithmetic: eight
   integer-or-boolean leaves per document, each `throttle` step uses two.
4. `components/mcp.md` — one server-level `security_policy`; per-tool policy
   needs explicit routes; business failures project to `isError: true` with
   `code` and `kind` only (ADR-0097, mcp-component spec).
5. `languages/rhai.md` and `testing/camel-test.md` — caps and the reserved
   `match` word are deliberate; unit documents boot the closed lean set with
   `direct:` stimulus only and read no `Camel.toml` (ADR-0064); routes with
   `security_policy` or `from: mcp:` belong in the integration tier, kept as
   thin adapters over a unit-tested `direct:` route.

Affected crates: none (docs only).

## Acceptance criteria

- All eight adjudicated statements appear on their target pages, each citing a
  crate CONTEXT or ADR (two-source rule).
- `mdbook build docs` exits 0 and `mdbook test docs` exits 0.
- No page duplicates an existing section; existing statements are sharpened in
  place.

## Risk budget

Risk is limited to prose. Worst case: a wrong sentence misleads a user. The
verification pass checks every statement against its citation source before
writing. Out of bounds: behavior changes, spec deltas, new pages, SUMMARY edits.

Bd: rc-whtym
