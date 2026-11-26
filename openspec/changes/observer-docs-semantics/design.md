# Design: observer-docs-semantics

## Context

The observer adjudication (`.opencode/fleet/orders/observer-findings-2026-11-26.md`,
adjudicated by e_opus) produced eight sentences the rendered guide must carry.
Statements 1, 2, and 6 are true on `main` as of d84bfd804: mission 613 landed
ADR-0097 (business-error envelope) and mission 614 landed the direct Concurrent
admission spec. Every statement was verified against its source before this
design was written:

| # | Statement (short) | Source verified |
|---|---|---|
| 1 | Business error = breaker success, both breakers, every state; `okStatusCodeRange` shapes technical failures | `docs/adr/0097-business-error-envelope.md:164-186` (records Q4); `camel-http` `okStatusCodeRange` default `(200, 299)`, non-ok → `HttpOperationFailed` (`crates/components/camel-http/src/lib.rs:302-306, 3665`) |
| 2 | Sequential direct: admission is FIFO, one in flight, wait counts toward `timeout_ms`; `concurrency: concurrent` opts out | `openspec/specs/direct-dispatch/spec.md:110-154` (admission serialization + inline timeout parity: boundary spans lookup, admission wait, execution) |
| 3 | Failed called route → catch from pre-call exchange; discard its headers/properties; use `throw.data` | `openspec/specs/do-try/spec.md:101-104, 152`; `camel-api/src/error.rs:449-459` (`data: Option<RedactedJson>`), ADR-0097:63 |
| 4 | `concurrency: concurrent` back-pressures; extra exchanges wait for a permit, never rejected | `docs/adr/0044-route-admission-backpressure.md` (permit acquired before dequeue → send back-pressures); `docs/src/architecture/index.md:53-57` |
| 5 | Typed env probe: max 8 leaves per document; each `throttle` step uses two | `crates/camel-dsl/src/env_int_probe.rs:62` (`MAX_PROBE_CANDIDATES = 8`); `ThrottleStepDef` has exactly two integer fields (`max_requests: usize`, `period_ms: u64`, `crates/camel-dsl/src/model.rs:465-470`); the route-document surface names them `max_requests` and `period_secs` (`crates/camel-dsl/src/yaml.rs:1786`, converted to ms at `:1812`) |
| 6 | `mcp:` block policy is per-server; per-tool needs explicit routes; error projection `isError: true`, payload `{"error", "kind"}` only | `openspec/specs/mcp-component/spec.md:838-853` (projection), `:484-490` (bind/TLS/policy are server properties, not per-tool URI options); ADR-0097 |
| 7 | Rhai caps and reserved `match` deliberate; unit docs boot the closed lean set, `direct:` stimulus only, no `Camel.toml` | ADR-0064:60-91 (lean set, creep rule); `camel-cli/src/commands/test/runner.rs:78-107` (`boot_context` loads no config); rhai token table (`match` is a keyword in rhai 1.27.0) |
| 8 | `security_policy` / `from: mcp:` routes → integration tier as thin adapters over unit-tested `direct:` | ADR-0064:60-91; `camel-dsl/src/compile.rs:60-69` (`security_policy` requires an authenticator from `Camel.toml`, which the lean boot never reads) |

## Goals / Non-goals

- Goal: one sharpening edit per page, at the point a reader hits the surprise.
- Non-goals: new pages, SUMMARY.md edits, spec deltas, code, examples changes.
  All statements fit existing sections; inline code snippets are avoided —
  YAML parameter names appear as code spans, not fenced examples.

## Decisions

- **Edit site per statement.** Statement 1 → new `## Business errors are
  breaker-neutral` section in `eip/circuit-breaker.md` (the page has no
  business-error text today). Statements 2-4 → `components/direct.md`
  (sharpen the Producer paragraph; extend Error handling; one new paragraph on
  concurrency back-pressure — no separate concurrency page exists, and the
  architecture page already carries the memory-bounding rationale, so direct.md
  links there). Statement 5 → `configuration/env-interpolation.md`, in the
  route-documents section that already states the cap; add the throttle
  arithmetic. Statement 6 → `components/mcp.md` Security paragraph (per-tool
  escape hatch) + a short error-projection paragraph in the Server section.
  Statement 7 → `languages/rhai.md` resource-limits paragraph (deliberate caps,
  reserved `match`) plus the unit-boot note; statement 8 →
  `testing/camel-test.md` after the ADR-0064 reference (integration-tier
  routing rule).
- **Citations satisfy the two-source rule.** Every statement cites an ADR, a
  crate CONTEXT, or the governing spec, written the way this book cites
  specs: a sentence of the form
  `openspec/specs/<capability>/spec.md in the repository, outside the
  rendered book`.
- **STE flavored mode** per docs/AGENTS.md: short sentences, active voice, no
  banned verbs, no em-dashes.
- **No duplication.** Where a page already carries part of a statement (direct.md
  Producer paragraph already says "FIFO admission"; env-interpolation.md already
  states the eight-leaf cap), sharpen that sentence in place instead of adding a
  parallel one.

## Risks / Trade-offs

- A wrong citation is worse than no citation: the verification table above is
  the check. The blessing gate re-checks it.
- mdbook treats untagged fences as Rust; this change adds no fenced code, only
  prose and code spans.

## Migration

None. Prose lands with the next docs build.

## Phases

Single phase. Six file edits, one verification pass.
